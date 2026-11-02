//! REST client: one per account.
//!
//! Reads go through [`Rest::get`]. Writes go through [`Rest::write`], which needs
//! a [`UserIntent`]. Every path is checked against the privacy policy first,
//! and rate limits are respected pre-emptively.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{CaptchaChallenge, CaptchaSolution};
use crate::fidelity::{ClientIdentity, RestHeaderInput, last_viewed_days};
use crate::forum::{ForumQuery, PostData, ThreadSearch};
use crate::intent::UserIntent;
use crate::model::{Channel, Id, Message};
use crate::privacy::Policy;
use crate::snowflake::Snowflake;
use crate::transport::{Method, Transport};
use crate::{Error, Result};

mod auth;
mod calls;
mod dms;
mod forums;
mod gifs;
mod interactions;
pub mod media;
mod messages;
mod moderation;
mod nav;
mod stage;
mod profile;
mod reactions;
mod relationships;
mod settings;

mod governor;
pub use auth::{LoginOutcome, MfaChallenge};
pub use forums::NewForumPost;
pub use nav::{channel_override_body, mute_body, SearchQuery, MUTE_WINDOWS};
pub(crate) use messages::attachment_json;
pub use messages::{ForwardSource, LocalFile, OutgoingMessage, UploadedAttachment, VoiceMeta};

/// Longest we will wait on our own for a rate-limit reset before surfacing it to the UI.
const MAX_SILENT_WAIT: Duration = Duration::from_secs(5);

/// Set while a script (DevTools command file, perf harness) drives the app. Nothing automated may talk to
/// Discord over REST: no request is ever sent on its behalf, so a run cannot look like a self-bot
/// (docs/ARCHITECTURE.md). Scripts work on the cached store and the already-open gateway session.
pub static AUTOMATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_automation(on: bool) {
    AUTOMATION.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub struct Rest {
    pub transport: Arc<Transport>,
    pub identity: Arc<ClientIdentity>,
    pub policy: Arc<RwLock<Policy>>,
    token: RwLock<Option<String>>,
    fingerprint: Mutex<Option<String>>,
    referer: Mutex<String>,
    limits: Mutex<RateLimits>,
    governor: Mutex<governor::Governor>,
    ack_token: Mutex<Option<String>>,
}

#[derive(Default)]
struct RateLimits {
    buckets: HashMap<String, Bucket>,
    global_until: Option<Instant>,
}

struct Bucket {
    remaining: u32,
    reset_at: Instant,
}

/// Flatten Discord's nested `errors` object (`{content: {_errors: [{message}]}}`) into
/// `path: message` lines, at most a few.
pub(crate) fn error_details(v: &Value) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        if out.len() >= 3 {
            return;
        }
        if let Some(list) = v["_errors"].as_array() {
            for e in list {
                if let Some(m) = e["message"].as_str() {
                    out.push(if path.is_empty() { m.to_string() } else { format!("{path}: {m}") });
                }
            }
        }
        if let Some(obj) = v.as_object() {
            for (k, child) in obj.iter().filter(|(k, _)| *k != "_errors") {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                walk(child, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(v, "", &mut out);
    out
}

impl Rest {
    pub fn new(
        transport: Arc<Transport>,
        identity: Arc<ClientIdentity>,
        policy: Arc<RwLock<Policy>>,
    ) -> Self {
        Self {
            transport,
            identity,
            policy,
            token: RwLock::new(None),
            fingerprint: Mutex::new(None),
            referer: Mutex::new("https://discord.com/channels/@me".into()),
            limits: Mutex::new(RateLimits::default()),
            governor: Mutex::new(governor::Governor::default()),
            ack_token: Mutex::new(None),
        }
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write() = token;
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().clone()
    }

    /// Mirror the URL the official client would be on, for `Referer`.
    pub fn set_location(&self, guild: Option<Id>, channel: Option<Id>) {
        let g = guild.map_or("@me".to_string(), |g| g.to_string());
        *self.referer.lock() = match channel {
            Some(c) => format!("https://discord.com/channels/{g}/{c}"),
            None => format!("https://discord.com/channels/{g}"),
        };
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.identity.profile.api_base, path)
    }

    /// The human-rate ceiling on writes (see `governor`): refused before anything is sent.
    fn pace(&self, method: &Method, path: &str) -> Result<()> {
        self.governor.lock().check(Instant::now(), method, path).map_err(|wait| {
            tracing::warn!(target: "unison_core::rest", "write paused by the rate ceiling: {} {} (wait {:?})", method, path.split('?').next().unwrap_or(path), wait);
            Error::Refused(format!("That's a lot at once. Unison paused it to protect your account; try again in {} s.", wait.as_secs().max(1)))
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let bytes = self.request(Method::GET, path, None, None).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn write<T: DeserializeOwned>(
        &self,
        intent: UserIntent,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        intent.redeem()?;
        self.pace(&method, path)?;
        let bytes = self.request(method, path, body, None).await?;
        if bytes.is_empty() {
            return Ok(serde_json::from_value(Value::Null)?);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// A write that carries an audit-log reason.
    pub async fn write_reason<T: DeserializeOwned>(
        &self,
        intent: UserIntent,
        method: Method,
        path: &str,
        body: Option<Value>,
        reason: &str,
    ) -> Result<T> {
        intent.redeem()?;
        self.pace(&method, path)?;
        let bytes = self.request_with(method, path, body, None, Some(reason)).await?;
        if bytes.is_empty() {
            return Ok(serde_json::from_value(Value::Null)?);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// A POST that only reads. The official client sends these on its own while content is on
    /// screen (forum previews), so they need no gesture; nothing here may change server state.
    async fn post_read<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T> {
        let bytes = self.request(Method::POST, path, Some(body), None).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Retry a write that came back with [`Error::Captcha`], carrying the user's solution.
    pub async fn write_with_captcha<T: DeserializeOwned>(
        &self,
        intent: UserIntent,
        method: Method,
        path: &str,
        body: Option<Value>,
        solution: CaptchaSolution,
    ) -> Result<T> {
        intent.redeem()?;
        let bytes = self.request(method, path, body, Some(solution)).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        captcha: Option<CaptchaSolution>,
    ) -> Result<bytes::Bytes> {
        self.request_with(method, path, body, captcha, None).await
    }

    /// `request` with the moderator's reason, sent as `X-Audit-Log-Reason` (URL-encoded UTF-8, up to
    /// 512 characters) so it shows in the server's audit log.
    async fn request_with(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        captcha: Option<CaptchaSolution>,
        reason: Option<&str>,
    ) -> Result<bytes::Bytes> {
        if AUTOMATION.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::BlockedByPolicy("automation"));
        }
        if let Err(c) = self.policy.read().check_route(path) {
            return Err(Error::BlockedByPolicy(c.label()));
        }
        let key = bucket_key(&method, path);
        self.wait_for_bucket(&key).await?;

        let payload = body.map(|b| serde_json::to_vec(&b)).transpose()?;
        let mut extra: Vec<(&str, String)> = Vec::new();
        if let Some(c) = &captcha {
            extra.push(("x-captcha-key", c.key.clone()));
            if let Some(t) = &c.rqtoken {
                extra.push(("x-captcha-rqtoken", t.clone()));
            }
            if let Some(s) = &c.session_id {
                extra.push(("x-captcha-session-id", s.clone()));
            }
        }

        if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
            extra.push(("x-audit-log-reason", audit_reason_header(r)));
        }

        for attempt in 0..2 {
            let token = self.token();
            let cookie = self.transport.cookie_header();
            let fingerprint = if token.is_none() {
                self.fingerprint.lock().clone()
            } else {
                None
            };
            let referer = self.referer.lock().clone();
            let has_body = payload.is_some();
            let headers = self.identity.rest_headers(&RestHeaderInput {
                token: token.as_deref(),
                content_type: has_body.then_some("application/json"),
                content_length: payload.as_ref().map(Vec::len),
                sends_origin: method != Method::GET && method != Method::HEAD,
                referer: Some(&referer),
                cookie: Some(&cookie),
                fingerprint: fingerprint.as_deref(),
                extra: &extra,
            });
            let resp = self
                .transport
                .send(method.clone(), &self.url(path), headers, payload.clone())
                .await?;
            self.record_limits(&key, &resp);
            // Paths only (no bodies, no token): enough to count what a run sent when something looks off.
            tracing::debug!(target: "unison_core::rest", "{} {} -> {}", method, path.split('?').next().unwrap_or(path), resp.status);

            match resp.status {
                200..=299 => return Ok(resp.body),
                401 => return Err(Error::Unauthorized),
                429 => {
                    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
                    let retry = Duration::from_secs_f64(v["retry_after"].as_f64().unwrap_or(1.0));
                    if v["global"].as_bool().unwrap_or(false) {
                        self.limits.lock().global_until = Some(Instant::now() + retry);
                    }
                    if attempt == 0 && retry <= MAX_SILENT_WAIT {
                        tokio::time::sleep(retry).await;
                        continue;
                    }
                    return Err(Error::RateLimited(retry));
                }
                status => {
                    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
                    if v["code"].as_u64() == Some(40002) {
                        tracing::warn!(target: "unison_core::rest", "Discord asked for account verification on {} {}", method, path.split('?').next().unwrap_or(path));
                    }
                    if v.get("captcha_key").is_some() || v.get("captcha_sitekey").is_some() {
                        let c: CaptchaChallenge =
                            serde_json::from_value(v).unwrap_or(CaptchaChallenge {
                                captcha_key: vec![],
                                captcha_sitekey: None,
                                captcha_service: None,
                                captcha_rqdata: None,
                                captcha_rqtoken: None,
                                captcha_session_id: None,
                            });
                        return Err(Error::Captcha(Box::new(c)));
                    }
                    let mut message = v["message"].as_str().unwrap_or("").to_string();
                    let details = error_details(&v["errors"]);
                    if !details.is_empty() {
                        message = format!("{message} | {}", details.join("; "));
                    }
                    return Err(Error::Http { status, code: v["code"].as_u64(), message });
                }
            }
        }
        Err(Error::Other("request retries exhausted".into()))
    }

    async fn wait_for_bucket(&self, key: &str) -> Result<()> {
        let wait = {
            let l = self.limits.lock();
            let now = Instant::now();
            let global = l.global_until.filter(|t| *t > now).map(|t| t - now);
            let bucket = l
                .buckets
                .get(key)
                .filter(|b| b.remaining == 0 && b.reset_at > now)
                .map(|b| b.reset_at - now);
            global.max(bucket)
        };
        if let Some(w) = wait {
            if w > MAX_SILENT_WAIT {
                return Err(Error::RateLimited(w));
            }
            tokio::time::sleep(w).await;
        }
        Ok(())
    }

    fn record_limits(&self, key: &str, resp: &crate::transport::Response) {
        let remaining = resp
            .header("x-ratelimit-remaining")
            .and_then(|v| v.parse().ok());
        let reset_after = resp
            .header("x-ratelimit-reset-after")
            .and_then(|v| v.parse::<f64>().ok());
        if let (Some(remaining), Some(after)) = (remaining, reset_after) {
            self.limits.lock().buckets.insert(
                key.to_string(),
                Bucket {
                    remaining,
                    reset_at: Instant::now() + Duration::from_secs_f64(after),
                },
            );
        }
    }

    /// The login screen fetches experiments first; the response carries the pre-login fingerprint.
    pub async fn fetch_fingerprint(&self) -> Result<()> {
        let v: Value = self.get("/experiments?with_guild_experiments=true").await?;
        if let Some(fp) = v["fingerprint"].as_str() {
            *self.fingerprint.lock() = Some(fp.to_string());
        }
        Ok(())
    }

    /// Exchange a QR login ticket. The gesture happened on the user's phone, so no local intent exists;
    /// this is the only unauthenticated POST and it is reachable solely from the remote-auth flow.
    pub(crate) async fn remote_auth_login(&self, ticket: &str, captcha: Option<CaptchaSolution>) -> Result<String> {
        let body = json!({ "ticket": ticket });
        let bytes = self
            .request(
                Method::POST,
                "/users/@me/remote-auth/login",
                Some(body),
                captcha,
            )
            .await?;
        let v: Value = serde_json::from_slice(&bytes)?;
        v["encrypted_token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::Decode("no encrypted_token".into()))
    }
}

/// Rate-limit bucket key: method plus path with non-major IDs collapsed.
fn bucket_key(method: &Method, path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let mut out = String::with_capacity(path.len() + 8);
    out.push_str(method.as_str());
    let mut prev = "";
    for seg in path.split('/') {
        out.push('/');
        let major = matches!(prev, "channels" | "guilds" | "webhooks");
        if !major && !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
            out.push_str(":id");
        } else {
            out.push_str(seg);
        }
        prev = seg;
    }
    out
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_discord_errors_flatten_to_readable_lines() {
        let v = serde_json::json!({"content": {"_errors": [{"code": "BASE_TYPE_MAX_LENGTH", "message": "Must be 2000 or fewer in length."}]},
                                   "message_reference": {"channel_id": {"_errors": [{"message": "Unknown channel"}]}}});
        assert_eq!(error_details(&v), vec!["content: Must be 2000 or fewer in length.", "message_reference.channel_id: Unknown channel"]);
        assert!(error_details(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn buckets_keep_major_ids() {
        assert_eq!(
            bucket_key(&Method::POST, "/channels/12/messages/34/ack"),
            "POST//channels/12/messages/:id/ack"
        );
        assert_eq!(
            bucket_key(&Method::GET, "/users/99/profile?x=1"),
            "GET//users/:id/profile"
        );
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(encode("👍"), "%F0%9F%91%8D");
        assert_eq!(encode("name:123"), "name%3A123");
    }
}

/// The `X-Audit-Log-Reason` value: at most 512 characters, percent-encoded UTF-8.
pub(crate) fn audit_reason_header(reason: &str) -> String {
    let mut out = String::new();
    for ch in reason.chars().take(512) {
        let mut buf = [0u8; 4];
        for b in ch.encode_utf8(&mut buf).bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod reason_tests {
    use super::audit_reason_header;

    #[test]
    fn reasons_are_percent_encoded() {
        assert_eq!(audit_reason_header("spam link"), "spam%20link");
        assert_eq!(audit_reason_header("café"), "caf%C3%A9");
        assert_eq!(audit_reason_header(&"a".repeat(600)).len(), 512);
    }
}
