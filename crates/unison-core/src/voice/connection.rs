//! One voice connection: the voice gateway task plus the media and receive threads.
//!
//! The gateway task (tokio) speaks the control protocol and owns DAVE's MLS flow. Once the
//! session description arrives, two plain threads take over the media: the media thread wakes
//! on each capture callback, encodes 20 ms frames and sends them; the receive thread decrypts,
//! decodes and hands PCM to the playout mixer. Neither touches the async runtime.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::fidelity::ClientIdentity;
use crate::model::Id;
use crate::snowflake::Snowflake;
use crate::transport::Transport;
use crate::voice::audio::{self, Controls, Feeds};
use crate::voice::params;
use crate::voice::codec::{self, Decoder, Encoder, FRAME};
use crate::voice::crypto::{self, Transport as Aead};
use crate::voice::dave::{self, Dave, Out};
use crate::voice::protocol::{self, op};
use crate::voice::rtp;
use crate::voice::recv;
use crate::voice::stream::{self, Media, RemoteStream, Role, VideoIo, VideoState, VideoWant};

/// What the session needs to open a connection (from VOICE_STATE_UPDATE + VOICE_SERVER_UPDATE).
#[derive(Debug, Clone)]
pub struct Params {
    pub guild_id: Option<Id>,
    pub channel_id: Id,
    pub user_id: Id,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
    /// The channel's bitrate in bits per second.
    pub bitrate: i32,
    pub muted: bool,
    pub deaf: bool,
    pub role: Role,
    /// Go Live streams identify with the stream's own `rtc_server_id`.
    pub server_id: Option<Id>,
    pub video: Option<VideoIo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    Status(Status),
    /// End-to-end encryption came up (with its privacy code) or went away.
    Encryption { e2ee: bool, privacy_code: Option<String> },
    Speaking { user: Id, speaking: bool },
    /// The microphone or speakers couldn't be opened.
    AudioError(String),
    /// Round trip of the last voice-gateway heartbeat.
    Ping(u32),
}

pub type Emit = Arc<dyn Fn(VoiceEvent) + Send + Sync>;

impl Shared {
    fn video_out(&self, v: Value) {
        if let Some(o) = self.video.out_sender() {
            let _ = o.send(v);
        }
    }
}

struct Shared {
    muted: AtomicBool,
    stop: AtomicBool,
    controls: Arc<Controls>,
    dave: Mutex<Dave>,
    /// SSRC → user, from op 5.
    users: Mutex<HashMap<u32, u64>>,
    video: VideoState,
    role: Role,
    /// Users whose incoming video the app wants a fresh keyframe of (a tile shown again after it
    /// was hidden and stopped decoding). The receive loop turns each into a PLI.
    keyframe_wanted: Mutex<Vec<u64>>,
}

pub struct VoiceHandle {
    shared: Arc<Shared>,
    shutdown: mpsc::UnboundedSender<()>,
}

impl VoiceHandle {
    /// Ask `user`'s sender for a keyframe (PLI), and drop their P frames until it comes: the app
    /// stopped decoding while the tile was hidden, so it has no reference to decode them against.
    pub fn request_keyframe(&self, user: u64) {
        self.shared.keyframe_wanted.lock().push(user);
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    /// Silence playout. The session mutes the microphone alongside, as deafen implies.
    pub fn set_deaf(&self, deaf: bool) {
        self.shared.controls.deaf.store(deaf, Ordering::Relaxed);
    }

    /// Start sending video (camera on the call connection, the screen on a stream connection).
    pub fn start_video(&self, want: VideoWant) {
        self.shared.video.start(want);
    }

    pub fn stop_video(&self) {
        self.shared.video.stop();
    }

    pub fn sending_video(&self) -> bool {
        self.shared.video.sending()
    }

    /// One encoded access unit (Annex B) from the app's hardware encoder.
    pub fn push_video(&self, annexb: &[u8], pts_90k: u32) {
        if !self.shared.video.sending() {
            self.shared.video.note_idle();
            return;
        }
        let enc = self.shared.dave.lock().encrypt_video(annexb).map(|c| c.into_owned());
        match enc {
            Some(f) => self.shared.video.push(&f, pts_90k),
            None => self.shared.video.note_dave_refused(),
        }
    }

    /// Op 15: what we want forwarded (`any` percent for everything not listed).
    pub fn set_sink_wants(&self, any: u32, per_ssrc: &[(u32, u32)]) {
        self.shared.video_out(crate::voice::video::media_sink_wants(any, per_ssrc));
    }

    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = self.shutdown.send(());
    }
}

impl Drop for VoiceHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn spawn(p: Params, transport: Arc<Transport>, identity: Arc<ClientIdentity>, emit: Emit) -> VoiceHandle {
    let controls = Controls::new();
    controls.deaf.store(p.deaf, Ordering::Relaxed);
    let shared = Arc::new(Shared {
        muted: AtomicBool::new(p.muted || p.deaf),
        stop: AtomicBool::new(false),
        controls,
        dave: Mutex::new(Dave::new(p.user_id.0, p.channel_id.0)),
        users: Mutex::new(HashMap::new()),
        video: VideoState::default(),
        role: p.role,
        keyframe_wanted: Mutex::new(Vec::new()),
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let s = shared.clone();
    tokio::spawn(async move { run(p, transport, identity, s, emit, rx).await });
    VoiceHandle { shared, shutdown: tx }
}

enum Exit {
    Shutdown,
    Resume,
    Fatal,
}

/// State that survives a gateway resume.
#[derive(Default)]
struct Conn {
    ssrc: u32,
    seq_ack: Option<i64>,
    server_version: u8,
    experiments: Vec<String>,
    /// Set once media is running; a resume keeps it.
    media: bool,
    socket: Option<UdpSocket>,
    e2ee: bool,
}

async fn run(
    p: Params,
    transport: Arc<Transport>,
    identity: Arc<ClientIdentity>,
    shared: Arc<Shared>,
    emit: Emit,
    mut shutdown: mpsc::UnboundedReceiver<()>,
) {
    let mut conn = Conn::default();
    let mut failures = 0u32;
    loop {
        emit(VoiceEvent::Status(if conn.media { Status::Reconnecting } else { Status::Connecting }));
        match once(&p, &transport, &identity, &shared, &emit, &mut conn, &mut shutdown).await {
            Exit::Shutdown | Exit::Fatal => break,
            Exit::Resume => {
                failures += 1;
                if failures > 3 {
                    break;
                }
                // Backoff 1 s → 5 s, as the control socket's.
                tokio::time::sleep(Duration::from_millis((1000 * failures as u64).min(5000))).await;
            }
        }
    }
    shared.stop.store(true, Ordering::Relaxed);
    emit(VoiceEvent::Status(Status::Disconnected));
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

async fn once(
    p: &Params,
    transport: &Transport,
    identity: &ClientIdentity,
    shared: &Arc<Shared>,
    emit: &Emit,
    conn: &mut Conn,
    shutdown: &mut mpsc::UnboundedReceiver<()>,
) -> Exit {
    let server_id = p.server_id.unwrap_or_else(|| p.guild_id.unwrap_or(p.channel_id));
    // Tests point at a local plain-WebSocket server.
    let url = if cfg!(test) && p.endpoint.starts_with("ws://") {
        format!("{}/?v={}", p.endpoint, protocol::VERSION)
    } else {
        protocol::url(&p.endpoint)
    };
    let Ok(mut ws) = transport.websocket(&url, identity.websocket_headers()).await else {
        return Exit::Resume;
    };
    let hello = if conn.media {
        protocol::resume(&p.token, &p.session_id, server_id, p.channel_id, conn.seq_ack)
    } else {
        protocol::identify(&protocol::IdentifyInput {
            server_id,
            channel_id: p.channel_id,
            user_id: p.user_id,
            session_id: &p.session_id,
            token: &p.token,
            max_dave_protocol_version: dave::MAX_VERSION,
            screen: p.role != Role::Call,
        })
    };
    if ws.send(WsMessage::Text(hello.to_string().into())).await.is_err() {
        return Exit::Resume;
    }

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let mut beat: Option<tokio::time::Interval> = None;
    let mut acked = true;
    let mut sent_at: Option<std::time::Instant> = None;

    loop {
        let tick = async {
            match beat.as_mut() {
                Some(b) => b.tick().await,
                None => std::future::pending().await,
            }
        };
        let mut send: Vec<Out> = Vec::new();
        tokio::select! {
            _ = shutdown.recv() => {
                let _ = ws.close(1000, "").await;
                return Exit::Shutdown;
            }
            _ = tick => {
                if !acked {
                    return Exit::Resume;
                }
                acked = false;
                sent_at = Some(std::time::Instant::now());
                send.push(Out::Json(protocol::heartbeat(now_ms(), conn.seq_ack, conn.server_version)));
            }
            Some(v) = out_rx.recv() => send.push(Out::Json(v)),
            msg = ws.recv() => {
                let Some(Ok(msg)) = msg else { return Exit::Resume };
                match msg {
                    WsMessage::Text(t) => {
                        let Ok(f) = serde_json::from_str::<protocol::Frame>(&t) else { continue };
                        if let Some(s) = f.seq {
                            conn.seq_ack = Some(s);
                        }
                        match f.op {
                            op::HELLO => {
                                let h: protocol::Hello = serde_json::from_value(f.d).unwrap_or_default();
                                conn.server_version = h.v;
                                let every = Duration::from_millis(h.heartbeat_interval.max(1000.0) as u64);
                                let mut i = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
                                i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                beat = Some(i);
                            }
                            op::HEARTBEAT_ACK => {
                                acked = true;
                                if let Some(t) = sent_at.take() {
                                    emit(VoiceEvent::Ping(t.elapsed().as_millis().min(60_000) as u32));
                                }
                            }
                            op::READY => {
                                let r: protocol::Ready = serde_json::from_value(f.d).unwrap_or_default();
                                conn.ssrc = r.ssrc;
                                {
                                    // The stream we may publish: the server's offer, else audio SSRC + 1 and + 2.
                                    let (v, x) = r.streams.first().filter(|s| s.ssrc != 0).map(|s| (s.ssrc, s.rtx_ssrc)).unwrap_or((r.ssrc + 1, r.ssrc + 2));
                                    *shared.video.ssrcs.lock() = stream::Ssrcs { audio: r.ssrc, video: v, rtx: x };
                                }
                                conn.experiments = r.experiments.clone();
                                if conn.server_version >= 6 {
                                    send.push(Out::Json(protocol::backend_version()));
                                }
                                let Some(mode) = crypto::choose_mode(&r.modes) else { return Exit::Fatal };
                                let (ip, port, ssrc) = (r.ip.clone(), r.port, r.ssrc);
                                let found = tokio::task::spawn_blocking(move || discover(&ip, port, ssrc)).await.ok().flatten();
                                let Some((sock, addr, local_port)) = found else { return Exit::Resume };
                                conn.socket = Some(sock);
                                let rtc_connection_id = uuid::Uuid::new_v4().to_string();
                                send.push(Out::Json(protocol::select_protocol(&addr, local_port, mode, &rtc_connection_id, &protocol::client_experiments())));
                            }
                            op::SESSION_DESCRIPTION => {
                                let d: protocol::SessionDescription = serde_json::from_value(f.d).unwrap_or_default();
                                shared.dave.lock().on_session_description(d.dave_protocol_version);
                                if !conn.media {
                                    if let Some(sock) = conn.socket.take() {
                                        if start_media(p, sock, conn.ssrc, &d.mode, &d.secret_key, shared, emit, out_tx.clone(), p.video.clone()) {
                                            conn.media = true;
                                        }
                                    }
                                }
                                if shared.role != Role::StreamPublisher {
                                    // Ask the server to forward everything it has for us (op 15).
                                    send.push(Out::Json(crate::voice::video::media_sink_wants(100, &[])));
                                }
                                emit(VoiceEvent::Status(Status::Connected));
                            }
                            op::RESUMED => emit(VoiceEvent::Status(Status::Connected)),
                            op::VIDEO => {
                                // Someone's video SSRCs: `{user_id, audio_ssrc, video_ssrc, rtx_ssrc, streams}`.
                                if let Some(u) = f.d["user_id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                                    let mut users = shared.video.users.lock();
                                    for k in ["video_ssrc", "rtx_ssrc"] {
                                        if let Some(ss) = f.d[k].as_u64().filter(|s| *s != 0) {
                                            users.insert(ss as u32, u);
                                        }
                                    }
                                    let pair = |media: Option<u64>, rtx: Option<u64>| {
                                        if let (Some(m), Some(x)) = (media.filter(|s| *s != 0), rtx.filter(|s| *s != 0)) {
                                            shared.video.rtx_of.lock().insert(x as u32, m as u32);
                                        }
                                    };
                                    pair(f.d["video_ssrc"].as_u64(), f.d["rtx_ssrc"].as_u64());
                                    for st in f.d["streams"].as_array().into_iter().flatten() {
                                        for k in ["ssrc", "rtx_ssrc"] {
                                            if let Some(ss) = st[k].as_u64().filter(|s| *s != 0) {
                                                users.insert(ss as u32, u);
                                            }
                                        }
                                        pair(st["ssrc"].as_u64(), st["rtx_ssrc"].as_u64());
                                    }
                                    shared.dave.lock().clients_connect([u]);
                                }
                            }
                            op::SPEAKING => {
                                let s: protocol::SpeakingEvent = serde_json::from_value(f.d).unwrap_or_default();
                                if let Some(u) = s.user_id.and_then(|u| u.parse::<u64>().ok()) {
                                    shared.users.lock().insert(s.ssrc, u);
                                    shared.dave.lock().clients_connect([u]);
                                }
                            }
                            op::CLIENTS_CONNECT => {
                                let ids: Vec<u64> = f.d["user_ids"].as_array().into_iter().flatten()
                                    .filter_map(|v| v.as_str().and_then(|s| s.parse().ok())).collect();
                                shared.dave.lock().clients_connect(ids);
                            }
                            op::CLIENT_DISCONNECT => {
                                if let Some(u) = f.d["user_id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                                    shared.dave.lock().client_disconnect(u);
                                    shared.users.lock().retain(|_, v| *v != u);
                                    shared.video.users.lock().retain(|_, v| *v != u);
                                    emit(VoiceEvent::Speaking { user: Snowflake(u), speaking: false });
                                }
                            }
                            op::DAVE_PREPARE_TRANSITION => {
                                let t: protocol::Transition = serde_json::from_value(f.d).unwrap_or_default();
                                send.extend(shared.dave.lock().on_prepare_transition(t.transition_id, t.protocol_version));
                            }
                            op::DAVE_EXECUTE_TRANSITION => {
                                let t: protocol::Transition = serde_json::from_value(f.d).unwrap_or_default();
                                shared.dave.lock().on_execute_transition(t.transition_id);
                            }
                            op::DAVE_PREPARE_EPOCH => {
                                let t: protocol::Transition = serde_json::from_value(f.d).unwrap_or_default();
                                send.extend(shared.dave.lock().on_prepare_epoch(t.epoch, t.protocol_version));
                            }
                            _ => {}
                        }
                    }
                    WsMessage::Binary(b) => {
                        let Some((seq, opcode, payload)) = protocol::parse_binary(&b) else { continue };
                        conn.seq_ack = Some(seq as i64);
                        let mut d = shared.dave.lock();
                        send.extend(match opcode {
                            op::DAVE_EXTERNAL_SENDER => d.on_external_sender(payload),
                            op::DAVE_PROPOSALS => d.on_proposals(payload),
                            op::DAVE_ANNOUNCE_COMMIT => d.on_announce_commit(payload),
                            op::DAVE_WELCOME => d.on_welcome(payload),
                            _ => vec![],
                        });
                    }
                    WsMessage::Close(frame) => {
                        let code: u16 = frame.map(|f| f.code.into()).unwrap_or(1006);
                        // 4004 auth, 4006 session gone, 4011 server not found, 4015 server crashed (the official client
                        // stops on these four), 4014 kicked or channel deleted: don't resume.
                        return if matches!(code, 4004 | 4006 | 4011 | 4014 | 4015 | 4016) { Exit::Fatal } else { Exit::Resume };
                    }
                    _ => {}
                }
            }
        }
        for o in send {
            let r = match o {
                Out::Json(v) => ws.send(WsMessage::Text(v.to_string().into())).await,
                Out::Binary(b) => ws.send(WsMessage::Binary(b.into())).await,
            };
            if r.is_err() {
                return Exit::Resume;
            }
        }
        let (e2ee, code) = {
            let d = shared.dave.lock();
            (d.is_active(), d.privacy_code())
        };
        if e2ee != conn.e2ee {
            conn.e2ee = e2ee;
            emit(VoiceEvent::Encryption { e2ee, privacy_code: code });
        }
    }
}

/// IP discovery over a fresh socket connected to the voice server. Returns the socket and
/// our external address as the server sees it.
fn discover(ip: &str, port: u16, ssrc: u32) -> Option<(UdpSocket, String, u16)> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect((ip, port)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    let mut buf = [0u8; 74];
    for _ in 0..5 {
        sock.send(&rtp::discovery_request(ssrc)).ok()?;
        if let Ok(n) = sock.recv(&mut buf) {
            if let Some((addr, p)) = rtp::discovery_response(&buf[..n]) {
                return Some((sock, addr, p));
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn start_media(
    p: &Params,
    sock: UdpSocket,
    ssrc: u32,
    mode: &str,
    key: &[u8],
    shared: &Arc<Shared>,
    emit: &Emit,
    out: mpsc::UnboundedSender<Value>,
    video_io: Option<VideoIo>,
) -> bool {
    let (Some(seal), Some(open), Ok(rx_sock), Ok(tx_sock)) = (Aead::new(mode, key), Aead::new(mode, key), sock.try_clone(), sock.try_clone()) else {
        return false;
    };
    let seal = Arc::new(Mutex::new(seal));
    let cname = crate::voice::rtcp::random_cname();
    let media = Arc::new(Media { sock: tx_sock, seal: seal.clone(), cname: cname.clone() });
    shared.video.attach(media, out.clone(), video_io);
    let role = p.role;
    let (mixer, feeds) = audio::mixer(shared.controls.clone());
    let me = p.user_id;
    let bitrate = p.bitrate;
    {
        let shared = shared.clone();
        let emit = emit.clone();
        let feeds = role.has_audio().then_some(feeds);
        let _ = thread::Builder::new().name("unison-voice-rx".into()).spawn(move || receive(rx_sock, open, feeds, shared, emit));
    }
    if !role.has_audio() {
        // A stream connection carries video only: no microphone, no playout.
        return true;
    }
    if cfg!(test) {
        // Tests never open the microphone.
        return true;
    }
    let shared = shared.clone();
    let emit = emit.clone();
    thread::Builder::new()
        .name("unison-voice-tx".into())
        .spawn(move || {
            set_realtime_qos();
            let (device, capture) = match audio::open(mixer, thread::current()) {
                Ok(d) => d,
                Err(e) => {
                    emit(VoiceEvent::AudioError(e));
                    return;
                }
            };
            send_loop(sock, seal, cname, capture, ssrc, me, bitrate, &shared, &emit, &out);
            drop(device);
        })
        .is_ok()
}

#[cfg(target_os = "macos")]
fn set_realtime_qos() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_realtime_qos() {}

/// Silence frames sent after speech: `AudioTransport::ShouldSendSilence` in `discord_voice.node` stops
/// at a counter of 10 (`cmp w8, #9; b.gt`), each one the last packet's header and extension block
/// with `F8 FF FE` as payload (`discord::SilencePacket`).
const SILENCE_FRAMES: u32 = 10;

#[allow(clippy::too_many_arguments)]
fn send_loop(
    sock: UdpSocket,
    seal: Arc<Mutex<Aead>>,
    cname: String,
    mut capture: audio::Capture,
    ssrc: u32,
    me: Id,
    bitrate: i32,
    shared: &Shared,
    emit: &Emit,
    out: &mpsc::UnboundedSender<Value>,
) {
    let Some(mut enc) = Encoder::new(bitrate) else { return };
    let mut seq: u16 = fastrand::u16(..);
    let mut ts: u32 = fastrand::u32(..);
    let mut mono = [0f32; FRAME];
    let mut stereo = vec![0f32; FRAME * codec::CHANNELS];
    let (mut speaking, mut silence) = (false, 0u32);
    let pr = params::params();
    let mut gate = params::Gate::default();
    let mut speaking_gate = protocol::SpeakingGate::default();
    // Sender report cadence and counters for our audio stream (every 5 s while we send).
    let mut sr_clock = crate::voice::rtcp::Cadence::new(crate::voice::rtcp::Cadence::AUDIO_MS);
    let mut stats = crate::voice::video::SenderStats::default();
    tracing::debug!(target: "unison_core::voice", ids = "audio-level 1, abs-send-time 3", "audio rtp extensions in use");
    while !shared.stop.load(Ordering::Relaxed) {
        thread::park_timeout(Duration::from_millis(100));
        while capture.frame(&mut mono) {
            ts = ts.wrapping_add(rtp::FRAME_SAMPLES);
            let muted = shared.muted.load(Ordering::Relaxed);
            // Input volume first: the gate and the encoder both hear the adjusted signal.
            let gain = pr.input_gain();
            if (gain - 1.0).abs() > 1e-3 {
                for s in mono.iter_mut() {
                    *s = (*s * gain).clamp(-1.0, 1.0);
                }
            }
            let level_db = audio::level_db(&mono);
            let voice_now = pr.gate(&mut gate, level_db);
            let voice = !muted && voice_now;
            if voice != speaking {
                speaking = voice;
                let status = if voice { protocol::SPEAKING_VOICE } else { 0 };
                if speaking_gate.should_send(status, ssrc) {
                    let _ = out.send(protocol::speaking(status, ssrc));
                }
                emit(VoiceEvent::Speaking { user: me, speaking: voice });
                if !voice {
                    silence = SILENCE_FRAMES;
                }
            }
            let frame: Option<Vec<u8>> = if voice {
                for (i, s) in mono.iter().enumerate() {
                    stereo[2 * i] = *s;
                    stereo[2 * i + 1] = *s;
                }
                let Some(opus) = enc.encode(&stereo) else { continue };
                shared.dave.lock().encrypt_opus(opus).map(|c| c.into_owned())
            } else if silence > 0 {
                silence -= 1;
                Some(rtp::SILENCE.to_vec())
            } else {
                None
            };
            if let Some(payload) = frame {
                // Audio carries the audio-level and abs-send-time extensions like the official client; the
                // extension body is sealed in front of the payload.
                let (hdr, ext) = rtp::audio_header_with_extensions(seq, ts, ssrc, level_db, voice, now_ms());
                let sealed = seal.lock().seal(&hdr, &[&ext[..], &payload[..]].concat());
                if let Some(pkt) = sealed {
                    let _ = sock.send(&pkt);
                }
                seq = seq.wrapping_add(1);
                stats.on_packet(payload.len(), ts, now_ms());
                let now = now_ms();
                if sr_clock.due(now) {
                    let sr = crate::voice::rtcp::sender_compound(ssrc, now, stats.rtp_ts_at(now, 48), stats.packets, stats.octets, &cname);
                    if sr.len() > 8 {
                        if let Some(p) = seal.lock().seal(&sr[..8], &sr[8..]) {
                            let _ = sock.send(&p);
                            tracing::debug!(target: "unison_core::voice", packets = stats.packets, "rtcp sender report sent (audio)");
                        }
                    }
                }
            }
        }
    }
}

struct Stream {
    slot: usize,
    decoder: Decoder,
    last_seq: Option<u16>,
    last_voice: Instant,
    speaking: bool,
    gain_version: u32,
}

fn receive(sock: UdpSocket, open: Aead, mut feeds: Option<Feeds>, shared: Arc<Shared>, emit: Emit) {
    use crate::voice::rtcp::{Rtcp, TccRecorder};
    let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    let mut remote: HashMap<u32, RemoteStream> = HashMap::new();
    let mut buf = vec![0u8; 2048];
    let mut pcm = vec![0f32; FRAME * codec::CHANNELS];
    let mut next_slot = 0usize;
    let started = Instant::now();
    let mut tcc = TccRecorder::default();
    let mut tcc_clock = crate::voice::rtcp::Cadence::new(100);
    let mut report_clock = crate::voice::rtcp::Cadence::new(1000);
    let mut tcc_logged = false;
    while !shared.stop.load(Ordering::Relaxed) {
        let n = sock.recv(&mut buf).unwrap_or(0);
        let now = Instant::now();
        let now_ms = now_ms();
        if n > 0 && rtp::is_rtcp(&buf[..n]) {
            if let Some(plain) = stream::open_rtcp(&open, &buf[..n]) {
                let parsed = crate::voice::rtcp::parse(&plain);
                for p in &parsed {
                    if let Rtcp::SenderReport { ssrc, ntp, .. } = p {
                        if let Some(rs) = remote.get_mut(ssrc) {
                            rs.stats.on_sender_report(*ntp, now_ms);
                        }
                    }
                }
                shared.video.on_rtcp(now_ms, &parsed);
            }
        } else if n > 0 {
            if let Some(h) = rtp::parse(&buf[..n]) {
                if let Some((ext, mut payload)) = open.open_parts(&buf[..n], &h) {
                    if let Some(seq) = rtp::transport_seq(&ext) {
                        if !tcc_logged {
                            tcc_logged = true;
                            tracing::debug!(target: "unison_core::voice", "incoming packets carry the transport-wide sequence number; sending transport-cc feedback");
                        }
                        tcc.record(h.ssrc, seq, now.duration_since(started).as_micros() as i64);
                    }
                    let marker = buf[1] & 0x80 != 0;
                    if !rtp::strip_padding(&mut payload, h.padded) {
                        // Padding-only (a bandwidth probe) or malformed: nothing to play.
                    } else if h.payload_type == stream::H264_PT {
                        let rot = rtp::video_rotation(&ext);
                        receive_video(&shared, &mut remote, h.ssrc, h.seq, payload, marker, h.timestamp, rot, false, now_ms, now);
                    } else if h.payload_type == stream::H264_RTX_PT {
                        let media = shared.video.rtx_of.lock().get(&h.ssrc).copied();
                        if let (Some(media), Some((osn, orig))) = (media, crate::voice::h264::rtx_unwrap(&payload)) {
                            let rot = rtp::video_rotation(&ext);
                            receive_video(&shared, &mut remote, media, osn, orig.to_vec(), marker, h.timestamp, rot, true, now_ms, now);
                        }
                    } else if h.payload_type == rtp::OPUS_PT {
                        if let Some(feeds) = feeds.as_mut() {
                            receive_audio(&payload, &h, &shared, &emit, feeds, &mut streams, &mut pcm, &mut next_slot, now);
                        }
                    }
                }
            }
        }
        // Timeouts and retries, also when nothing arrived.
        let wanted = std::mem::take(&mut *shared.keyframe_wanted.lock());
        let ssrcs: Vec<u32> = remote.keys().copied().collect();
        for ssrc in ssrcs {
            if let Some(rs) = remote.get_mut(&ssrc) {
                if !wanted.is_empty() && shared.video.users.lock().get(&ssrc).is_some_and(|u| wanted.contains(u)) {
                    rs.want_keyframe();
                }
                let out = rs.poll(now_ms, now);
                deliver_video(&shared, ssrc, out, rs.rotation, &mut rs.count);
            }
        }
        if tcc_clock.due(now_ms) {
            let audio = shared.video.ssrcs.lock().audio;
            if let (Some(media), Some(fb)) = (shared.video.media(), tcc.build(audio)) {
                media.send_rtcp(&fb);
            }
        }
        if report_clock.due(now_ms) {
            send_receiver_reports(&shared, &mut remote, now_ms);
        }
        for (ssrc, st) in streams.iter_mut() {
            if st.speaking && now.duration_since(st.last_voice) > Duration::from_millis(250) {
                st.speaking = false;
                if let Some(u) = shared.users.lock().get(ssrc).copied() {
                    emit(VoiceEvent::Speaking { user: Snowflake(u), speaking: false });
                }
            }
        }
    }
}

/// One remote video packet (or its RTX copy): reorder, request what is missing, decode what completed.
#[allow(clippy::too_many_arguments)]
fn receive_video(shared: &Shared, remote: &mut HashMap<u32, RemoteStream>, ssrc: u32, seq: u16, payload: Vec<u8>, marker: bool, timestamp: u32, rotation: Option<u16>, via_rtx: bool, now_ms: u64, now: Instant) {
    if !shared.video.users.lock().contains_key(&ssrc) {
        return;
    }
    let rs = remote.entry(ssrc).or_insert_with(RemoteStream::new);
    if let Some(r) = rotation {
        rs.rotation = r;
    }
    let out = rs.push(seq, recv::Packet { payload, marker, timestamp }, via_rtx, now_ms, now);
    deliver_video(shared, ssrc, out, rs.rotation, &mut rs.count);
}

/// Send what a stream asked for (NACK, PLI) and hand finished frames to the app's decoder.
/// Frames that cannot be handed over are counted, so the 1 s summary says why video is missing.
fn deliver_video(shared: &Shared, ssrc: u32, out: stream::RxOut, rotation: u16, count: &mut stream::RxCount) {
    if out.pli || !out.nacks.is_empty() {
        if let Some(media) = shared.video.media() {
            let audio = shared.video.ssrcs.lock().audio;
            if !out.nacks.is_empty() {
                media.send_rtcp(&crate::voice::rtcp::nack(audio, ssrc, &out.nacks));
            }
            if out.pli {
                media.send_rtcp(&stream::pli(audio, ssrc));
            }
        }
    }
    let Some(user) = shared.video.users.lock().get(&ssrc).copied() else { return };
    let Some(io) = shared.video.io() else {
        count.no_sink += out.frames.len() as u32;
        return;
    };
    for (f, ts) in out.frames {
        match shared.dave.lock().decrypt_video(user, &f.annexb) {
            Some(annexb) => (io.frames)(stream::RemoteFrame { user, annexb, keyframe: f.keyframe, pts: ts, rotation }),
            None => count.dave_failed += 1,
        }
    }
}

/// Once a second: a receiver report (with our CNAME) for every video stream that sent something, and
/// one debug line per stream with what arrived, what was repaired and what was given up on.
fn send_receiver_reports(shared: &Shared, remote: &mut HashMap<u32, RemoteStream>, now_ms: u64) {
    let audio = shared.video.ssrcs.lock().audio;
    let media = shared.video.media();
    for (ssrc, rs) in remote.iter_mut() {
        let c = std::mem::take(&mut rs.count);
        if c.packets == 0 && c.recovered == 0 {
            continue;
        }
        let skipped = rs.take_skipped();
        tracing::debug!(target: "unison_core::voice", ssrc = *ssrc, packets = c.packets, recovered = c.recovered, nacked = c.nacked, given_up = skipped, frames = c.frames, dave_failed = c.dave_failed, no_sink = c.no_sink, keyframe_requests = c.plis, "video rx, last 1 s");
        if let (Some(media), Some(block)) = (media.as_ref(), rs.stats.block(*ssrc, now_ms)) {
            if audio != 0 {
                let mut pkt = crate::voice::rtcp::receiver_report(audio, &block);
                pkt.extend_from_slice(&crate::voice::rtcp::sdes_cname(audio, &media.cname));
                media.send_rtcp(&pkt);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn receive_audio(
    payload: &[u8],
    h: &rtp::Incoming,
    shared: &Shared,
    emit: &Emit,
    feeds: &mut Feeds,
    streams: &mut HashMap<u32, Stream>,
    pcm: &mut [f32],
    next_slot: &mut usize,
    now: Instant,
) {
    let Some(user) = shared.users.lock().get(&h.ssrc).copied() else { return };
    let Some(frame) = shared.dave.lock().decrypt(user, payload) else { return };
    let st = streams.entry(h.ssrc).or_insert_with(|| {
        let slot = *next_slot % audio::SLOTS;
        *next_slot += 1;
        Stream { slot, decoder: Decoder::new().expect("opus decoder"), last_seq: None, last_voice: now, speaking: false, gain_version: u32::MAX }
    });
    let ver = params::params().users_version();
    if st.gain_version != ver {
        st.gain_version = ver;
        let g = params::params().user_gain(user);
        shared.controls.gain[st.slot].store(g.to_bits(), Ordering::Relaxed);
    }
    let gap = st.last_seq.map(|l| h.seq.wrapping_sub(l));
    if gap.is_none_or(|g| g != 0 && g < 0x8000) {
        // Lost packets: recover the one just before from FEC, conceal the rest.
        if let Some(g @ 2..=4) = gap {
            for _ in 0..g - 2 {
                let k = st.decoder.decode(None, pcm, false);
                feeds.push(st.slot, &pcm[..k * codec::CHANNELS]);
            }
            let k = st.decoder.decode(Some(&frame), pcm, true);
            feeds.push(st.slot, &pcm[..k * codec::CHANNELS]);
        }
        let k = st.decoder.decode(Some(&frame), pcm, false);
        feeds.push(st.slot, &pcm[..k * codec::CHANNELS]);
        st.last_seq = Some(h.seq);
        if frame.as_slice() != rtp::SILENCE {
            st.last_voice = now;
            if !st.speaking {
                st.speaking = true;
                emit(VoiceEvent::Speaking { user: Snowflake(user), speaking: true });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fidelity::HostInfo;
    use crate::profile::IdentityProfile;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;

    /// A voice server that walks the handshake and records what the client sent.
    async fn mock_server(ws_port_tx: tokio::sync::oneshot::Sender<u16>) -> Vec<Value> {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_port = udp.local_addr().unwrap().port();
        thread::spawn(move || {
            let mut b = [0u8; 74];
            if let Ok((_, from)) = udp.recv_from(&mut b) {
                let mut r = [0u8; 74];
                r[1] = 2;
                r[3] = 70;
                r[8..17].copy_from_slice(b"127.0.0.1");
                r[72..74].copy_from_slice(&from.port().to_be_bytes());
                let _ = udp.send_to(&r, from);
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        ws_port_tx.send(listener.local_addr().unwrap().port()).unwrap();
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let mut got = Vec::new();
        let recv = |m: WsMessage| serde_json::from_str::<Value>(m.to_text().unwrap()).unwrap();
        got.push(recv(ws.next().await.unwrap().unwrap())); // identify
        ws.send(WsMessage::Text(json!({"op": 8, "d": {"v": 8, "heartbeat_interval": 13750}}).to_string().into())).await.unwrap();
        ws.send(WsMessage::Text(json!({"op": 2, "seq": 1, "d": {"ssrc": 77, "ip": "127.0.0.1", "port": udp_port,
            "modes": ["aead_xchacha20_poly1305_rtpsize", "aead_aes256_gcm_rtpsize"], "experiments": ["fixed_keyframe_interval"]}}).to_string().into())).await.unwrap();
        got.push(recv(ws.next().await.unwrap().unwrap())); // op 16
        got.push(recv(ws.next().await.unwrap().unwrap())); // select protocol
        ws.send(WsMessage::Text(json!({"op": 4, "seq": 2, "d": {"mode": "aead_aes256_gcm_rtpsize", "secret_key": vec![9u8; 32],
            "audio_codec": "opus", "dave_protocol_version": 0}}).to_string().into())).await.unwrap();
        // Hold the socket open until the client shuts down.
        while let Some(Ok(m)) = ws.next().await {
            if m.is_close() {
                break;
            }
        }
        got
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handshake_matches_the_official_sequence() {
        let (port_tx, port_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(mock_server(port_tx));
        let port = port_rx.await.unwrap();
        let host = HostInfo { os: crate::fidelity::HostOs::MacOs, os_release: "27.2.0".into(), arch: "arm64".into(), languages: vec!["en-AU".into()], timezone: "UTC".into() };
        let identity = Arc::new(ClientIdentity::new(IdentityProfile::bundled(), host));
        let transport = Arc::new(Transport::new(None).unwrap());
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let emit: Emit = Arc::new(move |e| {
            let _ = ev_tx.send(e);
        });
        let p = Params {
            guild_id: Some(Snowflake(1)),
            channel_id: Snowflake(2),
            user_id: Snowflake(3),
            session_id: "sess".into(),
            token: "tok".into(),
            endpoint: format!("ws://127.0.0.1:{port}"),
            bitrate: 64_000,
            muted: false,
            deaf: false,
            role: Role::Call,
            server_id: None,
            video: None,
        };
        let handle = spawn(p, transport, identity, emit);
        let connected = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(e) = ev_rx.recv().await {
                if e == VoiceEvent::Status(Status::Connected) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(connected, "reached Connected");
        handle.shutdown();
        let got = tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
        assert_eq!(got[0]["op"], 0);
        assert_eq!(got[0]["d"]["server_id"], "1");
        assert_eq!(got[0]["d"]["max_dave_protocol_version"], 1);
        assert_eq!(got[1], json!({"op": 16, "d": {}}), "op 16 straight after READY on v >= 6");
        let sp = &got[2]["d"];
        assert_eq!(got[2]["op"], 1);
        assert_eq!(sp["mode"], "aead_aes256_gcm_rtpsize", "our preference, not the server's order");
        assert_eq!(sp["data"]["address"], "127.0.0.1");
        assert_eq!(sp["experiments"], json!(["fixed_keyframe_interval"]));
        assert!(sp["rtc_connection_id"].as_str().is_some_and(|s| s.len() == 36));
    }
}
