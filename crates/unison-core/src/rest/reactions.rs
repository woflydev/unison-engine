use super::*;

impl Rest {
    pub async fn react(
        &self,
        intent: UserIntent,
        channel: Id,
        message: Id,
        emoji: &str,
        add: bool,
    ) -> Result<()> {
        let path = format!(
            "/channels/{channel}/messages/{message}/reactions/{}/@me?location=Message&type=0",
            encode(emoji)
        );
        let method = if add { Method::PUT } else { Method::DELETE };
        let _: Value = self.write(intent, method, &path, None).await?;
        Ok(())
    }

    /// Who reacted with an emoji (`GET .../reactions/{emoji}`): `limit` users after `after` (paged by user id). `capture`: `type=0` as the web client sends.
    pub async fn reaction_users(&self, channel: Id, message: Id, emoji: &str, limit: u32, after: Option<Id>) -> Result<Value> {
        let after = after.map(|a| format!("&after={a}")).unwrap_or_default();
        self.get(&format!("/channels/{channel}/messages/{message}/reactions/{}?limit={limit}&type=0{after}", encode(emoji))).await
    }
}
