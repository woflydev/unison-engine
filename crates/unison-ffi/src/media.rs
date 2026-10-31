use super::*;

#[uniffi::export]
impl UnisonEngine {
    /// True when `url` is on one of Discord's own media hosts, i.e. `fetch_media` will load it.
    pub fn is_media_url(&self, url: String) -> bool {
        unison_core::rest::media::is_media_url(&url)
    }

    /// Bytes of a picture on Discord's media hosts, fetched on the core's transport with the
    /// client's own identity. Shells use this instead of their own HTTP stack for these hosts.
    pub async fn fetch_media(&self, url: String) -> Result<Vec<u8>, FfiError> {
        let Some(s) = self.inner.active() else {
            return Err(FfiError::Failed { message: "no account".into() });
        };
        let jh = self.inner.rt.spawn(async move { s.rest.fetch_media(&url).await });
        let out = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })?;
        if let Err(e) = &out {
            tracing::warn!("media fetch failed: {e}");
        }
        Ok(out?)
    }
}
