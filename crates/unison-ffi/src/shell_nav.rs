//! Navigation and forwarding commands every shell shares: history, the channel remembered per
//! server, stepping through channels and servers, and the forward sheet's destinations. Shells
//! call these and render the answer; none of this logic is repeated per platform.
use super::*;

use unison_core::navstate::{step, Stop};

#[derive(uniffi::Record, Clone, Debug)]
pub struct NavTarget {
    pub guild_id: Option<String>,
    pub channel_id: String,
}

fn target(s: Stop) -> NavTarget {
    NavTarget { guild_id: s.guild.map(|g| g.to_string()), channel_id: s.channel.to_string() }
}

/// A place the forward sheet can list: Direct Messages (`guild_id` None) or a server.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ForwardScope {
    pub guild_id: Option<String>,
    pub name: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ForwardTarget {
    pub channel_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon_url: Option<String>,
    pub is_dm: bool,
    pub is_group: bool,
}

fn is_chat(k: ChannelKind) -> bool {
    matches!(k, ChannelKind::Text | ChannelKind::Announcement | ChannelKind::Thread | ChannelKind::Dm | ChannelKind::GroupDm)
}

#[uniffi::export]
impl UnisonEngine {
    /// Record opening a channel. `from_history` is set when it was reached by `nav_travel`.
    pub fn nav_visit(&self, account_id: String, guild_id: Option<String>, channel_id: String, from_history: bool) {
        if let Some(s) = self.inner.session(id(&account_id)) {
            s.nav_visit(guild_id.as_deref().map(id), id(&channel_id), from_history);
        }
    }

    /// Back (-1) or forward (1) in history; the shell then opens the returned place with `from_history`.
    pub fn nav_travel(&self, account_id: String, delta: i32) -> Option<NavTarget> {
        self.inner.session(id(&account_id))?.nav.lock().travel(delta).map(target)
    }

    pub fn nav_can_travel(&self, account_id: String, delta: i32) -> bool {
        self.inner.session(id(&account_id)).is_some_and(|s| s.nav.lock().can_travel(delta))
    }

    /// The channel last open in a server (or in DMs when `guild_id` is None).
    pub fn nav_remembered(&self, account_id: String, guild_id: Option<String>) -> Option<String> {
        self.inner.session(id(&account_id))?.nav.lock().remembered(guild_id.as_deref().map(id)).map(|c| c.to_string())
    }

    pub fn nav_forget(&self, account_id: String, guild_id: Option<String>) {
        if let Some(s) = self.inner.session(id(&account_id)) {
            s.nav.lock().forget(guild_id.as_deref().map(id));
        }
    }

    /// Where the previous run ended, to reopen on launch.
    pub fn nav_last(&self, account_id: String) -> Option<NavTarget> {
        self.inner.session(id(&account_id))?.nav.lock().last().map(target)
    }

    /// Alt-Up/Down (and Alt-Shift for unread only): the next chat channel in the sidebar's order.
    pub fn nav_step_channel(&self, account_id: String, guild_id: Option<String>, current: Option<String>, delta: i32, unread_only: bool) -> Option<String> {
        let order: Vec<(String, bool)> = match &guild_id {
            None => self.private_channels(account_id).into_iter().map(|d| (d.channel_id, d.unread || d.mentions > 0)).collect(),
            Some(g) => self
                .guild_channels(account_id, g.clone())
                .into_iter()
                .filter(|c| is_chat(c.kind))
                .map(|c| (c.id, c.unread || c.mentions > 0))
                .collect(),
        };
        let ids: Vec<(usize, bool)> = order.iter().enumerate().map(|(i, (_, u))| (i, *u)).collect();
        let cur = current.and_then(|c| order.iter().position(|(id, _)| *id == c));
        step(&ids, cur.map(|i| i), delta, unread_only).map(|i| order[i].0.clone())
    }

    /// Cmd-Alt-Up/Down: Home, then every server in rail order, wrapping. The answer's `guild_id`
    /// is `None` for Home.
    pub fn nav_step_server(&self, account_id: String, current: Option<String>, delta: i32) -> Option<NavTarget> {
        let mut order: Vec<(Option<String>, bool)> = vec![(None, false)];
        for item in self.rail(account_id) {
            match item {
                RailItem::Guild { guild } => order.push((Some(guild.id), false)),
                RailItem::Folder { guilds, .. } => order.extend(guilds.into_iter().map(|g| (Some(g.id), false))),
            }
        }
        let ids: Vec<(usize, bool)> = (0..order.len()).map(|i| (i, false)).collect();
        let cur = order.iter().position(|(g, _)| *g == current).unwrap_or(0);
        let next = step(&ids, Some(cur), delta, false)?;
        Some(NavTarget { guild_id: order[next].0.clone(), channel_id: String::new() })
    }

    /// Direct Messages first, then every server.
    pub fn forward_scopes(&self, account_id: String) -> Vec<ForwardScope> {
        let mut out = vec![ForwardScope { guild_id: None, name: "Direct Messages".into() }];
        for item in self.rail(account_id) {
            match item {
                RailItem::Guild { guild } => out.push(ForwardScope { guild_id: Some(guild.id), name: guild.name }),
                RailItem::Folder { guilds, .. } => out.extend(guilds.into_iter().map(|g| ForwardScope { guild_id: Some(g.id), name: g.name })),
            }
        }
        out
    }

    /// Where a message can be forwarded within a scope: DMs, or the server's text channels the user can send in.
    pub fn forward_targets(&self, account_id: String, guild_id: Option<String>) -> Vec<ForwardTarget> {
        match guild_id {
            None => self
                .private_channels(account_id)
                .into_iter()
                .map(|d| ForwardTarget { channel_id: d.channel_id, title: d.name, subtitle: d.is_group.then(|| "Group DM".to_string()), icon_url: d.icon_url, is_dm: true, is_group: d.is_group })
                .collect(),
            Some(g) => {
                let name = self.guild(account_id.clone(), g.clone()).map(|x| x.name);
                self.guild_channels(account_id, g)
                    .into_iter()
                    .filter(|c| c.can_send && matches!(c.kind, ChannelKind::Text | ChannelKind::Announcement))
                    .map(|c| ForwardTarget { channel_id: c.id, title: format!("#{}", c.name), subtitle: name.clone(), icon_url: None, is_dm: false, is_group: false })
                    .collect()
            }
        }
    }
}

// ----- Search grammar (rules in unison_core::search_syntax) -----

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchSuggestion {
    /// Inserted after `key:` (or the whole filter key when `title` is the key).
    pub value: String,
    pub title: String,
    pub subtitle: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchSuggestions {
    /// "SEARCH OPTIONS" for the filter list, or the upper-cased filter key for its values.
    pub heading: String,
    /// The `key:` the values complete; `None` for the filter list itself.
    pub key: Option<String>,
    pub items: Vec<SearchSuggestion>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchSplit {
    pub head: String,
    pub last: String,
}

#[uniffi::export]
pub fn search_split(query: String) -> SearchSplit {
    let (h, l) = unison_core::search_syntax::split(&query);
    SearchSplit { head: h.to_string(), last: l.to_string() }
}

#[uniffi::export]
impl UnisonEngine {
    /// A query string as filters for `search_messages`. `tz_offset_secs` is the shell's current
    /// offset from UTC (date filters are local days).
    pub fn search_parse(&self, account_id: String, guild_id: Option<String>, channel_id: String, query: String, scope: SearchScopeKind, tz_offset_secs: i32, offset: u32) -> SearchFilters {
        let channels: Vec<ChannelItem> = guild_id.map(|g| self.guild_channels(account_id.clone(), g)).unwrap_or_default();
        let p = unison_core::search_syntax::parse(
            &query,
            tz_offset_secs,
            |name| {
                let hit = self.find_users(account_id.clone(), name.to_string(), 5).into_iter().find(|u| u.username.eq_ignore_ascii_case(name));
                hit.or_else(|| self.find_users(account_id.clone(), name.to_string(), 1).into_iter().next()).map(|u| u.user_id)
            },
            |name| channels.iter().find(|c| c.name.eq_ignore_ascii_case(name)).map(|c| c.id.clone()),
        );
        let mut channel_ids = p.channel_ids;
        // "This channel" is an `in:` filter the user didn't have to type.
        if scope == SearchScopeKind::Channel && channel_ids.is_empty() {
            channel_ids.push(channel_id);
        }
        SearchFilters {
            content: p.content,
            author_ids: p.author_ids,
            channel_ids,
            mentions: p.mentions,
            has: p.has,
            pinned: p.pinned,
            min_id: p.min_id,
            max_id: p.max_id,
            offset,
            oldest: false,
        }
    }

    /// What to suggest for the token being typed: filter names, or the values of `key:`.
    pub fn search_suggest(&self, account_id: String, guild_id: Option<String>, last: String) -> Option<SearchSuggestions> {
        use unison_core::search_syntax::{FILTERS, HAS_VALUES};
        let lower = last.to_lowercase();
        let pick = |all: &[&str], prefix: &str| -> Vec<SearchSuggestion> {
            all.iter().filter(|v| prefix.is_empty() || v.starts_with(&prefix.to_lowercase())).map(|v| SearchSuggestion { value: v.to_string(), title: v.to_string(), subtitle: String::new() }).collect()
        };
        let items;
        let key;
        if let Some(c) = last.find(':') {
            let k = lower[..c].to_string();
            let val = &last[c + 1..];
            items = match k.as_str() {
                "from" | "mentions" => self
                    .find_users(account_id, val.to_string(), 6)
                    .into_iter()
                    .map(|u| SearchSuggestion { value: u.username.clone(), title: u.name, subtitle: format!("@{}", u.username) })
                    .collect(),
                "in" => {
                    let p = val.trim_start_matches('#').to_lowercase();
                    guild_id
                        .map(|g| self.guild_channels(account_id, g))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|c| c.kind != ChannelKind::Category && (p.is_empty() || c.name.to_lowercase().starts_with(&p)))
                        .take(6)
                        .map(|c| SearchSuggestion { value: c.name.clone(), title: format!("#{}", c.name), subtitle: String::new() })
                        .collect()
                }
                "has" => pick(&HAS_VALUES, val),
                "pinned" => pick(&["true", "false"], val),
                _ => vec![],
            };
            key = Some(k);
        } else {
            let matches: Vec<_> = FILTERS.iter().filter(|(k, _)| lower.is_empty() || k.starts_with(&lower)).collect();
            if matches.is_empty() || lower.len() >= 8 || (!lower.is_empty() && matches.len() == FILTERS.len()) {
                return None;
            }
            items = matches.iter().map(|(k, h)| SearchSuggestion { value: k.to_string(), title: k.to_string(), subtitle: h.to_string() }).collect();
            key = None;
        }
        if items.is_empty() {
            return None;
        }
        let heading = key.as_ref().map_or("SEARCH OPTIONS".to_string(), |k| k.to_uppercase());
        Some(SearchSuggestions { heading, key, items })
    }
}
