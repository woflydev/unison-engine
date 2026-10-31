use super::*;

impl Rest {
    /// Close a 1:1 DM (it only disappears from the list) or leave a group DM.
    /// Only ever call this for a private channel: on a guild channel the same request deletes it.
    pub async fn close_private_channel(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::DELETE,
                &format!("/channels/{channel}?silent=false"),
                None,
            )
            .await?;
        Ok(())
    }

    /// Accept a message request so the DM joins the normal list (`capture`: endpoint and empty body are
    /// a guess, see docs/FIDELITY.md). Ignoring is `close_private_channel`.
    pub async fn accept_message_request(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let _: Value = self
            .write(intent, Method::PUT, &format!("/channels/{channel}/recipients/@me"), None)
            .await?;
        Ok(())
    }

    /// Mute or unmute a DM: `PATCH /users/@me/guilds/@me/settings` with a channel override. Muting
    /// is indefinite ("until I turn it back on": no end time, window -1).
    pub async fn set_dm_muted(&self, intent: UserIntent, channel: Id, muted: bool) -> Result<()> {
        let o = if muted {
            json!({ "muted": true, "mute_config": { "end_time": null, "selected_time_window": -1 } })
        } else {
            json!({ "muted": false, "mute_config": null })
        };
        let _: Value = self
            .write(
                intent,
                Method::PATCH,
                "/users/@me/guilds/@me/settings",
                Some(json!({ "channel_overrides": { channel.to_string(): o } })),
            )
            .await?;
        Ok(())
    }

    pub async fn open_dm(&self, intent: UserIntent, user: Id) -> Result<Channel> {
        self.write(
            intent,
            Method::POST,
            "/users/@me/channels",
            Some(json!({ "recipients": [user.to_string()] })),
        )
        .await
    }
}
