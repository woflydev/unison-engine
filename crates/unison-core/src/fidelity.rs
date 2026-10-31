//! Everything that makes a request look like the official desktop client.
//!
//! No other module constructs headers, super-properties or IDENTIFY payloads.
//! Every field and its order is sourced from the shipped client; see
//! `docs/FIDELITY.md` for the evidence ledger and the items still awaiting a
//! live capture.

use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use crate::profile::IdentityProfile;

/// Discord epoch (2015-01-01T00:00:00Z) in Unix milliseconds.
pub const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// The desktop OS the shell runs on. It decides the platform strings in headers and
/// super-properties, which must match what the official desktop client sends on that OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    MacOs,
    Windows,
    Linux,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::MacOs
        }
    }

    /// `os` in super-properties.
    pub fn super_property(self) -> &'static str {
        match self {
            Self::MacOs => "Mac OS X",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        }
    }

    /// `sec-ch-ua-platform`, quoted as sent.
    pub fn client_hint(self) -> &'static str {
        match self {
            Self::MacOs => "\"macOS\"",
            Self::Windows => "\"Windows\"",
            Self::Linux => "\"Linux\"",
        }
    }

    pub fn ua_platform(self) -> &'static str {
        match self {
            Self::MacOs => "Macintosh; Intel Mac OS X 10_15_7",
            Self::Windows => "Windows NT 10.0; Win64; x64",
            Self::Linux => "X11; Linux x86_64",
        }
    }
}

/// Super-properties for this machine; filled in once by the platform shell.
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub os: HostOs,
    /// `os.release()`, i.e. the Darwin kernel version such as `27.2.0`.
    pub os_release: String,
    /// `os.arch()` / `process.arch`: `arm64` or `x64`.
    pub arch: String,
    /// `navigator.languages`, most preferred first (e.g. `["en-US", "en"]`).
    pub languages: Vec<String>,
    /// IANA zone, e.g. `Australia/Sydney`.
    pub timezone: String,
}

impl HostInfo {
    pub fn detect() -> Self {
        Self {
            os: HostOs::current(),
            os_release: kernel_release(),
            arch: if cfg!(target_arch = "aarch64") { "arm64".into() } else { "x64".into() },
            languages: vec!["en-US".into()],
            timezone: std::env::var("TZ").unwrap_or_else(|_| "UTC".into()),
        }
    }

    pub fn system_locale(&self) -> &str {
        self.languages.first().map_or("en-US", String::as_str)
    }

    /// Discord's own `Accept-Language` builder: first tag bare, then `;q=0.9`, `0.8`… floored at `0.1`.
    pub fn accept_language(&self) -> String {
        let mut q = 10u32;
        let mut parts = Vec::with_capacity(self.languages.len());
        for lang in &self.languages {
            if q == 10 { parts.push(lang.clone()) } else { parts.push(format!("{lang};q=0.{q}")) }
            q = q.saturating_sub(1).max(1);
        }
        parts.join(",")
    }
}

#[cfg(unix)]
fn kernel_release() -> String {
    // SAFETY: uname fills a caller-owned struct; release is NUL-terminated.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) == 0 {
            std::ffi::CStr::from_ptr(u.release.as_ptr()).to_string_lossy().into_owned()
        } else {
            String::new()
        }
    }
}

/// Windows has no `uname`; its shell supplies the build number through `HostInfo`.
#[cfg(not(unix))]
fn kernel_release() -> String {
    String::new()
}

/// `client_app_state` values (`AppStates` enum in the client).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Active,
    Inactive,
    Background,
}

impl AppState {
    pub fn as_str(self) -> &'static str {
        match self {
            AppState::Active => "active",
            AppState::Inactive => "inactive",
            AppState::Background => "background",
        }
    }
}

/// The per-account, per-launch identity: what a separate official app instance would have.
pub struct ClientIdentity {
    pub profile: IdentityProfile,
    pub host: HostInfo,
    pub launch_id: String,
    heartbeat: Mutex<HeartbeatSession>,
    app_state: Mutex<AppState>,
    connect_count: Mutex<u32>,
    /// User's Discord locale setting (`X-Discord-Locale`).
    pub locale: Mutex<String>,
}

struct HeartbeatSession {
    id: String,
    last_active: Instant,
}

/// The analytics heartbeat session rolls over after this much inactivity.
const HEARTBEAT_SESSION_IDLE: Duration = Duration::from_secs(30 * 60);

impl ClientIdentity {
    pub fn new(profile: IdentityProfile, host: HostInfo) -> Self {
        let locale = host.system_locale().to_string();
        Self {
            profile,
            host,
            launch_id: uuid::Uuid::new_v4().to_string(),
            heartbeat: Mutex::new(HeartbeatSession { id: uuid::Uuid::new_v4().to_string(), last_active: Instant::now() }),
            app_state: Mutex::new(AppState::Active),
            connect_count: Mutex::new(0),
            locale: Mutex::new(locale),
        }
    }

    pub fn set_app_state(&self, s: AppState) {
        *self.app_state.lock() = s;
        if s == AppState::Active {
            self.touch();
        }
    }

    pub fn app_state(&self) -> AppState {
        *self.app_state.lock()
    }

    /// Record user activity for the heartbeat session.
    pub fn touch(&self) {
        let mut hb = self.heartbeat.lock();
        if hb.last_active.elapsed() > HEARTBEAT_SESSION_IDLE {
            hb.id = uuid::Uuid::new_v4().to_string();
        }
        hb.last_active = Instant::now();
    }

    /// The desktop super-properties object, in the client's key order.
    pub fn super_properties(&self) -> Map<String, Value> {
        let p = &self.profile;
        let mut m = Map::new();
        m.insert("os".into(), self.host.os.super_property().into());
        m.insert("browser".into(), "Discord Client".into());
        m.insert("release_channel".into(), p.release_channel.clone().into());
        m.insert("client_version".into(), p.client_version.clone().into());
        m.insert("os_version".into(), self.host.os_release.clone().into());
        m.insert("os_arch".into(), self.host.arch.clone().into());
        m.insert("app_arch".into(), self.host.arch.clone().into());
        m.insert("system_locale".into(), self.host.system_locale().into());
        // Truthful: this is not a modified official client.
        m.insert("has_client_mods".into(), false.into());
        m.insert("client_launch_id".into(), self.launch_id.clone().into());
        m.insert("browser_user_agent".into(), p.user_agent(self.host.os).into());
        m.insert("browser_version".into(), p.electron_version.clone().into());
        m.insert("client_build_number".into(), p.client_build_number.into());
        if let Some(n) = p.native_build_number {
            m.insert("native_build_number".into(), n.into());
        }
        m.insert("client_event_source".into(), Value::Null);
        // Produced by Discord's libdiscore integrity module. We do not reproduce it;
        // `null` is what the official code sends when that module is unavailable.
        m.insert("launch_signature".into(), Value::Null);
        self.touch_if_stale();
        m.insert("client_heartbeat_session_id".into(), self.heartbeat.lock().id.clone().into());
        m.insert("client_app_state".into(), self.app_state().as_str().into());
        m
    }

    fn touch_if_stale(&self) {
        let mut hb = self.heartbeat.lock();
        if hb.last_active.elapsed() > HEARTBEAT_SESSION_IDLE {
            hb.id = uuid::Uuid::new_v4().to_string();
            hb.last_active = Instant::now();
        }
    }

    pub fn super_properties_b64(&self) -> String {
        let json = serde_json::to_string(&Value::Object(self.super_properties())).unwrap();
        base64::engine::general_purpose::STANDARD.encode(json)
    }

    /// Ordered headers for a REST call to `https://discord.com/api/...`.
    pub fn rest_headers(&self, req: &RestHeaderInput<'_>) -> Vec<(String, String)> {
        let mut h: Vec<(&str, String)> = Vec::with_capacity(24);
        if let Some(len) = req.content_length {
            h.push(("content-length", len.to_string()));
        }
        h.push(("sec-ch-ua-platform", self.host.os.client_hint().into()));
        if let Some(token) = req.token {
            h.push(("authorization", token.to_string()));
        }
        h.push(("sec-ch-ua", self.profile.sec_ch_ua.clone()));
        h.push(("sec-ch-ua-mobile", "?0".into()));
        h.push(("x-discord-timezone", self.host.timezone.clone()));
        h.push(("x-super-properties", self.super_properties_b64()));
        h.push(("x-discord-locale", self.locale.lock().clone()));
        h.push(("user-agent", self.profile.user_agent(self.host.os)));
        if let Some(ct) = req.content_type {
            h.push(("content-type", ct.to_string()));
        }
        h.push(("accept", "*/*".into()));
        if req.sends_origin {
            h.push(("origin", "https://discord.com".into()));
        }
        h.push(("sec-fetch-site", "same-origin".into()));
        h.push(("sec-fetch-mode", "cors".into()));
        h.push(("sec-fetch-dest", "empty".into()));
        h.push(("referer", req.referer.unwrap_or("https://discord.com/channels/@me").to_string()));
        h.push(("accept-encoding", "gzip, deflate, br, zstd".into()));
        h.push(("accept-language", self.host.accept_language()));
        if let Some(c) = req.cookie {
            if !c.is_empty() {
                h.push(("cookie", c.to_string()));
            }
        }
        h.push(("priority", "u=1, i".into()));
        if let Some(fp) = req.fingerprint {
            h.push(("x-fingerprint", fp.to_string()));
        }
        for (k, v) in req.extra {
            h.push((k, v.clone()));
        }

        // Re-order to the profile's captured order; unknown names keep their relative position at the end.
        let order = &self.profile.rest_header_order;
        let rank = |name: &str| order.iter().position(|o| o == name).unwrap_or(order.len());
        let mut indexed: Vec<_> = h.into_iter().enumerate().collect();
        indexed.sort_by_key(|(i, (name, _))| (rank(name), *i));
        indexed.into_iter().map(|(_, (k, v))| (k.to_string(), v)).collect()
    }

    /// Headers of an `<img>` subresource request from the Discord web client to its CDN: the same
    /// browser identity as `rest_headers`, but no token, no cookies and no Discord-specific headers.
    pub fn media_headers(&self) -> Vec<(String, String)> {
        vec![
            ("sec-ch-ua".into(), self.profile.sec_ch_ua.clone()),
            ("sec-ch-ua-mobile".into(), "?0".into()),
            ("sec-ch-ua-platform".into(), self.host.os.client_hint().into()),
            ("user-agent".into(), self.profile.user_agent(self.host.os)),
            ("accept".into(), "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".into()),
            ("sec-fetch-site".into(), "cross-site".into()),
            ("sec-fetch-mode".into(), "no-cors".into()),
            ("sec-fetch-dest".into(), "image".into()),
            ("referer".into(), "https://discord.com/".into()),
            ("accept-encoding".into(), "gzip, deflate, br, zstd".into()),
            ("accept-language".into(), self.host.accept_language()),
            ("priority".into(), "i".into()),
        ]
    }

    /// Headers on the gateway / remote-auth WebSocket upgrade (Chromium order).
    pub fn websocket_headers(&self) -> Vec<(String, String)> {
        vec![
            ("pragma".into(), "no-cache".into()),
            ("cache-control".into(), "no-cache".into()),
            ("user-agent".into(), self.profile.user_agent(self.host.os)),
            ("origin".into(), "https://discord.com".into()),
            ("accept-encoding".into(), "gzip, deflate, br, zstd".into()),
            ("accept-language".into(), self.host.accept_language()),
        ]
    }

    /// The IDENTIFY `d` payload, matching `GatewaySocket._doIdentify`.
    pub fn identify(&self, token: &str, presence: Value, derived_qos: Option<&str>) -> Value {
        let mut props = self.super_properties();
        props.insert("client_app_state".into(), self.app_state().as_str().into());
        props.insert("is_fast_connect".into(), false.into());
        let reasons = {
            let mut n = self.connect_count.lock();
            *n += 1;
            if *n == 1 { "COLD_START" } else { "NO_REASONS" }
        };
        props.insert("gateway_connect_reasons".into(), reasons.into());
        json!({
            "token": token,
            "capabilities": self.profile.gateway_capabilities,
            "properties": props,
            "presence": presence,
            "compress": false,
            "client_state": { "guild_versions": {} },
            "qos_token": qos_token(self.app_state() == AppState::Active, derived_qos),
        })
    }
}

/// Inputs that vary per REST call.
#[derive(Default)]
pub struct RestHeaderInput<'a> {
    pub token: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub content_length: Option<usize>,
    /// Chromium sends `Origin` on same-origin requests whose method isn't GET/HEAD.
    pub sends_origin: bool,
    pub referer: Option<&'a str>,
    pub cookie: Option<&'a str>,
    pub fingerprint: Option<&'a str>,
    pub extra: &'a [(&'a str, String)],
}

/// Initial presence the client sends in IDENTIFY.
pub fn initial_presence() -> Value {
    json!({ "status": "unknown", "since": 0, "activities": [], "afk": false })
}

/// `discord_protos.qos_token.v1.QosToken`, base64:
/// field 1 `client_provided { 1: is_active }`, field 2 `derived` (opaque bytes from READY's `qos_token`).
pub fn qos_token(is_active: bool, derived_b64: Option<&str>) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut out = Vec::with_capacity(64);
    let client: &[u8] = if is_active { &[0x08, 0x01] } else { &[] };
    out.push(0x0a);
    out.push(client.len() as u8);
    out.extend_from_slice(client);
    if let Some(derived) = derived_b64.and_then(|d| b64.decode(d).ok()) {
        out.push(0x12);
        varint(&mut out, derived.len() as u64);
        out.extend_from_slice(&derived);
    }
    b64.encode(out)
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// `last_viewed` on acks: whole days since the Discord epoch, rounded up.
pub fn last_viewed_days(now_ms: u64) -> u64 {
    (now_ms.saturating_sub(DISCORD_EPOCH_MS)).div_ceil(86_400_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> ClientIdentity {
        let host = HostInfo {
            os: HostOs::MacOs,
            os_release: "27.2.0".into(),
            arch: "arm64".into(),
            languages: vec!["en-AU".into(), "en".into()],
            timezone: "Australia/Sydney".into(),
        };
        ClientIdentity::new(IdentityProfile::bundled(), host)
    }

    #[test]
    fn super_properties_key_order() {
        let keys: Vec<_> = ident().super_properties().keys().cloned().collect();
        assert_eq!(keys, [
            "os", "browser", "release_channel", "client_version", "os_version", "os_arch", "app_arch",
            "system_locale", "has_client_mods", "client_launch_id", "browser_user_agent", "browser_version",
            "client_build_number", "client_event_source", "launch_signature", "client_heartbeat_session_id",
            "client_app_state",
        ]);
    }

    #[test]
    fn accept_language_matches_client_builder() {
        assert_eq!(ident().host.accept_language(), "en-AU,en;q=0.9");
    }

    #[test]
    fn rest_headers_follow_profile_order() {
        let id = ident();
        let h = id.rest_headers(&RestHeaderInput {
            token: Some("t"),
            content_type: Some("application/json"),
            content_length: Some(2),
            sends_origin: true,
            ..Default::default()
        });
        let names: Vec<_> = h.iter().map(|(k, _)| k.as_str()).collect();
        let expect: Vec<_> = id.profile.rest_header_order.iter().map(String::as_str).filter(|n| names.contains(n)).collect();
        assert_eq!(names, expect);
        assert!(!names.contains(&"cookie"));
    }

    #[test]
    fn identify_shape() {
        let id = ident();
        let d = id.identify("tok", initial_presence(), None);
        let keys: Vec<_> = d.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["token", "capabilities", "properties", "presence", "compress", "client_state", "qos_token"]);
        assert_eq!(d["properties"]["gateway_connect_reasons"], "COLD_START");
        assert_eq!(d["properties"]["is_fast_connect"], false);
        assert_eq!(d["capabilities"], 1734653);
        assert_eq!(id.identify("tok", initial_presence(), None)["properties"]["gateway_connect_reasons"], "NO_REASONS");
    }

    #[test]
    fn qos_token_encoding() {
        assert_eq!(qos_token(true, None), "CgIIAQ==");
        assert_eq!(qos_token(false, None), "CgA=");
    }

    #[test]
    fn last_viewed() {
        assert_eq!(last_viewed_days(DISCORD_EPOCH_MS + 1), 1);
        assert_eq!(last_viewed_days(DISCORD_EPOCH_MS + 86_400_000), 1);
    }
}
