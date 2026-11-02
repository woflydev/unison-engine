//! Normalized in-memory state for one account.
//!
//! [`Store::apply`] takes a gateway dispatch and returns coarse [`Delta`]s; the
//! UI layer re-reads only what each delta names. Message history is windowed
//! per channel and capped, so a busy account stays small.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::forum::{self, ForumQuery, ForumState, Previews};
use crate::model::*;
use crate::snowflake::Snowflake;
use crate::{Error, Result};

const MAX_MESSAGES_PER_CHANNEL: usize = 300;
const MAX_CHANNELS_WITH_HISTORY: usize = 64;

/// How long a joined thread with no unread stays in the sidebar after its last message.
pub const THREAD_SIDEBAR_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

pub mod perm {
    pub const CREATE_INSTANT_INVITE: u64 = 1;
    pub const ADMINISTRATOR: u64 = 1 << 3;
    pub const VIEW_CHANNEL: u64 = 1 << 10;
    pub const SEND_MESSAGES: u64 = 1 << 11;
    pub const MANAGE_MESSAGES: u64 = 1 << 13;
    pub const ATTACH_FILES: u64 = 1 << 15;
    pub const ADD_REACTIONS: u64 = 1 << 6;
    pub const CONNECT: u64 = 1 << 20;
    pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
    pub const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
    pub const MANAGE_THREADS: u64 = 1 << 34;
    pub const KICK_MEMBERS: u64 = 1 << 1;
    pub const BAN_MEMBERS: u64 = 1 << 2;
    pub const MANAGE_CHANNELS: u64 = 1 << 4;
    pub const MANAGE_GUILD: u64 = 1 << 5;
    pub const VIEW_AUDIT_LOG: u64 = 1 << 7;
    pub const MUTE_MEMBERS: u64 = 1 << 22;
    pub const CHANGE_NICKNAME: u64 = 1 << 26;
    pub const MANAGE_NICKNAMES: u64 = 1 << 27;
    pub const MANAGE_ROLES: u64 = 1 << 28;
    pub const MANAGE_EVENTS: u64 = 1 << 33;
    pub const MODERATE_MEMBERS: u64 = 1 << 40;
    pub const ALL: u64 = u64::MAX;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Ready,
    /// A message in `channel` started, progressed, finished or failed sending.
    Sends { channel: Id },
    GuildsChanged,
    ChannelsChanged { guild: Option<Id> },
    /// A guild's scheduled events changed.
    GuildEvents { guild: Id },
    PrivateChannelsChanged,
    /// Favourite emoji/GIF settings changed.
    EmojiPrefs,
    MessagesReset { channel: Id },
    MessageInserted { channel: Id, message: Id },
    MessageUpdated { channel: Id, message: Id },
    MessageDeleted { channel: Id, message: Id },
    ReadState { channel: Id },
    Typing { channel: Id },
    MemberList { guild: Id },
    /// Members (op 8 reply) or emoji/stickers arrived: composer suggestions may have changed.
    MembersFound { guild: Id },
    UserUpdated { user: Id },
    Relationships,
    /// The current user's own status (online/idle/dnd/invisible) changed.
    OwnStatus,
    /// Preloaded user settings changed (here or on another device).
    SettingsChanged,
    /// A message that mentions the current user arrived (for notifications and the unified inbox).
    Mentioned { channel: Id, message: Id },
    /// A forum's post list or its previews changed.
    ForumChanged { channel: Id },
    /// Someone joined, left or changed state in one of a guild's voice channels.
    VoiceStates { guild: Id },
    /// Our own call changed: status, encryption, mute or who is speaking.
    Voice,
    /// Someone started a call in a DM that rings us. Never joins by itself.
    IncomingCall { channel: Id },
    /// An application asked us to show a modal (`Store::modal`).
    Modal,
    /// An application answered an option autocomplete request (`Store::autocomplete_choices`).
    Autocomplete { nonce: Id },
}

/// What a DM call carries besides who is in it (`CALL_CREATE` / `CALL_UPDATE`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DmCall {
    /// Users still being rung (the gateway's `ongoing_rings` keys, or an older `ringing` list).
    pub ringing: Vec<Id>,
    pub message_id: Option<Id>,
}

/// The call the user is in, as the UI shows it.
#[derive(Debug, Default, Clone)]
pub struct VoiceCall {
    pub guild: Option<Id>,
    pub channel: Option<Id>,
    pub status: Option<crate::voice::connection::Status>,
    pub e2ee: bool,
    pub privacy_code: Option<String>,
    pub speaking: HashSet<Id>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub audio_error: Option<String>,
    /// Last voice-gateway round trip, and the last few for an average.
    pub ping_ms: Option<u32>,
    pub pings: Vec<u32>,
    /// Go Live streams in our channel that we publish or watch (key, owner, viewers, paused).
    pub streams: Vec<StreamView>,
    /// Our camera is sending.
    pub camera: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamView {
    pub key: String,
    pub owner: Id,
    pub own: bool,
    /// A connection to it is open (we publish, or we watch).
    pub connected: bool,
    pub viewers: u32,
    pub paused: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ChannelHistory {
    pub messages: VecDeque<Message>,
    /// False once the oldest page has been reached.
    pub has_more_before: bool,
    /// True when the window ends at the newest message.
    pub at_latest: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MemberListState {
    pub member_count: u32,
    pub online_count: u32,
    pub items: Vec<MemberListItem>,
    /// Group sizes from the update's top-level `groups`; list items often omit them.
    #[serde(default)]
    pub group_counts: HashMap<String, u32>,
}

/// What the rail shows, top to bottom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RailEntry {
    Guild(Id),
    Folder(GuildFolder),
}

/// The statuses a user can set; anything else is never sent or accepted.
pub fn is_valid_status(s: &str) -> bool {
    matches!(s, "online" | "idle" | "dnd" | "invisible")
}

/// `serde(default)`: a cache written by an older build must still load after a field is added, otherwise
/// every upgrade throws the cached store away and the next launch waits for READY.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub me: Option<User>,
    /// Nicknames you gave friends, by user. Rebuilt whenever relationships change.
    #[serde(skip)]
    pub friend_nicks: HashMap<Id, String>,
    /// Messages being sent (uploading, waiting for Discord, or failed and not yet dismissed).
    #[serde(skip)]
    pub sends: Vec<crate::send_state::SendState>,
    /// Messages whose long code blocks the user opened (view state, lost on restart).
    #[serde(skip)]
    pub expanded_code: HashSet<Id>,
    /// Words that count as a mention for notifications (`comfort::Comfort::keywords`).
    #[serde(skip)]
    pub notify_keywords: Vec<String>,
    /// Gateway session id from READY: interactions carry it (`session_id`).
    #[serde(skip)]
    pub gateway_session: Option<String>,
    /// The modal an application opened and the user hasn't answered yet.
    #[serde(skip)]
    pub modal: Option<crate::interactions::ModalView>,
    /// Autocomplete answers by request nonce, newest few only.
    #[serde(skip)]
    pub autocomplete: HashMap<Id, Vec<(String, String)>>,
    /// Private notes on other users, loaded as profiles are opened.
    #[serde(skip)]
    pub notes: HashMap<Id, String>,
    pub users: HashMap<Id, User>,
    pub guilds: HashMap<Id, Guild>,
    pub guild_ready_order: Vec<Id>,
    pub folders: Vec<GuildFolder>,
    /// A rail edit is applied locally but not yet written; incoming settings must not undo it.
    #[serde(skip)]
    pub folders_unsaved: bool,
    /// Favourite and frecent emoji (settings-proto type 2), once loaded.
    #[serde(skip)]
    pub emoji_prefs: Option<settings_proto::EmojiPrefs>,
    pub guild_positions: Vec<Id>,
    pub channels: HashMap<Id, Channel>,
    pub guild_channels: HashMap<Id, Vec<Id>>,
    pub private_channels: Vec<Id>,
    pub roles: HashMap<Id, HashMap<Id, Role>>,
    /// My own member object per guild (for permissions and nick).
    pub my_members: HashMap<Id, Member>,
    /// Members seen in messages and member lists. Rebuilt live, not persisted.
    #[serde(skip)]
    pub members: HashMap<(Id, Id), Member>,
    pub read_states: HashMap<Id, ReadStateEntry>,
    pub guild_settings: HashMap<Id, UserGuildSettings>,
    pub dm_settings: Option<UserGuildSettings>,
    pub relationships: Vec<Relationship>,
    #[serde(skip)]
    pub history: HashMap<Id, ChannelHistory>,
    #[serde(skip)]
    history_lru: VecDeque<Id>,
    #[serde(skip)]
    pub typing: HashMap<Id, Vec<(Id, u64)>>,
    #[serde(skip)]
    pub member_lists: HashMap<Id, MemberListState>,
    /// The current user's status as Discord reports it across sessions.
    pub my_status: String,
    /// A status the user just chose: confirmations that still carry the old one (a stale
    /// `SESSIONS_REPLACE`, an older settings update) are ignored until the server agrees or it expires.
    #[serde(skip)]
    pub pending_status: Option<(String, std::time::Instant)>,
    /// Last known status of other users (friends, and members seen in READY_SUPPLEMENTAL), keyed by
    /// user. Absent means offline or unknown. Rebuilt live, not persisted.
    #[serde(skip)]
    pub presences: HashMap<Id, &'static str>,
    /// What others are doing (custom status, games, music), by user. Absent means nothing known.
    #[serde(skip)]
    pub activities: HashMap<Id, Vec<crate::activity::Activity>>,
    /// Preloaded user settings (settings-proto type 1), the slice Unison understands.
    #[serde(skip)]
    pub prefs: crate::prefs::Prefs,
    #[serde(skip)]
    pub unknown_events: HashMap<String, u64>,
    /// Messages that mentioned me, kept even when their channel isn't open (notifications, inbox).
    #[serde(skip)]
    pub recent_mentions: VecDeque<Message>,
    /// Archived-post pages per forum. Rebuilt on demand, not persisted.
    #[serde(skip)]
    pub forums: HashMap<Id, ForumState>,
    #[serde(skip)]
    pub previews: Previews,
    /// guild → user → voice state. Live only.
    #[serde(skip)]
    pub voice_states: HashMap<Id, HashMap<Id, VoiceState>>,
    /// Scheduled events by guild, and the ones I marked interested in.
    #[serde(skip)]
    pub scheduled_events: HashMap<Id, Vec<crate::guild_events::ScheduledEvent>>,
    #[serde(skip)]
    pub event_interest: HashSet<Id>,
    /// Live stage instances by stage channel.
    #[serde(skip)]
    pub stages: HashMap<Id, crate::guild_events::StageInstance>,
    /// DM channel → user → voice state, for calls in DMs and group DMs. Live only.
    #[serde(skip)]
    pub dm_voice: HashMap<Id, HashMap<Id, VoiceState>>,
    /// DM channel → call metadata (who is still ringing). Live only.
    #[serde(skip)]
    pub dm_calls: HashMap<Id, DmCall>,
    #[serde(skip)]
    pub voice: VoiceCall,
}

/// Whether the account is in a thread, and if not whether it could join (the composer area's bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadJoin {
    NotThread,
    Joined,
    /// Viewing a thread without being in it: a Join button, one click, is the only way in.
    CanJoin,
    /// Archived and not joined: Discord only lets you join an active thread (capture: whether the
    /// official client offers Join here), so it is read-only until someone posts and reopens it.
    Archived,
}

/// What a thread allows, from what its parent channel allows (`p`), as the official client
/// computes it: a private thread you are not in is invisible without Manage Threads; sending
/// takes Send Messages in Threads, and a locked thread also takes Manage Threads.
fn thread_permissions(thread: &Channel, p: u64) -> u64 {
    if p == perm::ALL {
        return p;
    }
    let manage = p & perm::MANAGE_THREADS != 0;
    if thread.kind == channel_type::PRIVATE_THREAD && thread.member.is_none() && !manage {
        return 0;
    }
    let send = p & perm::SEND_MESSAGES_IN_THREADS != 0 && (!thread.is_locked() || manage);
    if send { p | perm::SEND_MESSAGES } else { p & !perm::SEND_MESSAGES }
}

impl Store {
    pub fn my_id(&self) -> Option<Id> {
        self.me.as_ref().map(|u| u.id)
    }

    pub fn apply(&mut self, name: &str, data: &[u8]) -> Result<Vec<Delta>> {
        let term = unison_etf::RawTerm(data);
        macro_rules! de {
            () => {
                unison_etf::from_term(term).map_err(Error::from)?
            };
        }
        Ok(match name {
            "READY" => self.on_ready(de!()),
            "READY_SUPPLEMENTAL" => {
                let s: ReadySupplemental = de!();
                self.absorb_merged_members(&s.merged_members);
                self.absorb_merged_presences(&s.merged_presences);
                vec![Delta::GuildsChanged, Delta::PrivateChannelsChanged]
            }
            "GUILD_CREATE" | "GUILD_UPDATE" => {
                let g: Guild = de!();
                let id = g.id;
                self.upsert_guild(g, None);
                vec![Delta::GuildsChanged, Delta::ChannelsChanged { guild: Some(id) }]
            }
            "GUILD_DELETE" => {
                #[derive(Deserialize)]
                struct D {
                    id: Id,
                    #[serde(default)]
                    unavailable: bool,
                }
                let d: D = de!();
                if !d.unavailable {
                    self.remove_guild(d.id);
                }
                vec![Delta::GuildsChanged]
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "THREAD_CREATE" | "THREAD_UPDATE" => {
                let mut c: Channel = de!();
                let guild = c.guild_id;
                let parent = c.parent_id;
                // THREAD_UPDATE omits the thread member; keep the one we know.
                if c.member.is_none() {
                    c.member = self.channels.get(&c.id).and_then(|p| p.member.clone());
                }
                self.upsert_channel(c);
                let mut deltas = if guild.is_some() {
                    vec![Delta::ChannelsChanged { guild }]
                } else {
                    vec![Delta::PrivateChannelsChanged]
                };
                deltas.extend(self.forum_delta(parent));
                deltas
            }
            "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE" => {
                #[derive(Deserialize)]
                struct D {
                    channel_id: Id,
                    user: User,
                }
                let d: D = de!();
                let add = name == "CHANNEL_RECIPIENT_ADD";
                let uid = d.user.id;
                if add {
                    self.users.insert(uid, d.user);
                }
                if let Some(c) = self.channels.get_mut(&d.channel_id) {
                    c.recipient_ids.retain(|u| *u != uid);
                    if add {
                        c.recipient_ids.push(uid);
                    }
                }
                vec![Delta::PrivateChannelsChanged]
            }
            "CHANNEL_DELETE" | "THREAD_DELETE" => {
                let c: Channel = de!();
                let parent = self.channels.get(&c.id).and_then(|x| x.parent_id).or(c.parent_id);
                self.remove_channel(c.id);
                let mut deltas = if c.guild_id.is_none() && matches!(c.kind, channel_type::DM | channel_type::GROUP_DM) {
                    vec![Delta::PrivateChannelsChanged]
                } else {
                    vec![Delta::ChannelsChanged { guild: c.guild_id }]
                };
                deltas.extend(self.forum_delta(parent));
                deltas
            }
            "THREAD_LIST_SYNC" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    guild_id: Id,
                    threads: Vec<Channel>,
                    members: Vec<serde_json::Value>,
                }
                let d: D = de!();
                let mut parents = Vec::new();
                for mut t in d.threads {
                    t.guild_id = Some(d.guild_id);
                    if t.member.is_none() {
                        t.member = self.channels.get(&t.id).and_then(|p| p.member.clone());
                    }
                    if let Some(p) = t.parent_id.filter(|p| !parents.contains(p)) {
                        parents.push(p);
                    }
                    self.upsert_channel(t);
                }
                self.absorb_thread_members(&d.members);
                let mut deltas = vec![Delta::ChannelsChanged { guild: Some(d.guild_id) }];
                for p in parents {
                    deltas.extend(self.forum_delta(Some(p)));
                }
                deltas
            }
            "THREAD_MEMBER_UPDATE" => {
                #[derive(Deserialize)]
                struct D {
                    id: Id,
                }
                let raw: serde_json::Value = de!();
                let Ok(D { id }) = serde_json::from_value::<D>(raw.clone()) else { return Ok(vec![]) };
                let parent = self.channels.get_mut(&id).map(|c| {
                    c.member = Some(raw);
                    (c.guild_id, c.parent_id)
                });
                match parent {
                    Some((guild, parent)) => {
                        let mut deltas = vec![Delta::ChannelsChanged { guild }];
                        deltas.extend(self.forum_delta(parent));
                        deltas
                    }
                    None => vec![],
                }
            }
            "MESSAGE_CREATE" => {
                let m: Message = de!();
                self.on_message_create(m)
            }
            "MESSAGE_UPDATE" => {
                let p: MessagePatch = de!();
                self.on_message_update(p)
            }
            "MESSAGE_DELETE" => {
                let d: MessageDelete = de!();
                self.remove_message(d.channel_id, d.id)
            }
            "MESSAGE_DELETE_BULK" => {
                #[derive(Deserialize)]
                struct D {
                    ids: Vec<Id>,
                    channel_id: Id,
                }
                let d: D = de!();
                d.ids.into_iter().flat_map(|id| self.remove_message(d.channel_id, id)).collect()
            }
            "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
                let r: ReactionEvent = de!();
                self.on_reaction(r, name == "MESSAGE_REACTION_ADD")
            }
            // Sent instead of a burst of single adds (e.g. when a message gets popular). Same fold as
            // the official client's `addReactionBatch`: one add per (emoji, user).
            "MESSAGE_REACTION_ADD_MANY" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    channel_id: Id,
                    message_id: Id,
                    reactions: Vec<Batch>,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct Batch {
                    emoji: Emoji,
                    users: Vec<Id>,
                    burst: bool,
                }
                let d: D = de!();
                for b in d.reactions {
                    for u in b.users {
                        let r = ReactionEvent { user_id: u, channel_id: d.channel_id, message_id: d.message_id, emoji: b.emoji.clone(), burst: b.burst };
                        self.on_reaction(r, true);
                    }
                }
                vec![Delta::MessageUpdated { channel: d.channel_id, message: d.message_id }]
            }
            "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI" => {
                #[derive(Deserialize)]
                struct D {
                    channel_id: Id,
                    message_id: Id,
                    #[serde(default)]
                    emoji: Option<Emoji>,
                }
                let d: D = de!();
                if let Some(m) = self.message_mut(d.channel_id, d.message_id) {
                    match &d.emoji {
                        Some(e) => m.reactions.retain(|r| !same_emoji(&r.emoji, e)),
                        None => m.reactions.clear(),
                    }
                }
                vec![Delta::MessageUpdated { channel: d.channel_id, message: d.message_id }]
            }
            "MESSAGE_POLL_VOTE_ADD" | "MESSAGE_POLL_VOTE_REMOVE" => {
                let v: crate::interactions::PollVoteEvent = de!();
                let mine = Some(v.user_id) == self.my_id();
                match self.message_mut(v.channel_id, v.message_id) {
                    Some(m) => {
                        crate::interactions::apply_poll_vote(&mut m.poll, v.answer_id, name == "MESSAGE_POLL_VOTE_ADD", mine);
                        vec![Delta::MessageUpdated { channel: v.channel_id, message: v.message_id }]
                    }
                    None => vec![],
                }
            }
            "INTERACTION_MODAL_CREATE" => {
                let v: serde_json::Value = de!();
                self.modal = crate::interactions::parse_modal(&v, Id::default());
                vec![Delta::Modal]
            }
            // capture: event name and shape from the web client's handler (`nonce`, `choices`).
            "APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE" => {
                let v: serde_json::Value = de!();
                let Some(nonce) = v["nonce"].as_str().and_then(|n| n.parse::<u64>().ok()).map(Snowflake) else { return Ok(vec![]) };
                let choices = v["choices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        let value = match &c["value"] {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Null => return None,
                            other => other.to_string(),
                        };
                        Some((c["name"].as_str().unwrap_or(&value).to_string(), value))
                    })
                    .collect();
                if self.autocomplete.len() >= 16 {
                    self.autocomplete.clear();
                }
                self.autocomplete.insert(nonce, choices);
                vec![Delta::Autocomplete { nonce }]
            }
            "USER_NOTE_UPDATE" => {
                let v: serde_json::Value = de!();
                if let Some(id) = v["id"].as_str().and_then(|n| n.parse::<u64>().ok()).map(Snowflake) {
                    self.notes.insert(id, v["note"].as_str().unwrap_or_default().to_string());
                }
                vec![]
            }
            "THREAD_MEMBERS_UPDATE" => {
                let u: crate::interactions::ThreadMembersUpdate = de!();
                let me = self.my_id();
                let mut changed = false;
                if let Some(me) = me {
                    let me_s = me.to_string();
                    let added = u.added_members.iter().find(|m| m["user_id"].as_str() == Some(me_s.as_str())).cloned();
                    let removed = u.removed_member_ids.iter().any(|r| *r == me);
                    if let Some(c) = self.channels.get_mut(&u.id) {
                        if let Some(a) = added {
                            c.member = Some(a);
                            changed = true;
                        } else if removed {
                            c.member = None;
                            changed = true;
                        }
                        if let Some(n) = u.member_count {
                            c.member_count = Some(n);
                        }
                    }
                }
                let _ = changed;
                match self.channels.get(&u.id).map(|c| (c.guild_id, c.parent_id)) {
                    Some((guild, parent)) => {
                        let mut d = vec![Delta::ChannelsChanged { guild }];
                        d.extend(self.forum_delta(parent));
                        d
                    }
                    None => vec![],
                }
            }
            "MESSAGE_ACK" => {
                let a: MessageAck = de!();
                let rs = self.read_states.entry(a.channel_id).or_insert_with(|| ReadStateEntry { id: a.channel_id, ..Default::default() });
                rs.last_message_id = Some(a.message_id);
                rs.mention_count = a.mention_count.unwrap_or(0);
                vec![Delta::ReadState { channel: a.channel_id }]
            }
            "TYPING_START" => {
                let t: TypingStart = de!();
                if Some(t.user_id) == self.my_id() {
                    return Ok(vec![]);
                }
                let list = self.typing.entry(t.channel_id).or_default();
                list.retain(|(u, _)| *u != t.user_id);
                list.push((t.user_id, t.timestamp));
                if let (Some(g), Some(m)) = (t.guild_id, t.member) {
                    self.absorb_member(g, m);
                }
                vec![Delta::Typing { channel: t.channel_id }]
            }
            "GUILD_MEMBER_LIST_UPDATE" => {
                let u: MemberListUpdate = de!();
                let g = u.guild_id;
                self.on_member_list(u);
                vec![Delta::MemberList { guild: g }]
            }
            // Reply to op 8 (member search for @-autocomplete): keep the members, tell the UI.
            "GUILD_MEMBERS_CHUNK" => {
                #[derive(Deserialize)]
                struct D {
                    guild_id: Id,
                    #[serde(default)]
                    members: Vec<Member>,
                }
                let d: D = de!();
                for m in d.members {
                    self.absorb_member(d.guild_id, m);
                }
                vec![Delta::MembersFound { guild: d.guild_id }]
            }
            "GUILD_EMOJIS_UPDATE" => {
                #[derive(Deserialize)]
                struct D {
                    guild_id: Id,
                    #[serde(default)]
                    emojis: Vec<Emoji>,
                }
                let d: D = de!();
                if let Some(g) = self.guilds.get_mut(&d.guild_id) {
                    g.emojis = d.emojis;
                }
                vec![Delta::MembersFound { guild: d.guild_id }]
            }
            "GUILD_STICKERS_UPDATE" => {
                #[derive(Deserialize)]
                struct D {
                    guild_id: Id,
                    #[serde(default)]
                    stickers: Vec<GuildSticker>,
                }
                let d: D = de!();
                if let Some(g) = self.guilds.get_mut(&d.guild_id) {
                    g.stickers = d.stickers;
                }
                vec![Delta::MembersFound { guild: d.guild_id }]
            }
            "GUILD_MEMBER_UPDATE" | "GUILD_MEMBER_ADD" => {
                #[derive(Deserialize)]
                struct D {
                    guild_id: Id,
                    #[serde(flatten)]
                    member: Member,
                }
                let d: D = de!();
                let me = self.my_id();
                if d.member.id() == me {
                    self.my_members.insert(d.guild_id, d.member.clone());
                }
                self.absorb_member(d.guild_id, d.member);
                vec![Delta::ChannelsChanged { guild: Some(d.guild_id) }]
            }
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
                #[derive(Deserialize)]
                struct D {
                    guild_id: Id,
                    role: Role,
                }
                let d: D = de!();
                self.roles.entry(d.guild_id).or_default().insert(d.role.id, d.role);
                vec![Delta::ChannelsChanged { guild: Some(d.guild_id) }]
            }
            "GUILD_ROLE_DELETE" => {
                #[derive(Deserialize)]
                struct D {
                    guild_id: Id,
                    role_id: Id,
                }
                let d: D = de!();
                if let Some(r) = self.roles.get_mut(&d.guild_id) {
                    r.remove(&d.role_id);
                }
                vec![Delta::ChannelsChanged { guild: Some(d.guild_id) }]
            }
            "USER_UPDATE" => {
                let u: User = de!();
                let id = u.id;
                if Some(id) == self.my_id() {
                    self.me = Some(u.clone());
                }
                self.users.insert(id, u);
                vec![Delta::UserUpdated { user: id }]
            }
            "RELATIONSHIP_ADD" | "RELATIONSHIP_UPDATE" => {
                let r: Relationship = de!();
                if let Some(u) = &r.user {
                    self.users.insert(u.id, u.clone());
                }
                self.relationships.retain(|x| x.id != r.id);
                self.relationships.push(r);
                self.rebuild_friend_nicks();
                vec![Delta::Relationships]
            }
            "RELATIONSHIP_REMOVE" => {
                let r: Relationship = de!();
                self.relationships.retain(|x| x.id != r.id);
                self.rebuild_friend_nicks();
                vec![Delta::Relationships]
            }
            "USER_GUILD_SETTINGS_UPDATE" => {
                let s: UserGuildSettings = de!();
                match s.guild_id {
                    Some(g) => {
                        self.guild_settings.insert(g, s);
                        vec![Delta::ChannelsChanged { guild: Some(g) }]
                    }
                    None => {
                        self.dm_settings = Some(s);
                        vec![Delta::PrivateChannelsChanged]
                    }
                }
            }
            "USER_SETTINGS_PROTO_UPDATE" => {
                #[derive(Deserialize)]
                struct D {
                    settings: Settings,
                }
                #[derive(Deserialize)]
                struct Settings {
                    #[serde(rename = "type")]
                    kind: u8,
                    proto: String,
                }
                let d: D = de!();
                let mut deltas = vec![Delta::GuildsChanged];
                if d.settings.kind == 1 {
                    if self.absorb_settings_proto(&d.settings.proto) {
                        deltas.push(Delta::OwnStatus);
                    }
                } else if d.settings.kind == 2 {
                    self.absorb_frecency_proto(&d.settings.proto);
                }
                if d.settings.kind == 1 {
                    deltas.push(Delta::SettingsChanged);
                }
                deltas
            }
            "CHANNEL_PINS_UPDATE" => {
                #[derive(Deserialize)]
                struct D {
                    channel_id: Id,
                    last_pin_timestamp: Option<String>,
                }
                let d: D = de!();
                match self.channels.get_mut(&d.channel_id) {
                    Some(c) => {
                        c.last_pin_timestamp = d.last_pin_timestamp;
                        // The header's Pins dot follows this stamp.
                        vec![Delta::ChannelsChanged { guild: c.guild_id }]
                    }
                    None => vec![],
                }
            }
            // High-volume events we intentionally do not keep (presence churn outside the member list).
            "SESSIONS_REPLACE" => {
                let sessions: Vec<crate::model::SessionInfo> = de!();
                match crate::model::effective_status(&sessions) {
                    Some(st) if self.accept_status(&st) => {
                        self.my_status = st;
                        vec![Delta::OwnStatus]
                    }
                    _ => vec![],
                }
            }
            "PRESENCE_UPDATE" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct UserRef {
                    id: Id,
                }
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    user: UserRef,
                    status: String,
                    activities: serde_json::Value,
                }
                let d: D = de!();
                let status = self.set_presence(d.user.id, &d.status);
                let acts = self.set_activities(d.user.id, crate::activity::Activity::parse_list(&d.activities));
                match (status || acts, self.is_dm_recipient(d.user.id)) {
                    (true, true) => vec![Delta::PrivateChannelsChanged, Delta::UserUpdated { user: d.user.id }],
                    (true, false) => vec![Delta::UserUpdated { user: d.user.id }],
                    _ => vec![],
                }
            }
            "VOICE_STATE_UPDATE" => {
                let v: VoiceState = de!();
                if v.channel_id.is_some() && v.channel_id == self.voice.channel {
                    // Who in our call is muted, deafened or sending video, as Discord said it.
                    tracing::debug!(target: "unison_core::voice", user = v.user_id.0, self_mute = v.self_mute, self_deaf = v.self_deaf,
                        mute = v.mute, deaf = v.deaf, self_video = v.self_video, self_stream = ?v.self_stream, "call voice state");
                }
                let Some(g) = v.guild_id else {
                    // A DM or group-DM call.
                    for m in self.dm_voice.values_mut() {
                        m.remove(&v.user_id);
                    }
                    if let Some(c) = v.channel_id {
                        // Whoever answered is no longer ringing.
                        if let Some(call) = self.dm_calls.get_mut(&c) {
                            call.ringing.retain(|u| *u != v.user_id);
                        }
                        self.dm_voice.entry(c).or_default().insert(v.user_id, v);
                    }
                    self.dm_voice.retain(|_, m| !m.is_empty());
                    return Ok(vec![Delta::Voice]);
                };
                if let Some(m) = v.member.clone() {
                    self.absorb_member(g, m);
                }
                let states = self.voice_states.entry(g).or_default();
                if v.channel_id.is_some() {
                    states.insert(v.user_id, v);
                } else {
                    states.remove(&v.user_id);
                }
                vec![Delta::VoiceStates { guild: g }]
            }
            // The session routes it to the voice connection.
            "GUILD_SCHEDULED_EVENT_CREATE" | "GUILD_SCHEDULED_EVENT_UPDATE" => {
                let e: crate::guild_events::ScheduledEvent = de!();
                let g = e.guild_id;
                let list = self.scheduled_events.entry(g).or_default();
                match list.iter_mut().find(|x| x.id == e.id) {
                    Some(x) => {
                        // An update may omit the interested count; keep the one we had.
                        let count = e.user_count.or(x.user_count);
                        *x = e;
                        x.user_count = count;
                    }
                    None => list.push(e),
                }
                vec![Delta::GuildEvents { guild: g }]
            }
            "GUILD_SCHEDULED_EVENT_DELETE" => {
                let e: crate::guild_events::ScheduledEvent = de!();
                if let Some(l) = self.scheduled_events.get_mut(&e.guild_id) {
                    l.retain(|x| x.id != e.id);
                }
                self.event_interest.remove(&e.id);
                vec![Delta::GuildEvents { guild: e.guild_id }]
            }
            "GUILD_SCHEDULED_EVENT_USER_ADD" | "GUILD_SCHEDULED_EVENT_USER_REMOVE" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    guild_scheduled_event_id: Id,
                    guild_id: Id,
                    user_id: Id,
                }
                let d: D = de!();
                let add = name.ends_with("_ADD");
                if let Some(e) = self.scheduled_events.get_mut(&d.guild_id).and_then(|l| l.iter_mut().find(|x| x.id == d.guild_scheduled_event_id)) {
                    let n = e.user_count.unwrap_or(0);
                    e.user_count = Some(if add { n + 1 } else { n.saturating_sub(1) });
                }
                if Some(d.user_id) == self.my_id() {
                    if add { self.event_interest.insert(d.guild_scheduled_event_id); } else { self.event_interest.remove(&d.guild_scheduled_event_id); }
                }
                vec![Delta::GuildEvents { guild: d.guild_id }]
            }
            "STAGE_INSTANCE_CREATE" | "STAGE_INSTANCE_UPDATE" => {
                let s: crate::guild_events::StageInstance = de!();
                let g = s.guild_id;
                self.stages.insert(s.channel_id, s);
                vec![Delta::VoiceStates { guild: g }]
            }
            "STAGE_INSTANCE_DELETE" => {
                let s: crate::guild_events::StageInstance = de!();
                self.stages.remove(&s.channel_id);
                vec![Delta::VoiceStates { guild: s.guild_id }]
            }
            "CALL_CREATE" | "CALL_UPDATE" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    channel_id: Id,
                    message_id: Option<Id>,
                    voice_states: Vec<VoiceState>,
                    /// Keyed by user; the values are not needed.
                    ongoing_rings: Option<HashMap<Id, serde::de::IgnoredAny>>,
                    ringing: Option<Vec<Id>>,
                }
                let d: D = de!();
                if name == "CALL_CREATE" {
                    let states = self.dm_voice.entry(d.channel_id).or_default();
                    states.clear();
                    for mut v in d.voice_states {
                        v.channel_id = Some(d.channel_id);
                        states.insert(v.user_id, v);
                    }
                    self.dm_voice.retain(|_, m| !m.is_empty());
                }
                // CALL_UPDATE carries no voice states (the web client ignores them there); it only
                // moves the ring list, so participants are left to VOICE_STATE_UPDATE.
                let mut ringing: Vec<Id> = match (d.ongoing_rings, d.ringing) {
                    (Some(m), _) => m.into_keys().collect(),
                    (None, Some(v)) => v,
                    (None, None) => vec![],
                };
                ringing.sort();
                let me = self.my_id();
                let was_ringing_me = me.is_some_and(|m| self.dm_calls.get(&d.channel_id).is_some_and(|c| c.ringing.contains(&m)));
                let answered: Vec<Id> = self.dm_voice.get(&d.channel_id).map(|m| m.keys().copied().collect()).unwrap_or_default();
                ringing.retain(|u| !answered.contains(u));
                let ringing_me = me.is_some_and(|m| ringing.contains(&m));
                self.dm_calls.insert(d.channel_id, DmCall { ringing, message_id: d.message_id });
                let mut out = vec![Delta::Voice];
                if ringing_me && !was_ringing_me {
                    out.push(Delta::IncomingCall { channel: d.channel_id });
                }
                out
            }
            "CALL_DELETE" => {
                #[derive(Deserialize, Default)]
                #[serde(default)]
                struct D {
                    channel_id: Id,
                }
                let d: D = de!();
                self.dm_voice.remove(&d.channel_id);
                self.dm_calls.remove(&d.channel_id);
                vec![Delta::Voice]
            }
            "VOICE_SERVER_UPDATE" | "VOICE_CHANNEL_STATUS_UPDATE" | "VOICE_CHANNEL_START_TIME_UPDATE" => vec![],
            "CONVERSATION_SUMMARY_UPDATE" | "PASSIVE_UPDATE_V2" | "RESUMED" => vec![],
            // Real dispatch events we deliberately do not model: nothing on screen depends on them
            // (integrations, bans and invites are read through REST when the admin panels open; the
            // member list comes from GUILD_MEMBER_LIST_UPDATE; webhooks, the audit log and command
            // counts are read on demand). Accepting them keeps About's "not modelled yet" list for
            // events that are genuinely new.
            "GUILD_INTEGRATIONS_UPDATE"
            | "INTEGRATION_CREATE"
            | "INTEGRATION_UPDATE"
            | "INTEGRATION_DELETE"
            | "GUILD_BAN_ADD"
            | "GUILD_BAN_REMOVE"
            | "GUILD_MEMBER_REMOVE"
            | "GUILD_AUDIT_LOG_ENTRY_CREATE"
            | "WEBHOOKS_UPDATE"
            | "INVITE_CREATE"
            | "INVITE_DELETE"
            | "GUILD_APPLICATION_COMMAND_COUNTS_UPDATE"
            | "APPLICATION_COMMAND_PERMISSIONS_UPDATE"
            | "CHANNEL_UNREAD_UPDATE"
            | "CHANNEL_PINS_ACK"
            | "PRESENCES_REPLACE"
            | "GUILD_SOUNDBOARD_SOUND_CREATE"
            | "GUILD_SOUNDBOARD_SOUND_UPDATE"
            | "GUILD_SOUNDBOARD_SOUND_DELETE"
            | "GUILD_SOUNDBOARD_SOUNDS_UPDATE" => vec![],
            other => {
                *self.unknown_events.entry(other.to_string()).or_default() += 1;
                vec![]
            }
        })
    }

    fn on_ready(&mut self, r: Ready) -> Vec<Delta> {
        let fresh_history = std::mem::take(&mut self.history);
        let voice = std::mem::take(&mut self.voice);
        *self = Store { history: fresh_history, voice, ..Default::default() };
        // Fallback until the saved setting (below) says otherwise.
        self.my_status = crate::model::effective_status(&r.sessions).unwrap_or_else(|| "online".into());
        for u in r.users {
            self.users.insert(u.id, u);
        }
        self.users.insert(r.user.id, r.user.clone());
        self.me = Some(r.user);
        self.gateway_session = Some(r.session_id).filter(|s| !s.is_empty());
        self.guild_ready_order = r.guilds.iter().map(|g| g.id).collect();
        for (i, g) in r.guilds.into_iter().enumerate() {
            let mine = r.merged_members.get(i).and_then(|v| v.first()).cloned();
            self.upsert_guild(g, mine);
        }
        for mut c in r.private_channels {
            for u in c.recipients.drain(..) {
                if !c.recipient_ids.contains(&u.id) {
                    c.recipient_ids.push(u.id);
                }
                self.users.insert(u.id, u);
            }
            self.upsert_channel(c);
        }
        for rs in r.read_state.entries {
            self.read_states.insert(rs.id, rs);
        }
        for s in r.user_guild_settings.entries {
            match s.guild_id {
                Some(g) => {
                    self.guild_settings.insert(g, s);
                }
                None => self.dm_settings = Some(s),
            }
        }
        for rel in &r.relationships {
            if let Some(u) = &rel.user {
                self.users.insert(u.id, u.clone());
            }
        }
        self.relationships = r.relationships;
        self.rebuild_friend_nicks();
        // Friends' presences, when READY carries them (otherwise they arrive in READY_SUPPLEMENTAL).
        self.absorb_merged_presences(&r.merged_presences);
        self.absorb_presence_list(&r.presences);
        // The account's saved status (settings proto) wins over what sessions report at connect.
        if let Some(p) = r.user_settings_proto {
            self.absorb_settings_proto(&p);
        }
        vec![Delta::Ready, Delta::GuildsChanged, Delta::PrivateChannelsChanged]
    }

    /// Remember a status the user just picked (see `pending_status`).
    pub fn set_my_status_local(&mut self, st: &str) {
        self.my_status = st.to_string();
        self.pending_status = Some((st.to_string(), std::time::Instant::now()));
    }

    /// Whether a status reported by the server should replace ours.
    fn accept_status(&mut self, st: &str) -> bool {
        if let Some((want, at)) = &self.pending_status {
            if st == want {
                self.pending_status = None;
            } else if at.elapsed() < std::time::Duration::from_secs(8) {
                return false;
            } else {
                self.pending_status = None;
            }
        }
        st != self.my_status
    }

    /// Apply a settings-proto blob. Returns true when it changed the user's own status.
    pub fn absorb_settings_proto(&mut self, b64: &str) -> bool {
        use base64::Engine as _;
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { return false };
        if let Some(p) = crate::prefs::Prefs::decode(&bytes) {
            self.prefs.merge(&p);
        }
        if let Some((folders, positions)) = settings_proto::guild_folders(&bytes) {
            if !self.folders_unsaved {
                self.folders = folders;
                self.guild_positions = positions;
            }
        }
        match settings_proto::status(&bytes) {
            Some(st) if is_valid_status(&st) && self.accept_status(&st) => {
                self.my_status = st;
                true
            }
            _ => false,
        }
    }

    /// Record another user's status. Returns true when it changed.
    fn set_presence(&mut self, user: Id, status: &str) -> bool {
        if Some(user) == self.my_id() || user.0 == 0 {
            return false;
        }
        let new = match status {
            "online" => Some("online"),
            "idle" => Some("idle"),
            "dnd" => Some("dnd"),
            // Offline (and invisible, which others see as offline) are simply absent.
            _ => None,
        };
        let old = match new {
            Some(st) => self.presences.insert(user, st),
            None => self.presences.remove(&user),
        };
        old != new
    }

    /// Record what another user is doing. Returns true when it changed.
    fn set_activities(&mut self, user: Id, list: Vec<crate::activity::Activity>) -> bool {
        if Some(user) == self.my_id() || user.0 == 0 {
            return false;
        }
        if list.is_empty() {
            return self.activities.remove(&user).is_some();
        }
        self.activities.insert(user, list.clone()) != Some(list)
    }

    /// Activities of a user (empty when unknown or offline).
    pub fn user_activities(&self, user: Id) -> &[crate::activity::Activity] {
        self.activities.get(&user).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Status of another user as `online`, `idle`, `dnd` or `offline`.
    pub fn presence(&self, user: Id) -> &'static str {
        self.presences.get(&user).copied().unwrap_or("offline")
    }

    fn is_dm_recipient(&self, user: Id) -> bool {
        self.private_channels
            .iter()
            .filter_map(|c| self.channels.get(c))
            .any(|c| c.kind == channel_type::DM && c.recipient_ids.contains(&user))
    }

    /// `{friends: [presence], guilds: [[presence]]}` as sent in READY_SUPPLEMENTAL.
    fn absorb_merged_presences(&mut self, v: &serde_json::Value) {
        self.absorb_presence_list(&v["friends"]);
        if let Some(guilds) = v["guilds"].as_array() {
            for g in guilds {
                self.absorb_presence_list(g);
            }
        }
    }

    /// A list of `{user_id, status}` (or `{user: {id}, status}`) entries; anything else is ignored.
    fn absorb_presence_list(&mut self, v: &serde_json::Value) {
        let Some(list) = v.as_array() else { return };
        for p in list {
            let id = [&p["user_id"], &p["user"]["id"]].into_iter().find_map(|x| match x {
                serde_json::Value::String(s) => s.parse().ok(),
                serde_json::Value::Number(n) => n.as_u64(),
                _ => None,
            });
            if let (Some(id), Some(status)) = (id, p["status"].as_str()) {
                self.set_presence(Snowflake(id), status);
                self.set_activities(Snowflake(id), crate::activity::Activity::parse_list(&p["activities"]));
            }
        }
    }

    /// Record a DM's mute locally (the server confirms with `USER_GUILD_SETTINGS_UPDATE`).
    pub fn set_dm_muted(&mut self, channel: Id, muted: bool) -> Vec<Delta> {
        let s = self.dm_settings.get_or_insert_with(Default::default);
        match s.channel_overrides.iter_mut().find(|o| o.channel_id == channel) {
            Some(o) => o.muted = muted,
            None => s.channel_overrides.push(crate::model::ChannelOverride { channel_id: channel, muted, ..Default::default() }),
        }
        vec![Delta::PrivateChannelsChanged]
    }

    /// Drop a DM from the list (user closed it, left the group, or `CHANNEL_DELETE`).
    pub fn remove_private_channel(&mut self, id: Id) -> Vec<Delta> {
        if self.channels.get(&id).is_some_and(Channel::is_private) {
            self.remove_channel(id);
            vec![Delta::PrivateChannelsChanged]
        } else {
            vec![]
        }
    }

    pub fn absorb_frecency_proto(&mut self, b64: &str) {
        use base64::Engine as _;
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { return };
        if let Some(p) = settings_proto::emoji_prefs(&bytes) {
            self.emoji_prefs = Some(p);
        }
    }

    fn absorb_merged_members(&mut self, merged: &[Vec<Member>]) {
        let order = self.guild_ready_order.clone();
        for (gid, members) in order.iter().zip(merged) {
            for m in members {
                self.absorb_member(*gid, m.clone());
            }
        }
    }

    fn absorb_member(&mut self, guild: Id, mut m: Member) {
        if let Some(u) = m.user.take() {
            m.user_id = Some(u.id);
            self.users.insert(u.id, u);
        }
        if let Some(id) = m.user_id {
            self.members.insert((guild, id), m);
        }
    }

    fn upsert_guild(&mut self, mut g: Guild, mine: Option<Member>) {
        let id = g.id;
        if !g.voice_states.is_empty() || !g.channels.is_empty() {
            let states = self.voice_states.entry(id).or_default();
            states.clear();
            for mut v in std::mem::take(&mut g.voice_states).into_iter().filter(|v| v.channel_id.is_some()) {
                v.guild_id = Some(id);
                states.insert(v.user_id, v);
            }
        }
        if !g.guild_scheduled_events.is_empty() || !g.channels.is_empty() {
            let mut evs = std::mem::take(&mut g.guild_scheduled_events);
            evs.iter_mut().for_each(|e| e.guild_id = id);
            self.scheduled_events.insert(id, evs);
            self.stages.retain(|_, s| s.guild_id != id);
            for mut s in std::mem::take(&mut g.stage_instances) {
                s.guild_id = id;
                self.stages.insert(s.channel_id, s);
            }
        }
        for mut c in std::mem::take(&mut g.channels).into_iter().chain(std::mem::take(&mut g.threads)) {
            c.guild_id = Some(id);
            self.upsert_channel(c);
        }
        let roles = self.roles.entry(id).or_default();
        for r in std::mem::take(&mut g.roles) {
            roles.insert(r.id, r);
        }
        let me = self.my_id();
        for m in std::mem::take(&mut g.members) {
            if m.id() == me {
                self.my_members.insert(id, m.clone());
            }
            self.absorb_member(id, m);
        }
        if let Some(mut m) = mine {
            m.user_id = m.user_id.or(me);
            self.my_members.insert(id, m.clone());
            self.absorb_member(id, m);
        }
        // GUILD_UPDATE carries properties only; keep the previous ones when absent.
        if let Some(prev) = self.guilds.get(&id) {
            if g.properties.is_none() && g.inline.name.is_none() {
                g.properties = prev.properties.clone();
                g.inline = prev.inline.clone();
            }
        }
        if !self.guild_ready_order.contains(&id) {
            self.guild_ready_order.push(id);
        }
        self.guilds.insert(id, g);
        self.resort_guild_channels(id);
    }

    fn remove_guild(&mut self, id: Id) {
        self.guilds.remove(&id);
        self.guild_ready_order.retain(|g| *g != id);
        if let Some(chs) = self.guild_channels.remove(&id) {
            for c in chs {
                self.channels.remove(&c);
                self.history.remove(&c);
            }
        }
        self.roles.remove(&id);
        self.my_members.remove(&id);
    }

    fn upsert_channel(&mut self, c: Channel) {
        let id = c.id;
        let guild = c.guild_id;
        let private = c.is_private();
        self.channels.insert(id, c);
        match guild {
            Some(g) => {
                let list = self.guild_channels.entry(g).or_default();
                if !list.contains(&id) {
                    list.push(id);
                }
                self.resort_guild_channels(g);
            }
            None if private => {
                if !self.private_channels.contains(&id) {
                    self.private_channels.push(id);
                }
                self.resort_private();
            }
            None => {}
        }
    }

    fn remove_channel(&mut self, id: Id) {
        if let Some(c) = self.channels.remove(&id) {
            if let Some(g) = c.guild_id {
                if let Some(l) = self.guild_channels.get_mut(&g) {
                    l.retain(|x| *x != id);
                }
            } else {
                self.private_channels.retain(|x| *x != id);
            }
        }
        self.history.remove(&id);
    }

    fn resort_guild_channels(&mut self, guild: Id) {
        let channels = &self.channels;
        if let Some(list) = self.guild_channels.get_mut(&guild) {
            // Discord order: categories by position; within each, text-like before voice, by position then id.
            list.sort_by_key(|id| {
                let c = &channels[id];
                let voice = matches!(c.kind, channel_type::GUILD_VOICE | channel_type::GUILD_STAGE_VOICE);
                (voice, c.position, c.id)
            });
        }
    }

    /// Message requests waiting for an answer: spam-filtered ones when `spam`, else the rest. Newest first.
    pub fn message_requests(&self, spam: bool) -> Vec<Id> {
        self.private_channels
            .iter()
            .filter(|id| self.channels.get(id).is_some_and(|c| c.is_message_request && c.is_spam == spam))
            .copied()
            .collect()
    }

    /// Whether a DM is a pending message request (kept out of the normal DM list).
    pub fn is_message_request(&self, c: &Channel) -> bool {
        c.is_message_request
    }

    /// Mark a request accepted locally; the server confirms with `CHANNEL_UPDATE`.
    pub fn accept_message_request(&mut self, channel: Id) -> Vec<Delta> {
        match self.channels.get_mut(&channel) {
            Some(c) if c.is_message_request => {
                c.is_message_request = false;
                c.is_spam = false;
                vec![Delta::PrivateChannelsChanged]
            }
            _ => vec![],
        }
    }

    fn resort_private(&mut self) {
        let channels = &self.channels;
        self.private_channels
            .sort_by_key(|id| std::cmp::Reverse(channels.get(id).and_then(|c| c.last_message_id).unwrap_or(*id)));
    }

    fn on_message_create(&mut self, m: Message) -> Vec<Delta> {
        let channel = m.channel_id;
        let id = m.id;
        let author = m.author.id;
        let mut deltas = Vec::with_capacity(3);
        let mut forum_parent = None;
        if let Some(c) = self.channels.get_mut(&channel) {
            c.last_message_id = Some(id);
            if c.is_thread() {
                // The post's own first message has the thread's id; the rest are replies.
                if id != channel {
                    c.message_count = Some(c.message_count.unwrap_or(0) + 1);
                    c.total_message_sent = Some(c.total_message_sent.unwrap_or(0) + 1);
                }
                forum_parent = c.parent_id;
            }
        }
        if id == channel {
            self.previews.first.insert(channel, Some(m.clone()));
        }
        if let Some(d) = self.forum_delta(forum_parent) {
            deltas.push(d);
        }
        if let (Some(g), Some(member)) = (m.guild_id, m.member.clone()) {
            let mut member = member;
            member.user_id = Some(m.author.id);
            self.members.insert((g, m.author.id), member);
        }
        self.users.insert(m.author.id, m.author.clone());
        for u in &m.mentions {
            self.users.entry(u.id).or_insert_with(|| u.clone());
        }
        let mentions_me = self.mentions_me(&m);
        let direct_mention = self.my_id().is_some_and(|me| m.mentions.iter().any(|u| u.id == me));
        let from_me = Some(m.author.id) == self.my_id();
        let keyword_hit = !from_me && !mentions_me && crate::comfort::keyword_hit(&m.content, &self.notify_keywords);
        if mentions_me {
            self.recent_mentions.push_front(m.clone());
            self.recent_mentions.truncate(100);
        }

        if let Some(h) = self.history.get_mut(&channel) {
            if h.at_latest {
                // Replace a pending local echo with the same nonce.
                let nonce = m.nonce.as_ref().and_then(|n| n.as_str().map(str::to_string).or_else(|| n.as_u64().map(|v| v.to_string())));
                if let Some(n) = nonce {
                    h.messages.retain(|x| !(x.id.0 == 0 && x.nonce.as_ref().and_then(|v| v.as_str()) == Some(n.as_str())));
                }
                h.messages.push_back(m);
                while h.messages.len() > MAX_MESSAGES_PER_CHANNEL {
                    h.messages.pop_front();
                    h.has_more_before = true;
                }
                deltas.push(Delta::MessageInserted { channel, message: id });
            }
        }
        if from_me {
            let rs = self.read_states.entry(channel).or_insert_with(|| ReadStateEntry { id: channel, ..Default::default() });
            rs.last_message_id = Some(id);
            rs.mention_count = 0;
        } else if mentions_me {
            let rs = self.read_states.entry(channel).or_insert_with(|| ReadStateEntry { id: channel, ..Default::default() });
            rs.mention_count += 1;
            if self.should_notify(channel, id, &direct_mention) {
                deltas.push(Delta::Mentioned { channel, message: id });
            }
        } else if keyword_hit {
            // A keyword raises a banner but is not a mention: no badge, and the usual mute rules apply.
            if self.should_notify(channel, id, &false) {
                deltas.push(Delta::Mentioned { channel, message: id });
            }
        }
        deltas.push(Delta::ReadState { channel });
        if self.channels.get(&channel).is_some_and(Channel::is_private) {
            self.resort_private();
            deltas.push(Delta::PrivateChannelsChanged);
        }
        if let Some(t) = self.typing.get_mut(&channel) {
            let before = t.len();
            t.retain(|(u, _)| *u != author);
            if t.len() != before {
                deltas.push(Delta::Typing { channel });
            }
        }
        deltas
    }

    /// Whether a message that counts as a mention should raise a notification: muted channels and
    /// categories stay quiet, a muted server only lets a direct @mention through, and a
    /// "nothing" level (per channel, else per server) silences everything.
    fn should_notify(&self, channel: Id, _message: Id, direct_mention: &bool) -> bool {
        let Some(c) = self.channels.get(&channel) else { return true };
        if self.is_channel_muted(c) {
            return false;
        }
        let Some(g) = c.guild_id else { return true };
        let settings = self.guild_settings.get(&g);
        if settings.is_some_and(|s| s.muted) && !direct_mention {
            return false;
        }
        let level = settings
            .and_then(|s| {
                s.channel_overrides
                    .iter()
                    .find(|o| o.channel_id == channel || c.parent_id == Some(o.channel_id))
                    .map(|o| o.message_notifications)
                    .filter(|l| *l != 3)
                    .or(Some(s.message_notifications))
            })
            .unwrap_or(0);
        level != 2
    }

    /// Whether the person holds `role` in `guild`.
    pub fn has_role(&self, guild: Id, role: Id) -> bool {
        self.my_members.get(&guild).is_some_and(|m| m.roles.contains(&role))
    }

    /// Whether this message pings the person: names them, @everyone/@here, or a role they hold
    /// (each unless the server's settings suppress it). The one rule behind the row highlight, the
    /// inbox, the mention badge and notifications. Own messages never ping.
    pub fn pings_me(&self, m: &Message) -> bool {
        let Some(me) = self.my_id() else { return false };
        if m.author.id == me {
            return false;
        }
        if m.mentions.iter().any(|u| u.id == me) {
            return true;
        }
        let Some(g) = m.guild_id else { return false };
        let settings = self.guild_settings.get(&g);
        if m.mention_everyone && !settings.is_some_and(|s| s.suppress_everyone) {
            return true;
        }
        !settings.is_some_and(|s| s.suppress_roles) && m.mention_roles.iter().any(|r| self.has_role(g, *r))
    }

    /// What counts toward the mention badge and the inbox: a ping, or any message in a DM.
    fn mentions_me(&self, m: &Message) -> bool {
        let Some(me) = self.my_id() else { return false };
        if m.author.id == me {
            return false;
        }
        self.pings_me(m) || self.channels.get(&m.channel_id).is_some_and(Channel::is_private)
    }

    fn on_message_update(&mut self, p: MessagePatch) -> Vec<Delta> {
        let mut forum = None;
        if p.id == p.channel_id {
            if let Some(Some(first)) = self.previews.first.get_mut(&p.id) {
                if let Some(c) = &p.content {
                    first.content = c.clone();
                }
                if let Some(a) = &p.attachments {
                    first.attachments = a.clone();
                }
                if let Some(e) = &p.embeds {
                    first.embeds = e.clone();
                }
                forum = self.forum_delta(self.channels.get(&p.id).and_then(|c| c.parent_id));
            }
        }
        let Some(m) = self.message_mut(p.channel_id, p.id) else { return forum.into_iter().collect() };
        if let Some(c) = p.content {
            m.content = c;
        }
        if p.edited_timestamp.is_some() {
            m.edited_timestamp = p.edited_timestamp;
        }
        if let Some(a) = p.attachments {
            m.attachments = a;
        }
        if let Some(e) = p.embeds {
            m.embeds = e;
        }
        if let Some(pinned) = p.pinned {
            m.pinned = pinned;
        }
        if let Some(f) = p.flags {
            m.flags = f;
        }
        if let Some(c) = p.components {
            m.components = c;
        }
        if let Some(poll) = p.poll {
            m.poll = Some(poll);
        }
        if let Some(mn) = p.mentions {
            m.mentions = mn;
        }
        vec![Delta::MessageUpdated { channel: p.channel_id, message: p.id }]
    }

    fn remove_message(&mut self, channel: Id, id: Id) -> Vec<Delta> {
        if id == channel && self.previews.first.contains_key(&id) {
            self.previews.first.insert(id, None);
        }
        match self.history.get_mut(&channel) {
            Some(h) => {
                let before = h.messages.len();
                h.messages.retain(|m| m.id != id);
                if h.messages.len() != before {
                    vec![Delta::MessageDeleted { channel, message: id }]
                } else {
                    vec![]
                }
            }
            None => vec![],
        }
    }

    /// The signed-in user's own reaction, shown before Discord confirms it (and taken back if the request
    /// fails): `emoji` is a unicode emoji or `name:id`. Nothing changes, and no delta, when the message
    /// already shows that state. The gateway's echo of the same reaction is then ignored by `on_reaction`.
    pub fn apply_own_reaction(&mut self, channel: Id, message: Id, emoji: &str, add: bool) -> Vec<Delta> {
        let Some(user_id) = self.my_id() else { return vec![] };
        let emoji = match emoji.split_once(':') {
            Some((name, id)) => Emoji { id: id.parse().ok(), name: Some(name.to_string()), ..Default::default() },
            None => Emoji { name: Some(emoji.to_string()), ..Default::default() },
        };
        self.on_reaction(ReactionEvent { user_id, channel_id: channel, message_id: message, emoji, burst: false }, add)
    }

    fn on_reaction(&mut self, r: ReactionEvent, add: bool) -> Vec<Delta> {
        let me = self.my_id();
        let Some(m) = self.message_mut(r.channel_id, r.message_id) else { return vec![] };
        let mine = Some(r.user_id) == me;
        match m.reactions.iter_mut().position(|x| same_emoji(&x.emoji, &r.emoji)) {
            Some(i) => {
                let x = &mut m.reactions[i];
                // Already shown (our own change, applied before the gateway's echo of it arrived).
                if mine && !r.burst && x.me == add {
                    return vec![];
                }
                if add {
                    x.count += 1;
                    if r.burst {
                        x.burst_count += 1;
                    }
                    x.me |= mine && !r.burst;
                    x.me_burst |= mine && r.burst;
                } else {
                    x.count = x.count.saturating_sub(1);
                    if r.burst {
                        x.burst_count = x.burst_count.saturating_sub(1);
                    }
                    if mine {
                        if r.burst { x.me_burst = false } else { x.me = false }
                    }
                    if x.count == 0 {
                        m.reactions.remove(i);
                    }
                }
            }
            None if add => m.reactions.push(Reaction {
                emoji: r.emoji,
                count: 1,
                me: mine && !r.burst,
                burst_count: r.burst as u32,
                me_burst: mine && r.burst,
            }),
            None => return vec![],
        }
        vec![Delta::MessageUpdated { channel: r.channel_id, message: r.message_id }]
    }

    fn on_member_list(&mut self, u: MemberListUpdate) {
        let state = self.member_lists.entry(u.guild_id).or_default();
        state.member_count = u.member_count;
        state.online_count = u.online_count;
        if !u.groups.is_empty() {
            state.group_counts = u.groups.iter().map(|g| (g.id.clone(), g.count)).collect();
        }
        for op in u.ops {
            match op.op.as_str() {
                "SYNC" => {
                    if let Some((start, _)) = op.range {
                        let start = start as usize;
                        if state.items.len() < start + op.items.len() {
                            state.items.resize(start + op.items.len(), MemberListItem::default());
                        }
                        for (i, item) in op.items.into_iter().enumerate() {
                            state.items[start + i] = item;
                        }
                    }
                }
                "INSERT" => {
                    if let (Some(i), Some(item)) = (op.index, op.item) {
                        let i = (i as usize).min(state.items.len());
                        state.items.insert(i, item);
                    }
                }
                "UPDATE" => {
                    if let (Some(i), Some(item)) = (op.index, op.item) {
                        if let Some(slot) = state.items.get_mut(i as usize) {
                            *slot = item;
                        }
                    }
                }
                "DELETE" => {
                    if let Some(i) = op.index {
                        if (i as usize) < state.items.len() {
                            state.items.remove(i as usize);
                        }
                    }
                }
                "INVALIDATE" => {
                    if let Some((a, b)) = op.range {
                        for slot in state.items.iter_mut().skip(a as usize).take((b - a + 1) as usize) {
                            *slot = MemberListItem::default();
                        }
                    }
                }
                _ => {}
            }
        }
        let guild = u.guild_id;
        let items: Vec<_> = state.items.iter().filter_map(|i| i.member.as_ref().map(|m| m.member.clone())).collect();
        for m in items {
            self.absorb_member(guild, m);
        }
    }

    /// A message from open history or the recent-mentions buffer.
    pub fn find_message(&self, channel: Id, id: Id) -> Option<&Message> {
        self.history
            .get(&channel)
            .and_then(|h| h.messages.iter().find(|m| m.id == id))
            .or_else(|| self.recent_mentions.iter().find(|m| m.id == id))
    }

    fn message_mut(&mut self, channel: Id, id: Id) -> Option<&mut Message> {
        self.history.get_mut(&channel)?.messages.iter_mut().find(|m| m.id == id)
    }

    // ----- History windows (filled by the session from REST/cache) -----

    pub fn set_history_latest(&mut self, channel: Id, mut newest_first: Vec<Message>, full_page: bool) {
        newest_first.reverse();
        for m in &newest_first {
            self.users.insert(m.author.id, m.author.clone());
            for u in &m.mentions {
                self.users.entry(u.id).or_insert_with(|| u.clone());
            }
        }
        self.touch_history(channel);
        let h = self.history.entry(channel).or_default();
        h.messages = newest_first.into();
        h.has_more_before = full_page;
        h.at_latest = true;
    }

    /// A detached window (jump-to): not at the latest message, older pages still available.
    pub fn set_history_window(&mut self, channel: Id, mut newest_first: Vec<Message>) {
        newest_first.reverse();
        for m in &newest_first {
            self.users.insert(m.author.id, m.author.clone());
            for u in &m.mentions {
                self.users.entry(u.id).or_insert_with(|| u.clone());
            }
        }
        self.touch_history(channel);
        let h = self.history.entry(channel).or_default();
        h.messages = newest_first.into();
        h.has_more_before = true;
        h.at_latest = false;
        // The page around a recent message reaches the channel's newest one: it is the newest page.
        self.promote_if_latest(channel);
    }

    /// The newest message the channel is known to have (READY, the gateway, a fetched page).
    pub fn latest_known(&self, channel: Id) -> Option<Id> {
        self.channels.get(&channel).and_then(|c| c.last_message_id)
    }

    /// A window whose newest message is the channel's newest one *is* the newest page: later
    /// messages append to it, and the shell has nothing to jump to.
    pub fn promote_if_latest(&mut self, channel: Id) {
        let latest = self.latest_known(channel);
        let Some(h) = self.history.get_mut(&channel) else { return };
        if let (Some(l), Some(newest)) = (latest, h.messages.back().filter(|m| m.id.0 != 0)) {
            if newest.id >= l {
                h.at_latest = true;
            }
        }
    }

    /// Time between the newest loaded message and the channel's newest one, 0 for a window at the
    /// latest page or when either is unknown (input of `jump_present`).
    pub fn window_tail_ms(&self, channel: Id) -> u64 {
        let Some(h) = self.history.get(&channel).filter(|h| !h.at_latest) else { return 0 };
        let (Some(l), Some(n)) = (self.latest_known(channel), h.messages.back()) else { return 0 };
        l.timestamp_ms().saturating_sub(n.id.timestamp_ms())
    }

    /// A page of messages newer than the window's newest (`newest_first`, as REST returns them).
    /// A short page means the channel's newest message was reached: the window becomes the
    /// newest page, which the gateway then keeps current.
    pub fn append_history_newer(&mut self, channel: Id, mut newest_first: Vec<Message>, full_page: bool) {
        newest_first.sort_by_key(|m| m.id);
        for m in &newest_first {
            self.users.insert(m.author.id, m.author.clone());
            for u in &m.mentions {
                self.users.entry(u.id).or_insert_with(|| u.clone());
            }
        }
        let h = self.history.entry(channel).or_default();
        let newest = h.messages.back().map(|m| m.id).unwrap_or(Snowflake(0));
        for m in newest_first {
            if m.id > newest && !h.messages.iter().any(|x| x.id == m.id) {
                h.messages.push_back(m);
            }
        }
        while h.messages.len() > MAX_MESSAGES_PER_CHANNEL {
            h.messages.pop_front();
            h.has_more_before = true;
        }
        if !full_page {
            h.at_latest = true;
        }
        self.promote_if_latest(channel);
    }

    pub fn prepend_history(&mut self, channel: Id, newest_first: Vec<Message>, full_page: bool) {
        for m in &newest_first {
            self.users.insert(m.author.id, m.author.clone());
            for u in &m.mentions {
                self.users.entry(u.id).or_insert_with(|| u.clone());
            }
        }
        let h = self.history.entry(channel).or_default();
        for m in newest_first {
            if !h.messages.iter().any(|x| x.id == m.id) {
                h.messages.push_front(m);
            }
        }
        h.has_more_before = full_page;
    }

    /// Insert a pending local echo (id 0) that MESSAGE_CREATE will replace by nonce.
    pub fn push_pending(&mut self, channel: Id, content: String, nonce: Snowflake) {
        let Some(me) = self.me.clone() else { return };
        let h = self.history.entry(channel).or_default();
        h.messages.push_back(Message {
            id: Snowflake(0),
            channel_id: channel,
            author: me,
            content,
            timestamp: String::new(),
            nonce: Some(serde_json::Value::String(nonce.to_string())),
            ..Default::default()
        });
    }

    pub fn drop_pending(&mut self, channel: Id, nonce: Snowflake) {
        if let Some(h) = self.history.get_mut(&channel) {
            let n = nonce.to_string();
            h.messages.retain(|m| !(m.id.0 == 0 && m.nonce.as_ref().and_then(|v| v.as_str()) == Some(n.as_str())));
        }
    }

    /// Keep only the newest `keep` messages of a channel the user has left, so scrolling deep into
    /// one channel's history doesn't stay resident. A detached (jumped-to) window is dropped
    /// entirely; the next open loads the latest page again.
    pub fn trim_history(&mut self, channel: Id, keep: usize) {
        let Some(h) = self.history.get_mut(&channel) else { return };
        if !h.at_latest {
            self.history.remove(&channel);
            return;
        }
        let n = h.messages.len();
        if n > keep {
            h.messages.drain(..n - keep);
            h.has_more_before = true;
        }
    }

    fn touch_history(&mut self, channel: Id) {
        self.history_lru.retain(|c| *c != channel);
        self.history_lru.push_back(channel);
        while self.history_lru.len() > MAX_CHANNELS_WITH_HISTORY {
            if let Some(old) = self.history_lru.pop_front() {
                self.history.remove(&old);
            }
        }
    }

    // ----- Forums -----

    /// `ForumChanged` for `parent` if it is a forum or media channel.
    fn forum_delta(&self, parent: Option<Id>) -> Option<Delta> {
        let p = parent?;
        self.channels.get(&p).filter(|c| c.is_forum_like()).map(|_| Delta::ForumChanged { channel: p })
    }

    /// The current user's thread members (`{id: thread, user_id, join_timestamp, flags}`).
    fn absorb_thread_members(&mut self, members: &[serde_json::Value]) {
        let me = self.my_id().map(|m| m.to_string());
        for m in members {
            let mine = m["user_id"].as_str().is_none_or(|u| Some(u) == me.as_deref());
            let thread: Option<Id> = m["id"].as_str().and_then(|s| s.parse().ok());
            if let (true, Some(t)) = (mine, thread) {
                if let Some(c) = self.channels.get_mut(&t) {
                    c.member = Some(m.clone());
                }
            }
        }
    }

    pub fn forum_list(&self, forum: Id, query: &ForumQuery) -> Option<&forum::ArchivedList> {
        self.forums.get(&forum).and_then(|f| f.archived.get(query))
    }

    pub fn forum_list_mut(&mut self, forum: Id, query: &ForumQuery) -> &mut forum::ArchivedList {
        self.forums.entry(forum).or_default().archived.entry(query.clone()).or_default()
    }

    /// Post ids the forum shows for `query`, top to bottom.
    pub fn forum_posts(&self, forum: Id, query: &ForumQuery) -> Vec<Id> {
        let ids = self.guild_channels.get(&self.channels.get(&forum).and_then(|c| c.guild_id).unwrap_or_default());
        let channels = ids.into_iter().flatten().filter_map(|id| self.channels.get(id));
        forum::ordered_posts(forum, channels, self.forum_list(forum, query), query, |id| self.channels.get(&id))
    }

    /// One `threads/search` page for `query`, starting at `offset`.
    pub fn absorb_thread_search(&mut self, forum: Id, query: &ForumQuery, offset: u32, page: forum::ThreadSearch) -> Vec<Delta> {
        let guild = self.channels.get(&forum).and_then(|c| c.guild_id);
        let returned = page.threads.len() as u32;
        let mut ids = Vec::with_capacity(page.threads.len());
        for mut t in page.threads {
            t.guild_id = t.guild_id.or(guild);
            if t.member.is_none() {
                t.member = self.channels.get(&t.id).and_then(|p| p.member.clone());
            }
            ids.push(t.id);
            self.upsert_channel(t);
        }
        self.absorb_thread_members(&page.members);
        for m in page.first_messages {
            self.users.entry(m.author.id).or_insert_with(|| m.author.clone());
            self.previews.first.insert(m.channel_id, Some(m));
        }
        // Posts the search returned without a first message don't have one any more.
        for id in &ids {
            self.previews.first.entry(*id).or_insert(None);
        }
        let list = self.forum_list_mut(forum, query);
        if offset == 0 {
            list.ids.clear();
        }
        for id in ids {
            if !list.ids.contains(&id) {
                list.ids.push(id);
            }
        }
        list.next_offset = offset + returned;
        list.has_more = page.has_more;
        list.loading = false;
        list.loaded = true;
        vec![Delta::ForumChanged { channel: forum }, Delta::ChannelsChanged { guild }]
    }

    /// A post the user just created (`threads?use_nested_fields=true` response).
    pub fn insert_created_post(&mut self, mut post: Channel, first: Option<Message>) -> Vec<Delta> {
        let parent = post.parent_id;
        if post.member.is_none() {
            // The author joins their own post.
            post.member = Some(serde_json::json!({ "id": post.id.to_string() }));
        }
        let guild = post.guild_id;
        let id = post.id;
        self.upsert_channel(post);
        if let Some(m) = first {
            self.previews.first.insert(id, Some(m));
        }
        let mut deltas = vec![Delta::ChannelsChanged { guild }];
        deltas.extend(self.forum_delta(parent));
        deltas
    }

    /// A `post-data` response: first messages for the posts that were asked about.
    pub fn absorb_post_data(&mut self, forum: Id, asked: &[Id], data: forum::PostData) -> Vec<Delta> {
        for (id, entry) in data.threads {
            let Ok(id) = id.parse::<Id>() else { continue };
            if let Some(m) = &entry.first_message {
                self.users.entry(m.author.id).or_insert_with(|| m.author.clone());
            }
            self.previews.first.insert(id, entry.first_message);
        }
        for id in asked {
            self.previews.requested.remove(id);
            self.previews.first.entry(*id).or_insert(None);
        }
        vec![Delta::ForumChanged { channel: forum }]
    }

    // ----- Voice -----

    /// Voice states for one channel, sorted by display name like the sidebar.
    /// Open scheduled events of a guild, live first.
    pub fn guild_events(&self, guild: Id) -> Vec<&crate::guild_events::ScheduledEvent> {
        self.scheduled_events.get(&guild).map(|l| crate::guild_events::sorted(l)).unwrap_or_default()
    }

    /// Who speaks and who listens in a stage channel: (speakers, raised hands, audience).
    pub fn stage_roles(&self, guild: Id, channel: Id) -> (Vec<&VoiceState>, Vec<&VoiceState>, Vec<&VoiceState>) {
        let (mut speakers, mut hands, mut audience) = (vec![], vec![], vec![]);
        for v in self.voice_members(guild, channel) {
            if !v.suppress {
                speakers.push(v);
            } else if v.request_to_speak_timestamp.is_some() {
                hands.push(v);
            } else {
                audience.push(v);
            }
        }
        hands.sort_by(|a, b| a.request_to_speak_timestamp.cmp(&b.request_to_speak_timestamp));
        (speakers, hands, audience)
    }

    pub fn voice_members(&self, guild: Id, channel: Id) -> Vec<&VoiceState> {
        let mut v: Vec<&VoiceState> =
            self.voice_states.get(&guild).into_iter().flat_map(|m| m.values()).filter(|s| s.channel_id == Some(channel)).collect();
        v.sort_by_key(|s| self.display_name(Some(guild), s.user_id).to_lowercase());
        v
    }

    // ----- DM calls -----

    /// People in a DM's call, empty when there is none.
    pub fn dm_call_participants(&self, channel: Id) -> Vec<Id> {
        let mut v: Vec<Id> = self.dm_voice.get(&channel).map(|m| m.keys().copied().collect()).unwrap_or_default();
        v.sort();
        v
    }

    /// We are in a 1:1 DM call with nobody else in it: the one case where the official client
    /// leaves by itself after `session::CALL_IDLE_LEAVE` (its `CallIdleManager`). Servers and group
    /// DMs never qualify.
    pub fn alone_in_dm_call(&self) -> bool {
        let (Some(ch), Some(me)) = (self.voice.channel, self.my_id()) else { return false };
        self.channels.get(&ch).is_some_and(|c| c.kind == channel_type::DM)
            && self.dm_voice.get(&ch).is_none_or(|m| m.keys().all(|u| *u == me))
    }

    /// A call is ongoing while at least one person is in it, whether or not we are.
    pub fn dm_call_ongoing(&self, channel: Id) -> bool {
        self.dm_voice.get(&channel).is_some_and(|m| !m.is_empty())
    }

    /// Users still being rung in a DM call.
    pub fn dm_call_ringing(&self, channel: Id) -> Vec<Id> {
        self.dm_calls.get(&channel).map(|c| c.ringing.clone()).unwrap_or_default()
    }

    /// DMs whose call is ringing us and that we have not joined.
    pub fn incoming_calls(&self) -> Vec<Id> {
        let Some(me) = self.my_id() else { return vec![] };
        let mut v: Vec<Id> = self
            .dm_calls
            .iter()
            .filter(|(c, call)| call.ringing.contains(&me) && self.voice.channel != Some(**c) && self.dm_call_ongoing(**c))
            .map(|(c, _)| *c)
            .collect();
        v.sort();
        v
    }

    /// We declined or stopped the ring locally; the server confirms with `CALL_UPDATE`.
    pub fn stop_ringing_local(&mut self, channel: Id, user: Id) -> Vec<Delta> {
        if let Some(c) = self.dm_calls.get_mut(&channel) {
            c.ringing.retain(|u| *u != user);
        }
        vec![Delta::Voice]
    }

    // ----- Queries -----

    /// The rail as folder-layout entries (a server outside any folder is a one-server entry), which is
    /// what folder edits start from and write back.
    pub fn folder_layout(&self) -> Vec<GuildFolder> {
        self.rail()
            .into_iter()
            .map(|e| match e {
                RailEntry::Guild(g) => GuildFolder { id: None, name: None, color: None, guild_ids: vec![g] },
                RailEntry::Folder(f) => f,
            })
            .collect()
    }

    pub fn rail(&self) -> Vec<RailEntry> {
        let known: HashSet<Id> = self.guilds.keys().copied().collect();
        let mut placed = HashSet::new();
        let mut out = Vec::new();
        if !self.folders.is_empty() {
            for f in &self.folders {
                let ids: Vec<Id> = f.guild_ids.iter().copied().filter(|g| known.contains(g)).collect();
                if ids.is_empty() {
                    continue;
                }
                placed.extend(ids.iter().copied());
                if f.id.is_none() && ids.len() == 1 {
                    out.push(RailEntry::Guild(ids[0]));
                } else {
                    out.push(RailEntry::Folder(GuildFolder { guild_ids: ids, ..f.clone() }));
                }
            }
        } else {
            for g in self.guild_positions.iter().filter(|g| known.contains(g)) {
                placed.insert(*g);
                out.push(RailEntry::Guild(*g));
            }
        }
        // Guilds not in the saved layout go on top, newest join first (Discord's behaviour).
        let mut rest: Vec<&Guild> = self.guilds.values().filter(|g| !placed.contains(&g.id)).collect();
        rest.sort_by(|a, b| b.joined_at.cmp(&a.joined_at));
        let mut top: Vec<RailEntry> = rest.into_iter().map(|g| RailEntry::Guild(g.id)).collect();
        top.extend(out);
        top
    }

    /// My server-wide permissions before channel overwrites (`ALL` for the owner and administrators).
    pub fn guild_permissions(&self, g: Id) -> u64 {
        let Some(guild) = self.guilds.get(&g) else { return 0 };
        let Some(me) = self.my_id() else { return 0 };
        if guild.props().owner_id == Some(me) {
            return perm::ALL;
        }
        let roles = self.roles.get(&g);
        let parse = |s: &str| s.parse::<u64>().unwrap_or(0);
        let mut base = roles.and_then(|r| r.get(&g)).map_or(0, |r| parse(&r.permissions));
        for rid in self.my_members.get(&g).map(|m| m.roles.as_slice()).unwrap_or_default() {
            if let Some(r) = roles.and_then(|r| r.get(rid)) {
                base |= parse(&r.permissions);
            }
        }
        if base & perm::ADMINISTRATOR != 0 { perm::ALL } else { base }
    }

    pub fn permissions(&self, channel: &Channel) -> u64 {
        let Some(g) = channel.guild_id else { return perm::ALL };
        let Some(me) = self.my_id() else { return 0 };
        let base = self.guild_permissions(g);
        if base == perm::ALL {
            return perm::ALL;
        }
        let parse = |s: &str| s.parse::<u64>().unwrap_or(0);
        let my_roles: Vec<Id> = self.my_members.get(&g).map(|m| m.roles.clone()).unwrap_or_default();
        // Threads inherit from their parent channel.
        let source = if channel.is_thread() { channel.parent_id.and_then(|p| self.channels.get(&p)).unwrap_or(channel) } else { channel };
        let ow = &source.permission_overwrites;
        let mut p = base;
        if let Some(e) = ow.iter().find(|o| o.id == g) {
            p = (p & !parse(&e.deny)) | parse(&e.allow);
        }
        let (mut allow, mut deny) = (0u64, 0u64);
        for o in ow.iter().filter(|o| o.kind == 0 && my_roles.contains(&o.id)) {
            allow |= parse(&o.allow);
            deny |= parse(&o.deny);
        }
        p = (p & !deny) | allow;
        if let Some(o) = ow.iter().find(|o| o.kind == 1 && o.id == me) {
            p = (p & !parse(&o.deny)) | parse(&o.allow);
        }
        if channel.is_thread() {
            p = thread_permissions(channel, p);
        }
        p
    }

    /// Admit a thread the server told us about outside the gateway (`GET /channels/{id}` for a
    /// thread this account has not joined), as the official client's `THREAD_CREATE` does: it
    /// sits in the channel store under its parent, without a thread member. Nothing is joined.
    /// `Err` says why it cannot be shown: not a thread, its server or parent is not loaded, or
    /// this account may not see it (a private thread it is not in).
    pub fn admit_fetched_thread(&mut self, mut c: Channel, hint_guild: Option<Id>) -> std::result::Result<(), crate::open_target::Missing> {
        use crate::open_target::Missing;
        if !c.is_thread() {
            return Err(Missing::Unknown);
        }
        let Some(guild) = c.guild_id.or(hint_guild) else { return Err(Missing::Unknown) };
        c.guild_id = Some(guild);
        let parent = c.parent_id.and_then(|p| self.channels.get(&p));
        if !self.guilds.contains_key(&guild) || parent.is_none() {
            return Err(Missing::Unknown);
        }
        if let Some(known) = self.channels.get(&c.id) {
            // Never replace what the gateway told us (it carries our thread member).
            c.member = known.member.clone();
        }
        if !self.can_view(&c) {
            return Err(Missing::Hidden);
        }
        self.upsert_channel(c);
        Ok(())
    }

    pub fn thread_join(&self, channel: &Channel) -> ThreadJoin {
        match (channel.is_thread(), channel.member.is_some(), channel.is_archived()) {
            (false, ..) => ThreadJoin::NotThread,
            (_, true, _) => ThreadJoin::Joined,
            (_, false, true) => ThreadJoin::Archived,
            (_, false, false) => ThreadJoin::CanJoin,
        }
    }

    pub fn can_view(&self, channel: &Channel) -> bool {
        self.permissions(channel) & perm::VIEW_CHANNEL != 0
    }

    pub fn is_unread(&self, channel: &Channel) -> bool {
        let Some(last) = channel.last_message_id else { return false };
        match self.read_states.get(&channel.id) {
            Some(rs) => rs.last_message_id.is_none_or(|r| last > r),
            None => false,
        }
    }

    /// Whether a thread is listed in the sidebar under its parent. A joined, unarchived thread shows
    /// only while it has something for you (unread messages or a mention), is the one you have open,
    /// or saw a message in the last `THREAD_SIDEBAR_WINDOW_MS`; an old, quiet one lives in the
    /// Threads popout instead, as in the official client. Not-joined posts of a forum never list.
    pub fn thread_in_sidebar(&self, thread: &Channel, now_ms: u64, open: Option<Id>) -> bool {
        if !thread.is_thread() || thread.is_archived() {
            return false;
        }
        if open == Some(thread.id) {
            return true;
        }
        if thread.member.is_none() {
            return false;
        }
        let recent = thread
            .last_message_id
            .is_some_and(|m| now_ms.saturating_sub(m.timestamp_ms()) < THREAD_SIDEBAR_WINDOW_MS);
        recent || self.is_unread(thread) || self.mention_count(thread.id) > 0
    }

    /// Whether the header's Pins popup has something in it. Discord stamps a channel with the time of
    /// its latest pin (READY, `CHANNEL_PINS_UPDATE`), so no request is needed. Unpinning the last
    /// message does not clear the stamp, so this can say yes for a channel whose pins were all removed.
    pub fn has_pins(&self, channel: Id) -> bool {
        self.channels.get(&channel).is_some_and(|c| c.last_pin_timestamp.is_some())
    }

    /// Whether the header's Threads popup lists anything: a known, unarchived thread under `parent`.
    pub fn has_active_threads(&self, parent: Id) -> bool {
        self.channels.values().any(|c| c.parent_id == Some(parent) && c.is_thread() && !c.is_archived())
    }

    pub fn mention_count(&self, channel: Id) -> u32 {
        self.read_states.get(&channel).map_or(0, |r| r.mention_count)
    }

    pub fn is_channel_muted(&self, channel: &Channel) -> bool {
        let settings = match channel.guild_id {
            Some(g) => self.guild_settings.get(&g),
            None => self.dm_settings.as_ref(),
        };
        let Some(s) = settings else { return false };
        let target = channel.parent_id.filter(|_| channel.is_thread()).unwrap_or(channel.id);
        s.channel_overrides.iter().any(|o| (o.channel_id == channel.id || o.channel_id == target) && o.muted)
            || channel.parent_id.is_some_and(|p| s.channel_overrides.iter().any(|o| o.channel_id == p && o.muted))
    }

    pub fn is_guild_muted(&self, guild: Id) -> bool {
        self.guild_settings.get(&guild).is_some_and(|s| s.muted)
    }

    fn rebuild_friend_nicks(&mut self) {
        self.friend_nicks = self
            .relationships
            .iter()
            .filter(|r| r.kind == 1)
            .filter_map(|r| {
                let n = r.nickname.as_deref().map(str::trim).filter(|n| !crate::model::is_blank_name(n))?;
                Some((r.user_id.or(r.user.as_ref().map(|u| u.id)).unwrap_or(r.id), n.to_string()))
            })
            .collect();
    }

    /// Set or clear a friend's nickname in the local copy (the gateway confirms it later).
    pub fn set_friend_nick_local(&mut self, user: Id, nick: Option<String>) -> Vec<Delta> {
        let Some(r) = self.relationships.iter_mut().find(|r| r.kind == 1 && r.user_id.or(r.user.as_ref().map(|u| u.id)).unwrap_or(r.id) == user) else {
            return vec![];
        };
        r.nickname = nick;
        self.rebuild_friend_nicks();
        vec![Delta::Relationships]
    }

    /// The name you see for someone. In DMs and group DMs (no guild) your nickname for a friend
    /// comes first; in a server their server nickname does and a friend nickname is not used
    /// (web build 655747: `nick: friendNickname ?? name` only when the channel has no guild).
    /// Either way the fallback is their global name, then username.
    pub fn display_name(&self, guild: Option<Id>, user: Id) -> String {
        if guild.is_none() {
            if let Some(n) = self.friend_nicks.get(&user) {
                return n.clone();
            }
        }
        if let Some(g) = guild {
            if let Some(n) = self.members.get(&(g, user)).and_then(|m| m.nick.clone()).filter(|n| !crate::model::is_blank_name(n)) {
                return n;
            }
        }
        self.users.get(&user).map(|u| u.display_name().to_string()).unwrap_or_else(|| user.to_string())
    }

    /// People this session knows who answer to `typed`, best first: the name you gave a friend,
    /// their server nicknames, global name and username all count, fuzzily (prefix, word start,
    /// typo). One rule for `from:`/`mentions:` resolution and its autocomplete, so a name you can
    /// see anywhere finds the person everywhere.
    pub fn users_matching(&self, typed: &str) -> Vec<(Id, i32)> {
        let q = crate::find::fold(typed.trim_start_matches('@'));
        if q.is_empty() {
            return Vec::new();
        }
        let mut best: HashMap<Id, i32> = HashMap::new();
        let mut note = |id: Id, name: &str| {
            if let Some(sc) = crate::find::match_score(&q, &crate::find::fold(name)) {
                let e = best.entry(id).or_insert(i32::MIN);
                *e = (*e).max(sc);
            }
        };
        for (id, n) in &self.friend_nicks {
            note(*id, n);
        }
        for u in self.users.values().filter(|u| !u.bot) {
            note(u.id, u.display_name());
            note(u.id, &u.username);
        }
        for ((_, id), m) in &self.members {
            if let Some(n) = m.nick.as_deref().filter(|n| !crate::model::is_blank_name(n)) {
                if self.users.get(id).is_some_and(|u| !u.bot) {
                    note(*id, n);
                }
            }
        }
        let mut v: Vec<(Id, i32)> = best.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Highest coloured role colour for a member, like the official name colour.
    pub fn name_color(&self, guild: Option<Id>, user: Id) -> Option<u32> {
        let g = guild?;
        let member = self.members.get(&(g, user))?;
        let roles = self.roles.get(&g)?;
        member.roles.iter().filter_map(|r| roles.get(r)).filter(|r| r.color != 0).max_by_key(|r| r.position).map(|r| r.color)
    }

    pub fn typing_users(&self, channel: Id, now_secs: u64) -> Vec<Id> {
        self.typing
            .get(&channel)
            .map(|v| v.iter().filter(|(_, t)| now_secs.saturating_sub(*t) < 10).map(|(u, _)| *u).collect())
            .unwrap_or_default()
    }

    pub fn guild_has_unread(&self, guild: Id) -> bool {
        if self.is_guild_muted(guild) {
            return false;
        }
        self.guild_channels.get(&guild).is_some_and(|chs| {
            chs.iter().filter_map(|c| self.channels.get(c)).any(|c| {
                c.has_text_chat() && !c.is_thread() && !self.is_channel_muted(c) && self.can_view(c) && self.is_unread(c)
            })
        })
    }

    pub fn guild_mentions(&self, guild: Id) -> u32 {
        self.guild_channels.get(&guild).map_or(0, |chs| chs.iter().map(|c| self.mention_count(*c)).sum())
    }
}

fn same_emoji(a: &Emoji, b: &Emoji) -> bool {
    match (a.id, b.id) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.name == b.name,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod window_tests;
