use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct InboxItem {
    pub account_id: String,
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub author: String,
    pub preview: String,
    pub location: String,
    pub timestamp_ms: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchHitItem {
    pub channel_id: String,
    pub message_id: String,
    pub author: String,
    pub snippet: String,
    pub location: String,
    pub timestamp_ms: u64,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum SearchScopeKind {
    Server,
    Channel,
    Everywhere,
}

/// One place the search box can look, with the words the shell shows for it.
#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchScopeItem {
    pub kind: SearchScopeKind,
    /// Short tab title: "This server", "This channel", "Everywhere".
    pub title: String,
    /// What exactly is searched: "#general", "Unison HQ", "Every chat you've loaded".
    pub detail: String,
    pub placeholder: String,
    /// Whether Return can ask Discord for older results here.
    pub asks_discord: bool,
}

fn core_scope(k: SearchScopeKind) -> unison_core::search_syntax::Scope {
    use unison_core::search_syntax::Scope;
    match k {
        SearchScopeKind::Server => Scope::Server,
        SearchScopeKind::Channel => Scope::Channel,
        SearchScopeKind::Everywhere => Scope::Everywhere,
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug)]
pub enum SwitchKind {
    Account,
    Guild,
    Channel,
    Dm,
    /// A message found by content; `message_id` says which.
    Message,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SwitchItem {
    pub account_id: String,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub kind: SwitchKind,
    pub title: String,
    pub subtitle: String,
    pub score: i32,
    pub message_id: Option<String>,
}

/// Subsequence match with bonuses for prefix and word starts. `None` if not a match.
pub(crate) fn fuzzy(q: &str, s: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    if s.is_empty() {
        return None;
    }
    let sb = s.as_bytes();
    let mut score = 0i32;
    let mut si = 0usize;
    let mut last = None;
    for qc in q.bytes() {
        let found = sb[si..].iter().position(|&c| c == qc)?;
        let at = si + found;
        score += 1;
        if at == 0 {
            score += 8;
        } else if matches!(sb[at - 1], b' ' | b'-' | b'_' | b'#') {
            score += 6;
        }
        match last {
            Some(prev) if at == prev + 1 => score += 4,
            Some(prev) => score -= ((at - prev - 1) as i32).min(3),
            None => score -= ((at as i32) / 2).min(3),
        }
        last = Some(at);
        si = at + 1;
    }
    if s.starts_with(q) {
        score += 10;
    }
    Some(score - (s.len() as i32 / 8))
}

#[uniffi::export]
impl UnisonEngine {
    /// The places the toolbar search can look from here, default first.
    pub fn search_scopes(&self, account_id: String, guild_id: Option<String>, channel_id: String) -> Vec<SearchScopeItem> {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return vec![];
        };
        let st = s.store.read();
        let guild = guild_id.as_deref().map(id);
        let chan = st.channels.get(&id(&channel_id));
        let chan_name = chan.map(|c| if c.guild_id.is_some() { c.name.clone().unwrap_or_default() } else { dm_item(&st, c).name }).unwrap_or_default();
        let guild_name = guild.and_then(|g| st.guilds.get(&g)).map(|g| g.name().to_string()).unwrap_or_default();
        unison_core::search_syntax::Scope::options(guild.is_some())
            .iter()
            .map(|&sc| {
                use unison_core::search_syntax::Scope;
                let (kind, title, detail, placeholder) = match sc {
                    Scope::Server => (SearchScopeKind::Server, "This server".to_string(), guild_name.clone(), format!("Search {guild_name}")),
                    Scope::Channel if guild.is_some() => (SearchScopeKind::Channel, "This channel".to_string(), format!("#{chan_name}"), format!("Search #{chan_name}")),
                    Scope::Channel => (SearchScopeKind::Channel, "This chat".to_string(), chan_name.clone(), format!("Search {chan_name}")),
                    Scope::Everywhere => (SearchScopeKind::Everywhere, "Everywhere".to_string(), "Every chat you've loaded".to_string(), "Search all chats".to_string()),
                };
                SearchScopeItem { kind, title, detail, placeholder, asks_discord: sc.asks_discord() }
            })
            .collect()
    }

    pub fn search_local(&self, account_id: String, guild_id: Option<String>, channel_id: String, scope: SearchScopeKind, query: String) -> Vec<SearchHitItem> {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return vec![];
        };
        let hits = s.search_local_scoped(core_scope(scope), guild_id.as_deref().map(id), id(&channel_id), &query);
        let st = s.store.read();
        hits.into_iter()
            .map(|h| SearchHitItem {
                location: location(&st, h.channel),
                channel_id: h.channel.to_string(),
                message_id: h.message.to_string(),
                author: h.author,
                snippet: preview_text(&st, st.channels.get(&h.channel).and_then(|c| c.guild_id), &h.snippet, 200),
                timestamp_ms: h.message.timestamp_ms(),
            })
            .collect()
    }

    /// ⌘K: fuzzy search over accounts, servers, channels and DMs across every signed-in account.
    pub fn quick_switch(&self, query: String) -> Vec<SwitchItem> {
        let q = query.trim().to_lowercase();
        let mut out = Vec::new();
        let active = self.inner.active_id();
        for account in self.inner.accounts() {
            let Some(sess) = self.inner.session(account) else {
                continue;
            };
            let s = sess.store.read();
            let acc = account.to_string();
            let acc_name =
                s.me.as_ref()
                    .map(|u| u.display_name().to_string())
                    .unwrap_or_default();
            let boost = if Some(account) == active { 5 } else { 0 };
            let mut push = |kind,
                            guild: Option<Id>,
                            channel: Option<Id>,
                            title: String,
                            subtitle: String,
                            extra: i32| {
                if let Some(score) = fuzzy(&q, &title.to_lowercase()) {
                    out.push(SwitchItem {
                        account_id: acc.clone(),
                        guild_id: guild.map(|g| g.to_string()),
                        channel_id: channel.map(|c| c.to_string()),
                        kind,
                        title,
                        subtitle,
                        score: score + boost + extra,
                        message_id: None,
                    });
                }
            };
            push(
                SwitchKind::Account,
                None,
                None,
                acc_name.clone(),
                "Account".into(),
                -2,
            );
            for (gid, g) in &s.guilds {
                push(
                    SwitchKind::Guild,
                    Some(*gid),
                    None,
                    g.name().to_string(),
                    acc_name.clone(),
                    0,
                );
                for c in s
                    .guild_channels
                    .get(gid)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| s.channels.get(c))
                {
                    if c.kind == channel_type::GUILD_CATEGORY || c.is_thread() || !s.can_view(c) {
                        continue;
                    }
                    let unread = if s.is_unread(c) { 2 } else { 0 };
                    push(
                        SwitchKind::Channel,
                        Some(*gid),
                        Some(c.id),
                        c.name.clone().unwrap_or_default(),
                        g.name().to_string(),
                        unread,
                    );
                }
            }
            for c in s.private_channels.iter().filter_map(|c| s.channels.get(c)) {
                let d = dm_item(&s, c);
                push(
                    SwitchKind::Dm,
                    None,
                    Some(c.id),
                    d.name,
                    "Direct Message".into(),
                    3,
                );
            }
        }
        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.title.len().cmp(&b.title.len()))
        });
        out.truncate(40);
        // Messages from every chat follow the places: words only, so a short query stays fast.
        if query.trim().chars().count() >= 3 {
            for account in self.inner.accounts() {
                let Some(sess) = self.inner.session(account) else { continue };
                let hits = sess.search_local(query.trim());
                let st = sess.store.read();
                for h in hits.into_iter().take(6) {
                    out.push(SwitchItem {
                        account_id: account.to_string(),
                        guild_id: st.channels.get(&h.channel).and_then(|c| c.guild_id).map(|g| g.to_string()),
                        channel_id: Some(h.channel.to_string()),
                        kind: SwitchKind::Message,
                        title: preview_text(&st, st.channels.get(&h.channel).and_then(|c| c.guild_id), &h.snippet, 200),
                        subtitle: format!("{} · {}", h.author, location(&st, h.channel)),
                        score: 0,
                        message_id: Some(h.message.to_string()),
                    });
                }
            }
        }
        out
    }

    pub fn inbox(&self) -> Vec<InboxItem> {
        self.inner
            .inbox()
            .into_iter()
            .map(|e| {
                let (loc, preview) = self
                    .inner
                    .session(e.account)
                    .map(|s| {
                        let st = s.store.read();
                        (location(&st, e.channel), preview_text(&st, e.guild, &e.preview, 140))
                    })
                    .unwrap_or_else(|| (String::new(), e.preview.clone()));
                InboxItem {
                    account_id: e.account.to_string(),
                    guild_id: e.guild.map(|g| g.to_string()),
                    channel_id: e.channel.to_string(),
                    message_id: e.message.to_string(),
                    author: e.author,
                    preview,
                    location: loc,
                    timestamp_ms: e.message.timestamp_ms(),
                }
            })
            .collect()
    }

    pub fn clear_inbox(&self) {
        self.inner.clear_inbox();
    }
}
