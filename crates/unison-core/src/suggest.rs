//! Composer autocomplete: what the text before the caret is asking for (`@`, `#`, `:`), the
//! ranked candidates from the store, and `:name:` expansion on send.
//!
//! Trigger rules and result caps follow the official composer; the exact regexes are not in the
//! public bundle chunks we can read, so the details marked `capture` in docs/FIDELITY.md are
//! behavioural matches to verify against a live client.

use crate::emoji_catalog::{self, match_score};
use crate::model::{Id, channel_type};
use crate::store::Store;

/// Most rows any popup shows.
pub const MAX_RESULTS: usize = 10;
const MENTION_EVERYONE: u64 = 1 << 17;
/// Longest `@` query: a username is at most 32 characters.
const MAX_MENTION_QUERY: usize = 32;
/// `:` needs two characters before the popup opens.
const MIN_EMOJI_QUERY: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mention,
    Channel,
    Emoji,
}

/// What the caret is in the middle of typing. Offsets are UTF-16 units into the text passed to
/// [`parse_trigger`], which is what `NSTextView` ranges use. `start` is at the trigger character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub kind: Kind,
    pub query: String,
    pub start: usize,
    pub end: usize,
}

fn utf16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Look at `before`, the text up to the caret, and say which popup (if any) it asks for.
pub fn parse_trigger(before: &str) -> Option<Trigger> {
    // Only the tail matters: no query is longer than this, so typing in a long message stays flat.
    let tail_start = before.char_indices().rev().nth(MAX_MENTION_QUERY + 8).map_or(0, |(i, _)| i);
    let tail = &before[tail_start..];
    let base = utf16(&before[..tail_start]);
    let at_boundary = |idx: usize| idx == 0 && tail_start == 0 || tail[..idx].chars().next_back().is_some_and(char::is_whitespace);
    // `@` and `#`: the last one on the current line, at the start or after whitespace.
    for (trig, kind) in [('@', Kind::Mention), ('#', Kind::Channel)] {
        if let Some(i) = tail.rfind(trig) {
            let q = &tail[i + 1..];
            let ok = at_boundary(i)
                && !q.contains(['\n', '@', '#', '<', '>'])
                && match kind {
                    Kind::Mention => q.chars().count() <= MAX_MENTION_QUERY && !q.ends_with("  ") && !q.starts_with(' '),
                    _ => !q.contains(char::is_whitespace),
                };
            if ok {
                return Some(Trigger { kind, query: q.to_string(), start: base + utf16(&tail[..i]), end: base + utf16(tail) });
            }
        }
    }
    if let Some(i) = tail.rfind(':') {
        let q = &tail[i + 1..];
        let before_ok = i == 0 && tail_start == 0 || tail[..i].chars().next_back().is_some_and(|c| c.is_whitespace() || "(+".contains(c));
        if before_ok && q.chars().count() >= MIN_EMOJI_QUERY && !q.contains(|c: char| c.is_whitespace() || c == ':') {
            return Some(Trigger { kind: Kind::Emoji, query: q.to_string(), start: base + utf16(&tail[..i]), end: base + utf16(tail) });
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    User,
    Role,
    Everyone,
    Here,
    Channel,
    Unicode,
    Custom,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub kind: EntryKind,
    pub id: Option<Id>,
    /// What the row says: a display name, `#channel`, an emoji name.
    pub label: String,
    /// Secondary text: username, category, server.
    pub detail: String,
    /// What replaces the trigger text in the message.
    pub insert: String,
    /// What the composer shows for the inserted token (`@name`, `#channel`, `:name:`).
    pub display: String,
    /// Unicode glyph for unicode emoji.
    pub glyph: Option<String>,
    /// Avatar for people.
    pub image_url: Option<String>,
    pub animated: bool,
    /// Role or name colour (0 = none).
    pub color: u32,
    pub channel_kind: u8,
    /// Usable by the current user (custom emoji from other servers need Nitro).
    pub usable: bool,
}

impl Suggestion {
    fn new(kind: EntryKind, label: String, insert: String, display: String) -> Self {
        Suggestion {
            kind,
            id: None,
            label,
            detail: String::new(),
            insert,
            display,
            glyph: None,
            image_url: None,
            animated: false,
            color: 0,
            channel_kind: 0,
            usable: true,
        }
    }
}

pub fn has_nitro(s: &Store) -> bool {
    s.me.as_ref().and_then(|u| u.premium_type).is_some_and(|t| t > 0)
}

/// Candidates for a popup. `query` is what follows the trigger character.
pub fn suggest(s: &Store, channel: Id, kind: Kind, query: &str, limit: usize) -> Vec<Suggestion> {
    let q = query.to_lowercase();
    match kind {
        Kind::Mention => mentions(s, channel, &q, limit),
        Kind::Channel => channels(s, channel, &q, limit),
        Kind::Emoji => emoji(s, channel, &q, limit),
    }
}

fn mentions(s: &Store, channel: Id, q: &str, limit: usize) -> Vec<Suggestion> {
    let Some(ch) = s.channels.get(&channel) else { return vec![] };
    let guild = ch.guild_id;
    // (score, kind order, recency rank, name) → sort key.
    let mut rows: Vec<((u32, u8, usize, String), Suggestion)> = vec![];
    // People who spoke most recently in this channel come first among equals.
    let recent: Vec<Id> = s
        .history
        .get(&channel)
        .map(|h| h.messages.iter().rev().map(|m| m.author.id).collect())
        .unwrap_or_default();
    let recency = |u: Id| recent.iter().position(|r| *r == u).unwrap_or(usize::MAX);
    let me = s.my_id();
    let push_user = |uid: Id, nick: Option<&str>, rows: &mut Vec<_>| {
        let Some(u) = s.users.get(&uid) else { return };
        let display = nick.filter(|n| !crate::model::is_blank_name(n)).unwrap_or_else(|| u.display_name()).to_string();
        let best = [Some(display.as_str()), Some(u.username.as_str()), u.global_name.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|n| if q.is_empty() { Some(0) } else { match_score(q, &n.to_lowercase()) })
            .min();
        let Some(score) = best else { return };
        let mut sg = Suggestion::new(EntryKind::User, display.clone(), format!("<@{uid}>"), format!("@{display}"));
        sg.id = Some(uid);
        sg.image_url = Some(u.avatar_url(32));
        if display != u.username {
            sg.detail = u.username.clone();
        }
        sg.color = s.name_color(guild, uid).unwrap_or(0);
        rows.push(((score, 0, recency(uid), display.to_lowercase()), sg));
    };
    match guild {
        Some(g) => {
            for ((mg, uid), m) in s.members.iter() {
                if *mg == g {
                    push_user(*uid, m.nick.as_deref(), &mut rows);
                }
            }
            let can_everyone = s.permissions(ch) & MENTION_EVERYONE != 0;
            if let Some(roles) = s.roles.get(&g) {
                for r in roles.values().filter(|r| r.id != g && (r.mentionable || can_everyone)) {
                    let score = if q.is_empty() { Some(0) } else { match_score(q, &r.name.to_lowercase()) };
                    let Some(score) = score else { continue };
                    let mut sg = Suggestion::new(EntryKind::Role, r.name.clone(), format!("<@&{}>", r.id), format!("@{}", r.name));
                    sg.id = Some(r.id);
                    sg.color = r.color;
                    rows.push(((score, 1, usize::MAX, r.name.to_lowercase()), sg));
                }
            }
            if can_everyone {
                for (name, kind, order) in [("everyone", EntryKind::Everyone, 2u8), ("here", EntryKind::Here, 3)] {
                    let score = if q.is_empty() { Some(0) } else { match_score(q, name) };
                    if let Some(score) = score {
                        let sg = Suggestion::new(kind, format!("@{name}"), format!("@{name}"), format!("@{name}"));
                        rows.push(((score, order, usize::MAX, name.into()), sg));
                    }
                }
            }
        }
        None => {
            for uid in ch.recipient_ids.iter().copied().chain(ch.recipients.iter().map(|u| u.id)).collect::<std::collections::BTreeSet<_>>() {
                if Some(uid) != me {
                    push_user(uid, None, &mut rows);
                }
            }
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter().take(limit).map(|r| r.1).collect()
}

fn channels(s: &Store, channel: Id, q: &str, limit: usize) -> Vec<Suggestion> {
    let Some(guild) = s.channels.get(&channel).and_then(|c| c.guild_id) else { return vec![] };
    let mut rows: Vec<((u32, i32), Suggestion)> = vec![];
    for cid in s.guild_channels.get(&guild).into_iter().flatten() {
        let Some(c) = s.channels.get(cid) else { continue };
        let listed = matches!(
            c.kind,
            channel_type::GUILD_TEXT
                | channel_type::GUILD_VOICE
                | channel_type::GUILD_ANNOUNCEMENT
                | channel_type::GUILD_STAGE_VOICE
                | channel_type::GUILD_FORUM
                | channel_type::GUILD_MEDIA
        );
        let name = c.name.clone().unwrap_or_default();
        if !listed || !s.can_view(c) {
            continue;
        }
        let Some(score) = (if q.is_empty() { Some(0) } else { match_score(q, &name.to_lowercase()) }) else { continue };
        let mut sg = Suggestion::new(EntryKind::Channel, name.clone(), format!("<#{cid}>"), format!("#{name}"));
        sg.id = Some(*cid);
        sg.channel_kind = c.kind;
        sg.detail = c.parent_id.and_then(|p| s.channels.get(&p)).and_then(|p| p.name.clone()).unwrap_or_default();
        rows.push(((score, c.position), sg));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    rows.into_iter().take(limit).map(|r| r.1).collect()
}

/// Custom emoji the user can see: every server's with Nitro, else only this channel's server.
/// Returns (guild, guild name, emoji id, name, animated, usable).
pub fn custom_emoji(s: &Store, channel: Option<Id>) -> Vec<(Id, String, Id, String, bool, bool)> {
    let here = channel.and_then(|c| s.channels.get(&c)).and_then(|c| c.guild_id);
    let nitro = has_nitro(s);
    let mut out = vec![];
    let mut guilds: Vec<&crate::model::Guild> = s.guilds.values().collect();
    guilds.sort_by_key(|g| g.id);
    for g in guilds {
        for e in &g.emojis {
            let (Some(id), Some(name)) = (e.id, e.name.clone()) else { continue };
            if e.available == Some(false) {
                continue;
            }
            // Other servers' emoji (and animated ones anywhere) are Nitro features.
            let usable = (here == Some(g.id) || nitro) && (!e.animated || nitro);
            out.push((g.id, g.name().to_string(), id, name, e.animated, usable));
        }
    }
    out
}

fn emoji(s: &Store, channel: Id, q: &str, limit: usize) -> Vec<Suggestion> {
    let frequent = s.emoji_prefs.as_ref().map(|p| p.frequent.clone()).unwrap_or_default();
    let rank = |n: &str| frequent.iter().position(|f| f == n).unwrap_or(usize::MAX);
    let here = s.channels.get(&channel).and_then(|c| c.guild_id);
    let mut rows: Vec<((u32, u8, usize), Suggestion)> = vec![];
    for (g, gname, id, name, animated, usable) in custom_emoji(s, Some(channel)) {
        if !usable {
            continue;
        }
        let Some(score) = match_score(q, &name.to_lowercase()) else { continue };
        let tag = if animated { "a" } else { "" };
        let mut sg = Suggestion::new(EntryKind::Custom, name.clone(), format!("<{tag}:{name}:{id}>"), format!(":{name}:"));
        sg.id = Some(id);
        sg.animated = animated;
        sg.detail = gname;
        // This server's emoji win ties with other servers'.
        rows.push(((score, u8::from(here != Some(g)), usize::MAX), sg));
    }
    for e in emoji_catalog::search(q, limit, &rank) {
        let best = e.names.iter().filter_map(|n| match_score(q, n)).min().unwrap_or(3);
        let mut sg = Suggestion::new(EntryKind::Unicode, e.names[0].to_string(), e.emoji.to_string(), e.emoji.to_string());
        sg.glyph = Some(e.emoji.to_string());
        sg.detail = e.names.get(1).map(|a| a.to_string()).unwrap_or_default();
        rows.push(((best, 2, rank(e.names[0])), sg));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.len().cmp(&b.1.label.len())));
    rows.into_iter().take(limit).map(|r| r.1).collect()
}

/// The guild to ask the gateway for members of (op 8), when the store can't be complete for it
/// and `query` isn't empty.
pub fn needs_member_search(s: &Store, channel: Id, query: &str) -> Option<Id> {
    if query.is_empty() {
        return None;
    }
    let g = s.channels.get(&channel)?.guild_id?;
    let have = s.members.keys().filter(|(mg, _)| *mg == g).count() as u32;
    let total = s.guilds.get(&g)?.member_count.unwrap_or(0);
    (have < total).then_some(g)
}

/// Replace `:name:` with the emoji it names, as the official client does when sending: unicode
/// shortcodes become the character, a custom emoji's name becomes `<:name:id>` when usable.
/// Leaves code spans, code blocks and anything inside `<...>` alone.
pub fn expand_shortcodes(s: &Store, channel: Id, text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let custom = custom_emoji(s, Some(channel));
    let find_custom = |name: &str| {
        custom
            .iter()
            .filter(|c| c.5)
            .find(|c| c.3 == name)
            .or_else(|| custom.iter().filter(|c| c.5).find(|c| c.3.eq_ignore_ascii_case(name)))
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut in_code = false;
    let mut in_angle = false;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with('`') {
            let run = rest.chars().take_while(|c| *c == '`').count();
            if run >= 3 || !in_code || run == 1 {
                in_code = !in_code;
            }
            out.push_str(&rest[..run]);
            rest = &rest[run..];
            continue;
        }
        match c {
            '<' if !in_code => in_angle = true,
            '>' => in_angle = false,
            ':' if !in_code && !in_angle => {
                if let Some(end) = rest[1..].find(':') {
                    let name = &rest[1..1 + end];
                    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    // Not the middle of a longer word like `a:b:c`.
                    let prev_word = out.chars().next_back().is_some_and(char::is_alphanumeric);
                    if ok && !prev_word {
                        let replacement = find_custom(name)
                            .map(|c| format!("<{}:{}:{}>", if c.4 { "a" } else { "" }, c.3, c.2))
                            .or_else(|| emoji_catalog::by_name(&name.to_lowercase()).map(|e| e.emoji.to_string()));
                        if let Some(r) = replacement {
                            out.push_str(&r);
                            rest = &rest[end + 2..];
                            continue;
                        }
                    }
                }
            }
            _ => {}
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// A mention, channel or custom-emoji token in raw message text, for showing it as a pill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpan {
    /// UTF-16 offset and length of the raw token in the text.
    pub start: usize,
    pub len: usize,
    pub raw: String,
    pub display: String,
}

/// The `<@id>`, `<@&id>`, `<#id>` and `<:name:id>` tokens in `text` that the store can name.
pub fn tokens(s: &Store, channel: Id, text: &str) -> Vec<TokenSpan> {
    if !text.contains('<') {
        return vec![];
    }
    let guild = s.channels.get(&channel).and_then(|c| c.guild_id);
    let mut out = vec![];
    let mut offset16 = 0usize;
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        offset16 += rest[..i].encode_utf16().count();
        rest = &rest[i..];
        let Some(end) = rest.find('>') else { break };
        let raw = &rest[..=end];
        let inner = &raw[1..raw.len() - 1];
        let id_of = |t: &str| t.parse::<u64>().ok().map(crate::snowflake::Snowflake);
        let display = if let Some(t) = inner.strip_prefix("@&") {
            id_of(t).and_then(|r| guild.and_then(|g| s.roles.get(&g)).and_then(|m| m.get(&r))).map(|r| format!("@{}", r.name))
        } else if let Some(t) = inner.strip_prefix('@') {
            id_of(t.trim_start_matches('!')).filter(|u| s.users.contains_key(u)).map(|u| format!("@{}", s.display_name(guild, u)))
        } else if let Some(t) = inner.strip_prefix('#') {
            id_of(t).and_then(|c| s.channels.get(&c)).and_then(|c| c.name.clone()).map(|n| format!("#{n}"))
        } else if let Some(t) = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':')) {
            t.rsplit_once(':').filter(|(n, i)| !n.is_empty() && i.parse::<u64>().is_ok()).map(|(n, _)| format!(":{n}:"))
        } else {
            None
        };
        let len16 = raw.encode_utf16().count();
        if let Some(display) = display {
            out.push(TokenSpan { start: offset16, len: len16, raw: raw.to_string(), display });
        }
        offset16 += len16;
        rest = &rest[end + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, Guild, Member, Role, User};
    use crate::snowflake::Snowflake as Id;

    fn t(s: &str) -> Option<(Kind, String, usize, usize)> {
        parse_trigger(s).map(|t| (t.kind, t.query, t.start, t.end))
    }

    #[test]
    fn triggers() {
        assert_eq!(t("hi @jo"), Some((Kind::Mention, "jo".into(), 3, 6)));
        assert_eq!(t("@"), Some((Kind::Mention, "".into(), 0, 1)));
        assert_eq!(t("mail me@jo"), None);
        assert_eq!(t("hello #gen"), Some((Kind::Channel, "gen".into(), 6, 10)));
        assert_eq!(t("a #gen eral"), None);
        assert_eq!(t("go :smi"), Some((Kind::Emoji, "smi".into(), 3, 7)));
        assert_eq!(t("go :s"), None);
        assert_eq!(t("12:30"), None);
        assert_eq!(t(":smile: "), None);
        assert_eq!(t(":smile:x"), None);
        // Names may contain one space.
        assert_eq!(t("@John Do"), Some((Kind::Mention, "John Do".into(), 0, 8)));
        assert_eq!(t("@john\nrest"), None);
        // UTF-16 offsets: the emoji is two units.
        assert_eq!(t("😀 @a"), Some((Kind::Mention, "a".into(), 3, 5)));
        // A long message is only inspected at its tail.
        let long = format!("{} @bo", "word ".repeat(500));
        assert_eq!(t(&long).map(|x| x.1), Some("bo".into()));
        assert_eq!(t(&format!("{}@a", "x".repeat(300))), None);
    }

    fn store() -> (Store, Id) {
        let mut s = Store::default();
        let (g, c) = (Id(1), Id(10));
        s.me = Some(User { id: Id(100), username: "me".into(), premium_type: Some(0), ..Default::default() });
        s.my_members.insert(g, Member::default());
        s.guilds.insert(g, Guild { id: g, member_count: Some(5), ..Default::default() });
        s.channels.insert(c, Channel { id: c, guild_id: Some(g), name: Some("general".into()), ..Default::default() });
        s.channels.insert(Id(11), Channel { id: Id(11), guild_id: Some(g), name: Some("gaming".into()), kind: channel_type::GUILD_VOICE, ..Default::default() });
        s.channels.insert(Id(12), Channel { id: Id(12), guild_id: Some(g), name: Some("cat".into()), kind: channel_type::GUILD_CATEGORY, ..Default::default() });
        s.guild_channels.insert(g, vec![c, Id(11), Id(12)]);
        for (id, name, nick) in [(200u64, "alice", None), (201, "bob", Some("Alicia")), (202, "zed", None)] {
            s.users.insert(Id(id), User { id: Id(id), username: name.into(), ..Default::default() });
            s.members.insert((g, Id(id)), Member { user_id: Some(Id(id)), nick: nick.map(String::from), ..Default::default() });
        }
        s.roles.entry(g).or_default().insert(Id(300), Role { id: Id(300), name: "Alpha Team".into(), mentionable: true, ..Default::default() });
        s.roles.entry(g).or_default().insert(Id(301), Role { id: Id(301), name: "Hidden".into(), mentionable: false, ..Default::default() });
        s.guilds.get_mut(&g).unwrap().emojis = vec![
            crate::model::Emoji { id: Some(Id(900)), name: Some("pepe".into()), animated: false, available: None },
            crate::model::Emoji { id: Some(Id(901)), name: Some("dance".into()), animated: true, available: None },
        ];
        (s, c)
    }

    #[test]
    fn mention_ranking_and_encoding() {
        let (s, c) = store();
        let r = suggest(&s, c, Kind::Mention, "ali", 10);
        let labels: Vec<_> = r.iter().map(|x| x.label.as_str()).collect();
        // alice (prefix on username) and Alicia (prefix on nick) first, then the role by word prefix.
        assert_eq!(&labels[..2], ["alice", "Alicia"]);
        assert!(labels.contains(&"Alpha Team") == false);
        assert_eq!(r[0].insert, "<@200>");
        let r = suggest(&s, c, Kind::Mention, "al", 10);
        let role = r.iter().find(|x| x.kind == EntryKind::Role).unwrap();
        assert_eq!(role.insert, "<@&300>");
        // Unmentionable roles stay hidden without Mention Everyone... but the owner-less test store has no perms.
        assert!(!r.iter().any(|x| x.label == "Hidden"));
    }

    #[test]
    fn channels_skip_categories() {
        let (mut s, c) = store();
        // Viewing needs permissions; the owner has them all.
        s.guilds.get_mut(&Id(1)).unwrap().inline.owner_id = Some(Id(100));
        let r = suggest(&s, c, Kind::Channel, "g", 10);
        let names: Vec<_> = r.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(names, ["gaming", "general"]);
        assert_eq!(r[1].insert, "<#10>");
        assert!(suggest(&s, c, Kind::Channel, "cat", 10).is_empty());
    }

    #[test]
    fn emoji_rules() {
        let (mut s, c) = store();
        let r = suggest(&s, c, Kind::Emoji, "pep", 10);
        assert_eq!(r[0].insert, "<:pepe:900>");
        // Animated needs Nitro.
        assert!(suggest(&s, c, Kind::Emoji, "danc", 10).iter().all(|x| x.kind != EntryKind::Custom));
        s.me.as_mut().unwrap().premium_type = Some(2);
        assert_eq!(suggest(&s, c, Kind::Emoji, "danc", 10)[0].insert, "<a:dance:901>");
        // Unicode.
        assert_eq!(suggest(&s, c, Kind::Emoji, "thumbsup", 3)[0].insert, "👍");
    }

    #[test]
    fn token_spans() {
        let (s, c) = store();
        let t = tokens(&s, c, "😀 <@200> hi <#10> <:pepe:900> <@999> <@&300>");
        let d: Vec<_> = t.iter().map(|x| (x.start, x.len, x.display.as_str())).collect();
        assert_eq!(d, [(3, 6, "@alice"), (13, 5, "#general"), (19, 11, ":pepe:"), (38, 7, "@Alpha Team")]);
    }

    #[test]
    fn shortcodes() {
        let (s, c) = store();
        assert_eq!(expand_shortcodes(&s, c, "hi :pepe: and :thumbsup:!"), "hi <:pepe:900> and 👍!");
        assert_eq!(expand_shortcodes(&s, c, "`:pepe:` stays"), "`:pepe:` stays");
        assert_eq!(expand_shortcodes(&s, c, "<:pepe:900> a:b:c 12:30:45 :nope:"), "<:pepe:900> a:b:c 12:30:45 :nope:");
        assert_eq!(expand_shortcodes(&s, c, "```\n:pepe:\n``` :pepe:"), "```\n:pepe:\n``` <:pepe:900>");
    }

    #[test]
    fn member_search_only_when_incomplete() {
        let (mut s, c) = store();
        assert_eq!(needs_member_search(&s, c, "x"), Some(Id(1)));
        assert_eq!(needs_member_search(&s, c, ""), None);
        s.guilds.get_mut(&Id(1)).unwrap().member_count = Some(3);
        assert_eq!(needs_member_search(&s, c, "x"), None);
    }
}
