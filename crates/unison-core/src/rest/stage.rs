//! Scheduled events and stage channels.
use super::*;

impl Rest {
    /// Mark or unmark interest in an event (`capture`: the user-scoped route is read from memory of the
    /// web client, see docs/FIDELITY.md).
    pub async fn set_event_interest(&self, intent: UserIntent, guild: Id, event: Id, interested: bool) -> Result<()> {
        let method = if interested { Method::PUT } else { Method::DELETE };
        let _: Value = self
            .write(intent, method, &format!("/guilds/{guild}/scheduled-events/{event}/users/@me"), None)
            .await?;
        Ok(())
    }

    /// Raise or lower your hand in a stage (documented "Modify Current User Voice State").
    pub async fn stage_request_to_speak(&self, intent: UserIntent, guild: Id, channel: Id, raise: bool) -> Result<()> {
        let ts = if raise { json!(crate::timefmt::ms_to_iso(now_ms())) } else { Value::Null };
        let _: Value = self
            .write(
                intent,
                Method::PATCH,
                &format!("/guilds/{guild}/voice-states/@me"),
                Some(json!({ "channel_id": channel.to_string(), "request_to_speak_timestamp": ts })),
            )
            .await?;
        Ok(())
    }

    /// Become a speaker yourself (needs Mute Members) or move to the audience.
    pub async fn stage_set_suppress(&self, intent: UserIntent, guild: Id, channel: Id, user: Option<Id>, suppress: bool) -> Result<()> {
        let who = user.map_or("@me".to_string(), |u| u.to_string());
        let _: Value = self
            .write(
                intent,
                Method::PATCH,
                &format!("/guilds/{guild}/voice-states/{who}"),
                Some(json!({ "channel_id": channel.to_string(), "suppress": suppress })),
            )
            .await?;
        Ok(())
    }

    /// `POST /guilds/{g}/scheduled-events`. Body shape is the documented one; the official client's
    /// extras (recurrence rule, analytics) are not sent (`capture`, docs/FIDELITY.md).
    pub async fn create_event(&self, intent: UserIntent, guild: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::POST, &format!("/guilds/{guild}/scheduled-events"), Some(body)).await
    }

    pub async fn edit_event(&self, intent: UserIntent, guild: Id, event: Id, body: Value) -> Result<Value> {
        self.write(intent, Method::PATCH, &format!("/guilds/{guild}/scheduled-events/{event}"), Some(body)).await
    }

    pub async fn delete_event(&self, intent: UserIntent, guild: Id, event: Id) -> Result<()> {
        let _: Value = self.write(intent, Method::DELETE, &format!("/guilds/{guild}/scheduled-events/{event}"), None).await?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
