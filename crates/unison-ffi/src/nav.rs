//! Navigation, notification and social commands: mark read, mute and notification levels, the
//! mentions inbox, filtered search, friends by username, group DMs.
use super::*;

use unison_core::rest::{MUTE_WINDOWS, SearchQuery};

/// Muted-until choices the official menus offer, in seconds; -1 is "until I turn it back on".
#[uniffi::export]
pub fn mute_windows() -> Vec<i64> {
    MUTE_WINDOWS.to_vec()
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct NotifyState {
    pub muted: bool,
    /// 0 all messages, 1 only mentions, 2 nothing, 3 inherit (channel overrides only).
    pub level: u8,
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MentionItem {
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub author: String,
    pub avatar_url: String,
    /// Plain text with mentions shown as names (`messages::preview_text`); never the raw message.
    pub content: String,
    /// The same text with its styling (role and user pills), for the inbox row.
    pub rich: RichText,
    pub location: String,
    pub timestamp_ms: u64,
}

#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct SearchFilters {
    pub content: String,
    pub author_ids: Vec<String>,
    pub channel_ids: Vec<String>,
    pub mentions: Vec<String>,
    pub has: Vec<String>,
    pub pinned: Option<bool>,
    /// Snowflake bounds (`before:` / `after:` / `during:` become `max_id` / `min_id`).
    pub min_id: Option<String>,
    pub max_id: Option<String>,
    pub offset: u32,
    pub oldest: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchResult {
    pub channel_id: String,
    pub message_id: String,
    pub author: String,
    pub avatar_url: String,
    pub content: String,
    pub location: String,
    pub timestamp_ms: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SearchPage {
    pub total: u32,
    pub results: Vec<SearchResult>,
}

impl SearchFilters {
    fn query(&self) -> SearchQuery {
        let ids = |v: &Vec<String>| v.iter().map(|s| id(s)).collect::<Vec<_>>();
        SearchQuery {
            content: self.content.clone(),
            author_ids: ids(&self.author_ids),
            channel_ids: ids(&self.channel_ids),
            mentions: ids(&self.mentions),
            has: self.has.clone(),
            pinned: self.pinned,
            min_id: self.min_id.as_deref().map(id),
            max_id: self.max_id.as_deref().map(id),
            offset: self.offset,
            oldest: self.oldest,
        }
    }
}

fn window(secs: i64) -> Option<i64> {
    (secs != 0).then_some(secs)
}

#[uniffi::export]
impl UnisonEngine {
    /// Server "Mark as Read".
    pub fn mark_guild_read(&self, account_id: String, guild_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            let items = s.unread_targets(id(&guild_id), None);
            s.mark_read_bulk(i, items).await
        });
    }

    /// Category "Mark as Read" (its unread children).
    pub fn mark_category_read(&self, account_id: String, guild_id: String, category_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            let items = s.unread_targets(id(&guild_id), Some(id(&category_id)));
            s.mark_read_bulk(i, items).await
        });
    }

    /// "Mark Unread" from a message's menu (`capture`: manual ack of the preceding message).
    pub fn mark_message_unread(&self, account_id: String, channel_id: String, message_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.mark_unread(i, id(&channel_id), id(&message_id)).await
        });
    }

    /// Mark one channel read (the manual ack the context menu sends).
    pub fn mark_channel_read(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.mark_read(i, id(&channel_id)).await
        });
    }

    /// Unread channel ids in a guild in channel order, for Alt-Shift-Up/Down and the jump bar.
    pub fn unread_channels(&self, account_id: String, guild_id: String) -> Vec<String> {
        let g = id(&guild_id);
        self.with_store(&account_id, |s| {
            s.guild_channels
                .get(&g)
                .into_iter()
                .flatten()
                .filter_map(|c| s.channels.get(c))
                .filter(|c| (s.is_unread(c) || s.mention_count(c.id) > 0) && !s.is_channel_muted(c))
                .map(|c| c.id.to_string())
                .collect()
        })
        .unwrap_or_default()
    }

    /// The channel's read marker (last acked message id) when it has newer messages: where the NEW
    /// separator goes. Call before opening the channel, which acks it.
    pub fn read_marker(&self, account_id: String, channel_id: String) -> Option<String> {
        let c = id(&channel_id);
        self.with_store(&account_id, |s| {
            let ch = s.channels.get(&c)?;
            s.is_unread(ch).then_some(())?;
            s.read_states.get(&c)?.last_message_id.map(|m| m.to_string())
        })
        .flatten()
    }

    /// Total mentions across a guild's channels (rail badge) and whether any unmuted channel is unread.
    pub fn guild_unread(&self, account_id: String, guild_id: String) -> GuildUnread {
        let g = id(&guild_id);
        self.with_store(&account_id, |s| {
            let mut out = GuildUnread { mentions: 0, unread: false };
            for c in s.guild_channels.get(&g).into_iter().flatten().filter_map(|c| s.channels.get(c)) {
                if s.is_channel_muted(c) {
                    continue;
                }
                out.mentions += s.mention_count(c.id);
                out.unread |= s.is_unread(c);
            }
            out
        })
        .unwrap_or(GuildUnread { mentions: 0, unread: false })
    }

    pub fn guild_notify_state(&self, account_id: String, guild_id: String) -> NotifyState {
        let g = id(&guild_id);
        self.with_store(&account_id, |s| {
            s.guild_settings.get(&g).map_or(
                NotifyState { muted: false, level: 0, suppress_everyone: false, suppress_roles: false },
                |x| NotifyState {
                    muted: x.muted,
                    level: x.message_notifications,
                    suppress_everyone: x.suppress_everyone,
                    suppress_roles: x.suppress_roles,
                },
            )
        })
        .unwrap_or(NotifyState { muted: false, level: 0, suppress_everyone: false, suppress_roles: false })
    }

    pub fn channel_notify_state(&self, account_id: String, guild_id: String, channel_id: String) -> NotifyState {
        let (g, c) = (id(&guild_id), id(&channel_id));
        self.with_store(&account_id, |s| {
            let o = s.guild_settings.get(&g).and_then(|x| x.channel_overrides.iter().find(|o| o.channel_id == c));
            NotifyState {
                muted: o.is_some_and(|o| o.muted),
                level: o.map_or(3, |o| o.message_notifications),
                suppress_everyone: false,
                suppress_roles: false,
            }
        })
        .unwrap_or(NotifyState { muted: false, level: 3, suppress_everyone: false, suppress_roles: false })
    }

    /// `window_secs`: one of `mute_windows()`, or 0 to unmute.
    pub fn set_guild_muted(&self, account_id: String, guild_id: String, window_secs: i64, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.mute_guild(i, id(&guild_id), window(window_secs)).await
        });
    }

    /// Mute a channel or category (not a DM: use `set_dm_muted`).
    pub fn set_channel_muted(&self, account_id: String, guild_id: String, channel_id: String, window_secs: i64, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.mute_channel(i, id(&guild_id), id(&channel_id), window(window_secs)).await
        });
    }

    pub fn set_channel_notification_level(&self, account_id: String, guild_id: String, channel_id: String, level: u8, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.set_channel_notifications(i, id(&guild_id), id(&channel_id), level.min(3)).await
        });
    }

    pub fn set_guild_suppress(&self, account_id: String, guild_id: String, everyone: Option<bool>, roles: Option<bool>, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.set_suppress(i, id(&guild_id), everyone, roles).await
        });
    }

    pub fn leave_guild(&self, account_id: String, guild_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.leave_guild(i, id(&guild_id)).await
        });
    }

    /// Add a friend by username (`POST /users/@me/relationships`).
    pub fn add_friend_by_username(&self, account_id: String, username: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.rest.add_friend_by_username(i, username.trim()).await
        });
    }

    /// Create a group DM; navigation arrives via `on_navigate`.
    pub fn create_group_dm(&self, account_id: String, user_ids: Vec<String>, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        let intent = gesture.intent();
        let listener = self.listener.clone();
        let users: Vec<Id> = user_ids.iter().map(|u| id(u)).collect();
        self.inner.rt.spawn(async move {
            match s.rest.create_group_dm(intent, &users).await {
                Ok(c) => listener.on_navigate(account_id, None, c.id.to_string()),
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }

    /// Create an invite link for a channel; returns the full `discord.gg` URL.
    pub async fn create_invite(&self, account_id: String, channel_id: String, max_age_secs: u32, max_uses: u32, gesture: UserGesture) -> Result<String, FfiError> {
        let s = self
            .inner
            .session(id(&account_id))
            .ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let intent = gesture.intent();
        let jh = self.inner.rt.spawn(async move { s.rest.create_invite(intent, id(&channel_id), max_age_secs, max_uses, false).await });
        let code = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(format!("https://discord.gg/{code}"))
    }

    pub fn add_group_member(&self, account_id: String, channel_id: String, user_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.rest.add_recipient(i, id(&channel_id), id(&user_id)).await
        });
    }

    pub fn remove_group_member(&self, account_id: String, channel_id: String, user_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.rest.remove_recipient(i, id(&channel_id), id(&user_id)).await
        });
    }

    /// Inbox "Mentions" tab: `GET /users/@me/mentions`, paged with `before`.
    pub async fn fetch_mentions(&self, account_id: String, before: Option<String>) -> Result<Vec<MentionItem>, FfiError> {
        let s = self
            .inner
            .session(id(&account_id))
            .ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let jh = self.inner.rt.spawn(async move {
            let msgs = s.recent_mentions(before.as_deref().map(id), None).await?;
            let st = s.store.read();
            Ok::<_, unison_core::Error>(
                msgs.into_iter()
                    .map(|m| MentionItem {
                        guild_id: m.guild_id.map(|g| g.to_string()),
                        channel_id: m.channel_id.to_string(),
                        message_id: m.id.to_string(),
                        author: m.author.display_name().to_string(),
                        avatar_url: m.author.avatar_url(64),
                        content: messages::preview_text(&st, m.guild_id, &m.content, 400),
                        rich: messages::rich(&st, m.guild_id, &m.content),
                        location: messages::location(&st, m.channel_id),
                        timestamp_ms: m.id.timestamp_ms(),
                    })
                    .collect(),
            )
        });
        jh.await
            .map_err(|e| FfiError::Failed { message: e.to_string() })?
            .map_err(Into::into)
    }

    /// Filtered search in a guild (or the DM `channel_id` when `guild_id` is None).
    pub async fn search_messages(
        &self,
        account_id: String,
        guild_id: Option<String>,
        channel_id: String,
        filters: SearchFilters,
    ) -> Result<SearchPage, FfiError> {
        let s = self
            .inner
            .session(id(&account_id))
            .ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let jh = self.inner.rt.spawn(async move {
            let v = s.search_filtered(guild_id.as_deref().map(id), id(&channel_id), &filters.query()).await?;
            let st = s.store.read();
            let mut results = Vec::new();
            for group in v["messages"].as_array().into_iter().flatten() {
                // Each group is context around a hit; the hit carries `hit: true`.
                let hit = group
                    .as_array()
                    .and_then(|g| g.iter().find(|m| m["hit"] == true).or_else(|| g.first()));
                let Some(m) = hit.and_then(|m| serde_json::from_value::<Message>(m.clone()).ok()) else { continue };
                results.push(SearchResult {
                    channel_id: m.channel_id.to_string(),
                    message_id: m.id.to_string(),
                    author: m.author.display_name().to_string(),
                    avatar_url: m.author.avatar_url(64),
                    content: messages::preview_text(&st, st.channels.get(&m.channel_id).and_then(|c| c.guild_id), &m.content, 400),
                    location: messages::location(&st, m.channel_id),
                    timestamp_ms: m.id.timestamp_ms(),
                });
            }
            Ok::<_, unison_core::Error>(SearchPage { total: v["total_results"].as_u64().unwrap_or(0) as u32, results })
        });
        jh.await
            .map_err(|e| FfiError::Failed { message: e.to_string() })?
            .map_err(Into::into)
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct UserRef {
    pub user_id: String,
    pub name: String,
    pub username: String,
    pub avatar_url: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FriendStatus {
    pub user_id: String,
    pub status: String,
}

#[uniffi::export]
impl UnisonEngine {
    /// Users known to this session whose username or display name starts with `prefix`
    /// (search `from:` / `mentions:` autocomplete, group DM picker). At most `limit`.
    pub fn find_users(&self, account_id: String, prefix: String, limit: u32) -> Vec<UserRef> {
        let p = prefix.trim_start_matches('@').to_lowercase();
        self.with_store(&account_id, |s| {
            let mut v: Vec<UserRef> = s
                .users
                .values()
                .filter(|u| {
                    !u.bot
                        && (p.is_empty()
                            || u.username.to_lowercase().starts_with(&p)
                            || u.display_name().to_lowercase().starts_with(&p))
                })
                .map(|u| UserRef {
                    user_id: u.id.to_string(),
                    name: u.display_name().to_string(),
                    username: u.username.clone(),
                    avatar_url: u.avatar_url(64),
                })
                .collect();
            v.sort_by_key(|u| u.name.to_lowercase());
            v.truncate(limit as usize);
            v
        })
        .unwrap_or_default()
    }

    /// Presence of every friend (the friends list "Online" tab).
    pub fn friend_statuses(&self, account_id: String) -> Vec<FriendStatus> {
        self.with_store(&account_id, |s| {
            s.relationships
                .iter()
                .filter(|r| r.kind == 1)
                .map(|r| {
                    let u = r.user_id.or(r.user.as_ref().map(|u| u.id)).unwrap_or(r.id);
                    FriendStatus { user_id: u.to_string(), status: s.presence(u).to_string() }
                })
                .collect()
        })
        .unwrap_or_default()
    }
}

#[derive(uniffi::Record, Clone, Copy, Debug)]
pub struct GuildUnread {
    pub mentions: u32,
    pub unread: bool,
}
