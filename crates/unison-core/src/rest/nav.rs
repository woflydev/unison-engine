//! Navigation, notification and social requests: bulk/guild ack, the mentions inbox, guild and
//! channel notification settings, filtered search, friends by username, group DMs, leaving a guild.
//! Shapes read from the web bundle (see docs/FIDELITY.md, "Navigation and social").
use super::*;

/// Mute durations offered by the official menus, in seconds (`selected_time_window`); -1 is "until I turn it back on".
pub const MUTE_WINDOWS: [i64; 6] = [900, 3600, 10_800, 28_800, 86_400, -1];

/// `mute_config` for a window starting at `now_ms`; `None` window means unmute.
pub fn mute_body(window_secs: Option<i64>, now_ms: u64) -> Value {
    match window_secs {
        None => json!({ "muted": false, "mute_config": null }),
        Some(w) if w < 0 => {
            json!({ "muted": true, "mute_config": { "end_time": null, "selected_time_window": -1 } })
        }
        Some(w) => json!({
            "muted": true,
            "mute_config": { "end_time": iso8601(now_ms + (w as u64) * 1000), "selected_time_window": w }
        }),
    }
}

/// `channel_overrides` patch for one channel (or category).
pub fn channel_override_body(channel: Id, over: Value) -> Value {
    json!({ "channel_overrides": { channel.to_string(): over } })
}

/// UTC `YYYY-MM-DDTHH:MM:SS.mmmZ` (what `Date#toISOString` produces).
pub fn iso8601(ms: u64) -> String {
    crate::timefmt::ms_to_iso(ms)
}

/// Search filters, serialised as the web client's `queryKey`s: repeated keys for lists.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct SearchQuery {
    pub content: String,
    pub author_ids: Vec<Id>,
    pub channel_ids: Vec<Id>,
    pub mentions: Vec<Id>,
    pub has: Vec<String>,
    pub pinned: Option<bool>,
    pub min_id: Option<Id>,
    pub max_id: Option<Id>,
    pub offset: u32,
    /// Sort oldest first instead of newest first.
    pub oldest: bool,
}

impl SearchQuery {
    /// Query string without the leading `?`.
    pub fn to_query(&self) -> String {
        let mut p: Vec<String> = Vec::new();
        for a in &self.author_ids {
            p.push(format!("author_id={a}"));
        }
        for c in &self.channel_ids {
            p.push(format!("channel_id={c}"));
        }
        for m in &self.mentions {
            p.push(format!("mentions={m}"));
        }
        for h in &self.has {
            p.push(format!("has={}", encode(h)));
        }
        if let Some(b) = self.pinned {
            p.push(format!("pinned={b}"));
        }
        if let Some(i) = self.min_id {
            p.push(format!("min_id={i}"));
        }
        if let Some(i) = self.max_id {
            p.push(format!("max_id={i}"));
        }
        let c = self.content.trim();
        if !c.is_empty() {
            p.push(format!("content={}", encode(c)));
        }
        p.push("sort_by=timestamp".into());
        p.push(format!("sort_order={}", if self.oldest { "asc" } else { "desc" }));
        if self.offset > 0 {
            p.push(format!("offset={}", self.offset));
        }
        p.join("&")
    }
}

impl Rest {
    /// `POST /read-states/ack-bulk` `{read_states:[{channel_id,message_id,read_state_type:0}]}`, at most
    /// 100 per request with one second between requests, as `BULK_ACK` does.
    pub async fn bulk_ack(&self, intent: UserIntent, items: &[(Id, Id)]) -> Result<()> {
        intent.redeem()?;
        for (i, chunk) in items.chunks(100).enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let read_states: Vec<Value> = chunk
                .iter()
                .map(|(c, m)| json!({ "channel_id": c.to_string(), "message_id": m.to_string(), "read_state_type": 0 }))
                .collect();
            self.request(Method::POST, "/read-states/ack-bulk", Some(json!({ "read_states": read_states })), None)
                .await?;
        }
        Ok(())
    }

    /// `GET /users/@me/mentions?limit&before&guild_id&roles&everyone` (the inbox "Mentions" tab).
    pub async fn mentions(&self, before: Option<Id>, limit: u32, guild: Option<Id>, roles: bool, everyone: bool) -> Result<Vec<Message>> {
        let mut q = format!("limit={limit}&roles={roles}&everyone={everyone}");
        if let Some(b) = before {
            q.push_str(&format!("&before={b}"));
        }
        if let Some(g) = guild {
            q.push_str(&format!("&guild_id={g}"));
        }
        self.get(&format!("/users/@me/mentions?{q}")).await
    }

    /// `GET /guilds/{id}/messages/search?...` (`/channels/{id}/messages/search` for DMs).
    pub async fn search(&self, guild: Option<Id>, channel: Id, q: &SearchQuery) -> Result<Value> {
        let base = match guild {
            Some(g) => format!("/guilds/{g}/messages/search"),
            None => format!("/channels/{channel}/messages/search"),
        };
        self.get(&format!("{base}?{}", q.to_query())).await
    }

    /// Add a friend by username: `POST /users/@me/relationships {username, discriminator: null}`.
    pub async fn add_friend_by_username(&self, intent: UserIntent, username: &str) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::POST,
                "/users/@me/relationships",
                Some(json!({ "username": username, "discriminator": null })),
            )
            .await?;
        Ok(())
    }

    /// `POST /users/@me/channels {recipients}` with more than one recipient makes a group DM.
    pub async fn create_group_dm(&self, intent: UserIntent, users: &[Id]) -> Result<Channel> {
        let r: Vec<String> = users.iter().map(|u| u.to_string()).collect();
        self.write(intent, Method::POST, "/users/@me/channels", Some(json!({ "recipients": r })))
            .await
    }

    /// `POST /channels/{c}/invites`: a new invite link code (`capture`: body keys from the web
    /// client's invite modal; `max_age` seconds, 0 = never; `max_uses` 0 = unlimited).
    pub async fn create_invite(&self, intent: UserIntent, channel: Id, max_age: u32, max_uses: u32, temporary: bool) -> Result<String> {
        let v: Value = self
            .write(
                intent,
                Method::POST,
                &format!("/channels/{channel}/invites"),
                Some(json!({ "max_age": max_age, "max_uses": max_uses, "temporary": temporary, "flags": 0, "unique": true })),
            )
            .await?;
        v["code"].as_str().map(str::to_string).ok_or_else(|| Error::Other("invite response had no code".into()))
    }

    /// `PUT /channels/{c}/recipients/{u}` adds a member to a group DM.
    pub async fn add_recipient(&self, intent: UserIntent, channel: Id, user: Id) -> Result<()> {
        let _: Value = self
            .write(intent, Method::PUT, &format!("/channels/{channel}/recipients/{user}"), Some(json!({})))
            .await?;
        Ok(())
    }

    /// `DELETE /channels/{c}/recipients/{u}` removes a member (the owner only).
    pub async fn remove_recipient(&self, intent: UserIntent, channel: Id, user: Id) -> Result<()> {
        let _: Value = self
            .write(intent, Method::DELETE, &format!("/channels/{channel}/recipients/{user}"), None)
            .await?;
        Ok(())
    }

    /// `DELETE /users/@me/guilds/{id}` `{lurking:false}`.
    pub async fn leave_guild(&self, intent: UserIntent, guild: Id) -> Result<()> {
        let _: Value = self
            .write(intent, Method::DELETE, &format!("/users/@me/guilds/{guild}"), Some(json!({ "lurking": false })))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake as Id;

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
    }

    #[test]
    fn mute_shapes() {
        assert_eq!(mute_body(None, 0)["muted"], false);
        assert_eq!(mute_body(Some(-1), 0)["mute_config"]["selected_time_window"], -1);
        let b = mute_body(Some(3600), 0);
        assert_eq!(b["mute_config"]["end_time"], "1970-01-01T01:00:00.000Z");
        assert_eq!(b["mute_config"]["selected_time_window"], 3600);
    }

    #[test]
    fn search_query_shape() {
        let q = SearchQuery {
            content: "hello world".into(),
            author_ids: vec![Id(5)],
            channel_ids: vec![Id(6)],
            has: vec!["image".into(), "link".into()],
            pinned: Some(true),
            offset: 25,
            ..Default::default()
        };
        assert_eq!(
            q.to_query(),
            "author_id=5&channel_id=6&has=image&has=link&pinned=true&content=hello%20world&sort_by=timestamp&sort_order=desc&offset=25"
        );
    }
}
