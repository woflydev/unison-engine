use super::*;

impl Rest {
    /// Decline a ring or stop ringing someone: `POST /channels/{id}/call/stop-ringing {recipients}`.
    pub async fn stop_ringing(
        &self,
        intent: UserIntent,
        channel: Id,
        recipients: Vec<Id>,
    ) -> Result<()> {
        let ids: Vec<String> = recipients.iter().map(|r| r.to_string()).collect();
        let _: Value = self
            .write(
                intent,
                Method::POST,
                &format!("/channels/{channel}/call/stop-ringing"),
                Some(json!({ "recipients": ids })),
            )
            .await?;
        Ok(())
    }

    /// Ring the other side of a DM call we just joined: `POST /channels/{id}/call/ring`, everyone
    /// (`recipients: null`).
    pub async fn ring_call(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::POST,
                &format!("/channels/{channel}/call/ring"),
                Some(json!({ "recipients": null })),
            )
            .await?;
        Ok(())
    }
}
