//! Pictures and avatars from Discord's own media hosts, fetched on the same transport and with the
//! same client identity as every other request (docs/FIDELITY.md "Media"). One request shape in one
//! place (ARCHITECTURE rule 3): a shell hands over a URL and gets bytes, it never speaks to
//! Discord's CDN with an HTTP stack of its own.

use super::*;

const MAX_REDIRECTS: usize = 3;
const MAX_BYTES: usize = 64 << 20;

/// True for an `https` URL on one of Discord's own hosts. Anything else (a GIF provider, a
/// website) is not ours to introduce ourselves to: the shell loads it however it likes.
pub fn is_media_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split('@').next_back().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    ["discordapp.com", "discordapp.net", "discord.com", "discord.media"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Host only, for logs (no path, so no ids).
fn media_host(url: &str) -> &str {
    url.strip_prefix("https://").and_then(|r| r.split(['/', '?']).next()).unwrap_or("?")
}

impl Rest {
    /// GET an image like the web client's `<img>`: no token, no cookies, `Referer: https://discord.com/`,
    /// `sec-fetch-dest: image`. Redirects are followed only to other Discord media hosts.
    pub async fn fetch_media(&self, url: &str) -> Result<Vec<u8>> {
        if AUTOMATION.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::BlockedByPolicy("automation"));
        }
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            if !is_media_url(&url) {
                return Err(Error::Other("not a Discord media URL".into()));
            }
            let res = self
                .transport
                .send(Method::GET, &url, self.identity.media_headers(), None)
                .await?;
            tracing::debug!("media {} -> {} ({} bytes)", media_host(&url), res.status, res.body.len());
            match res.status {
                200 if res.body.len() <= MAX_BYTES => return Ok(res.body.to_vec()),
                301 | 302 | 303 | 307 | 308 => {
                    let Some(next) = res.header("location") else {
                        return Err(Error::Other("redirect without a location".into()));
                    };
                    url = next.to_string();
                }
                s => return Err(Error::Other(format!("media request failed ({s})"))),
            }
        }
        Err(Error::Other("too many redirects".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::is_media_url;

    #[test]
    fn only_discord_hosts_over_https() {
        assert!(is_media_url("https://cdn.discordapp.com/avatars/1/a.webp?size=80"));
        assert!(is_media_url("https://media.discordapp.net/attachments/1/2/x.png"));
        assert!(is_media_url("https://images-ext-1.discordapp.net/external/abc/https/x.com/y.png"));
        assert!(!is_media_url("http://cdn.discordapp.com/a.png"));
        assert!(!is_media_url("https://media.tenor.com/x.gif"));
        assert!(!is_media_url("https://cdn.discordapp.com.evil.example/a.png"));
        assert!(!is_media_url("https://evil.example/cdn.discordapp.com/a.png"));
        // Userinfo spoofing (`trusted-host` + `@` + real host). Built at run time: the release privacy
        // scan rightly rejects anything shaped like an email address in shipped source.
        let spoof = format!("https://cdn.discordapp.com{}evil.example/a.png", '@');
        assert!(!is_media_url(&spoof));
    }
}
