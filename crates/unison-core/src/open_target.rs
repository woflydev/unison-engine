//! Where a channel, thread or message lives, so every surface that points at one opens it in the
//! right place (docs/UX.md section 8). A surface knows at best a channel id: REST messages carry
//! no `guild_id` (Inbox mentions, search and saved rows get `None` for server messages), a
//! notification's server can be stale, and a thread sits under its parent. The store is the only
//! authority, so this module answers from it and ignores what the caller thought.

use serde::{Deserialize, Serialize};

use crate::model::{Channel, Id, channel_type};
use crate::store::Store;

/// Why a target could not be opened where it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Missing {
    /// Not in the store (a thread never joined, a channel of a server that has not loaded).
    Unknown,
    /// Known, but this account cannot view it (permissions changed, hidden channel).
    Hidden,
}

/// A place resolved against the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTarget {
    /// Its server; `None` for a DM or group DM.
    pub guild: Option<Id>,
    pub channel: Id,
    /// A thread's parent (text channel or forum): the sidebar row to highlight when the thread
    /// itself is not listed.
    pub parent: Option<Id>,
    /// The category the channel (or a thread's parent) sits in: opened when it is collapsed.
    pub category: Option<Id>,
}

/// What opening a channel should do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Open(OpenTarget),
    /// Cannot open the channel; `guild` is the server worth showing instead, if one is known.
    Fallback { guild: Option<Id>, why: Missing },
}

/// The server a channel belongs to, from the channel itself, else from the per-server lists.
pub fn guild_of(store: &Store, channel: &Channel) -> Option<Id> {
    if channel.is_private() {
        return None;
    }
    channel
        .guild_id
        .or_else(|| store.guild_channels.iter().find(|(_, ids)| ids.contains(&channel.id)).map(|(g, _)| *g))
}

/// `hint_guild` is the surface's own guess; it is used only when the channel is not in the store,
/// to at least land in the right server.
pub fn resolve(store: &Store, channel: Id, hint_guild: Option<Id>) -> Resolution {
    let Some(c) = store.channels.get(&channel) else {
        let guild = hint_guild.filter(|g| store.guilds.contains_key(g));
        return Resolution::Fallback { guild, why: Missing::Unknown };
    };
    let guild = guild_of(store, c);
    if !c.is_private() && !store.can_view(c) {
        return Resolution::Fallback { guild, why: Missing::Hidden };
    }
    let parent = if c.is_thread() { c.parent_id } else { None };
    let anchor = parent.and_then(|p| store.channels.get(&p)).unwrap_or(c);
    let category = anchor
        .parent_id
        .filter(|p| store.channels.get(p).is_some_and(|x| x.kind == channel_type::GUILD_CATEGORY));
    Resolution::Open(OpenTarget { guild, channel, parent, category })
}

/// How long a "no such channel / not yours" answer for a fetched id is remembered, so a second
/// click on the same dead link asks nothing (the official client keeps `NOT_FOUND` until the next
/// reconnect; ARCHITECTURE rule 10).
pub const MISS_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// Ids the server already said no to (see `Session::preview_thread`).
#[derive(Default)]
pub struct Misses(parking_lot::Mutex<std::collections::HashMap<Id, (Missing, std::time::Instant)>>);

impl Misses {
    pub fn get(&self, id: Id, ttl: std::time::Duration) -> Option<Missing> {
        self.0.lock().get(&id).filter(|(_, at)| at.elapsed() < ttl).map(|(why, _)| *why)
    }
    pub fn record(&self, id: Id, why: Missing) {
        let mut m = self.0.lock();
        m.insert(id, (why, std::time::Instant::now()));
        if m.len() > 256 {
            m.retain(|_, (_, at)| at.elapsed() < MISS_TTL);
        }
    }
}

/// Whether a discord.com link to a place we do not know yet is still ours to open: it points into
/// a server this account is in (a thread there may exist without being joined). A link into a
/// server we are not in, or to a DM we do not know, stays a web link.
pub fn link_may_be_ours(store: &Store, t: &LinkTarget) -> bool {
    match resolve(store, t.channel, t.guild) {
        Resolution::Open(_) => true,
        Resolution::Fallback { why: Missing::Hidden, .. } => false,
        Resolution::Fallback { why: Missing::Unknown, .. } => t.guild.is_some_and(|g| store.guilds.contains_key(&g)),
    }
}

/// True when `message` is already in the loaded window of `channel`, so a jump needs no request.
pub fn message_loaded(store: &Store, channel: Id, message: Id) -> bool {
    store.history.get(&channel).is_some_and(|h| h.messages.iter().any(|m| m.id == message))
}

/// A discord.com link to a place: `https://discord.com/channels/<server or @me>/<channel>[/<message>]`
/// (also ptb, canary and discordapp.com). Anything else, including a server-only link, is `None`:
/// it is an ordinary web link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTarget {
    pub guild: Option<Id>,
    pub channel: Id,
    pub message: Option<Id>,
}

pub fn parse_link(url: &str) -> Option<LinkTarget> {
    let rest = url.trim().strip_prefix("https://").or_else(|| url.trim().strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if !matches!(host, "discord.com" | "ptb.discord.com" | "canary.discord.com" | "discordapp.com") {
        return None;
    }
    let path = path.split(['?', '#']).next()?;
    let mut parts = path.trim_end_matches('/').split('/');
    if parts.next()? != "channels" {
        return None;
    }
    let guild = match parts.next()? {
        "@me" => None,
        g => Some(g.parse::<u64>().ok().filter(|n| *n > 0).map(crate::snowflake::Snowflake)?),
    };
    let id = |p: &str| p.parse::<u64>().ok().filter(|n| *n > 0).map(crate::snowflake::Snowflake);
    let channel = id(parts.next()?)?;
    let message = match parts.next() {
        Some(m) => Some(id(m)?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(LinkTarget { guild, channel, message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Guild, Message, Role, User};
    use crate::store::perm;
    use crate::snowflake::Snowflake;

    fn chan(id: u64, kind: u8, guild: Option<u64>, parent: Option<u64>) -> Channel {
        Channel { id: Snowflake(id), kind, guild_id: guild.map(Snowflake), parent_id: parent.map(Snowflake), ..Default::default() }
    }

    /// Server 100 (@everyone may view) with category 10 holding text 11 and forum 12, forum post
    /// 13 under 12, and a DM 20.
    fn world() -> Store {
        let mut s = Store::default();
        let g = Snowflake(100);
        s.me = Some(User { id: Snowflake(1), ..Default::default() });
        s.guilds.insert(g, Guild { id: g, ..Default::default() });
        s.roles.insert(
            g,
            [(g, Role { id: g, permissions: perm::VIEW_CHANNEL.to_string(), ..Default::default() })].into_iter().collect(),
        );
        for c in [
            chan(10, channel_type::GUILD_CATEGORY, Some(100), None),
            chan(11, channel_type::GUILD_TEXT, Some(100), Some(10)),
            chan(12, channel_type::GUILD_FORUM, Some(100), Some(10)),
            chan(13, channel_type::PUBLIC_THREAD, Some(100), Some(12)),
            chan(20, channel_type::DM, None, None),
        ] {
            s.channels.insert(c.id, c);
        }
        s.guild_channels.insert(g, vec![Snowflake(10), Snowflake(11), Snowflake(12)]);
        s
    }

    #[test]
    fn server_message_resolves_to_its_server_and_category() {
        // The Inbox bug: a mention came back with no guild_id, so it opened under Direct Messages.
        let s = world();
        let Resolution::Open(t) = resolve(&s, Snowflake(11), None) else { panic!() };
        assert_eq!((t.guild, t.category, t.parent), (Some(Snowflake(100)), Some(Snowflake(10)), None));
    }

    #[test]
    fn guild_comes_from_the_lists_when_the_channel_lacks_it() {
        let mut s = world();
        s.channels.get_mut(&Snowflake(11)).unwrap().guild_id = None;
        let Resolution::Open(t) = resolve(&s, Snowflake(11), None) else { panic!() };
        assert_eq!(t.guild, Some(Snowflake(100)));
    }

    #[test]
    fn the_store_beats_the_callers_guess() {
        let s = world();
        let Resolution::Open(t) = resolve(&s, Snowflake(11), Some(Snowflake(999))) else { panic!() };
        assert_eq!(t.guild, Some(Snowflake(100)));
        let Resolution::Open(dm) = resolve(&s, Snowflake(20), Some(Snowflake(100))) else { panic!() };
        assert_eq!((dm.guild, dm.category), (None, None), "a DM is never in a server");
    }

    #[test]
    fn a_thread_names_its_parent_and_opens_the_parents_category() {
        let s = world();
        let Resolution::Open(t) = resolve(&s, Snowflake(13), None) else { panic!() };
        assert_eq!((t.guild, t.parent, t.category), (Some(Snowflake(100)), Some(Snowflake(12)), Some(Snowflake(10))));
    }

    #[test]
    fn unknown_channel_falls_back_to_a_known_server_only() {
        let s = world();
        let fb = |g| Resolution::Fallback { guild: g, why: Missing::Unknown };
        assert_eq!(resolve(&s, Snowflake(77), Some(Snowflake(100))), fb(Some(Snowflake(100))));
        assert_eq!(resolve(&s, Snowflake(77), Some(Snowflake(5))), fb(None));
        assert_eq!(resolve(&s, Snowflake(77), None), fb(None));
    }

    #[test]
    fn a_channel_that_cannot_be_viewed_is_not_opened() {
        let mut s = world();
        s.channels.get_mut(&Snowflake(11)).unwrap().permission_overwrites = vec![crate::model::PermissionOverwrite {
            id: Snowflake(100),
            kind: 0,
            allow: "0".into(),
            deny: perm::VIEW_CHANNEL.to_string(),
        }];
        assert_eq!(resolve(&s, Snowflake(11), None), Resolution::Fallback { guild: Some(Snowflake(100)), why: Missing::Hidden });
    }

    #[test]
    fn loaded_window_is_checked_in_history_only() {
        let mut s = world();
        let (c, m) = (Snowflake(11), Snowflake(500));
        assert!(!message_loaded(&s, c, m));
        s.set_history_latest(c, vec![Message { id: m, channel_id: c, ..Default::default() }], false);
        assert!(message_loaded(&s, c, m));
        assert!(!message_loaded(&s, c, Snowflake(501)));
    }

    #[test]
    fn discord_links_parse_to_places() {
        let l = |u: &str| parse_link(u);
        let t = l("https://discord.com/channels/100/11/500").unwrap();
        assert_eq!((t.guild, t.channel, t.message), (Some(Snowflake(100)), Snowflake(11), Some(Snowflake(500))));
        let dm = l("https://ptb.discord.com/channels/@me/20?x=1").unwrap();
        assert_eq!((dm.guild, dm.channel, dm.message), (None, Snowflake(20), None));
        assert!(l("https://canary.discord.com/channels/100/11/").is_some());
        assert!(l("https://discord.com/channels/100").is_none(), "server-only link stays a web link");
        assert!(l("https://discord.com/channels/100/abc").is_none());
        assert!(l("https://evil.example/channels/100/11").is_none());
        assert!(l("https://discord.com.evil.example/channels/100/11").is_none());
        assert!(l("https://discord.com/channels/100/11/500/9").is_none());
        assert!(l("discord.com/channels/100/11").is_none());
    }

    // ----- Threads this account has not joined -----

    fn thread(id: u64, kind: u8, parent: u64, member: bool) -> Channel {
        let mut c = chan(id, kind, Some(100), Some(parent));
        c.member = member.then(|| serde_json::json!({"id": id.to_string()}));
        c
    }

    fn grant(s: &mut Store, bits: u64) {
        let g = Snowflake(100);
        s.roles.get_mut(&g).unwrap().get_mut(&g).unwrap().permissions = (perm::VIEW_CHANNEL | bits).to_string();
    }

    #[test]
    fn unknown_thread_resolves_after_it_is_admitted_under_its_parent() {
        let mut s = world();
        let t = thread(500, channel_type::PUBLIC_THREAD, 11, false);
        assert!(matches!(resolve(&s, Snowflake(500), None), Resolution::Fallback { why: Missing::Unknown, .. }));
        s.admit_fetched_thread(t, None).unwrap();
        let Resolution::Open(o) = resolve(&s, Snowflake(500), None) else { panic!("should open") };
        assert_eq!(o.guild, Some(Snowflake(100)));
        assert_eq!(o.parent, Some(Snowflake(11)));
        assert_eq!(o.category, Some(Snowflake(10)));
        let c = s.channels[&Snowflake(500)].clone();
        assert_eq!(s.thread_join(&c), crate::store::ThreadJoin::CanJoin, "viewing is not joining");
        assert!(c.member.is_none());
        assert!(s.thread_in_sidebar(&c, 0, Some(c.id)), "listed under its parent while open");
        assert!(!s.thread_in_sidebar(&c, 0, None), "and gone again when left");
        assert!(!s.is_unread(&c) && s.mention_count(c.id) == 0, "nothing to ack");
    }

    #[test]
    fn a_fetched_thread_without_a_guild_takes_the_callers_hint_and_needs_a_loaded_parent() {
        let mut s = world();
        let mut t = thread(501, channel_type::PUBLIC_THREAD, 11, false);
        t.guild_id = None;
        assert!(s.admit_fetched_thread(t.clone(), None).is_err(), "no server to put it in");
        s.admit_fetched_thread(t, Some(Snowflake(100))).unwrap();
        assert_eq!(s.channels[&Snowflake(501)].guild_id, Some(Snowflake(100)));
        // Parent not loaded, server not loaded, not a thread at all: unknown, nothing stored.
        assert_eq!(s.admit_fetched_thread(thread(502, channel_type::PUBLIC_THREAD, 999, false), None), Err(Missing::Unknown));
        let mut other = thread(503, channel_type::PUBLIC_THREAD, 11, false);
        other.guild_id = Some(Snowflake(777));
        assert_eq!(s.admit_fetched_thread(other, None), Err(Missing::Unknown));
        assert_eq!(s.admit_fetched_thread(chan(504, channel_type::GUILD_TEXT, Some(100), Some(10)), None), Err(Missing::Unknown));
        assert!(!s.channels.contains_key(&Snowflake(502)) && !s.channels.contains_key(&Snowflake(504)));
    }

    #[test]
    fn private_thread_you_are_not_in_is_hidden_unless_you_manage_threads() {
        let mut s = world();
        let t = thread(510, channel_type::PRIVATE_THREAD, 11, false);
        assert_eq!(s.admit_fetched_thread(t.clone(), None), Err(Missing::Hidden));
        assert!(!s.channels.contains_key(&Snowflake(510)));
        grant(&mut s, perm::MANAGE_THREADS);
        s.admit_fetched_thread(t, None).unwrap();
        // A private thread you are in needs nothing extra.
        let mut s = world();
        s.admit_fetched_thread(thread(511, channel_type::PRIVATE_THREAD, 11, true), None).unwrap();
    }

    #[test]
    fn gateway_member_survives_a_refetch() {
        let mut s = world();
        s.admit_fetched_thread(thread(520, channel_type::PUBLIC_THREAD, 11, true), None).unwrap();
        s.admit_fetched_thread(thread(520, channel_type::PUBLIC_THREAD, 11, false), None).unwrap();
        assert!(s.channels[&Snowflake(520)].member.is_some());
    }

    #[test]
    fn sending_in_a_thread_follows_send_in_threads_and_the_lock() {
        let mut s = world();
        let open = thread(530, channel_type::PUBLIC_THREAD, 11, false);
        let mut locked = thread(531, channel_type::PUBLIC_THREAD, 11, false);
        locked.thread_metadata = Some(serde_json::json!({"locked": true}));
        let can = |s: &Store, c: &Channel| s.permissions(c) & perm::SEND_MESSAGES != 0;
        assert!(!can(&s, &open), "view only");
        grant(&mut s, perm::SEND_MESSAGES_IN_THREADS);
        assert!(can(&s, &open));
        assert!(!can(&s, &locked), "locked needs Manage Threads");
        grant(&mut s, perm::SEND_MESSAGES_IN_THREADS | perm::MANAGE_THREADS);
        assert!(can(&s, &locked));
        // Send Messages in the parent alone does not let you post in its threads.
        grant(&mut s, perm::SEND_MESSAGES);
        assert!(!can(&s, &open));
    }

    #[test]
    fn archived_thread_cannot_be_joined() {
        let s = world();
        let mut t = thread(540, channel_type::PUBLIC_THREAD, 11, false);
        t.thread_metadata = Some(serde_json::json!({"archived": true}));
        assert_eq!(s.thread_join(&t), crate::store::ThreadJoin::Archived);
        assert_eq!(s.thread_join(&chan(11, channel_type::GUILD_TEXT, Some(100), None)), crate::store::ThreadJoin::NotThread);
    }

    #[test]
    fn dead_ids_are_remembered_for_the_window_only() {
        let m = Misses::default();
        let id = Snowflake(9);
        assert_eq!(m.get(id, MISS_TTL), None);
        m.record(id, Missing::Hidden);
        assert_eq!(m.get(id, MISS_TTL), Some(Missing::Hidden));
        assert_eq!(m.get(id, std::time::Duration::ZERO), None, "stale after the window");
    }

    #[test]
    fn links_into_our_servers_are_ours_even_for_unknown_threads() {
        let s = world();
        let link = |g: Option<u64>, c: u64| LinkTarget { guild: g.map(Snowflake), channel: Snowflake(c), message: None };
        assert!(link_may_be_ours(&s, &link(Some(100), 600)), "unknown thread in a server we are in");
        assert!(!link_may_be_ours(&s, &link(Some(777), 600)), "server we are not in: web link");
        assert!(!link_may_be_ours(&s, &link(None, 600)), "unknown DM: web link");
        assert!(link_may_be_ours(&s, &link(Some(100), 11)), "known channel");
    }
}
