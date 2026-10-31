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
    /// What exactly is searched: "#general", "Unison HQ", "Every chat on this device, all accounts".
    pub detail: String,
    pub placeholder: String,
    /// Whether Return can ask Discord for older results here.
    pub asks_discord: bool,
}

#[uniffi::export]
impl UnisonEngine {
    /// The places the search can look from here, broadest first (the order of the scope chips).
    pub fn search_scopes(&self, account_id: String, guild_id: Option<String>, channel_id: String) -> Vec<SearchScopeItem> {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return vec![];
        };
        let st = s.store.read();
        let guild = guild_id.as_deref().map(id);
        let chan = st.channels.get(&id(&channel_id));
        let chan_name = chan.map(|c| if c.guild_id.is_some() { c.name.clone().unwrap_or_default() } else { dm_item(&st, c).name }).unwrap_or_default();
        let guild_name = guild.and_then(|g| st.guilds.get(&g)).map(|g| g.name().to_string()).unwrap_or_default();
        unison_core::search_syntax::Scope::tabs(guild.is_some())
            .iter()
            .map(|&sc| {
                use unison_core::search_syntax::Scope;
                let (kind, title, detail, placeholder) = match sc {
                    Scope::Server => (SearchScopeKind::Server, "This server".to_string(), guild_name.clone(), format!("Search {guild_name}")),
                    Scope::Channel if guild.is_some() => (SearchScopeKind::Channel, "This channel".to_string(), format!("#{chan_name}"), format!("Search #{chan_name}")),
                    Scope::Channel => (SearchScopeKind::Channel, "This chat".to_string(), chan_name.clone(), format!("Search {chan_name}")),
                    Scope::Everywhere => (SearchScopeKind::Everywhere, "Everywhere".to_string(), "Every chat on this device, all accounts".to_string(), "Jump to a chat, or search every message".to_string()),
                };
                SearchScopeItem { kind, title, detail, placeholder, asks_discord: sc.asks_discord() }
            })
            .collect()
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
