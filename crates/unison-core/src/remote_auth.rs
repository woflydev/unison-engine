//! QR-code login through `remote-auth-gateway.discord.gg` (protocol v2), the
//! same flow as the official login screen. Unison never sees a password: the
//! user approves the login on their phone and we receive an encrypted token.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::error::CaptchaSolution;
use crate::rest::Rest;
use crate::transport::WsMessage;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub enum RemoteAuthEvent {
    /// Show this URL as a QR code.
    Qr { url: String },
    /// The phone scanned it; show who is signing in while they confirm.
    Scanned { user_id: String, username: String, avatar: Option<String> },
    LoggedIn { token: String },
    /// Password sign-in needs a second factor.
    Mfa { ticket: String, totp: bool, sms: bool, backup: bool, webauthn: bool, login_instance_id: Option<String> },
    /// Password or MFA step answered with a captcha challenge; the shell shows it and retries.
    Captcha { service: Option<String>, sitekey: Option<String>, rqdata: Option<String>, rqtoken: Option<String>, session_id: Option<String> },
    /// New login location: Discord emailed a link to approve it.
    VerifyEmail { message: String },
    Cancelled,
    Failed { message: String },
}

/// `solutions` carries the answer to a captcha Discord asks for after the phone approves (the
/// shell shows it, as for the password flow).
pub fn start(
    rest: Arc<Rest>,
    events: mpsc::UnboundedSender<RemoteAuthEvent>,
    solutions: mpsc::UnboundedReceiver<CaptchaSolution>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run(rest, &events, solutions).await {
            // The sentence a person can act on, never the debug text ("captcha required").
            let _ = events.send(RemoteAuthEvent::Failed { message: e.user_message() });
        }
    })
}

async fn run(
    rest: Arc<Rest>,
    events: &mpsc::UnboundedSender<RemoteAuthEvent>,
    mut solutions: mpsc::UnboundedReceiver<CaptchaSolution>,
) -> Result<()> {
    // The login page loads experiments first; the response sets the pre-login fingerprint and cookies.
    match tokio::time::timeout(Duration::from_secs(8), rest.fetch_fingerprint()).await {
        Ok(Err(e)) => tracing::debug!("experiments fetch failed: {e}"),
        Err(_) => tracing::debug!("experiments fetch timed out"),
        Ok(Ok(_)) => {}
    }
    let key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut rand::thread_rng(), 2048))
        .await
        .map_err(|e| Error::Crypto(e.to_string()))?
        .map_err(|e| Error::Crypto(e.to_string()))?;
    let spki = key.to_public_key().to_public_key_der().map_err(|e| Error::Crypto(e.to_string()))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let decrypt = |data: &str| -> Result<Vec<u8>> {
        let bytes = b64.decode(data).map_err(|e| Error::Crypto(e.to_string()))?;
        key.decrypt(Oaep::new::<Sha256>(), &bytes).map_err(|e| Error::Crypto(e.to_string()))
    };

    let url = rest.identity.profile.remote_auth_url.clone();
    let mut ws = tokio::time::timeout(Duration::from_secs(15), rest.transport.websocket(&url, rest.identity.websocket_headers()))
        .await
        .map_err(|_| Error::Other("Couldn't reach Discord. Check your connection.".into()))??;
    let mut heartbeat = Duration::from_secs(41);
    let mut next_beat: Option<Instant> = None;

    loop {
        let deadline = next_beat.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            msg = ws.recv() => {
                let text = match msg {
                    None => return Err(Error::Other("remote auth socket closed".into())),
                    Some(m) => match m? {
                        WsMessage::Text(t) => t.to_string(),
                        WsMessage::Close(_) => {
                            let _ = events.send(RemoteAuthEvent::Cancelled);
                            return Ok(());
                        }
                        _ => continue,
                    },
                };
                let v: Value = serde_json::from_str(&text)?;
                match v["op"].as_str().unwrap_or("") {
                    "hello" => {
                        heartbeat = Duration::from_millis(v["heartbeat_interval"].as_u64().unwrap_or(41_250));
                        next_beat = Some(Instant::now() + heartbeat);
                        let init = json!({ "op": "init", "encoded_public_key": b64.encode(spki.as_bytes()) });
                        ws.send(WsMessage::Text(init.to_string().into())).await?;
                    }
                    "nonce_proof" => {
                        let nonce = decrypt(v["encrypted_nonce"].as_str().unwrap_or(""))?;
                        let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(&nonce));
                        ws.send(WsMessage::Text(json!({ "op": "nonce_proof", "proof": proof }).to_string().into())).await?;
                    }
                    "pending_remote_init" => {
                        let fp = v["fingerprint"].as_str().unwrap_or("");
                        let _ = events.send(RemoteAuthEvent::Qr { url: format!("https://discord.com/ra/{fp}") });
                    }
                    "pending_ticket" => {
                        let payload = decrypt(v["encrypted_user_payload"].as_str().unwrap_or(""))?;
                        let s = String::from_utf8_lossy(&payload);
                        let mut parts = s.splitn(4, ':');
                        let user_id = parts.next().unwrap_or("").to_string();
                        let _discriminator = parts.next();
                        let avatar = parts.next().filter(|a| !a.is_empty() && *a != "0").map(str::to_string);
                        let username = parts.next().unwrap_or("").to_string();
                        let _ = events.send(RemoteAuthEvent::Scanned { user_id, username, avatar });
                    }
                    "pending_login" => {
                        let ticket = v["ticket"].as_str().unwrap_or("");
                        // Discord may ask for a captcha before it hands over the token (a new place or a
                        // risky network). Show it, keep the socket alive while it is solved, retry.
                        let mut solved: Option<CaptchaSolution> = None;
                        let encrypted = loop {
                            match rest.remote_auth_login(ticket, solved.take()).await {
                                Ok(e) => break e,
                                Err(Error::Captcha(c)) => {
                                    let c = *c;
                                    let _ = events.send(RemoteAuthEvent::Captcha {
                                        service: c.captcha_service, sitekey: c.captcha_sitekey, rqdata: c.captcha_rqdata,
                                        rqtoken: c.captcha_rqtoken, session_id: c.captcha_session_id,
                                    });
                                    let mut open = true;
                                    loop {
                                        tokio::select! {
                                            s = solutions.recv() => match s {
                                                Some(s) => { solved = Some(s); break }
                                                None => return Ok(()),
                                            },
                                            m = ws.recv(), if open => { open = m.is_some(); }
                                            _ = tokio::time::sleep(heartbeat), if open => {
                                                let _ = ws.send(WsMessage::Text(json!({ "op": "heartbeat" }).to_string().into())).await;
                                            }
                                        }
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        };
                        let token = String::from_utf8(decrypt(&encrypted)?).map_err(|e| Error::Crypto(e.to_string()))?;
                        let _ = events.send(RemoteAuthEvent::LoggedIn { token });
                        let _ = ws.close(1000u16, "").await;
                        return Ok(());
                    }
                    "cancel" => {
                        let _ = events.send(RemoteAuthEvent::Cancelled);
                        return Ok(());
                    }
                    "heartbeat_ack" => {}
                    other => tracing::debug!("remote auth op {other}"),
                }
            }
            _ = tokio::time::sleep_until(deadline), if next_beat.is_some() => {
                ws.send(WsMessage::Text(json!({ "op": "heartbeat" }).to_string().into())).await?;
                next_beat = Some(Instant::now() + heartbeat);
            }
        }
    }
}
