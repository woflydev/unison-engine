//! Gateway v9, `encoding=etf&compress=zstd-stream`, as the desktop client connects.
//!
//! One task per account owns the socket. It decompresses the zstd stream,
//! splits frames without decoding `d`, keeps the heartbeat, and forwards
//! dispatches to the session. Reconnects resume when possible and back off with
//! jitter otherwise.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::fidelity::{ClientIdentity, initial_presence};
use crate::model::Id;
use crate::transport::{Transport, WsMessage};
use crate::{Error, Result};

pub mod op {
    pub const DISPATCH: u8 = 0;
    pub const HEARTBEAT: u8 = 1;
    pub const IDENTIFY: u8 = 2;
    pub const PRESENCE_UPDATE: u8 = 3;
    pub const VOICE_STATE_UPDATE: u8 = 4;
    pub const RESUME: u8 = 6;
    pub const RECONNECT: u8 = 7;
    pub const REQUEST_GUILD_MEMBERS: u8 = 8;
    pub const INVALID_SESSION: u8 = 9;
    pub const HELLO: u8 = 10;
    pub const HEARTBEAT_ACK: u8 = 11;
    pub const GUILD_SUBSCRIPTIONS_BULK: u8 = 37;
}

/// Close codes after which reconnecting is pointless.
fn is_fatal(code: u16) -> bool {
    matches!(code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014)
}

#[derive(Debug)]
pub enum GatewayEvent {
    Connecting,
    /// A dispatch: event name and the raw ETF term of `d` (no version byte).
    Dispatch { name: String, data: Vec<u8> },
    Closed { code: u16, reason: String, fatal: bool },
}

#[derive(Debug)]
pub enum GatewayCommand {
    Send { op: u8, d: Value },
    /// Close the socket for good (logout or account removal).
    Shutdown,
}

#[derive(Clone)]
pub struct GatewayHandle {
    tx: mpsc::UnboundedSender<GatewayCommand>,
}

impl GatewayHandle {
    pub fn send(&self, op: u8, d: Value) {
        let _ = self.tx.send(GatewayCommand::Send { op, d });
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(GatewayCommand::Shutdown);
    }

    /// Op 4 as `voiceStateUpdate` builds it. `preferred_region(s)` are only added once a
    /// latency test has run; we don't run one yet, which matches the client before its first test.
    pub fn voice_state(&self, guild: Option<Id>, channel: Option<Id>, self_mute: bool, self_deaf: bool) {
        self.send(op::VOICE_STATE_UPDATE, voice_state_payload(guild, channel, self_mute, self_deaf));
    }

    pub fn update_presence(&self, status: &str, afk: bool) {
        self.send(op::PRESENCE_UPDATE, json!({ "status": status, "since": 0, "activities": [], "afk": afk }));
    }
}

pub fn voice_state_payload(guild: Option<Id>, channel: Option<Id>, self_mute: bool, self_deaf: bool) -> Value {
    json!({
        "guild_id": guild.map(|g| g.to_string()),
        "channel_id": channel.map(|c| c.to_string()),
        "self_mute": self_mute,
        "self_deaf": self_deaf,
        "self_video": false,
        "flags": 0,
        // A desktop client with video support puts its stream parameters on every voice state
        // (`voiceStateUpdate`: `tracks` from `getVideoStreamParameters()`, web bundle 2026-10-30).
        "tracks": [{ "type": "video", "rid": "100", "quality": 100 }],
    })
}

pub struct GatewayConfig {
    pub transport: Arc<Transport>,
    pub identity: Arc<ClientIdentity>,
    pub token: String,
    pub derived_qos: Option<String>,
    /// Wait before the first connection (secondary accounts at launch, see `Engine::restore_account`).
    pub start_delay: Duration,
}

pub fn spawn(cfg: GatewayConfig, events: mpsc::UnboundedSender<GatewayEvent>) -> GatewayHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(cfg, rx, events));
    GatewayHandle { tx }
}

struct Session {
    id: Option<String>,
    seq: Option<u64>,
    resume_url: Option<String>,
}

async fn run(
    mut cfg: GatewayConfig,
    mut rx: mpsc::UnboundedReceiver<GatewayCommand>,
    events: mpsc::UnboundedSender<GatewayEvent>,
) {
    let mut session = Session { id: None, seq: None, resume_url: None };
    let mut backoff = Backoff::new();
    if !cfg.start_delay.is_zero() {
        tokio::time::sleep(cfg.start_delay).await;
    }
    loop {
        let _ = events.send(GatewayEvent::Connecting);
        let outcome = connect_once(&mut cfg, &mut session, &mut rx, &events).await;
        match outcome {
            Ok(Exit::Shutdown) => return,
            Ok(Exit::Reconnect { resume }) => {
                if !resume {
                    session.id = None;
                    session.seq = None;
                }
                backoff.succeeded();
                // A fresh IDENTIFY (invalid session) waits 1-5 s, like the official client's random
                // delay, so a persistent refusal can never become a once-a-second login loop.
                let wait = if resume { 500..1500 } else { 1000..5000 };
                tokio::time::sleep(Duration::from_millis(fastrand::u64(wait))).await;
            }
            Err(Error::GatewayClosed { code, reason }) => {
                let fatal = is_fatal(code);
                let _ = events.send(GatewayEvent::Closed { code, reason, fatal });
                if fatal {
                    return;
                }
                tokio::time::sleep(backoff.next()).await;
            }
            Err(e @ Error::UpgradeRefused(_)) => {
                tracing::warn!("gateway error: {e}; not retrying");
                let _ = events.send(GatewayEvent::Closed { code: 0, reason: e.to_string(), fatal: true });
                return;
            }
            Err(e) => {
                tracing::warn!("gateway error: {e}");
                let _ = events.send(GatewayEvent::Closed { code: 0, reason: e.to_string(), fatal: false });
                tokio::time::sleep(backoff.next()).await;
            }
        }
    }
}

enum Exit {
    Shutdown,
    Reconnect { resume: bool },
}

async fn connect_once(
    cfg: &mut GatewayConfig,
    session: &mut Session,
    rx: &mut mpsc::UnboundedReceiver<GatewayCommand>,
    events: &mpsc::UnboundedSender<GatewayEvent>,
) -> Result<Exit> {
    let base = session.resume_url.clone().unwrap_or_else(|| cfg.identity.profile.gateway_url.clone());
    let url = cfg.identity.profile.gateway_connect_url(&base);
    let mut ws = cfg.transport.websocket(&url, cfg.identity.websocket_headers()).await?;
    let mut inflate = ZstdStream::new()?;

    let mut heartbeat_at: Option<Instant> = None;
    let mut interval = Duration::from_secs(41);
    let mut awaiting_ack = false;
    let mut identified = false;

    loop {
        let hb_deadline = heartbeat_at.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            msg = ws.recv() => {
                let Some(msg) = msg else {
                    return Err(Error::GatewayClosed { code: 0, reason: "socket ended".into() });
                };
                let bytes = match msg? {
                    WsMessage::Binary(b) => inflate.feed(&b)?,
                    WsMessage::Text(t) => t.as_bytes().to_vec(),
                    WsMessage::Close(frame) => {
                        let (code, reason) = frame.map(|f| (u16::from(f.code), f.reason.to_string())).unwrap_or((1000, String::new()));
                        return Err(Error::GatewayClosed { code, reason });
                    }
                    _ => continue,
                };
                if bytes.is_empty() {
                    continue;
                }
                let frame = unison_etf::split_frame(&bytes)?;
                if let Some(s) = frame.seq {
                    session.seq = Some(s);
                }
                match frame.op {
                    op::HELLO => {
                        let d: Value = frame.data.map(unison_etf::from_term).transpose()?.unwrap_or(Value::Null);
                        interval = Duration::from_millis(d["heartbeat_interval"].as_u64().unwrap_or(41_250));
                        // First beat at a random point in the interval, like every Discord client.
                        heartbeat_at = Some(Instant::now() + interval.mul_f64(fastrand::f64()));
                        if let (Some(id), Some(seq)) = (&session.id, session.seq) {
                            send(&mut ws, op::RESUME, json!({ "token": cfg.token, "session_id": id, "seq": seq })).await?;
                        } else {
                            let d = cfg.identity.identify(&cfg.token, initial_presence(), cfg.derived_qos.as_deref());
                            send(&mut ws, op::IDENTIFY, d).await?;
                        }
                        identified = true;
                    }
                    op::HEARTBEAT => {
                        send(&mut ws, op::HEARTBEAT, json!(session.seq)).await?;
                    }
                    op::HEARTBEAT_ACK => awaiting_ack = false,
                    op::RECONNECT => return Ok(Exit::Reconnect { resume: true }),
                    op::INVALID_SESSION => {
                        let resumable: bool = frame.data.map(unison_etf::from_term).transpose()?.unwrap_or(false);
                        return Ok(Exit::Reconnect { resume: resumable });
                    }
                    op::DISPATCH => {
                        let name = frame.event.unwrap_or_default().to_string();
                        let data = frame.data.map(|d| d.0.to_vec()).unwrap_or_default();
                        if name == "READY" {
                            #[derive(serde::Deserialize)]
                            struct R { session_id: String, resume_gateway_url: Option<String>, qos_token: Option<String> }
                            if let Some(d) = frame.data {
                                let r: R = unison_etf::from_term(d)?;
                                session.id = Some(r.session_id);
                                session.resume_url = r.resume_gateway_url;
                                if r.qos_token.is_some() {
                                    cfg.derived_qos = r.qos_token;
                                }
                            }
                        }
                        if events.send(GatewayEvent::Dispatch { name, data }).is_err() {
                            return Ok(Exit::Shutdown);
                        }
                    }
                    other => tracing::debug!("unhandled gateway op {other}"),
                }
            }
            _ = sleep_until(hb_deadline), if heartbeat_at.is_some() => {
                if awaiting_ack {
                    // Zombied connection: no ACK since the last beat.
                    return Ok(Exit::Reconnect { resume: true });
                }
                send(&mut ws, op::HEARTBEAT, json!(session.seq)).await?;
                awaiting_ack = true;
                heartbeat_at = Some(Instant::now() + interval);
            }
            cmd = rx.recv() => match cmd {
                None | Some(GatewayCommand::Shutdown) => {
                    let _ = ws.close(1000u16, "").await;
                    return Ok(Exit::Shutdown);
                }
                Some(GatewayCommand::Send { op, d }) => {
                    if identified {
                        if op != 1 { tracing::debug!(op, payload = %d, "gateway send"); }
                        send(&mut ws, op, d).await?;
                    } else {
                        tracing::debug!(op, "gateway send dropped: not identified yet");
                    }
                }
            },
        }
    }
}

async fn send(ws: &mut crate::transport::WebSocket, op: u8, d: Value) -> Result<()> {
    let bytes = unison_etf::encode_value(&json!({ "op": op, "d": d }));
    ws.send(WsMessage::Binary(bytes.into())).await?;
    Ok(())
}

/// Persistent zstd decompression context: each WebSocket message is a flushed chunk of one stream.
struct ZstdStream {
    ctx: zstd_safe::DCtx<'static>,
    /// Reused scratch the decompressor writes into (zeroed once, not per message).
    buf: Vec<u8>,
}

impl ZstdStream {
    fn new() -> Result<Self> {
        let ctx = zstd_safe::DCtx::try_create().ok_or_else(|| Error::Other("zstd context".into()))?;
        Ok(Self { ctx, buf: vec![0u8; 1 << 16] })
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(chunk.len() * 4);
        let mut input = zstd_safe::InBuffer::around(chunk);
        loop {
            let mut output = zstd_safe::OutBuffer::around(&mut self.buf[..]);
            self.ctx.decompress_stream(&mut output, &mut input).map_err(|c| Error::Decode(zstd_safe::get_error_name(c).into()))?;
            let produced = output.pos();
            let full = produced == self.buf.len();
            out.extend_from_slice(&self.buf[..produced]);
            if input.pos() == chunk.len() && !full {
                break;
            }
        }
        Ok(out)
    }
}

struct Backoff {
    attempt: u32,
}

impl Backoff {
    fn new() -> Self {
        Self { attempt: 0 }
    }
    fn succeeded(&mut self) {
        self.attempt = 0;
    }
    fn next(&mut self) -> Duration {
        self.attempt = (self.attempt + 1).min(7);
        let base = 1000u64 << (self.attempt - 1); // 1s .. 64s
        Duration::from_millis(base.min(60_000) + fastrand::u64(0..1000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_state_update_shape() {
        use crate::snowflake::Snowflake;
        assert_eq!(
            voice_state_payload(Some(Snowflake(1)), Some(Snowflake(2)), false, true).to_string(),
            r#"{"guild_id":"1","channel_id":"2","self_mute":false,"self_deaf":true,"self_video":false,"flags":0,"tracks":[{"type":"video","rid":"100","quality":100}]}"#
        );
        assert_eq!(
            voice_state_payload(Some(Snowflake(1)), None, false, false).to_string(),
            r#"{"guild_id":"1","channel_id":null,"self_mute":false,"self_deaf":false,"self_video":false,"flags":0,"tracks":[{"type":"video","rid":"100","quality":100}]}"#
        );
    }

    #[test]
    fn zstd_stream_across_chunks() {
        let frames: Vec<Vec<u8>> = (0..3).map(|i| unison_etf::encode_value(&json!({"op": 0, "s": i, "t": "X", "d": {"n": i}}))).collect();
        let mut cctx = zstd_safe::CCtx::create();
        let mut chunks = Vec::new();
        for f in &frames {
            let mut out = vec![0u8; 4096];
            let mut ob = zstd_safe::OutBuffer::around(&mut out[..]);
            let mut ib = zstd_safe::InBuffer::around(&f[..]);
            cctx.compress_stream2(&mut ob, &mut ib, zstd_safe::zstd_sys::ZSTD_EndDirective::ZSTD_e_flush).unwrap();
            let n = ob.pos();
            chunks.push(out[..n].to_vec());
        }
        let mut z = ZstdStream::new().unwrap();
        for (i, c) in chunks.iter().enumerate() {
            let bytes = z.feed(c).unwrap();
            let f = unison_etf::split_frame(&bytes).unwrap();
            assert_eq!(f.seq, Some(i as u64));
        }
    }

    #[test]
    fn backoff_grows_and_caps() {
        let mut b = Backoff::new();
        let first = b.next();
        assert!(first >= Duration::from_secs(1) && first < Duration::from_secs(2));
        for _ in 0..10 {
            assert!(b.next() <= Duration::from_secs(61));
        }
    }
}
