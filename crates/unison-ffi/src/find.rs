//! The one search surface (⌘K and ⌘F): places and people from memory, messages from the on-device
//! text index, and Discord's own search on request. Ranking, folding, marks and recents are
//! `unison_core::find`; this file only gathers from the stores and shapes records for a screen.

use super::*;
use unison_core::find as rules;
use unison_core::store::Store;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum PlaceKind {
    Account,
    Server,
    Text,
    Announcement,
    Voice,
    Stage,
    Forum,
    Thread,
    Dm,
    GroupDm,
    Person,
}

/// A run of characters to draw highlighted, in UTF-16 units of the text it belongs to.
#[derive(uniffi::Record, Clone, Debug)]
pub struct MarkRange {
    pub start: u32,
    pub len: u32,
}

fn ranges(text: &str, terms: &[String]) -> Vec<MarkRange> {
    rules::marks(text, terms).into_iter().map(|(start, len)| MarkRange { start, len }).collect()
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct PlaceHit {
    pub account_id: String,
    pub guild_id: Option<String>,
    /// The channel to open (`None` for a server or an account).
    pub channel_id: Option<String>,
    /// A friend without a DM yet: opening starts one.
    pub user_id: Option<String>,
    pub kind: PlaceKind,
    pub title: String,
    /// The server a channel is in, "Direct Message", "@username", the account name.
    pub context: String,
    pub icon_url: Option<String>,
    pub unread: bool,
    pub mentions: u32,
    pub muted: bool,
    /// Drawn with the account's avatar when it is not the active one.
    pub other_account: bool,
    pub title_marks: Vec<MarkRange>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct PlaceSection {
    pub title: String,
    pub places: Vec<PlaceHit>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MessageHit {
    pub account_id: String,
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub author: String,
    pub avatar_url: String,
    /// Mentions as names, markdown flattened, cut around the first match.
    pub text: String,
    pub marks: Vec<MarkRange>,
    /// "Server · #channel" or the DM's name.
    pub location: String,
    pub timestamp_ms: u64,
    pub other_account: bool,
    /// Which account, when several are signed in.
    pub account_name: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FindRequest {
    pub query: String,
    pub scope: SearchScopeKind,
    /// The active account and where it is looking now.
    pub account_id: String,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    /// Messages already shown; the page continues after them.
    pub offset: u32,
    pub limit: u32,
    pub tz_offset_secs: i32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FindResult {
    pub sections: Vec<PlaceSection>,
    pub messages: Vec<MessageHit>,
    /// Matches in the on-device index (at most 1000).
    pub message_total: u32,
    pub message_total_capped: bool,
    /// The query has `has:`/`pinned:`/`mentions:`, which only Discord can answer.
    pub discord_only: bool,
    /// Where Discord would be asked from here, and the words for the action.
    pub discord_scope: SearchScopeKind,
    pub discord_label: String,
    pub empty_query: bool,
    pub micros: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct DiscordPage {
    pub messages: Vec<MessageHit>,
    pub total: u32,
}

const MESSAGE_CHARS: usize = 220;
const SECTION_CAPS: [(&str, usize); 6] = [("Servers", 4), ("Channels", 8), ("Threads", 4), ("Direct messages", 5), ("People", 4), ("Accounts", 2)];

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn place_kind(c: &Channel) -> PlaceKind {
    match channels::kind(c) {
        ChannelKind::Voice => PlaceKind::Voice,
        ChannelKind::Stage => PlaceKind::Stage,
        ChannelKind::Announcement => PlaceKind::Announcement,
        ChannelKind::Forum | ChannelKind::Media => PlaceKind::Forum,
        ChannelKind::Thread => PlaceKind::Thread,
        ChannelKind::Dm => PlaceKind::Dm,
        ChannelKind::GroupDm => PlaceKind::GroupDm,
        _ => PlaceKind::Text,
    }
}

fn section_of(k: PlaceKind) -> usize {
    match k {
        PlaceKind::Server => 0,
        PlaceKind::Text | PlaceKind::Announcement | PlaceKind::Voice | PlaceKind::Stage | PlaceKind::Forum => 1,
        PlaceKind::Thread => 2,
        PlaceKind::Dm | PlaceKind::GroupDm => 3,
        PlaceKind::Person => 4,
        PlaceKind::Account => 5,
    }
}

/// A place before its strings are built: scanning thousands of channels allocates nothing but
/// this, and only the few survivors become `PlaceHit`s.
struct Cand {
    score: i32,
    acc: usize,
    kind: PlaceKind,
    guild: Option<Id>,
    channel: Option<Id>,
    user: Option<Id>,
}

impl UnisonEngine {
    /// Everything a row needs for one candidate.
    fn build_place(&self, c: &Cand, accounts: &[Id], active: Option<Id>, terms: &[String]) -> Option<PlaceHit> {
        let account = accounts[c.acc];
        let sess = self.inner.session(account)?;
        let s = sess.store.read();
        let acc_s = account.to_string();
        let other = Some(account) != active && accounts.len() > 1;
        let hit = |title: String, context: String, icon: Option<String>, unread, mentions, muted| PlaceHit {
            account_id: acc_s.clone(),
            guild_id: c.guild.map(|g| g.to_string()),
            channel_id: c.channel.map(|x| x.to_string()),
            user_id: c.user.map(|u| u.to_string()),
            kind: c.kind,
            title_marks: ranges(&title, terms),
            title,
            context,
            icon_url: icon,
            unread,
            mentions,
            muted,
            other_account: other,
        };
        match c.kind {
            PlaceKind::Account => Some(hit(s.me.as_ref()?.display_name().to_string(), "Account".into(), s.me.as_ref().map(|u| u.avatar_url(64)), false, 0, false)),
            PlaceKind::Server => {
                let g = guild_item(&s, c.guild?);
                Some(hit(g.name, self.account_name_in(&s, accounts.len()), g.icon_url, g.unread, g.mentions, g.muted))
            }
            PlaceKind::Person => {
                let u = s.users.get(&c.user?)?;
                Some(hit(s.display_name(None, u.id), format!("@{}", u.username), Some(u.avatar_url(64)), false, 0, false))
            }
            PlaceKind::Dm | PlaceKind::GroupDm => {
                let ch = s.channels.get(&c.channel?)?;
                let d = dm_item(&s, ch);
                Some(hit(d.name, if d.is_group { "Group message".into() } else { "Direct message".into() }, d.icon_url, d.unread && !d.muted, d.mentions, d.muted))
            }
            _ => {
                let ch = s.channels.get(&c.channel?)?;
                let guild = ch.guild_id.and_then(|g| s.guilds.get(&g)).map(|g| g.name().to_string()).unwrap_or_default();
                let context = if ch.is_thread() {
                    let parent = ch.parent_id.and_then(|p| s.channels.get(&p)).and_then(|p| p.name.clone()).unwrap_or_default();
                    format!("{guild} · #{parent}")
                } else {
                    guild
                };
                Some(hit(ch.name.clone().unwrap_or_default(), context, None, ch.has_text_chat() && s.is_unread(ch) && !s.is_channel_muted(ch), s.mention_count(ch.id), s.is_channel_muted(ch)))
            }
        }
    }

    fn account_name_in(&self, s: &Store, accounts: usize) -> String {
        if accounts > 1 { s.me.as_ref().map(|u| u.display_name().to_string()).unwrap_or_default() } else { "Server".into() }
    }

    /// Score every place the query could mean, across the accounts in scope.
    fn scan_places(&self, req: &FindRequest, accounts: &[Id], active: Option<Id>, terms: &[String], recents: &rules::Recents, now: u64) -> Vec<Cand> {
        let mut out: Vec<Cand> = Vec::new();
        let mut buf = String::new();
        let fold_into = |buf: &mut String, s: &str| {
            buf.clear();
            buf.extend(s.chars().map(rules::fold_char));
        };
        let in_guild = req.scope == SearchScopeKind::Server && req.guild_id.is_some();
        let want_guild = req.guild_id.as_deref().map(id);
        for (ai, &account) in accounts.iter().enumerate() {
            let Some(sess) = self.inner.session(account) else { continue };
            let s = sess.store.read();
            let acc_s = account.to_string();
            let boost = if Some(account) == active { rules::ACTIVE_ACCOUNT_FIRST } else { 0 };
            let points = |ch: Id| recents.points(&acc_s, &ch.to_string(), now);
            let cue = |unread: bool, mentions: u32| if mentions > 0 { 15 } else if unread { 8 } else { 0 };
            if accounts.len() > 1 && !in_guild {
                if let Some(me) = &s.me {
                    fold_into(&mut buf, me.display_name());
                    if let Some(sc) = rules::score_place(terms, &buf, "") {
                        out.push(Cand { score: sc - 2 + boost, acc: ai, kind: PlaceKind::Account, guild: None, channel: None, user: None });
                    }
                }
            }
            let mut guild_f = String::new();
            for (gid, g) in &s.guilds {
                if in_guild && Some(*gid) != want_guild {
                    continue;
                }
                fold_into(&mut guild_f, g.name());
                if !in_guild {
                    if let Some(sc) = rules::score_place(terms, &guild_f, "") {
                        let (unread, mentions) = (s.guild_has_unread(*gid), s.guild_mentions(*gid));
                        out.push(Cand { score: sc + boost + 6 + cue(unread, mentions), acc: ai, kind: PlaceKind::Server, guild: Some(*gid), channel: None, user: None });
                    }
                }
                for c in s.guild_channels.get(gid).into_iter().flatten().filter_map(|c| s.channels.get(c)) {
                    if c.kind == channel_type::GUILD_CATEGORY {
                        continue;
                    }
                    if c.is_thread() && c.is_archived() {
                        continue;
                    }
                    let Some(name) = c.name.as_deref() else { continue };
                    fold_into(&mut buf, name);
                    let Some(sc) = rules::score_place(terms, &buf, &guild_f) else { continue };
                    if !s.can_view(c) {
                        continue;
                    }
                    let unread = c.has_text_chat() && s.is_unread(c);
                    out.push(Cand {
                        score: sc + boost + points(c.id) + cue(unread, s.mention_count(c.id)) - if s.is_channel_muted(c) { 12 } else { 0 },
                        acc: ai,
                        kind: place_kind(c),
                        guild: Some(*gid),
                        channel: Some(c.id),
                        user: None,
                    });
                }
            }
            if in_guild {
                continue;
            }
            let mut dm_with: std::collections::HashSet<Id> = Default::default();
            for c in s.private_channels.iter().filter_map(|c| s.channels.get(c)) {
                if c.kind == channel_type::DM {
                    dm_with.extend(c.recipient_ids.iter().copied());
                }
                let d = dm_item(&s, c);
                fold_into(&mut buf, &d.name);
                let Some(sc) = rules::score_place(terms, &buf, "") else { continue };
                out.push(Cand { score: sc + boost + 4 + points(c.id) + cue(d.unread && !d.muted, d.mentions) - if d.muted { 12 } else { 0 }, acc: ai, kind: place_kind(c), guild: None, channel: Some(c.id), user: None });
            }
            // Friends with no DM yet: opening one starts it.
            if !terms.is_empty() {
                for r in s.relationships.iter().filter(|r| r.kind == 1) {
                    let uid = r.user_id.or(r.user.as_ref().map(|u| u.id)).unwrap_or(r.id);
                    if dm_with.contains(&uid) {
                        continue;
                    }
                    let name = s.display_name(None, uid);
                    fold_into(&mut buf, &name);
                    let uname = s.users.get(&uid).map(|u| rules::fold(&u.username)).unwrap_or_default();
                    if let Some(sc) = rules::score_place(terms, &buf, &uname) {
                        out.push(Cand { score: sc + boost, acc: ai, kind: PlaceKind::Person, guild: None, channel: None, user: Some(uid) });
                    }
                }
            }
        }
        out
    }

    /// What the empty query lists: places with something for you, then the ones you opened last.
    fn recent_places(&self, req: &FindRequest, accounts: &[Id], active: Option<Id>, recents: &rules::Recents) -> Vec<PlaceSection> {
        let want_guild = (req.scope == SearchScopeKind::Server).then(|| req.guild_id.as_deref().map(id)).flatten();
        let current = req.channel_id.as_deref();
        let mut unread: Vec<(i32, Cand)> = Vec::new();
        let mut recent: Vec<Cand> = Vec::new();
        for (ai, &account) in accounts.iter().enumerate() {
            let Some(sess) = self.inner.session(account) else { continue };
            let s = sess.store.read();
            let acc_s = account.to_string();
            for (cid, ch) in &s.channels {
                if ch.kind == channel_type::GUILD_CATEGORY || ch.is_archived() || Some(cid.to_string().as_str()) == current && Some(account) == active {
                    continue;
                }
                if want_guild.is_some() && ch.guild_id != want_guild {
                    continue;
                }
                let mentions = s.mention_count(*cid);
                if (mentions > 0 || (ch.has_text_chat() && s.is_unread(ch))) && !s.is_channel_muted(ch) && s.can_view(ch) && (!ch.is_thread() || ch.member.is_some()) {
                    let key = mentions as i32 * 1000 + recents.points(&acc_s, &cid.to_string(), now_ms());
                    unread.push((key, Cand { score: key, acc: ai, kind: place_kind(ch), guild: ch.guild_id, channel: Some(*cid), user: None }));
                }
            }
        }
        unread.sort_by_key(|(k, _)| std::cmp::Reverse(*k));
        unread.truncate(5);
        let skip: std::collections::HashSet<(usize, Option<Id>)> = unread.iter().map(|(_, c)| (c.acc, c.channel)).collect();
        for v in recents.latest(40) {
            let Some(ai) = accounts.iter().position(|a| a.to_string() == v.account) else { continue };
            let ch = id(&v.channel);
            if Some(v.channel.as_str()) == current && Some(accounts[ai]) == active || skip.contains(&(ai, Some(ch))) {
                continue;
            }
            let Some(sess) = self.inner.session(accounts[ai]) else { continue };
            let s = sess.store.read();
            let Some(c) = s.channels.get(&ch) else { continue };
            if (want_guild.is_some() && c.guild_id != want_guild) || !s.can_view(c) || c.is_archived() {
                continue;
            }
            recent.push(Cand { score: 0, acc: ai, kind: place_kind(c), guild: c.guild_id, channel: Some(ch), user: None });
            if recent.len() == 8 {
                break;
            }
        }
        let mut sections = Vec::new();
        for (title, cands) in [("Mentions and unread", unread.into_iter().map(|(_, c)| c).collect::<Vec<_>>()), ("Recent", recent)] {
            let places: Vec<PlaceHit> = cands.iter().filter_map(|c| self.build_place(c, accounts, active, &[])).collect();
            if !places.is_empty() {
                sections.push(PlaceSection { title: title.into(), places });
            }
        }
        sections
    }

    /// One message record from a stored or fetched message, in the words and marks the rows draw.
    fn message_hit(&self, st: &Store, account: Id, active: Option<Id>, multi: bool, m: &Message, author_fallback: &str, terms: &[String]) -> MessageHit {
        let guild = st.channels.get(&m.channel_id).and_then(|c| c.guild_id);
        let author = if m.author.id.0 != 0 { st.display_name(guild, m.author.id) } else { author_fallback.to_string() };
        let plain = messages::preview_text(st, guild, &m.content, 4000);
        let text = rules::excerpt(&plain, terms, MESSAGE_CHARS);
        MessageHit {
            account_id: account.to_string(),
            guild_id: guild.map(|g| g.to_string()),
            channel_id: m.channel_id.to_string(),
            message_id: m.id.to_string(),
            author,
            avatar_url: if m.author.id.0 != 0 { m.author.avatar_url(64) } else { String::new() },
            marks: ranges(&text, terms),
            text,
            location: messages::location(st, m.channel_id),
            timestamp_ms: m.id.timestamp_ms(),
            other_account: multi && Some(account) != active,
            account_name: if multi { st.me.as_ref().map(|u| u.display_name().to_string()).unwrap_or_default() } else { String::new() },
        }
    }
}

/// `from:name` `in:channel` and the other filters of a typed query, resolved on this device.
struct LocalFilters {
    words: Vec<String>,
    authors: Vec<String>,
    channels: Vec<Id>,
    min_id: Option<u64>,
    max_id: Option<u64>,
    /// Filters the text index cannot answer.
    discord_only: bool,
    any: bool,
}

/// The name the on-device index stores for the person `typed` means (their global name), found
/// through the same fuzzy rule as everywhere else (friend nickname included). An unknown name
/// stays as typed, so it still matches author text literally.
fn local_author(eng: &UnisonEngine, active: Option<Id>, typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let resolved = active.and_then(|a| eng.inner.session(a)).and_then(|sess| {
        let s = sess.store.read();
        let (uid, _) = *s.users_matching(typed).first()?;
        s.users.get(&uid).map(|u| u.display_name().to_string())
    });
    Some(resolved.unwrap_or_else(|| typed.to_string()))
}

fn local_filters(eng: &UnisonEngine, accounts: &[Id], active: Option<Id>, guild: Option<Id>, query: &str, tz: i32) -> LocalFilters {
    let find_channel = |name: &str| -> Option<String> {
        let n = rules::fold(name);
        let mut found = None;
        for &a in accounts {
            let Some(sess) = eng.inner.session(a) else { continue };
            let s = sess.store.read();
            for c in s.channels.values().filter(|c| c.guild_id.is_some() && !c.is_thread()) {
                if c.name.as_deref().is_some_and(|x| rules::fold(x) == n) {
                    // A name in the server being searched wins over the same name elsewhere.
                    if c.guild_id == guild && Some(a) == active {
                        return Some(c.id.to_string());
                    }
                    found.get_or_insert(c.id.to_string());
                }
            }
        }
        found
    };
    let p = unison_core::search_syntax::parse(query, tz, |n| local_author(eng, active, n), |n| (!n.is_empty()).then(|| find_channel(n)).flatten());
    let discord_only = !p.has.is_empty() || p.pinned.is_some() || !p.mentions.is_empty();
    let min = p.min_id.as_deref().and_then(|s| s.parse().ok());
    let max = p.max_id.as_deref().and_then(|s| s.parse().ok());
    LocalFilters {
        any: !p.author_ids.is_empty() || !p.channel_ids.is_empty() || min.is_some() || max.is_some() || discord_only,
        words: p.content.split_whitespace().map(str::to_string).collect(),
        authors: p.author_ids,
        channels: p.channel_ids.iter().map(|c| id(c)).collect(),
        min_id: min,
        max_id: max,
        discord_only,
    }
}

#[uniffi::export]
impl UnisonEngine {
    /// Everything the palette shows from this device for what was typed: places, people, and a
    /// page of messages from the on-device index. Fast enough to call on every keystroke (from a
    /// background thread); never touches the network.
    pub fn find_local(&self, req: FindRequest) -> FindResult {
        let t0 = std::time::Instant::now();
        let now = now_ms();
        let active = self.inner.active_id();
        let me = id(&req.account_id);
        let guild = req.guild_id.as_deref().map(id);
        let scope = match req.scope {
            SearchScopeKind::Server if guild.is_none() => SearchScopeKind::Channel,
            s => s,
        };
        let req = FindRequest { scope, ..req };
        let accounts: Vec<Id> = if scope == SearchScopeKind::Everywhere { self.inner.accounts() } else { vec![me] };
        let multi = self.inner.accounts().len() > 1;
        let f = local_filters(self, &accounts, active, guild, &req.query, req.tz_offset_secs);
        let terms: Vec<String> = f.words.iter().map(|w| rules::fold(w)).filter(|t| t.chars().any(char::is_alphanumeric)).collect();
        let empty = req.query.trim().is_empty();
        let recents = self.inner.local.recents();

        // Places and people: only for plain words, and not when scoped to one channel.
        let mut sections = Vec::new();
        if scope != SearchScopeKind::Channel && !f.any {
            if empty {
                sections = self.recent_places(&req, &accounts, active, &recents);
            } else if !terms.is_empty() {
                let mut cands = self.scan_places(&req, &accounts, active, &terms, &recents, now);
                cands.sort_by_key(|c| std::cmp::Reverse(c.score));
                let mut counts = [0usize; 6];
                let mut best = [i32::MIN; 6];
                let mut kept: Vec<Cand> = Vec::new();
                for c in cands {
                    let sct = section_of(c.kind);
                    if counts[sct] < SECTION_CAPS[sct].1 {
                        counts[sct] += 1;
                        best[sct] = best[sct].max(c.score);
                        kept.push(c);
                    }
                }
                let mut order: Vec<usize> = (0..6).filter(|&i| counts[i] > 0).collect();
                order.sort_by_key(|&i| std::cmp::Reverse(best[i]));
                for i in order {
                    let places: Vec<PlaceHit> = kept.iter().filter(|c| section_of(c.kind) == i).filter_map(|c| self.build_place(c, &accounts, active, &terms)).collect();
                    sections.push(PlaceSection { title: SECTION_CAPS[i].0.into(), places });
                }
            }
        }

        // Messages from the on-device index.
        let mut messages = Vec::new();
        let (mut total, mut capped) = (0u32, false);
        if !empty && !f.discord_only && (!terms.is_empty() || f.any) {
            let mut q = unison_core::cache::LocalQuery { terms: f.words.clone(), min_id: f.min_id, max_id: f.max_id, authors: f.authors.clone(), limit: req.limit.max(1), want_total: true, ..Default::default() };
            let scoped: Option<Vec<Id>> = if !f.channels.is_empty() {
                Some(f.channels.clone())
            } else {
                match scope {
                    SearchScopeKind::Channel => Some(req.channel_id.as_deref().map(|c| vec![id(c)]).unwrap_or_default()),
                    SearchScopeKind::Server => me_session_channels(self, me, guild),
                    SearchScopeKind::Everywhere => None,
                }
            };
            q.channels = scoped;
            let single = accounts.len() == 1;
            // Several accounts: each is asked for enough to merge, then the page is cut from the merge.
            if single {
                q.offset = req.offset;
            } else {
                q.limit = req.offset + req.limit.max(1);
            }
            let mut hits: Vec<(Id, unison_core::cache::LocalHit)> = Vec::new();
            for &a in &accounts {
                let Some(sess) = self.inner.session(a) else { continue };
                let page = sess.search_local_page(&q);
                total += page.total;
                capped |= page.capped;
                hits.extend(page.hits.into_iter().map(|h| (a, h)));
            }
            if !single {
                rules::sort_active_first(&mut hits, |(a, _)| Some(*a) == active, |(_, h)| h.message.0);
                hits = hits.into_iter().skip(req.offset as usize).take(req.limit.max(1) as usize).collect();
            }
            for (a, h) in hits {
                let Some(sess) = self.inner.session(a) else { continue };
                let st = sess.store.read();
                let m = h.full.clone().unwrap_or_else(|| Message { id: h.message, channel_id: h.channel, content: h.content.clone(), ..Default::default() });
                messages.push(self.message_hit(&st, a, active, multi, &m, &h.author, &terms));
            }
        }

        // What asking Discord would mean from here.
        let discord_scope = if guild.is_some() && scope != SearchScopeKind::Channel { SearchScopeKind::Server } else { SearchScopeKind::Channel };
        let discord_label = self
            .with_store(&req.account_id, |s| {
                let place = match (discord_scope, req.channel_id.as_deref().map(id).and_then(|c| s.channels.get(&c))) {
                    (SearchScopeKind::Server, _) => guild.and_then(|g| s.guilds.get(&g)).map(|g| g.name().to_string()).unwrap_or_default(),
                    (_, Some(c)) if c.guild_id.is_some() => format!("#{}", c.name.as_deref().unwrap_or("")),
                    (_, Some(c)) => dm_item(s, c).name,
                    _ => String::new(),
                };
                place
            })
            .unwrap_or_default();
        FindResult {
            sections,
            messages,
            message_total: total,
            message_total_capped: capped,
            discord_only: f.discord_only,
            discord_scope,
            discord_label,
            empty_query: empty,
            micros: t0.elapsed().as_micros() as u64,
        }
    }

    /// One page (25) of Discord's own search for the typed query, as the official client asks it:
    /// the server's endpoint, or the DM's, with the filters of the typed grammar. Only called on a
    /// gesture; answered from memory when the same page was fetched a moment ago.
    pub async fn find_discord(&self, req: FindRequest) -> Result<DiscordPage, FfiError> {
        let account = id(&req.account_id);
        let s = self.inner.session(account).ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let channel = req.channel_id.clone().ok_or_else(|| FfiError::Failed { message: "no chat to search".into() })?;
        let scope = if req.guild_id.is_some() && req.scope != SearchScopeKind::Channel { SearchScopeKind::Server } else { SearchScopeKind::Channel };
        let filters = self.search_parse(req.account_id.clone(), req.guild_id.clone(), channel.clone(), req.query.clone(), scope, req.tz_offset_secs, req.offset);
        let terms: Vec<String> = filters.content.split_whitespace().map(rules::fold).filter(|t| t.chars().any(char::is_alphanumeric)).collect();
        let active = self.inner.active_id();
        let multi = self.inner.accounts().len() > 1;
        let guild = req.guild_id.clone();
        let jh = self.inner.rt.spawn(async move {
            let v = s.search_filtered(guild.as_deref().map(id), id(&channel), &filters.query()).await?;
            Ok::<_, unison_core::Error>(v)
        });
        let v = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        let st = self.inner.session(account).ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let st = st.store.read();
        let mut out = Vec::new();
        for group in v["messages"].as_array().into_iter().flatten() {
            // Each group is context around a hit; the hit carries `hit: true`.
            let hit = group.as_array().and_then(|g| g.iter().find(|m| m["hit"] == true).or_else(|| g.first()));
            let Some(m) = hit.and_then(|m| serde_json::from_value::<Message>(m.clone()).ok()) else { continue };
            out.push(self.message_hit(&st, account, active, multi, &m, "", &terms));
        }
        Ok(DiscordPage { total: v["total_results"].as_u64().unwrap_or(0) as u32, messages: out })
    }
}

/// Every channel of a server (threads included) in an account, for a server-wide local search.
fn me_session_channels(eng: &UnisonEngine, account: Id, guild: Option<Id>) -> Option<Vec<Id>> {
    let g = guild?;
    eng.with_store(&account.to_string(), |s| s.guild_channels.get(&g).cloned().unwrap_or_default())
}
