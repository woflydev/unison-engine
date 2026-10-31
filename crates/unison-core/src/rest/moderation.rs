//! Moderation and server administration. Shapes follow Discord's documented API; reads need no
//! gesture, every change does. Moderation actions take an optional reason, sent as `X-Audit-Log-Reason`.
use super::*;

impl Rest {
    pub async fn kick_member(&self, intent: UserIntent, guild: Id, user: Id, reason: &str) -> Result<()> {
        let _: Value = self.write_reason(intent, Method::DELETE, &format!("/guilds/{guild}/members/{user}"), None, reason).await?;
        Ok(())
    }

    /// Ban and delete up to 7 days of the member's messages.
    pub async fn ban_member(&self, intent: UserIntent, guild: Id, user: Id, delete_message_seconds: u64, reason: &str) -> Result<()> {
        let _: Value = self
            .write_reason(
                intent,
                Method::PUT,
                &format!("/guilds/{guild}/bans/{user}"),
                Some(json!({ "delete_message_seconds": delete_message_seconds.min(604_800) })),
                reason,
            )
            .await?;
        Ok(())
    }

    pub async fn unban_member(&self, intent: UserIntent, guild: Id, user: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/guilds/{guild}/bans/{user}"), None).await?;
        Ok(())
    }

    /// `until` is an ISO timestamp, or `None` to remove the timeout.
    pub async fn timeout_member(&self, intent: UserIntent, guild: Id, user: Id, until: Option<String>, reason: &str) -> Result<()> {
        let _: Value = self
            .write_reason(
                intent,
                Method::PATCH,
                &format!("/guilds/{guild}/members/{user}"),
                Some(json!({ "communication_disabled_until": until })),
                reason,
            )
            .await?;
        Ok(())
    }

    /// Change a nickname; `user` of `None` is yourself. An empty nick resets it.
    pub async fn set_nickname(&self, intent: UserIntent, guild: Id, user: Option<Id>, nick: &str) -> Result<()> {
        let who = user.map_or("@me".to_string(), |u| u.to_string());
        let nick = if nick.is_empty() { Value::Null } else { json!(nick) };
        let _: Value = self
            .write(intent, Method::PATCH, &format!("/guilds/{guild}/members/{who}"), Some(json!({ "nick": nick })))
            .await?;
        Ok(())
    }

    pub async fn set_member_role(&self, intent: UserIntent, guild: Id, user: Id, role: Id, add: bool) -> Result<()> {
        let method = if add { Method::PUT } else { Method::DELETE };
        let _: Value = self.write(intent, method, &format!("/guilds/{guild}/members/{user}/roles/{role}"), None).await?;
        Ok(())
    }

    pub async fn guild_bans(&self, guild: Id) -> Result<Value> {
        self.get(&format!("/guilds/{guild}/bans?limit=200")).await
    }

    pub async fn guild_audit_log(&self, guild: Id) -> Result<Value> {
        self.get(&format!("/guilds/{guild}/audit-logs?limit=50")).await
    }

    pub async fn guild_invites(&self, guild: Id) -> Result<Value> {
        self.get(&format!("/guilds/{guild}/invites")).await
    }

    pub async fn revoke_invite(&self, intent: UserIntent, code: &str) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/invites/{code}"), None).await?;
        Ok(())
    }

    /// `PATCH /guilds/{id}` with only the fields that changed.
    pub async fn edit_guild(&self, intent: UserIntent, guild: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::PATCH, &format!("/guilds/{guild}"), Some(body)).await
    }

    pub async fn create_role(&self, intent: UserIntent, guild: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::POST, &format!("/guilds/{guild}/roles"), Some(body)).await
    }

    pub async fn edit_role(&self, intent: UserIntent, guild: Id, role: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::PATCH, &format!("/guilds/{guild}/roles/{role}"), Some(body)).await
    }

    pub async fn delete_role(&self, intent: UserIntent, guild: Id, role: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/guilds/{guild}/roles/{role}"), None).await?;
        Ok(())
    }

    pub async fn edit_channel(&self, intent: UserIntent, channel: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::PATCH, &format!("/channels/{channel}"), Some(body)).await
    }

    pub async fn delete_channel(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/channels/{channel}"), None).await?;
        Ok(())
    }

    /// `POST /guilds/{id}/emojis {name, image: data URI}`.
    pub async fn create_emoji(&self, intent: UserIntent, guild: Id, name: &str, image: &str) -> Result<Value> {
        self.write(intent, Method::POST, &format!("/guilds/{guild}/emojis"), Some(json!({ "name": name, "image": image, "roles": [] }))).await
    }

    pub async fn rename_emoji(&self, intent: UserIntent, guild: Id, emoji: Id, name: &str) -> Result<()> {
        let _: Value = self.write(intent, Method::PATCH, &format!("/guilds/{guild}/emojis/{emoji}"), Some(json!({ "name": name }))).await?;
        Ok(())
    }

    pub async fn delete_emoji(&self, intent: UserIntent, guild: Id, emoji: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/guilds/{guild}/emojis/{emoji}"), None).await?;
        Ok(())
    }

    pub async fn delete_sticker(&self, intent: UserIntent, guild: Id, sticker: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/guilds/{guild}/stickers/{sticker}"), None).await?;
        Ok(())
    }

    /// What an invite leads to (`GET /invites/{code}`), with member counts.
    pub async fn invite_preview(&self, code: &str) -> Result<Value> {
        self.get(&format!("/invites/{code}?with_counts=true&with_expiration=true")).await
    }

    /// Join through an invite (`POST /invites/{code}`). `capture`: the web client sends the gateway
    /// `session_id`; Discord may answer with a captcha, which comes back as an error.
    pub async fn accept_invite(&self, intent: UserIntent, code: &str, session_id: &str) -> Result<Value> {
        self.write(intent, Method::POST, &format!("/invites/{code}"), Some(json!({ "session_id": session_id }))).await
    }

    /// Create a server (`POST /guilds`); the response is the new guild.
    pub async fn create_guild(&self, intent: UserIntent, body: Value) -> Result<Value> {
        self.write(intent, Method::POST, "/guilds", Some(body)).await
    }

    /// The membership screening form (`GET /guilds/{id}/member-verification`). `capture`.
    pub async fn member_verification(&self, guild: Id) -> Result<Value> {
        self.get(&format!("/guilds/{guild}/member-verification?with_guild=false")).await
    }

    /// Pass screening (`PUT /guilds/{id}/requests/@me`, the form echoed with responses). `capture`.
    pub async fn submit_member_verification(&self, intent: UserIntent, guild: Id, body: Value) -> Result<()> {
        let _: Value = self.write(intent, Method::PUT, &format!("/guilds/{guild}/requests/@me"), Some(body)).await?;
        Ok(())
    }

    /// The onboarding prompts (`GET /guilds/{id}/onboarding`). `capture`.
    pub async fn onboarding(&self, guild: Id) -> Result<Value> {
        self.get(&format!("/guilds/{guild}/onboarding")).await
    }

    /// Answer onboarding (`POST /guilds/{id}/onboarding-responses`). `capture`.
    pub async fn submit_onboarding(&self, intent: UserIntent, guild: Id, body: Value) -> Result<()> {
        let _: Value = self.write(intent, Method::POST, &format!("/guilds/{guild}/onboarding-responses"), Some(body)).await?;
        Ok(())
    }

    /// `POST /guilds/{g}/channels` (documented body; `capture` for the official client's extras).
    pub async fn create_channel(&self, intent: UserIntent, guild: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::POST, &format!("/guilds/{guild}/channels"), Some(body)).await
    }

    /// `PATCH /guilds/{g}/channels` with `[{id, position}]` for the channels that moved.
    pub async fn reorder_channels(&self, intent: UserIntent, guild: Id, changes: Vec<Value>) -> Result<()> {
        let _: Value = self.write(intent, Method::PATCH, &format!("/guilds/{guild}/channels"), Some(Value::Array(changes))).await?;
        Ok(())
    }

    /// `PATCH /guilds/{g}/roles` with `[{id, position}]` for the roles that moved.
    pub async fn reorder_roles(&self, intent: UserIntent, guild: Id, changes: Vec<Value>) -> Result<()> {
        let _: Value = self.write(intent, Method::PATCH, &format!("/guilds/{guild}/roles"), Some(Value::Array(changes))).await?;
        Ok(())
    }

    /// `PUT /channels/{c}/permissions/{target}`.
    pub async fn put_overwrite(&self, intent: UserIntent, channel: Id, target: Id, body: Value) -> Result<()> {
        let _: Value = self.write(intent, Method::PUT, &format!("/channels/{channel}/permissions/{target}"), Some(body)).await?;
        Ok(())
    }

    pub async fn delete_overwrite(&self, intent: UserIntent, channel: Id, target: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/channels/{channel}/permissions/{target}"), None).await?;
        Ok(())
    }
}
