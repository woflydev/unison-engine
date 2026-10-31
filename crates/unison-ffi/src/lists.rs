//! List shaping every shell would otherwise repeat: friends tabs, the unread list, the "new
//! messages" count and the CDN resize URL.

use super::*;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum FriendsTab {
    Online,
    All,
    Pending,
    Blocked,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FriendsView {
    pub items: Vec<FriendItem>,
    /// Incoming requests, for the tab's badge.
    pub pending: u32,
}

/// Which header popups have something in them (see `header_dots`).
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct HeaderDots {
    pub pins: bool,
    pub saved: bool,
    pub threads: bool,
    /// The Inbox's Unreads tab has rows (the Mentions tab is fetched when opened, so it is not counted).
    pub inbox: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct UnreadList {
    pub dms: Vec<DmItem>,
    pub channels: Vec<ChannelItem>,
}

/// Messages newer than the read marker in the open channel.
#[derive(uniffi::Record, Clone, Debug)]
pub struct NewSince {
    /// Capped at `NEW_CAP`; `capped` says there are more.
    pub count: u32,
    pub capped: bool,
    pub first_id: Option<String>,
    pub first_timestamp_ms: Option<u64>,
}

const NEW_CAP: u32 = 50;

pub(crate) fn filter_friends(all: Vec<FriendItem>, online: &[String], tab: FriendsTab, query: &str) -> FriendsView {
    let pending = all.iter().filter(|f| f.kind == RelationshipKind::Incoming).count() as u32;
    let q = query.to_lowercase();
    let items = all
        .into_iter()
        .filter(|f| match tab {
            FriendsTab::Online => f.kind == RelationshipKind::Friend && online.contains(&f.user_id),
            FriendsTab::All => f.kind == RelationshipKind::Friend,
            FriendsTab::Pending => matches!(f.kind, RelationshipKind::Incoming | RelationshipKind::Outgoing),
            FriendsTab::Blocked => f.kind == RelationshipKind::Blocked,
        })
        .filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q) || f.username.to_lowercase().contains(&q))
        .collect();
    FriendsView { items, pending }
}

#[uniffi::export]
impl UnisonEngine {
    pub fn friends_view(&self, account_id: String, tab: FriendsTab, query: String) -> FriendsView {
        let online: Vec<String> = self
            .friend_statuses(account_id.clone())
            .into_iter()
            .filter(|s| s.status != "offline")
            .map(|s| s.user_id)
            .collect();
        filter_friends(self.friends(account_id), &online, tab, &query)
    }

    /// Unread, unmuted DMs and (in `guild_id`, if given) channels. Categories never count.
    pub fn unread_list(&self, account_id: String, guild_id: Option<String>) -> UnreadList {
        let dms = self.private_channels(account_id.clone()).into_iter().filter(|d| d.unread && !d.muted).collect();
        let channels = guild_id
            .map(|g| self.guild_channels(account_id, g))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.unread && !c.muted && c.kind != ChannelKind::Category)
            .collect();
        UnreadList { dms, channels }
    }

    /// Which header popups (Pins, Saved, Threads, Inbox) have something in them, from data already
    /// held (no request). The shell draws a small neutral dot on the icon of each that does.
    pub fn header_dots(&self, account_id: String, channel_id: Option<String>, guild_id: Option<String>) -> HeaderDots {
        let c = channel_id.as_deref().map(id);
        let saved = !self.inner.local.saved().is_empty();
        let unreads = self.unread_list(account_id.clone(), guild_id);
        let inbox = !unreads.dms.is_empty() || !unreads.channels.is_empty();
        let (pins, threads) = self
            .with_store(&account_id, |s| {
                let ch = c.and_then(|c| s.channels.get(&c));
                let parent = ch.and_then(|ch| if ch.is_thread() { ch.parent_id } else { Some(ch.id) });
                let in_guild = ch.is_some_and(|ch| ch.guild_id.is_some());
                (
                    c.is_some_and(|c| s.has_pins(c)),
                    in_guild && parent.is_some_and(|p| s.has_active_threads(p)),
                )
            })
            .unwrap_or((false, false));
        HeaderDots { pins, saved, threads, inbox }
    }

    pub fn new_since(&self, account_id: String, channel_id: String, marker: String) -> NewSince {
        let marker: u64 = marker.parse().unwrap_or(0);
        let c = id(&channel_id);
        self.with_store(&account_id, |s| {
            let guild = s.channels.get(&c).and_then(|c| c.guild_id);
            let newer: Vec<&Message> = s.history.get(&c).map(|h| h.messages.iter().filter(|m| m.id.0 > marker).collect()).unwrap_or_default();
            let first = newer.first().map(|m| message_item(s, guild, m));
            NewSince {
                count: (newer.len() as u32).min(NEW_CAP),
                capped: newer.len() as u32 >= NEW_CAP,
                first_id: first.as_ref().map(|m| m.id.clone()),
                first_timestamp_ms: first.map(|m| m.timestamp_ms),
            }
        })
        .unwrap_or(NewSince { count: 0, capped: false, first_id: None, first_timestamp_ms: None })
    }
}

/// `url` rewritten to ask Discord's media proxy for a small WebP of this pixel size.
#[uniffi::export]
pub fn resized_media_url(url: String, width: f64, height: f64) -> String {
    unison_core::media_url::resized(&url, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, name: &str, kind: RelationshipKind) -> FriendItem {
        FriendItem { user_id: id.into(), name: name.into(), username: name.to_lowercase(), avatar_url: String::new(), kind }
    }

    #[test]
    fn friends_tabs_and_filter() {
        let all = vec![
            f("1", "Ann", RelationshipKind::Friend),
            f("2", "Bob", RelationshipKind::Friend),
            f("3", "Cy", RelationshipKind::Incoming),
            f("4", "Di", RelationshipKind::Outgoing),
            f("5", "Ed", RelationshipKind::Blocked),
        ];
        let online = vec!["2".to_string()];
        assert_eq!(filter_friends(all.clone(), &online, FriendsTab::Online, "").items.len(), 1);
        assert_eq!(filter_friends(all.clone(), &online, FriendsTab::All, "an").items.len(), 1);
        let p = filter_friends(all.clone(), &online, FriendsTab::Pending, "");
        assert_eq!((p.items.len(), p.pending), (2, 1));
        assert_eq!(filter_friends(all, &online, FriendsTab::Blocked, "").items[0].user_id, "5");
    }
}
