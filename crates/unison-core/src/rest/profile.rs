use super::*;

impl Rest {
    /// The private note you keep on someone (`GET /users/@me/notes/{id}`); empty when there is none.
    pub async fn user_note(&self, user: Id) -> String {
        let v: Result<Value> = self.get(&format!("/users/@me/notes/{user}")).await;
        v.ok().and_then(|v| v["note"].as_str().map(str::to_string)).unwrap_or_default()
    }

    /// `PUT /users/@me/notes/{id} {note}` (up to 256 characters; empty clears it).
    pub async fn set_user_note(&self, intent: UserIntent, user: Id, note: &str) -> Result<()> {
        let _: Value = self.write(intent, Method::PUT, &format!("/users/@me/notes/{user}"), Some(json!({ "note": note }))).await?;
        Ok(())
    }

    pub async fn user_profile(&self, user: Id, guild: Option<Id>) -> Result<Value> {
        let mut path = format!(
            "/users/{user}/profile?type=popout&with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false"
        );
        if let Some(g) = guild {
            path.push_str(&format!("&guild_id={g}"));
        }
        self.get(&path).await
    }
}
