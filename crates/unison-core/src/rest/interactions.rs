use super::*;

impl Rest {
    /// `POST /interactions` (component, command, autocomplete and modal submit all use it).
    /// The body comes from `crate::interactions`.
    pub async fn interact(&self, intent: UserIntent, body: Value) -> Result<()> {
        let _: Value = self.write(intent, Method::POST, "/interactions", Some(body)).await?;
        Ok(())
    }

    /// Choose poll answers: the full set (empty removes the vote).
    pub async fn poll_vote(&self, intent: UserIntent, channel: Id, message: Id, answers: &[u32]) -> Result<()> {
        let path = format!("/channels/{channel}/polls/{message}/answers/@me");
        let body = crate::interactions::poll_vote_body(answers);
        let _: Value = self.write(intent, Method::PUT, &path, Some(body)).await?;
        Ok(())
    }

    /// Follow (`POST`) or leave (`DELETE`) a thread: `/channels/{thread}/thread-members/@me`.
    /// capture: the web client appends `?location=<analytics location>`.
    pub async fn thread_membership(&self, intent: UserIntent, thread: Id, join: bool) -> Result<()> {
        let path = format!("/channels/{thread}/thread-members/@me");
        let method = if join { Method::POST } else { Method::DELETE };
        let _: Value = self.write(intent, method, &path, None).await?;
        Ok(())
    }

    /// `GET /channels/{id}`: one channel by id, no side effects. Used to show a thread this account
    /// has not joined (the official client's `ThreadLoader.loadThread`, FIDELITY "Unjoined threads").
    pub async fn channel(&self, channel: Id) -> Result<Channel> {
        self.get(&format!("/channels/{channel}")).await
    }

    /// Start a thread on a message (`CHANNEL_MESSAGE_THREADS`) or without one (`CHANNEL_THREADS`).
    /// Body as the web composer builds it; capture: its `location` field.
    pub async fn create_thread(
        &self,
        intent: UserIntent,
        channel: Id,
        message: Option<Id>,
        name: &str,
        kind: u8,
        auto_archive_minutes: u32,
    ) -> Result<Channel> {
        let path = match message {
            Some(m) => format!("/channels/{channel}/messages/{m}/threads"),
            None => format!("/channels/{channel}/threads"),
        };
        let body = json!({"name": name, "type": kind, "auto_archive_duration": auto_archive_minutes});
        self.write(intent, Method::POST, &path, Some(body)).await
    }
}

impl Rest {
    /// The slash commands usable in a channel: `GET /guilds/{g}/application-command-index` in a
    /// server, `/channels/{c}/application-command-index` in a DM. capture: both paths are what the
    /// web client's `ApplicationCommandIndex` fetch uses; no query parameters.
    pub async fn command_index(&self, guild: Option<Id>, channel: Id) -> Result<Value> {
        match guild {
            Some(g) => self.get(&format!("/guilds/{g}/application-command-index")).await,
            None => self.get(&format!("/channels/{channel}/application-command-index")).await,
        }
    }
}
