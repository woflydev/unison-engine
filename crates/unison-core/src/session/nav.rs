//! Session commands for navigation and notification state (mark read in bulk, mentions inbox, filtered search).
use super::*;
use crate::rest::{SearchQuery, channel_override_body, mute_body};

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Session {
    /// Channels of a guild (or children of a category) that are unread or carry mentions, with the message to ack.
    pub fn unread_targets(&self, guild: Id, category: Option<Id>) -> Vec<(Id, Id)> {
        let s = self.store.read();
        s.guild_channels
            .get(&guild)
            .into_iter()
            .flatten()
            .filter_map(|c| s.channels.get(c))
            .filter(|c| category.is_none_or(|p| c.parent_id == Some(p)))
            .filter(|c| s.is_unread(c) || s.mention_count(c.id) > 0)
            .filter_map(|c| c.last_message_id.map(|m| (c.id, m)))
            .collect()
    }

    /// The unread channels of several servers (a folder), in one list so they go out as one ack.
    pub fn unread_targets_in(&self, guilds: &[Id]) -> Vec<(Id, Id)> {
        let mut seen = std::collections::HashSet::new();
        guilds.iter().flat_map(|g| self.unread_targets(*g, None)).filter(|(c, _)| seen.insert(*c)).collect()
    }

    /// "Mark as Read" for a server or category: one `ack-bulk` (read-state ack bodies per FIDELITY), then local state.
    pub async fn mark_read_bulk(&self, intent: UserIntent, items: Vec<(Id, Id)>) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        self.rest.bulk_ack(intent, &items).await?;
        let mut deltas = Vec::with_capacity(items.len());
        let mut s = self.store.write();
        for (channel, message) in items {
            let rs = s
                .read_states
                .entry(channel)
                .or_insert_with(|| crate::model::ReadStateEntry { id: channel, ..Default::default() });
            rs.last_message_id = Some(message);
            rs.mention_count = 0;
            deltas.push(Delta::ReadState { channel });
        }
        drop(s);
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(())
    }

    /// "Mark Unread" on a message: manually ack the message just before it (or the snowflake
    /// one below it when it is the oldest loaded), so it and everything after reads as new.
    pub async fn mark_unread(&self, intent: UserIntent, channel: Id, message: Id) -> Result<()> {
        let prev = {
            let s = self.store.read();
            s.history
                .get(&channel)
                .and_then(|h| h.messages.iter().rev().find(|m| m.id.0 != 0 && m.id < message).map(|m| m.id))
                .unwrap_or(Snowflake(message.0.saturating_sub(1)))
        };
        self.rest.ack_manual(intent, channel, prev).await?;
        let mut s = self.store.write();
        let rs = s.read_states.entry(channel).or_insert_with(|| crate::model::ReadStateEntry { id: channel, ..Default::default() });
        rs.last_message_id = Some(prev);
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::ReadState { channel }]));
        Ok(())
    }

    /// Mute a guild (`window_secs` None unmutes).
    pub async fn mute_guild(&self, intent: UserIntent, guild: Id, window_secs: Option<i64>) -> Result<()> {
        self.update_guild_settings(intent, guild, mute_body(window_secs, now_ms())).await
    }

    /// Mute a guild channel or category through `channel_overrides`.
    pub async fn mute_channel(&self, intent: UserIntent, guild: Id, channel: Id, window_secs: Option<i64>) -> Result<()> {
        self.update_guild_settings(intent, guild, channel_override_body(channel, mute_body(window_secs, now_ms()))).await
    }

    /// Server-wide level: 0 all messages, 1 only mentions, 2 nothing.
    pub async fn set_guild_notifications(&self, intent: UserIntent, guild: Id, level: u8) -> Result<()> {
        self.update_guild_settings(intent, guild, serde_json::json!({ "message_notifications": level })).await
    }

    /// Per-channel level: 0, 1, 2, or 3 to inherit the server's.
    pub async fn set_channel_notifications(&self, intent: UserIntent, guild: Id, channel: Id, level: u8) -> Result<()> {
        self.update_guild_settings(intent, guild, channel_override_body(channel, serde_json::json!({ "message_notifications": level }))).await
    }

    pub async fn set_suppress(&self, intent: UserIntent, guild: Id, everyone: Option<bool>, roles: Option<bool>) -> Result<()> {
        let mut b = serde_json::Map::new();
        if let Some(v) = everyone {
            b.insert("suppress_everyone".into(), v.into());
        }
        if let Some(v) = roles {
            b.insert("suppress_roles".into(), v.into());
        }
        self.update_guild_settings(intent, guild, Value::Object(b)).await
    }

    pub async fn leave_guild(&self, intent: UserIntent, guild: Id) -> Result<()> {
        self.rest.leave_guild(intent, guild).await
    }

    /// Inbox "Mentions" tab. `before` pages backwards.
    pub async fn recent_mentions(&self, before: Option<Id>, guild: Option<Id>) -> Result<Vec<Message>> {
        self.rest.mentions(before, 25, guild, true, true).await
    }

    /// How long a Discord search page stays good: the same search inside this window is answered
    /// from memory (ARCHITECTURE rule 10). A search is only ever sent on a gesture, never faster
    /// than a person asks, and never while automation is on (`rest::set_automation`).
    pub const SEARCH_FRESH: std::time::Duration = std::time::Duration::from_secs(45);

    /// What Discord's search returned joins the on-device index (hits and their context messages).
    fn index_found(&self, response: &Value) {
        let mut by_channel: std::collections::HashMap<Id, Vec<Message>> = std::collections::HashMap::new();
        for group in response.get("messages").and_then(Value::as_array).into_iter().flatten() {
            for m in group.as_array().into_iter().flatten() {
                if let Ok(m) = serde_json::from_value::<Message>(m.clone()) {
                    by_channel.entry(m.channel_id).or_default().push(m);
                }
            }
        }
        for (channel, msgs) in by_channel {
            let _ = self.cache.index_found(channel, &msgs);
        }
    }

    pub async fn search_filtered(&self, guild: Option<Id>, channel: Id, q: &SearchQuery) -> Result<Value> {
        let key = format!("{guild:?}/{channel}?{}", q.to_query());
        if let Some(v) = self.search_pages.lock().get(&key, Self::SEARCH_FRESH) {
            return Ok(v);
        }
        let flight = format!("search:{key}");
        if !self.reads.begin(&flight, std::time::Duration::ZERO) {
            return Err(Error::Other("That search is already running.".into()));
        }
        let r = self.rest.search(guild, channel, q).await;
        self.reads.finish(&flight, false);
        if let Ok(v) = &r {
            self.search_pages.lock().put(key, v.clone());
            self.index_found(v);
        }
        r
    }
}
