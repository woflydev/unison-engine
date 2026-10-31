use super::*;

impl Rest {
    /// Send a friend request or accept one (`type` absent), or block (`type` 2):
    /// `PUT /users/@me/relationships/{id}`.
    pub async fn put_relationship(
        &self,
        intent: UserIntent,
        user: Id,
        kind: Option<u8>,
    ) -> Result<()> {
        let body = kind.map_or(json!({}), |t| json!({ "type": t }));
        let _: Value = self
            .write(
                intent,
                Method::PUT,
                &format!("/users/@me/relationships/{user}"),
                Some(body),
            )
            .await?;
        Ok(())
    }

    /// Set or clear (`None`) the nickname only you see for a friend:
    /// `PATCH /users/@me/relationships/{id}` `{nickname}`. `capture`: body shape unverified.
    pub async fn set_friend_nickname(&self, intent: UserIntent, user: Id, nickname: Option<&str>) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::PATCH,
                &format!("/users/@me/relationships/{user}"),
                Some(json!({ "nickname": nickname })),
            )
            .await?;
        Ok(())
    }

    /// Unfriend, cancel a request, ignore a request or unblock: `DELETE /users/@me/relationships/{id}`.
    pub async fn delete_relationship(&self, intent: UserIntent, user: Id) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::DELETE,
                &format!("/users/@me/relationships/{user}"),
                None,
            )
            .await?;
        Ok(())
    }
}
