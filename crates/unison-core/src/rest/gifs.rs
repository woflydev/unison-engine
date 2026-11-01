//! GIF picker and sticker endpoints, as the web client's `GIFPickerActionCreators` and
//! `MessageActionCreators.sendStickers` call them.

use super::*;

/// `media_format` the desktop (non-Linux) client asks for: `webm`.
const MEDIA_FORMAT: &str = "webm";
const LOCALE: &str = "en-US";

impl Rest {
    /// `GET /gifs/trending`: the category tiles (`{categories: [{name, src}], gifs: [first]}`).
    pub async fn gifs_trending(&self) -> Result<Value> {
        self.get(&format!("/gifs/trending?locale={LOCALE}&media_format={MEDIA_FORMAT}")).await
    }

    /// `GET /gifs/trending-gifs`: the "Trending GIFs" tile's grid.
    pub async fn gifs_trending_gifs(&self, limit: u32) -> Result<Value> {
        self.get(&format!("/gifs/trending-gifs?media_format={MEDIA_FORMAT}&locale={LOCALE}&limit={limit}")).await
    }

    /// `GET /gifs/search`: `[{id, url, src, gif_src, width, height}]`.
    pub async fn gifs_search(&self, q: &str, limit: u32) -> Result<Value> {
        self.get(&format!("/gifs/search?q={}&media_format={MEDIA_FORMAT}&locale={LOCALE}&limit={limit}", encode(q))).await
    }

    /// `POST /gifs/select` after a GIF is picked: tells Discord which result answered `q`.
    pub async fn gifs_select(&self, intent: UserIntent, id: &str, q: &str) -> Result<()> {
        let _: Value = self.write(intent, Method::POST, "/gifs/select", Some(json!({ "id": id, "q": q }))).await?;
        Ok(())
    }

    /// `GET /sticker-packs`: Discord's own sticker packs, `{sticker_packs: [{id, name, stickers: [...]}]}`.
    pub async fn sticker_packs(&self) -> Result<Value> {
        self.get("/sticker-packs").await
    }

    /// A message that is just a sticker: `POST /channels/{c}/messages` with `sticker_ids`.
    pub async fn send_sticker(&self, intent: UserIntent, channel: Id, nonce: Snowflake, sticker: Id) -> Result<()> {
        let body = sticker_body(nonce, sticker);
        let _: Value = self.write(intent, Method::POST, &format!("/channels/{channel}/messages"), Some(body)).await?;
        Ok(())
    }
}

impl Rest {
    /// A message that is just a poll (`crate::poll_create::body`, `capture`).
    pub async fn send_poll(&self, intent: UserIntent, channel: Id, nonce: Snowflake, draft: &crate::poll_create::PollDraft) -> Result<()> {
        let body = crate::poll_create::body(nonce, draft);
        let _: Value = self.write(intent, Method::POST, &format!("/channels/{channel}/messages"), Some(body)).await?;
        Ok(())
    }
}

fn sticker_body(nonce: Snowflake, sticker: Id) -> Value {
    json!({
        "mobile_network_type": "unknown",
        "content": "",
        "nonce": nonce.to_string(),
        "tts": false,
        "flags": 0,
        "sticker_ids": [sticker.to_string()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticker_message_body() {
        let b = sticker_body(Snowflake(5), Snowflake(77));
        assert_eq!(b["sticker_ids"], json!(["77"]));
        assert_eq!(b["content"], "");
        assert_eq!(b["nonce"], "5");
    }
}
