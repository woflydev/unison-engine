//! The identity profile: which official Discord desktop build Unison presents as.
//!
//! The watcher (`tools/watcher`) derives these values from Discord's own published
//! assets and signs them. The app ships a bundled default and accepts a newer
//! profile only if its Ed25519 signature verifies against [`PROFILE_PUBLIC_KEY`].

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::fidelity::HostOs;
use crate::{Error, Result};

/// Public half of the profile signing key, set by `watcher keygen`.
/// Empty means only the bundled profile is trusted.
pub const PROFILE_PUBLIC_KEY: &str = include_str!("../profile_pubkey.txt");

pub const BUNDLED_PROFILE: &str = include_str!("../profile.default.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IdentityProfile {
    /// Monotonic; a fetched profile must be newer than the current one.
    pub serial: u64,
    /// Host app version, `discord/<this>` in the UA and `client_version` in super-properties.
    pub client_version: String,
    pub release_channel: String,
    pub electron_version: String,
    pub chrome_version: String,
    /// `client_build_number` from the web client's `GLOBAL_ENV.BUILD_NUMBER`.
    pub client_build_number: u64,
    /// Only sent when the native host reports one (Windows); `None` on macOS.
    pub native_build_number: Option<u64>,
    pub api_version: u8,
    pub gateway_capabilities: u64,
    pub api_base: String,
    pub gateway_url: String,
    pub remote_auth_url: String,
    /// `sec-ch-ua` brand list as Electron's Chromium generates it for this major version.
    pub sec_ch_ua: String,
    /// Lower-case header names in the order Chromium emits them for an API fetch.
    pub rest_header_order: Vec<String>,
    /// Native features the maintainer has routed to the web view after a Discord change broke them.
    #[serde(default)]
    pub web_fallback_features: Vec<String>,
    /// Free-form facts from the watcher (route counts, schema hashes) for diagnostics.
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}

impl IdentityProfile {
    pub fn bundled() -> Self {
        serde_json::from_str(BUNDLED_PROFILE).expect("bundled profile is valid JSON")
    }

    /// The desktop client's user agent for the OS the shell runs on (`capture`: Windows and Linux
    /// strings follow Electron's defaults and must be checked against the official client).
    pub fn user_agent(&self, os: HostOs) -> String {
        format!(
            "Mozilla/5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) discord/{} Chrome/{} Electron/{} Safari/537.36",
            os.ua_platform(),
            self.client_version,
            self.chrome_version,
            self.electron_version
        )
    }

    pub fn gateway_connect_url(&self, base: &str) -> String {
        format!("{}/?encoding=etf&v={}&compress=zstd-stream", base.trim_end_matches('/'), self.api_version)
    }

    pub fn chrome_major(&self) -> u32 {
        self.chrome_version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0)
    }
}

/// A profile as published: the exact JSON bytes plus a detached signature over them.
#[derive(Debug, Deserialize, Serialize)]
pub struct SignedProfile {
    pub profile: Box<serde_json::value::RawValue>,
    /// Base64 Ed25519 signature over `profile`'s raw bytes.
    pub signature: String,
}

impl SignedProfile {
    pub fn verify(&self, public_key_b64: &str) -> Result<IdentityProfile> {
        let key = public_key_b64.trim();
        if key.is_empty() {
            return Err(Error::Crypto("no profile public key configured".into()));
        }
        let b64 = base64::engine::general_purpose::STANDARD;
        let key: [u8; 32] = b64
            .decode(key)
            .map_err(|e| Error::Crypto(e.to_string()))?
            .try_into()
            .map_err(|_| Error::Crypto("public key must be 32 bytes".into()))?;
        let sig: [u8; 64] = b64
            .decode(&self.signature)
            .map_err(|e| Error::Crypto(e.to_string()))?
            .try_into()
            .map_err(|_| Error::Crypto("signature must be 64 bytes".into()))?;
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|e| Error::Crypto(e.to_string()))?;
        vk.verify_strict(self.profile.get().as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|e| Error::Crypto(e.to_string()))?;
        Ok(serde_json::from_str(self.profile.get())?)
    }
}

/// Pick the profile to use: a verified, newer signed profile, or the bundled default.
pub fn select(signed_json: Option<&str>) -> IdentityProfile {
    let bundled = IdentityProfile::bundled();
    let Some(json) = signed_json else { return bundled };
    match serde_json::from_str::<SignedProfile>(json).map_err(Error::from).and_then(|s| s.verify(PROFILE_PUBLIC_KEY)) {
        Ok(p) if p.serial > bundled.serial => p,
        Ok(_) => bundled,
        Err(e) => {
            tracing::warn!("ignoring fetched identity profile: {e}");
            bundled
        }
    }
}

/// Chromium's GREASE brand for a major version (`components/embedder_support/user_agent_utils.cc`).
/// Electron reports only the GREASE brand and "Chromium", in seed-dependent order.
pub fn sec_ch_ua_for(major: u32) -> String {
    const CHARS: [char; 11] = [' ', '(', ':', '-', '.', '/', ')', ';', '=', '?', '_'];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    let s = major as usize;
    let grease = format!(
        "\"Not{}A{}Brand\";v=\"{}\"",
        CHARS[s % CHARS.len()],
        CHARS[(s + 1) % CHARS.len()],
        VERSIONS[s % VERSIONS.len()]
    );
    let chromium = format!("\"Chromium\";v=\"{major}\"");
    if s % 2 == 0 { format!("{grease}, {chromium}") } else { format!("{chromium}, {grease}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_profile_parses_and_is_consistent() {
        let p = IdentityProfile::bundled();
        assert_eq!(p.chrome_major(), 152);
        assert_eq!(p.sec_ch_ua, sec_ch_ua_for(152));
        assert!(p.user_agent(HostOs::MacOs).contains("discord/0.0.420 Chrome/152.0.7977.167 Electron/44.9.3"));
    }

    #[test]
    fn grease_matches_known_chrome_133() {
        // Chrome 133 shipped `"Not(A:Brand";v="99"`.
        assert!(sec_ch_ua_for(133).contains("\"Not(A:Brand\";v=\"99\""));
    }

    #[test]
    fn signature_round_trip() {
        use ed25519_dalek::Signer;
        let sk = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let raw = serde_json::to_string(&IdentityProfile { serial: 99, ..IdentityProfile::bundled() }).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let signed = format!(
            "{{\"profile\":{raw},\"signature\":\"{}\"}}",
            b64.encode(sk.sign(raw.as_bytes()).to_bytes())
        );
        let pk = b64.encode(sk.verifying_key().to_bytes());
        let s: SignedProfile = serde_json::from_str(&signed).unwrap();
        assert_eq!(s.verify(&pk).unwrap().serial, 99);
        let tampered = signed.replace("\"serial\":99", "\"serial\":98");
        let s: SignedProfile = serde_json::from_str(&tampered).unwrap();
        assert!(s.verify(&pk).is_err());
    }
}
