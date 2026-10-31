//! Lenient entity types.
//!
//! Every field is optional or defaulted so a new or missing field from Discord
//! never fails decoding. Unknown enum values are kept as raw numbers.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::snowflake::Snowflake;

pub type Id = Snowflake;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: Id,
    pub username: String,
    pub global_name: Option<String>,
    pub discriminator: Option<String>,
    pub avatar: Option<String>,
    pub bot: bool,
    pub system: bool,
    pub public_flags: u64,
    pub avatar_decoration_data: Option<Value>,
    /// Only present on the current user (READY): shown read-only in settings.
    pub email: Option<String>,
    pub phone: Option<String>,
    pub verified: bool,
    pub mfa_enabled: bool,
    /// Only present on the current user (READY): 0 none, 1 Classic, 2 Nitro, 3 Basic.
    pub premium_type: Option<u8>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().filter(|s| !is_blank_name(s)).unwrap_or(&self.username)
    }

    pub fn avatar_url(&self, size: u32) -> String {
        match &self.avatar {
            Some(h) => {
                let ext = if h.starts_with("a_") { "gif" } else { "webp" };
                format!("https://cdn.discordapp.com/avatars/{}/{}.{}?size={}", self.id, h, ext, size)
            }
            None => {
                let idx = (self.id.0 >> 22) % 6;
                format!("https://cdn.discordapp.com/embed/avatars/{idx}.png")
            }
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Member {
    pub user: Option<User>,
    /// Present instead of `user` in deduplicated READY payloads.
    pub user_id: Option<Id>,
    pub nick: Option<String>,
    pub avatar: Option<String>,
    pub roles: Vec<Id>,
    pub joined_at: Option<String>,
    pub communication_disabled_until: Option<String>,
    pub flags: u64,
    /// Has not passed the server's membership screening yet.
    pub pending: bool,
}

impl Member {
    pub fn id(&self) -> Option<Id> {
        self.user.as_ref().map(|u| u.id).or(self.user_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: Id,
    pub name: String,
    pub color: u32,
    pub position: i32,
    pub permissions: String,
    pub hoist: bool,
    pub mentionable: bool,
    pub icon: Option<String>,
    pub unicode_emoji: Option<String>,
    /// Owned by an integration (a bot's role, a booster role): can't be given or taken by hand.
    pub managed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionOverwrite {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: u8,
    pub allow: String,
    pub deny: String,
}

pub mod channel_type {
    pub const GUILD_TEXT: u8 = 0;
    pub const DM: u8 = 1;
    pub const GUILD_VOICE: u8 = 2;
    pub const GROUP_DM: u8 = 3;
    pub const GUILD_CATEGORY: u8 = 4;
    pub const GUILD_ANNOUNCEMENT: u8 = 5;
    pub const ANNOUNCEMENT_THREAD: u8 = 10;
    pub const PUBLIC_THREAD: u8 = 11;
    pub const PRIVATE_THREAD: u8 = 12;
    pub const GUILD_STAGE_VOICE: u8 = 13;
    pub const GUILD_DIRECTORY: u8 = 14;
    pub const GUILD_FORUM: u8 = 15;
    pub const GUILD_MEDIA: u8 = 16;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: u8,
    pub guild_id: Option<Id>,
    pub name: Option<String>,
    pub position: i32,
    pub parent_id: Option<Id>,
    pub topic: Option<String>,
    pub nsfw: bool,
    pub last_message_id: Option<Id>,
    pub last_pin_timestamp: Option<String>,
    pub recipient_ids: Vec<Id>,
    pub recipients: Vec<User>,
    pub permission_overwrites: Vec<PermissionOverwrite>,
    pub icon: Option<String>,
    pub owner_id: Option<Id>,
    pub flags: u64,
    pub rate_limit_per_user: u32,
    pub thread_metadata: Option<Value>,
    pub member_count: Option<u32>,
    pub message_count: Option<u32>,
    pub available_tags: Vec<ForumTag>,
    /// Forum posts: the parent's tag ids applied to this post.
    pub applied_tags: Vec<Id>,
    pub default_auto_archive_duration: Option<u32>,
    /// Forums: 0 = latest activity, 1 = creation date.
    pub default_sort_order: Option<u8>,
    /// Forums: `match_some` or `match_all`.
    pub default_tag_setting: Option<String>,
    /// Threads: the current user's thread member, present once they've joined.
    pub member: Option<Value>,
    pub total_message_sent: Option<u32>,
    /// Voice channels: bits per second, and the member cap (0 = none).
    pub bitrate: Option<u32>,
    pub user_limit: Option<u32>,
    /// DMs: a stranger's first message, held until accepted (`is_message_request`).
    pub is_message_request: bool,
    /// DMs: a message request the spam filter flagged (`is_spam`).
    pub is_spam: bool,
}

/// A forum or media channel tag (`available_tags`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ForumTag {
    pub id: Id,
    pub name: String,
    pub moderated: bool,
    pub emoji_id: Option<Id>,
    pub emoji_name: Option<String>,
}

pub mod channel_flags {
    /// A forum post pinned to the top of its forum.
    pub const PINNED: u64 = 1 << 1;
    /// Forum or media channels that require a tag on new posts.
    pub const REQUIRE_TAG: u64 = 1 << 4;
}

impl Channel {
    pub fn is_thread(&self) -> bool {
        matches!(self.kind, channel_type::ANNOUNCEMENT_THREAD | channel_type::PUBLIC_THREAD | channel_type::PRIVATE_THREAD)
    }
    pub fn is_private(&self) -> bool {
        matches!(self.kind, channel_type::DM | channel_type::GROUP_DM)
    }
    pub fn is_forum_like(&self) -> bool {
        matches!(self.kind, channel_type::GUILD_FORUM | channel_type::GUILD_MEDIA)
    }
    pub fn is_archived(&self) -> bool {
        self.thread_metadata.as_ref().is_some_and(|m| m["archived"].as_bool() == Some(true))
    }
    pub fn is_locked(&self) -> bool {
        self.thread_metadata.as_ref().is_some_and(|m| m["locked"].as_bool() == Some(true))
    }
    pub fn is_text_like(&self) -> bool {
        matches!(
            self.kind,
            channel_type::GUILD_TEXT | channel_type::GUILD_ANNOUNCEMENT | channel_type::DM | channel_type::GROUP_DM
        ) || self.is_thread()
    }
}

/// Who is in which voice channel (GUILD_CREATE `voice_states`, VOICE_STATE_UPDATE).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceState {
    pub guild_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub user_id: Id,
    pub session_id: String,
    pub member: Option<Member>,
    pub mute: bool,
    pub deaf: bool,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub self_video: bool,
    pub self_stream: Option<bool>,
    pub suppress: bool,
    /// Stage channels: when the user asked to speak (ISO timestamp).
    pub request_to_speak_timestamp: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: Option<Id>,
    pub name: Option<String>,
    pub animated: bool,
    pub available: Option<bool>,
}

/// A custom sticker of a guild (GUILD_CREATE `stickers`, GUILD_STICKERS_UPDATE).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildSticker {
    pub id: Id,
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<String>,
    pub format_type: u8,
    pub available: Option<bool>,
}

/// Guild properties: top-level in GUILD_CREATE v1, nested under `properties` with CLIENT_STATE_V2.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildProperties {
    // `Guild` flattens these next to its own `id`; writing a second `id` made the cached store
    // undecodable, so launch waited for READY instead of painting from cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub owner_id: Option<Id>,
    pub description: Option<String>,
    pub features: Vec<String>,
    pub premium_tier: u8,
    pub system_channel_id: Option<Id>,
    pub rules_channel_id: Option<Id>,
    pub afk_channel_id: Option<Id>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Guild {
    pub id: Id,
    pub properties: Option<GuildProperties>,
    #[serde(flatten)]
    pub inline: GuildProperties,
    pub channels: Vec<Channel>,
    pub threads: Vec<Channel>,
    pub roles: Vec<Role>,
    pub emojis: Vec<Emoji>,
    pub stickers: Vec<GuildSticker>,
    pub members: Vec<Member>,
    pub member_count: Option<u32>,
    pub joined_at: Option<String>,
    pub voice_states: Vec<VoiceState>,
    pub guild_scheduled_events: Vec<crate::guild_events::ScheduledEvent>,
    pub stage_instances: Vec<crate::guild_events::StageInstance>,
    pub large: bool,
    pub unavailable: bool,
    pub lazy: bool,
    pub data_mode: Option<String>,
}

impl Guild {
    pub fn props(&self) -> &GuildProperties {
        self.properties.as_ref().unwrap_or(&self.inline)
    }
    pub fn name(&self) -> &str {
        self.props().name.as_deref().unwrap_or("")
    }
    pub fn icon_url(&self, size: u32) -> Option<String> {
        self.props().icon.as_ref().map(|h| {
            let ext = if h.starts_with("a_") { "gif" } else { "webp" };
            format!("https://cdn.discordapp.com/icons/{}/{}.{}?size={}", self.id, h, ext, size)
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub id: Id,
    pub filename: String,
    pub size: u64,
    pub url: String,
    pub proxy_url: String,
    pub content_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub description: Option<String>,
    pub duration_secs: Option<f64>,
    pub waveform: Option<String>,
    pub flags: u64,
    pub placeholder: Option<String>,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.content_type.as_deref().is_some_and(|c| c.starts_with("image/"))
    }
    pub fn is_video(&self) -> bool {
        self.content_type.as_deref().is_some_and(|c| c.starts_with("video/"))
    }
    pub fn is_spoiler(&self) -> bool {
        self.filename.starts_with("SPOILER_") || self.flags & (1 << 3) != 0
    }
    /// `CONTAINS_EXPLICIT_MEDIA` (1 << 4), set by Discord's content scan. `capture`: bit from the API docs.
    pub fn is_explicit(&self) -> bool {
        self.flags & (1 << 4) != 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedMedia {
    pub url: Option<String>,
    pub proxy_url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedText {
    pub name: Option<String>,
    pub text: Option<String>,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub proxy_icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Embed {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub timestamp: Option<String>,
    pub author: Option<EmbedText>,
    pub footer: Option<EmbedText>,
    pub provider: Option<EmbedText>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub fields: Vec<EmbedField>,
    /// `CONTAINS_EXPLICIT_MEDIA` is bit 4, as on attachments. `capture`: bit from the API docs.
    pub flags: u64,
}

impl Embed {
    pub fn is_explicit(&self) -> bool {
        self.flags & (1 << 4) != 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub emoji: Emoji,
    pub count: u32,
    pub me: bool,
    pub burst_count: u32,
    pub me_burst: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageReference {
    #[serde(rename = "type")]
    pub kind: u8,
    pub message_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub guild_id: Option<Id>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StickerItem {
    pub id: Id,
    pub name: String,
    pub format_type: u8,
}

pub mod message_type {
    pub const DEFAULT: u8 = 0;
    pub const RECIPIENT_ADD: u8 = 1;
    pub const RECIPIENT_REMOVE: u8 = 2;
    pub const CALL: u8 = 3;
    pub const CHANNEL_NAME_CHANGE: u8 = 4;
    pub const CHANNEL_ICON_CHANGE: u8 = 5;
    pub const CHANNEL_PINNED_MESSAGE: u8 = 6;
    pub const USER_JOIN: u8 = 7;
    pub const GUILD_BOOST: u8 = 8;
    pub const THREAD_CREATED: u8 = 18;
    pub const REPLY: u8 = 19;
    pub const CHAT_INPUT_COMMAND: u8 = 20;
    pub const THREAD_STARTER_MESSAGE: u8 = 21;
    pub const CONTEXT_MENU_COMMAND: u8 = 23;
    /// Types rendered natively; anything else gets the generic "open in web view" card.
    pub fn is_supported(t: u8) -> bool {
        matches!(t, 0..=8 | 18..=21 | 23)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: Id,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub author: User,
    pub member: Option<Member>,
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    pub tts: bool,
    pub mention_everyone: bool,
    pub mentions: Vec<User>,
    pub mention_roles: Vec<Id>,
    pub attachments: Vec<Attachment>,
    pub embeds: Vec<Embed>,
    pub reactions: Vec<Reaction>,
    pub pinned: bool,
    #[serde(rename = "type")]
    pub kind: u8,
    pub flags: u64,
    pub message_reference: Option<MessageReference>,
    pub referenced_message: Option<Box<Message>>,
    pub sticker_items: Vec<StickerItem>,
    pub components: Vec<Value>,
    pub poll: Option<Value>,
    /// Forwarded content (`message_reference.type == 1`): `[{message: {...}}]`.
    pub message_snapshots: Vec<Value>,
    /// Set on messages sent by an application (components POST this as `application_id`).
    pub application_id: Option<Id>,
    pub thread: Option<Channel>,
    pub nonce: Option<Value>,
    pub webhook_id: Option<Id>,
}

/// Partial update (MESSAGE_UPDATE may omit fields).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessagePatch {
    pub id: Id,
    pub channel_id: Id,
    pub content: Option<String>,
    pub edited_timestamp: Option<String>,
    pub attachments: Option<Vec<Attachment>>,
    pub embeds: Option<Vec<Embed>>,
    pub pinned: Option<bool>,
    pub flags: Option<u64>,
    pub components: Option<Vec<Value>>,
    pub poll: Option<Value>,
    pub mentions: Option<Vec<User>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadStateEntry {
    pub id: Id,
    pub last_message_id: Option<Id>,
    pub mention_count: u32,
    pub last_pin_timestamp: Option<String>,
    pub read_state_type: u8,
    pub flags: u64,
    pub last_viewed: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Versioned<T> {
    pub entries: Vec<T>,
    pub partial: bool,
    pub version: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelOverride {
    pub channel_id: Id,
    pub muted: bool,
    pub message_notifications: u8,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserGuildSettings {
    pub guild_id: Option<Id>,
    pub muted: bool,
    pub message_notifications: u8,
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
    pub channel_overrides: Vec<ChannelOverride>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Relationship {
    pub id: Id,
    #[serde(rename = "type")]
    pub kind: u8,
    pub user_id: Option<Id>,
    pub user: Option<User>,
    pub nickname: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ready {
    pub v: u8,
    pub user: User,
    pub users: Vec<User>,
    pub guilds: Vec<Guild>,
    pub private_channels: Vec<Channel>,
    pub relationships: Vec<Relationship>,
    pub read_state: Versioned<ReadStateEntry>,
    pub user_guild_settings: Versioned<UserGuildSettings>,
    pub merged_members: Vec<Vec<Member>>,
    pub session_id: String,
    pub resume_gateway_url: Option<String>,
    pub user_settings_proto: Option<String>,
    pub auth_token: Option<String>,
    pub qos_token: Option<String>,
    pub country_code: Option<String>,
    pub sessions: Vec<SessionInfo>,
    /// Friends' presences, when READY includes them (shape is read leniently).
    pub merged_presences: Value,
    pub presences: Value,
}

/// One of the user's gateway sessions; the `all` session carries the effective status.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SessionInfo {
    pub session_id: String,
    pub status: String,
}

/// The effective status across sessions: the `all` aggregate if present, else the first.
pub fn effective_status(sessions: &[SessionInfo]) -> Option<String> {
    sessions.iter().find(|s| s.session_id == "all").or(sessions.first()).map(|s| s.status.clone()).filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReadySupplemental {
    pub merged_presences: Value,
    pub merged_members: Vec<Vec<Member>>,
    pub guilds: Vec<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TypingStart {
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub user_id: Id,
    pub timestamp: u64,
    pub member: Option<Member>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReactionEvent {
    pub user_id: Id,
    pub channel_id: Id,
    pub message_id: Id,
    pub emoji: Emoji,
    pub burst: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessageDelete {
    pub id: Id,
    pub channel_id: Id,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MessageAck {
    pub channel_id: Id,
    pub message_id: Id,
    pub mention_count: Option<u32>,
}

/// Op 37/14 member-list sync (GUILD_MEMBER_LIST_UPDATE).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemberListUpdate {
    pub guild_id: Id,
    pub id: String,
    pub member_count: u32,
    pub online_count: u32,
    pub groups: Vec<MemberListGroup>,
    pub ops: Vec<MemberListOp>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemberListGroup {
    #[serde(deserialize_with = "string_or_int")]
    pub id: String,
    pub count: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemberListOp {
    pub op: String,
    pub range: Option<(u32, u32)>,
    pub index: Option<u32>,
    pub items: Vec<MemberListItem>,
    pub item: Option<MemberListItem>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MemberListItem {
    pub group: Option<MemberListGroup2>,
    pub member: Option<MemberWithPresence>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MemberListGroup2 {
    #[serde(deserialize_with = "string_or_int")]
    pub id: String,
    pub count: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MemberWithPresence {
    #[serde(flatten)]
    pub member: Member,
    pub presence: Option<Presence>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Presence {
    pub status: String,
    pub activities: Vec<Value>,
    pub client_status: HashMap<String, String>,
}

/// Guild folder layout from `user_settings_proto` (PreloadedUserSettings.guild_folders = 14).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GuildFolder {
    pub id: Option<i64>,
    pub name: Option<String>,
    pub color: Option<u64>,
    pub guild_ids: Vec<Id>,
}

pub mod settings_proto {
    //! Just enough protobuf to read guild folder order out of PreloadedUserSettings.

    use super::{GuildFolder, Id};
    use crate::snowflake::Snowflake;

    struct Reader<'a> {
        b: &'a [u8],
        p: usize,
    }

    impl<'a> Reader<'a> {
        fn varint(&mut self) -> Option<u64> {
            let mut v = 0u64;
            for shift in (0..64).step_by(7) {
                let byte = *self.b.get(self.p)?;
                self.p += 1;
                v |= ((byte & 0x7f) as u64) << shift;
                if byte & 0x80 == 0 {
                    return Some(v);
                }
            }
            None
        }
        fn bytes(&mut self) -> Option<&'a [u8]> {
            let n = self.varint()? as usize;
            let s = self.b.get(self.p..self.p + n)?;
            self.p += n;
            Some(s)
        }
        fn fixed64(&mut self) -> Option<u64> {
            let s = self.b.get(self.p..self.p + 8)?;
            self.p += 8;
            Some(u64::from_le_bytes(s.try_into().ok()?))
        }
        fn skip(&mut self, wire: u64) -> Option<()> {
            match wire {
                0 => self.varint().map(|_| ()),
                1 => self.fixed64().map(|_| ()),
                2 => self.bytes().map(|_| ()),
                5 => {
                    self.p += 4;
                    Some(())
                }
                _ => None,
            }
        }
        fn fields(&mut self, mut f: impl FnMut(&mut Self, u64, u64) -> Option<bool>) -> Option<()> {
            while self.p < self.b.len() {
                let tag = self.varint()?;
                let (num, wire) = (tag >> 3, tag & 7);
                if !f(self, num, wire)? {
                    self.skip(wire)?;
                }
            }
            Some(())
        }
    }

    fn wrapper_varint(b: &[u8]) -> Option<u64> {
        let mut r = Reader { b, p: 0 };
        let mut out = None;
        r.fields(|r, n, w| {
            if n == 1 && w == 0 {
                out = Some(r.varint()?);
                Some(true)
            } else {
                Some(false)
            }
        })?;
        out
    }

    fn wrapper_string(b: &[u8]) -> Option<String> {
        let mut r = Reader { b, p: 0 };
        let mut out = None;
        r.fields(|r, n, w| {
            if n == 1 && w == 2 {
                out = r.bytes().map(|s| String::from_utf8_lossy(s).into_owned());
                Some(true)
            } else {
                Some(false)
            }
        })?;
        out
    }

    fn ids(r: &mut Reader, wire: u64, out: &mut Vec<Id>) -> Option<()> {
        if wire == 2 {
            let packed = r.bytes()?;
            for chunk in packed.chunks_exact(8) {
                out.push(Snowflake(u64::from_le_bytes(chunk.try_into().ok()?)));
            }
        } else {
            out.push(Snowflake(r.fixed64()?));
        }
        Some(())
    }

    /// `PreloadedUserSettings.status` (field 11) -> `StatusSettings.status` (1) -> `StringValue.value` (1).
    pub fn status(blob: &[u8]) -> Option<String> {
        let mut out = None;
        let mut top = Reader { b: blob, p: 0 };
        top.fields(|r, n, w| {
            if n != 11 || w != 2 {
                return Some(false);
            }
            let mut st = Reader { b: r.bytes()?, p: 0 };
            st.fields(|r, n, w| {
                if n == 1 && w == 2 {
                    out = wrapper_string(r.bytes()?);
                    Some(true)
                } else {
                    Some(false)
                }
            })?;
            Some(true)
        })?;
        out
    }

    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn put_len_delimited(out: &mut Vec<u8>, field: u64, payload: &[u8]) {
        put_varint(out, field << 3 | 2);
        put_varint(out, payload.len() as u64);
        out.extend_from_slice(payload);
    }

    /// A `PreloadedUserSettings` holding only `status { status { value } }`, for the settings-proto
    /// PATCH: protobuf merge semantics leave every other setting untouched.
    pub fn encode_status(status: &str) -> Vec<u8> {
        let mut string_value = Vec::new();
        put_len_delimited(&mut string_value, 1, status.as_bytes());
        let mut status_settings = Vec::new();
        put_len_delimited(&mut status_settings, 1, &string_value);
        let mut out = Vec::new();
        put_len_delimited(&mut out, 11, &status_settings);
        out
    }

    fn put_fixed64_packed(out: &mut Vec<u8>, field: u64, ids: &[Id]) {
        let mut payload = Vec::with_capacity(ids.len() * 8);
        for id in ids {
            payload.extend_from_slice(&id.0.to_le_bytes());
        }
        put_len_delimited(out, field, &payload);
    }

    /// A `PreloadedUserSettings` holding only `guild_folders { folders }` (field 14), for the
    /// settings-proto PATCH. `capture`: field numbers follow the reader above (`GuildFolder`:
    /// guild_ids 1, id 2 as Int64Value, name 3 as StringValue, color 4 as UInt64Value);
    /// whether the server replaces or appends the repeated field is unverified.
    pub fn encode_folders(folders: &[GuildFolder]) -> Vec<u8> {
        let mut gf = Vec::new();
        for f in folders {
            let mut m = Vec::new();
            put_fixed64_packed(&mut m, 1, &f.guild_ids);
            if let Some(id) = f.id {
                let mut w = Vec::new();
                put_varint(&mut w, 1 << 3);
                put_varint(&mut w, id as u64);
                put_len_delimited(&mut m, 2, &w);
            }
            if let Some(name) = &f.name {
                let mut w = Vec::new();
                put_len_delimited(&mut w, 1, name.as_bytes());
                put_len_delimited(&mut m, 3, &w);
            }
            if let Some(color) = f.color {
                let mut w = Vec::new();
                put_varint(&mut w, 1 << 3);
                put_varint(&mut w, color);
                put_len_delimited(&mut m, 4, &w);
            }
            put_len_delimited(&mut gf, 1, &m);
        }
        let mut out = Vec::new();
        put_len_delimited(&mut out, 14, &gf);
        out
    }

    /// Returns (folders, guild_positions); `None` if the blob is malformed or has no folder data
    /// (a partial update that doesn't touch folders must not clear them).
    pub fn guild_folders(blob: &[u8]) -> Option<(Vec<GuildFolder>, Vec<Id>)> {
        let mut folders = Vec::new();
        let mut positions = Vec::new();
        let mut found = false;
        let mut top = Reader { b: blob, p: 0 };
        top.fields(|r, n, w| {
            if n != 14 || w != 2 {
                return Some(false);
            }
            found = true;
            let mut gf = Reader { b: r.bytes()?, p: 0 };
            gf.fields(|r, n, w| match n {
                1 if w == 2 => {
                    let mut f = Reader { b: r.bytes()?, p: 0 };
                    let mut folder = GuildFolder::default();
                    f.fields(|r, n, w| match n {
                        1 => ids(r, w, &mut folder.guild_ids).map(|_| true),
                        2 if w == 2 => {
                            folder.id = wrapper_varint(r.bytes()?).map(|v| v as i64);
                            Some(true)
                        }
                        3 if w == 2 => {
                            folder.name = wrapper_string(r.bytes()?);
                            Some(true)
                        }
                        4 if w == 2 => {
                            folder.color = wrapper_varint(r.bytes()?);
                            Some(true)
                        }
                        _ => Some(false),
                    })?;
                    folders.push(folder);
                    Some(true)
                }
                2 => ids(r, w, &mut positions).map(|_| true),
                _ => Some(false),
            })?;
            Some(true)
        })?;
        found.then_some((folders, positions))
    }

    /// Favourite and frequently used emoji from FrecencyUserSettings (settings-proto type 2).
    /// Keys are a custom emoji's id or a unicode emoji's Discord name ("thumbsup").
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct EmojiPrefs {
        /// `favorite_emojis` (5), in the user's order.
        pub favorites: Vec<String>,
        /// `emoji_reaction_frecency` (13), most frecent first; what the reaction picker shows.
        /// Falls back to `emoji_frecency` (6) when the account has no reaction history yet.
        pub frequent: Vec<String>,
        /// `favorite_gifs` (2) -> `FavoriteGIFs.gifs` (1), by the user's `order`.
        pub gifs: Vec<FavoriteGif>,
    }

    /// FavoriteGIF {format = 1 (1 image, 2 video), src = 2, width = 3, height = 4, order = 5};
    /// the map key (the GIF page URL) is `url`. Layout from the web client's proto, `capture`.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct FavoriteGif {
        pub url: String,
        pub format: u32,
        pub src: String,
        pub width: u32,
        pub height: u32,
        pub order: u32,
    }

    fn put_field_varint(out: &mut Vec<u8>, field: u64, v: u64) {
        put_varint(out, field << 3);
        put_varint(out, v);
    }

    /// A partial `FrecencyUserSettings` that adds or replaces one favourite GIF; the server merges
    /// maps key by key.
    pub fn encode_favorite_gif(g: &FavoriteGif) -> Vec<u8> {
        let mut gif = Vec::new();
        put_field_varint(&mut gif, 1, g.format as u64);
        put_len_delimited(&mut gif, 2, g.src.as_bytes());
        put_field_varint(&mut gif, 3, g.width as u64);
        put_field_varint(&mut gif, 4, g.height as u64);
        put_field_varint(&mut gif, 5, g.order as u64);
        favorite_gif_patch(&g.url, Some(&gif))
    }

    /// Removal: the key with no value (`capture`: how the web client deletes a map entry).
    pub fn encode_remove_favorite_gif(url: &str) -> Vec<u8> {
        favorite_gif_patch(url, None)
    }

    fn favorite_gif_patch(url: &str, value: Option<&[u8]>) -> Vec<u8> {
        let mut entry = Vec::new();
        put_len_delimited(&mut entry, 1, url.as_bytes());
        if let Some(v) = value {
            put_len_delimited(&mut entry, 2, v);
        }
        let mut gifs = Vec::new();
        put_len_delimited(&mut gifs, 1, &entry);
        let mut top = Vec::new();
        put_len_delimited(&mut top, 2, &gifs);
        top
    }

    fn favorite_gifs(b: &[u8]) -> Option<Vec<FavoriteGif>> {
        let mut out = Vec::new();
        let mut gifs = Reader { b, p: 0 };
        gifs.fields(|r, n, w| {
            if n != 1 || w != 2 {
                return Some(false);
            }
            let mut g = FavoriteGif::default();
            let mut entry = Reader { b: r.bytes()?, p: 0 };
            entry.fields(|r, n, w| match (n, w) {
                (1, 2) => {
                    g.url = String::from_utf8_lossy(r.bytes()?).into_owned();
                    Some(true)
                }
                (2, 2) => {
                    let mut v = Reader { b: r.bytes()?, p: 0 };
                    v.fields(|r, n, w| match (n, w) {
                        (1, 0) => { g.format = r.varint()? as u32; Some(true) }
                        (2, 2) => { g.src = String::from_utf8_lossy(r.bytes()?).into_owned(); Some(true) }
                        (3, 0) => { g.width = r.varint()? as u32; Some(true) }
                        (4, 0) => { g.height = r.varint()? as u32; Some(true) }
                        (5, 0) => { g.order = r.varint()? as u32; Some(true) }
                        _ => Some(false),
                    })?;
                    Some(true)
                }
                _ => Some(false),
            })?;
            if !g.url.is_empty() && !g.src.is_empty() {
                out.push(g);
            }
            Some(true)
        })?;
        out.sort_by_key(|g| g.order);
        Some(out)
    }

    /// FrecencyItem {total_uses = 1, recent_uses = 2, frecency = 3, score = 4}: order by the
    /// stored frecency, or by score and uses where the client hadn't computed it yet (-1).
    fn frecency_map(b: &[u8]) -> Option<Vec<String>> {
        let mut out: Vec<(String, i64, i64, u64)> = Vec::new();
        let mut map = Reader { b, p: 0 };
        map.fields(|r, n, w| {
            if n != 1 || w != 2 {
                return Some(false);
            }
            let mut entry = Reader { b: r.bytes()?, p: 0 };
            let (mut key, mut frecency, mut score, mut uses) = (None, -1i64, 0i64, 0u64);
            entry.fields(|r, n, w| match (n, w) {
                (1, 2) => {
                    key = Some(String::from_utf8_lossy(r.bytes()?).into_owned());
                    Some(true)
                }
                (2, 2) => {
                    let mut item = Reader { b: r.bytes()?, p: 0 };
                    item.fields(|r, n, w| match (n, w) {
                        (1, 0) => {
                            uses = r.varint()?;
                            Some(true)
                        }
                        (3, 0) => {
                            frecency = r.varint()? as i64 as i32 as i64;
                            Some(true)
                        }
                        (4, 0) => {
                            score = r.varint()? as i64 as i32 as i64;
                            Some(true)
                        }
                        _ => Some(false),
                    })?;
                    Some(true)
                }
                _ => Some(false),
            })?;
            if let Some(k) = key {
                out.push((k, frecency, score, uses));
            }
            Some(true)
        })?;
        out.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(b.3.cmp(&a.3)));
        Some(out.into_iter().map(|e| e.0).collect())
    }

    pub fn emoji_prefs(blob: &[u8]) -> Option<EmojiPrefs> {
        let mut prefs = EmojiPrefs::default();
        let mut general = Vec::new();
        let mut top = Reader { b: blob, p: 0 };
        top.fields(|r, n, w| match (n, w) {
            (5, 2) => {
                let mut fav = Reader { b: r.bytes()?, p: 0 };
                fav.fields(|r, n, w| {
                    if n == 1 && w == 2 {
                        prefs.favorites.push(String::from_utf8_lossy(r.bytes()?).into_owned());
                        Some(true)
                    } else {
                        Some(false)
                    }
                })?;
                Some(true)
            }
            (6, 2) => {
                general = frecency_map(r.bytes()?)?;
                Some(true)
            }
            (13, 2) => {
                prefs.frequent = frecency_map(r.bytes()?)?;
                Some(true)
            }
            (2, 2) => {
                prefs.gifs = favorite_gifs(r.bytes()?)?;
                Some(true)
            }
            _ => Some(false),
        })?;
        if prefs.frequent.is_empty() {
            prefs.frequent = general;
        }
        Some(prefs)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn field(num: u8, body: &[u8]) -> Vec<u8> {
            let mut v = vec![(num << 3) | 2, body.len() as u8];
            v.extend_from_slice(body);
            v
        }

        fn frecent(key: &str, frecency: u8, uses: u8) -> Vec<u8> {
            let item = [0x08, uses, 0x18, frecency];
            let mut entry = field(1, key.as_bytes());
            entry.extend(field(2, &item));
            field(1, &entry)
        }

        #[test]
        fn favorite_gif_roundtrips() {
            let g = FavoriteGif { url: "https://tenor.com/x".into(), format: 2, src: "https://media.tenor.com/x.mp4".into(), width: 320, height: 180, order: 3 };
            let p = emoji_prefs(&encode_favorite_gif(&g)).unwrap();
            assert_eq!(p.gifs, vec![g]);
            // A removal patch carries no value, so it decodes to nothing.
            assert!(emoji_prefs(&encode_remove_favorite_gif("https://tenor.com/x")).unwrap().gifs.is_empty());
        }

        #[test]
        fn decodes_emoji_prefs() {
            // favorite_emojis { "thumbsup" "123" }, emoji_frecency { fire }, emoji_reaction_frecency { joy: 40, eyes: 90 }
            let mut fav = field(1, b"thumbsup");
            fav.extend(field(1, b"123"));
            let mut blob = field(5, &fav);
            blob.extend(field(6, &frecent("fire", 50, 3)));
            let mut reactions = frecent("joy", 40, 2);
            reactions.extend(frecent("eyes", 90, 9));
            blob.extend(field(13, &reactions));
            let p = emoji_prefs(&blob).unwrap();
            assert_eq!(p.favorites, vec!["thumbsup", "123"]);
            assert_eq!(p.frequent, vec!["eyes", "joy"]);
            // No reaction history yet: the general emoji frecency stands in.
            let only_general = field(6, &frecent("fire", 50, 3));
            assert_eq!(emoji_prefs(&only_general).unwrap().frequent, vec!["fire"]);
        }

        #[test]
        fn decodes_folders() {
            // guild_folders { folders { guild_ids: [5, 6] (packed) name { "Fun" } } guild_positions: 7 }
            let mut folder = vec![0x0a, 16];
            folder.extend_from_slice(&5u64.to_le_bytes());
            folder.extend_from_slice(&6u64.to_le_bytes());
            folder.extend_from_slice(&[0x1a, 5, 0x0a, 3, b'F', b'u', b'n']);
            let mut gf = vec![0x0a, folder.len() as u8];
            gf.extend_from_slice(&folder);
            gf.push(0x11);
            gf.extend_from_slice(&7u64.to_le_bytes());
            let mut blob = vec![0x72, gf.len() as u8]; // field 14, wire 2
            blob.extend_from_slice(&gf);
            let (folders, pos) = guild_folders(&blob).unwrap();
            assert_eq!(folders[0].guild_ids, vec![Snowflake(5), Snowflake(6)]);
            assert_eq!(folders[0].name.as_deref(), Some("Fun"));
            assert_eq!(pos, vec![Snowflake(7)]);
        }

        #[test]
        fn status_encoding_is_partial_and_roundtrips() {
            assert_eq!(encode_status("idle"), [0x5a, 0x08, 0x0a, 0x06, 0x0a, 0x04, b'i', b'd', b'l', b'e']);
            for st in ["online", "idle", "dnd", "invisible"] {
                assert_eq!(status(&encode_status(st)).as_deref(), Some(st));
            }
            // No folder data in a status-only blob, so applying it can't clear folders.
            assert!(guild_folders(&encode_status("dnd")).is_none());
        }

        #[test]
        fn status_decodes_among_other_fields() {
            // field 1 (versions, length-delimited) ahead of status, then guild_folders after.
            let mut blob = vec![0x0a, 2, 0x08, 0x05];
            blob.extend_from_slice(&encode_status("dnd"));
            blob.extend_from_slice(&[0x72, 0]);
            assert_eq!(status(&blob).as_deref(), Some("dnd"));
            assert!(status(&[0x0a, 0]).is_none());
            assert!(status(&[0x5a, 0x09, 0x0a]).is_none(), "truncated blob");
        }
    }
}

/// ETF sends some ids as integers that JSON sends as strings (member-list group ids are role ids).
fn string_or_int<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    struct V;
    impl serde::de::Visitor<'_> for V {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or an integer")
        }
        fn visit_str<E>(self, v: &str) -> Result<String, E> { Ok(v.to_string()) }
        fn visit_string<E>(self, v: String) -> Result<String, E> { Ok(v) }
        fn visit_u64<E>(self, v: u64) -> Result<String, E> { Ok(v.to_string()) }
        fn visit_i64<E>(self, v: i64) -> Result<String, E> { Ok(v.to_string()) }
        fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<String, E> {
            String::from_utf8(v.to_vec()).map_err(E::custom)
        }
    }
    d.deserialize_any(V)
}

/// True when a name would draw as nothing (or as missing-glyph boxes): only whitespace,
/// invisible formatting characters, or code points people use to fake an empty name.
pub fn is_blank_name(s: &str) -> bool {
    s.chars().all(|c| {
        c.is_whitespace()
            || matches!(c,
                '\u{00AD}' | '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{180E}'
                | '\u{1CBB}' | '\u{1CBC}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}'
                | '\u{2800}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFA0}' | '\u{E0000}'..='\u{E007F}')
    })
}
