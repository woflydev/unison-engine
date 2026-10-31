//! CDN resize parameters for images. Discord's media proxy returns a small WebP when asked for
//! `width`/`height`/`format`, which is far less to download and decode than the original.
//! The request shape lives here so every shell asks the same way (see `docs/FIDELITY.md`).

use reqwest::Url;

const CAP: f64 = 4096.0;
const OWN_PARAMS: [&str; 4] = ["width", "height", "keep_aspect_ratio", "format"];

/// `url` rewritten to ask the proxy for a `width` x `height` pixel WebP. Other hosts, unparsable
/// URLs and empty sizes pass through unchanged.
pub fn resized(url: &str, width: f64, height: f64) -> String {
    let Ok(mut u) = Url::parse(url) else { return url.into() };
    let Some(host) = u.host_str().map(str::to_owned) else { return url.into() };
    let proxy = host == "media.discordapp.net" || (host.ends_with(".discordapp.net") && u.path().starts_with("/external/"));
    if !proxy || width < 1.0 || height < 1.0 {
        return url.into();
    }
    let w = width.min(CAP).round() as u32;
    let h = height.min(CAP).round() as u32;
    let kept: Vec<(String, String)> = u.query_pairs().filter(|(k, _)| !OWN_PARAMS.contains(&k.as_ref())).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    u.query_pairs_mut()
        .clear()
        .extend_pairs(kept.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .append_pair("width", &w.to_string())
        .append_pair("height", &h.to_string())
        .append_pair("keep_aspect_ratio", "true")
        .append_pair("format", "webp");
    u.into()
}

#[cfg(test)]
mod tests {
    use super::resized;

    #[test]
    fn proxy_urls_get_size_and_webp() {
        let r = resized("https://media.discordapp.net/attachments/1/2/a.png?ex=1&width=9", 200.4, 100.0);
        assert!(r.contains("ex=1") && r.contains("width=200") && r.contains("height=100") && r.contains("format=webp"));
        assert_eq!(r.matches("width=").count(), 1);
    }

    #[test]
    fn size_is_capped_and_others_pass_through() {
        assert!(resized("https://media.discordapp.net/a.png", 9000.0, 10.0).contains("width=4096"));
        assert_eq!(resized("https://example.com/a.png", 10.0, 10.0), "https://example.com/a.png");
        assert_eq!(resized("https://media.discordapp.net/a.png", 0.0, 10.0), "https://media.discordapp.net/a.png");
        assert_eq!(resized("not a url", 10.0, 10.0), "not a url");
    }
}
