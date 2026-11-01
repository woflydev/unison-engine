//! App updates, Sparkle-style but dependency-free: a signed appcast (Ed25519 over the exact JSON
//! bytes, the same scheme as signed identity profiles) names a notarised DMG and its SHA-256.
//! The shell fetches the appcast only on an explicit "Check for Updates" (or, if the user opted in,
//! at most once a day at launch), verifies it here, downloads the DMG, checks the digest, and opens
//! it so the user drags the app into Applications. Nothing is installed behind the user's back, and
//! no request is made unless the user asked for one.
//!
//! "Update and restart" builds on this: this file decides *what* is offered (which asset fits this
//! OS and architecture, the channel, the minimum OS); `update_install` decides whether and when it may
//! replace the running app and owns the state machine and the sentences. A shell only does the OS part.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Public key that appcasts must be signed with. Empty means this build has no update channel.
pub const UPDATE_PUBLIC_KEY: &str = include_str!("../update_pubkey.txt");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Appcast {
    /// Marketing version, `MAJOR.MINOR.PATCH`.
    pub version: String,
    /// Monotonic build number (commit count).
    pub build: u64,
    pub url: String,
    /// Lower-case hex SHA-256 of the DMG.
    pub sha256: String,
    /// Plain-text release notes (the changelog section for this version).
    #[serde(default)]
    pub notes: String,
    /// Minimum OS version for the shell's platform, e.g. `26.0`.
    #[serde(default)]
    pub min_os: String,
    /// Release channel: `stable` (also when empty) or `beta`. Additive.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel: String,
    /// Per-platform downloads. Additive: older shells read only `url` and `sha256` above, which stay
    /// the macOS DMG. When this list is empty, that pair is the (only) macOS asset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Asset>,
}

/// One downloadable build of a release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    /// `macos`, `windows` or `linux`.
    pub os: String,
    /// `arm64`, `x86_64`, or `universal` (runs on both).
    pub arch: String,
    pub url: String,
    /// Lower-case hex SHA-256 of the file at `url`.
    pub sha256: String,
    /// Bytes, for the progress bar; 0 when unknown.
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SignedAppcast {
    pub appcast: Box<serde_json::value::RawValue>,
    /// Base64 Ed25519 signature over `appcast`'s raw bytes.
    pub signature: String,
}

pub fn verify(signed_json: &str, public_key_b64: &str) -> Result<Appcast> {
    let key = public_key_b64.trim();
    if key.is_empty() {
        return Err(Error::Crypto("this build has no update channel".into()));
    }
    let s: SignedAppcast = serde_json::from_str(signed_json)?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b64
        .decode(key)
        .map_err(|e| Error::Crypto(e.to_string()))?
        .try_into()
        .map_err(|_| Error::Crypto("public key must be 32 bytes".into()))?;
    let sig: [u8; 64] = b64
        .decode(&s.signature)
        .map_err(|e| Error::Crypto(e.to_string()))?
        .try_into()
        .map_err(|_| Error::Crypto("signature must be 64 bytes".into()))?;
    let vk = ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|e| Error::Crypto(e.to_string()))?;
    vk.verify_strict(s.appcast.get().as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|e| Error::Crypto(e.to_string()))?;
    let a: Appcast = serde_json::from_str(s.appcast.get())?;
    if !a.url.starts_with("https://") {
        return Err(Error::Crypto("appcast URL must be https".into()));
    }
    Ok(a)
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Is `candidate` a newer marketing version than `current` (numeric, component-wise)?
pub fn is_newer(current: &str, current_build: u64, candidate: &Appcast) -> bool {
    let (a, b) = (parts(current), parts(&candidate.version));
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return y > x;
        }
    }
    candidate.build > current_build
}

/// Lower-case hex SHA-256 comparison for the downloaded DMG.
pub fn digest_matches(sha256_hex: &str, bytes_digest: &[u8]) -> bool {
    let hex: String = bytes_digest.iter().map(|b| format!("{b:02x}")).collect();
    hex.eq_ignore_ascii_case(sha256_hex.trim())
}

/// Which build is running, as the shell knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Platform {
    /// `macos`, `windows`, `linux`.
    pub os: String,
    /// `arm64` or `x86_64`.
    pub arch: String,
    /// Dotted OS version, e.g. `26.0.1`.
    pub os_version: String,
}

/// An update this build may install: the asset for this platform plus the release facts.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub version: String,
    pub build: u64,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub notes: String,
    pub channel: String,
}

pub const STABLE: &str = "stable";
pub const BETA: &str = "beta";

fn norm_channel(c: &str) -> &'static str {
    if c.trim().eq_ignore_ascii_case(BETA) { BETA } else { STABLE }
}

/// May someone on channel `wanted` take a release published to `offered`? Stable releases reach
/// everyone; beta releases only reach people who opted in to beta.
pub fn channel_accepts(offered: &str, wanted: &str) -> bool {
    norm_channel(offered) == STABLE || norm_channel(wanted) == BETA
}

/// The feed to ask: stable uses `base` itself, beta the sibling `<name>-beta.json`.
/// What the person chose about updates, plus when we last asked the feed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePrefs {
    pub auto_check: bool,
    pub beta: bool,
    pub last_check_ms: u64,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self { auto_check: true, beta: false, last_check_ms: 0 }
    }
}

impl UpdatePrefs {
    pub fn channel(&self) -> &'static str {
        if self.beta { "beta" } else { "stable" }
    }

    /// The launch check runs only when it is on and a day has passed since the last one.
    pub fn check_due(&self, now_ms: u64) -> bool {
        self.auto_check && (self.last_check_ms == 0 || now_ms.saturating_sub(self.last_check_ms) >= 86_400_000)
    }
}

pub fn feed_url(base: &str, channel: &str) -> String {
    if norm_channel(channel) == STABLE {
        return base.to_string();
    }
    match base.rsplit_once(".json") {
        Some((stem, rest)) if rest.is_empty() || rest.starts_with('?') => format!("{stem}-beta.json{rest}"),
        _ => base.to_string(),
    }
}

/// The asset for this OS and architecture. An exact architecture wins over `universal`. A feed
/// without an `assets` list is the old macOS-only shape and serves only macOS.
pub fn pick_asset(a: &Appcast, os: &str, arch: &str) -> Option<Asset> {
    if a.assets.is_empty() {
        return (os == "macos").then(|| Asset { os: "macos".into(), arch: "universal".into(), url: a.url.clone(), sha256: a.sha256.clone(), size: 0 });
    }
    let ok = |x: &&Asset| x.os == os && x.url.starts_with("https://");
    a.assets.iter().filter(ok).find(|x| x.arch == arch).or_else(|| a.assets.iter().filter(ok).find(|x| x.arch == "universal")).cloned()
}

/// Is `os_version` at least `min_os` (empty means any)?
pub fn os_supported(min_os: &str, os_version: &str) -> bool {
    if min_os.trim().is_empty() {
        return true;
    }
    let (m, c) = (parts(min_os), parts(os_version));
    for i in 0..m.len().max(c.len()) {
        let (x, y) = (c.get(i).copied().unwrap_or(0), m.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    true
}

/// Verify a signed feed and decide what this build should be offered. `Ok(None)` means there is
/// nothing to install (not newer, a channel this person did not choose, or no build for this platform).
pub fn evaluate(signed_json: &str, public_key_b64: &str, platform: &Platform, current_version: &str, current_build: u64, channel: &str) -> Result<Option<Offer>> {
    let a = verify(signed_json, public_key_b64)?;
    if !is_newer(current_version, current_build, &a) || !channel_accepts(&a.channel, channel) {
        return Ok(None);
    }
    let Some(asset) = pick_asset(&a, &platform.os, &platform.arch) else { return Ok(None) };
    if !os_supported(&a.min_os, &platform.os_version) {
        return Err(Error::Refused(crate::update_install::Refusal::OsTooOld(a.min_os).text()));
    }
    Ok(Some(Offer { version: a.version, build: a.build, url: asset.url, sha256: asset.sha256, size: asset.size, notes: a.notes, channel: norm_channel(&a.channel).into() }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn launch_check_waits_a_day_and_respects_the_switch() {
        use super::UpdatePrefs;
        let day = 86_400_000;
        let mut p = UpdatePrefs::default();
        assert!(p.check_due(5));
        p.last_check_ms = 1_000;
        assert!(!p.check_due(1_000 + day - 1));
        assert!(p.check_due(1_000 + day));
        p.auto_check = false;
        assert!(!p.check_due(1_000 + 10 * day));
        assert_eq!(UpdatePrefs { beta: true, ..Default::default() }.channel(), "beta");
    }

    use super::*;
    use ed25519_dalek::Signer;

    fn sample(version: &str, build: u64) -> Appcast {
        Appcast { version: version.into(), build, url: "https://example.com/u.dmg".into(), sha256: "ab".into(), notes: "n".into(), min_os: "26.0".into(), channel: String::new(), assets: vec![] }
    }

    fn sign(a: &Appcast, seed: u8) -> (String, String) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let raw = serde_json::to_string(a).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        (
            format!("{{\"appcast\":{raw},\"signature\":\"{}\"}}", b64.encode(sk.sign(raw.as_bytes()).to_bytes())),
            b64.encode(sk.verifying_key().to_bytes()),
        )
    }

    #[test]
    fn verifies_and_rejects_tampering() {
        let (json, pk) = sign(&sample("0.2.0", 10), 3);
        assert_eq!(verify(&json, &pk).unwrap().version, "0.2.0");
        assert!(verify(&json.replace("0.2.0", "9.9.9"), &pk).is_err());
        let (_, other) = sign(&sample("0.2.0", 10), 4);
        assert!(verify(&json, &other).is_err(), "wrong key");
        assert!(verify(&json, "").is_err(), "no channel configured");
    }

    #[test]
    fn rejects_non_https() {
        let mut a = sample("0.2.0", 1);
        a.url = "http://example.com/u.dmg".into();
        let (json, pk) = sign(&a, 3);
        assert!(verify(&json, &pk).is_err());
    }

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.1.0", 5, &sample("0.1.1", 1)));
        assert!(is_newer("0.9.0", 5, &sample("0.10.0", 1)), "numeric not lexical");
        assert!(!is_newer("1.0.0", 5, &sample("0.9.9", 99)));
        assert!(is_newer("0.1.0", 5, &sample("0.1.0", 6)));
        assert!(!is_newer("0.1.0", 6, &sample("0.1.0", 6)));
    }

    #[test]
    fn digest() {
        assert!(digest_matches("0aff", &[0x0a, 0xff]));
        assert!(!digest_matches("0aff", &[0x0a, 0xfe]));
    }

    fn platform() -> Platform {
        Platform { os: "macos".into(), arch: "arm64".into(), os_version: "26.1".into() }
    }

    fn asset(os: &str, arch: &str) -> Asset {
        Asset { os: os.into(), arch: arch.into(), url: format!("https://example.com/{os}-{arch}.bin"), sha256: "cd".into(), size: 10 }
    }

    fn with_assets(mut a: Appcast, assets: Vec<Asset>) -> Appcast {
        a.assets = assets;
        a
    }

    #[test]
    fn legacy_feed_serves_macos_only() {
        let a = sample("0.3.0", 9);
        assert_eq!(pick_asset(&a, "macos", "arm64").unwrap().url, "https://example.com/u.dmg");
        assert!(pick_asset(&a, "windows", "x86_64").is_none());
    }

    #[test]
    fn asset_per_os_and_arch() {
        let a = with_assets(sample("0.3.0", 9), vec![asset("macos", "universal"), asset("macos", "arm64"), asset("windows", "x86_64")]);
        assert_eq!(pick_asset(&a, "macos", "arm64").unwrap().arch, "arm64", "exact arch beats universal");
        assert_eq!(pick_asset(&a, "macos", "x86_64").unwrap().arch, "universal");
        assert!(pick_asset(&a, "linux", "x86_64").is_none());
        let mut bad = asset("linux", "x86_64");
        bad.url = "http://example.com/x".into();
        assert!(pick_asset(&with_assets(sample("1.0.0", 1), vec![bad]), "linux", "x86_64").is_none(), "https only");
    }

    #[test]
    fn channels() {
        assert!(channel_accepts("", "stable") && channel_accepts("stable", "beta") && channel_accepts("beta", "beta"));
        assert!(!channel_accepts("beta", "stable") && !channel_accepts("beta", ""));
        assert_eq!(feed_url("https://x.test/appcast.json", "stable"), "https://x.test/appcast.json");
        assert_eq!(feed_url("https://x.test/appcast.json", "beta"), "https://x.test/appcast-beta.json");
        assert_eq!(feed_url("https://x.test/feed", "beta"), "https://x.test/feed");
    }

    #[test]
    fn min_os() {
        assert!(os_supported("", "1.0") && os_supported("26.0", "26.0.1") && os_supported("26.0", "27"));
        assert!(!os_supported("26.1", "26.0.9") && !os_supported("26.0", "15.7"));
    }

    #[test]
    fn evaluate_picks_signed_offer() {
        let mut a = with_assets(sample("0.3.0", 9), vec![asset("macos", "arm64")]);
        a.channel = "beta".into();
        let (json, pk) = sign(&a, 3);
        let o = evaluate(&json, &pk, &platform(), "0.2.0", 5, "beta").unwrap().unwrap();
        assert_eq!((o.version.as_str(), o.url.as_str(), o.size, o.channel.as_str()), ("0.3.0", "https://example.com/macos-arm64.bin", 10, "beta"));
        assert!(evaluate(&json, &pk, &platform(), "0.2.0", 5, "stable").unwrap().is_none(), "beta not offered to stable");
        assert!(evaluate(&json, &pk, &platform(), "0.3.0", 9, "beta").unwrap().is_none(), "not newer");
        let intel = Platform { arch: "x86_64".into(), ..platform() };
        assert!(evaluate(&json, &pk, &intel, "0.2.0", 5, "beta").unwrap().is_none(), "no build for this arch");
        let old = Platform { os_version: "15.0".into(), ..platform() };
        assert!(evaluate(&json, &pk, &old, "0.2.0", 5, "beta").is_err(), "needs a newer OS");
    }

    #[test]
    fn old_feed_bytes_unchanged_by_new_fields() {
        // Without channel/assets nothing new is serialised, so previously signed feeds stay identical.
        let raw = serde_json::to_string(&sample("0.2.0", 1)).unwrap();
        assert!(!raw.contains("assets") && !raw.contains("channel"));
        // And a feed written before these fields existed still parses.
        let old = r#"{"version":"0.1.0","build":1,"url":"https://e.test/u.dmg","sha256":"ab","notes":"","min_os":"26.0"}"#;
        let a: Appcast = serde_json::from_str(old).unwrap();
        assert!(a.assets.is_empty() && a.channel.is_empty());
    }
}
