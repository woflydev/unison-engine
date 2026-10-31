use super::*;

impl Rest {
    /// `GET /users/@me/settings-proto/{type}` → the base64 settings blob (`UserSettingsProtoStore`
    /// loads type 2, frecency and favourites, the first time a picker needs it).
    pub async fn settings_proto(&self, kind: u8) -> Result<String> {
        let v: Value = self
            .get(&format!("/users/@me/settings-proto/{kind}"))
            .await?;
        Ok(v["settings"].as_str().unwrap_or_default().to_string())
    }

    /// `PATCH /users/@me/settings-proto/{kind}` with a base64 protobuf; the server merges it into the saved settings.
    pub async fn update_settings_proto(
        &self,
        intent: UserIntent,
        kind: u8,
        settings_b64: &str,
    ) -> Result<()> {
        let _: Value = self
            .write(
                intent,
                Method::PATCH,
                &format!("/users/@me/settings-proto/{kind}"),
                Some(json!({ "settings": settings_b64 })),
            )
            .await?;
        Ok(())
    }

    /// `PATCH /users/@me/settings-proto/{kind}` with `{ settings: base64(partial proto) }`, as the web
    /// client's persistChanges does when online. Returns the full settings (base64) from the response.
    pub async fn patch_settings_proto(
        &self,
        intent: UserIntent,
        kind: u8,
        partial: &[u8],
    ) -> Result<Option<String>> {
        use base64::Engine as _;
        let body = json!({ "settings": base64::engine::general_purpose::STANDARD.encode(partial) });
        let v: Value = self
            .write(
                intent,
                Method::PATCH,
                &format!("/users/@me/settings-proto/{kind}"),
                Some(body),
            )
            .await?;
        Ok(v["settings"].as_str().map(str::to_string))
    }

    /// `PATCH /users/@me` (global_name, avatar data URI, ...).
    pub async fn patch_me(&self, intent: UserIntent, body: Value) -> Result<Value> {
        self.write(intent, Method::PATCH, "/users/@me", Some(body))
            .await
    }

    /// `PATCH /users/@me/profile` (bio, pronouns, accent_color, banner).
    pub async fn patch_profile(&self, intent: UserIntent, body: Value) -> Result<Value> {
        self.write(intent, Method::PATCH, "/users/@me/profile", Some(body))
            .await
    }

    /// `PATCH /users/@me/guilds/{id}/settings` (message_notifications, muted, ...).
    pub async fn patch_guild_settings(
        &self,
        intent: UserIntent,
        guild: Id,
        body: Value,
    ) -> Result<Value> {
        self.write(
            intent,
            Method::PATCH,
            &format!("/users/@me/guilds/{guild}/settings"),
            Some(body),
        )
        .await
    }
}
