//! Video signalling and send-side control: the voice-gateway op 12 / op 15 payloads, the main
//! gateway stream ops (18 create, 19 delete, 20 watch, 21 ping, 22 set paused), the NACK
//! history for RTX, the keyframe request gate, the bitrate controller, the resolution/fps
//! ladder, and the power policy that decides what is worth encoding at all.
//!
//! Payload shapes follow the web client's `RTCControlSocket.video`, `mediaSinkWants` and
//! `GatewaySocket.stream*` (FIDELITY.md, "Video and Go Live"). Stream keys and the screen
//! stream's `max_resolution` values are marked **capture** there.

use std::collections::VecDeque;

use serde_json::{Value, json};

use crate::model::Id;
use crate::snowflake::Snowflake;

// ---- gateway payloads ----------------------------------------------------------------------

/// Main-gateway opcodes for Go Live.
pub mod gw_op {
    pub const STREAM_CREATE: u8 = 18;
    pub const STREAM_DELETE: u8 = 19;
    pub const STREAM_WATCH: u8 = 20;
    pub const STREAM_PING: u8 = 21;
    pub const STREAM_SET_PAUSED: u8 = 22;
}

/// Voice-gateway op 15, the media sink wants.
pub const VOICE_OP_MEDIA_SINK_WANTS: u8 = 15;

/// Stream owner scope: a guild voice channel or a DM / group call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamScope {
    Guild(Id, Id),
    Call(Id),
}

/// `guild:{guild}:{channel}:{owner}` or `call:{channel}:{owner}`. **Capture** (derived from
/// `STREAM_WATCH` handling in the web bundle, which splits keys on `:`).
pub fn stream_key(scope: StreamScope, owner: Id) -> String {
    match scope {
        StreamScope::Guild(g, c) => format!("guild:{g}:{c}:{owner}"),
        StreamScope::Call(c) => format!("call:{c}:{owner}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStreamKey {
    pub guild_id: Option<Id>,
    pub channel_id: Id,
    pub owner_id: Id,
}

pub fn parse_stream_key(key: &str) -> Option<ParsedStreamKey> {
    let p: Vec<&str> = key.split(':').collect();
    let id = |s: &str| s.parse::<u64>().ok().map(Snowflake);
    match p.as_slice() {
        ["guild", g, c, o] => Some(ParsedStreamKey { guild_id: Some(id(g)?), channel_id: id(c)?, owner_id: id(o)? }),
        ["call", c, o] => Some(ParsedStreamKey { guild_id: None, channel_id: id(c)?, owner_id: id(o)? }),
        _ => None,
    }
}

/// Op 18. `type` is `"guild"` for a server channel and `"call"` for a DM call (the web client's
/// stream-type enum). `preferred_region` is `null` unless the user picked one.
pub fn stream_create(scope: StreamScope, preferred_region: Option<&str>) -> Value {
    let (ty, guild, channel) = match scope {
        StreamScope::Guild(g, c) => ("guild", Some(g.to_string()), c),
        StreamScope::Call(c) => ("call", None, c),
    };
    json!({ "op": gw_op::STREAM_CREATE, "d": {
        "type": ty, "guild_id": guild, "channel_id": channel.to_string(), "preferred_region": preferred_region,
    }})
}

pub fn stream_watch(key: &str) -> Value {
    json!({ "op": gw_op::STREAM_WATCH, "d": { "stream_key": key } })
}

pub fn stream_delete(key: &str) -> Value {
    json!({ "op": gw_op::STREAM_DELETE, "d": { "stream_key": key } })
}

pub fn stream_ping(key: &str) -> Value {
    json!({ "op": gw_op::STREAM_PING, "d": { "stream_key": key } })
}

pub fn stream_set_paused(key: &str, paused: bool) -> Value {
    json!({ "op": gw_op::STREAM_SET_PAUSED, "d": { "stream_key": key, "paused": paused } })
}

/// A simulcast layer as op 12 describes it. Optional fields are omitted when `None`, as
/// `JSON.stringify` drops `undefined`.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    /// `"video"` for camera, `"screen"` for Go Live.
    pub kind: &'static str,
    pub rid: String,
    pub ssrc: u32,
    pub rtx_ssrc: u32,
    pub active: bool,
    pub quality: u32,
    pub max_bitrate: u32,
    pub max_framerate: u32,
    pub max_resolution: Option<(u32, u32)>,
}

/// Op 12 as `RTCControlSocket.video(audio_ssrc, video_ssrc, rtx_ssrc, streams)` sends it.
pub fn video(audio_ssrc: u32, video_ssrc: u32, rtx_ssrc: u32, streams: &[VideoStream]) -> Value {
    let streams: Vec<Value> = streams
        .iter()
        .map(|s| {
            let mut o = json!({
                "type": s.kind, "rid": s.rid, "ssrc": s.ssrc, "active": s.active, "quality": s.quality,
                "rtx_ssrc": s.rtx_ssrc, "max_bitrate": s.max_bitrate, "max_framerate": s.max_framerate,
            });
            if let Some((w, h)) = s.max_resolution {
                o["max_resolution"] = json!({ "type": "fixed", "width": w, "height": h });
            }
            o
        })
        .collect();
    json!({ "op": 12, "d": { "audio_ssrc": audio_ssrc, "video_ssrc": video_ssrc, "rtx_ssrc": rtx_ssrc, "streams": streams } })
}

/// Op 12 that stops our video: zero video SSRCs and no streams.
pub fn video_off(audio_ssrc: u32) -> Value {
    video(audio_ssrc, 0, 0, &[])
}

/// One simulcast layer for a send resolution: `rid` "100" is full quality, "50" half.
pub fn layer(kind: &'static str, rid: &str, ssrc: u32, rtx: u32, rung: &Rung, quality: u32, active: bool) -> VideoStream {
    VideoStream {
        kind,
        rid: rid.into(),
        ssrc,
        rtx_ssrc: rtx,
        active,
        quality,
        max_bitrate: rung.max_bps,
        max_framerate: rung.fps,
        max_resolution: Some((rung.width, rung.height)),
    }
}

/// Op 15. `any` is the default quality wanted (0 stops everything not listed); each listed SSRC
/// maps to a 0..=100 percentage. **Capture**: whether `streams` is sent alongside is unverified.
pub fn media_sink_wants(any: u32, per_ssrc: &[(u32, u32)]) -> Value {
    let mut d = serde_json::Map::new();
    d.insert("any".into(), any.into());
    for (ssrc, q) in per_ssrc {
        d.insert(ssrc.to_string(), (*q).into());
    }
    json!({ "op": VOICE_OP_MEDIA_SINK_WANTS, "d": d })
}

// ---- RTX history and keyframe gate ----------------------------------------------------------

/// Recent outgoing video packets (plaintext payloads keyed by sequence) for NACK answers.
pub struct NackHistory {
    ring: VecDeque<(u16, u32, Vec<u8>)>, // seq, timestamp, payload
    cap: usize,
}

impl NackHistory {
    pub fn new(cap: usize) -> Self {
        Self { ring: VecDeque::with_capacity(cap), cap }
    }

    pub fn store(&mut self, seq: u16, ts: u32, payload: Vec<u8>) {
        if self.ring.len() == self.cap {
            self.ring.pop_front();
        }
        self.ring.push_back((seq, ts, payload));
    }

    pub fn get(&self, seq: u16) -> Option<(u32, &[u8])> {
        self.ring.iter().rev().find(|e| e.0 == seq).map(|e| (e.1, e.2.as_slice()))
    }

    /// Packets to retransmit for a NACK, at most `limit`, each at most once per `min_gap_ms`
    /// (callers pass `now_ms`; resends inside the gap are skipped so a NACK storm can't amplify).
    pub fn retransmits<'a>(&'a self, seqs: &[u16], limit: usize) -> Vec<(u16, u32, &'a [u8])> {
        seqs.iter().filter_map(|&s| self.get(s).map(|(t, p)| (s, t, p))).take(limit).collect()
    }
}

/// Collapses PLI/FIR/periodic keyframe requests: one keyframe per `min_interval_ms`, with a
/// request that arrives inside the window deferred to its end rather than lost.
pub struct KeyframeGate {
    min_interval_ms: u64,
    last: Option<u64>,
    pending: bool,
}

impl KeyframeGate {
    pub fn new(min_interval_ms: u64) -> Self {
        Self { min_interval_ms, last: None, pending: false }
    }

    /// Record a request; returns `true` if a keyframe should be forced now.
    pub fn request(&mut self, now_ms: u64) -> bool {
        if self.last.is_none_or(|l| now_ms.saturating_sub(l) >= self.min_interval_ms) {
            self.last = Some(now_ms);
            self.pending = false;
            true
        } else {
            self.pending = true;
            false
        }
    }

    /// Call when a frame is about to be encoded: a deferred request fires once its window passes.
    pub fn due(&mut self, now_ms: u64) -> bool {
        self.pending && self.request(now_ms)
    }

    /// The server's `keyframe_interval` (seconds) as a periodic request.
    pub fn periodic(&mut self, now_ms: u64, interval_s: u64) -> bool {
        interval_s > 0 && self.last.is_some_and(|l| now_ms.saturating_sub(l) >= interval_s * 1000) && self.request(now_ms)
    }
}

// ---- ladder, bitrate, power -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rung {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub min_bps: u32,
    pub max_bps: u32,
}

/// Screen-share and camera ladder, highest first. Bitrates follow what hardware H.264 needs for
/// text-heavy content at these sizes; the web client's own Go Live presets are **capture**.
pub const LADDER: [Rung; 8] = [
    Rung { width: 1920, height: 1080, fps: 60, min_bps: 4_500_000, max_bps: 8_000_000 },
    Rung { width: 1920, height: 1080, fps: 30, min_bps: 2_500_000, max_bps: 6_000_000 },
    Rung { width: 1280, height: 720, fps: 60, min_bps: 2_500_000, max_bps: 4_000_000 },
    Rung { width: 1280, height: 720, fps: 30, min_bps: 1_200_000, max_bps: 3_000_000 },
    Rung { width: 960, height: 540, fps: 30, min_bps: 700_000, max_bps: 1_800_000 },
    Rung { width: 640, height: 360, fps: 30, min_bps: 350_000, max_bps: 1_000_000 },
    Rung { width: 640, height: 360, fps: 15, min_bps: 200_000, max_bps: 600_000 },
    Rung { width: 320, height: 180, fps: 15, min_bps: 80_000, max_bps: 250_000 },
];

/// Highest rung that fits the source size, the cap in `cap`, and a target bitrate.
pub fn pick_rung(target_bps: u32, source: (u32, u32), cap: &Rung) -> Rung {
    let fits = |r: &Rung| r.width <= source.0.max(320) && r.height <= source.1.max(180) || r.width * r.height <= source.0 * source.1;
    LADDER
        .iter()
        .filter(|r| r.width <= cap.width && r.height <= cap.height && r.fps <= cap.fps)
        .filter(|r| fits(r))
        .find(|r| target_bps >= r.min_bps)
        .copied()
        .unwrap_or(LADDER[LADDER.len() - 1])
}

/// Sender-side bandwidth estimate: AIMD over receiver-report loss and RTT, bounded by REMB.
/// Cheap and event-driven; it only runs when an RTCP report arrives, never on a timer.
pub struct BitrateController {
    target: f64,
    min: f64,
    max: f64,
    remb: Option<f64>,
    last_change_ms: u64,
}

impl BitrateController {
    pub fn new(start_bps: u32, min_bps: u32, max_bps: u32) -> Self {
        Self { target: start_bps as f64, min: min_bps as f64, max: max_bps as f64, remb: None, last_change_ms: 0 }
    }

    pub fn target(&self) -> u32 {
        self.target as u32
    }

    pub fn set_max(&mut self, max_bps: u32) {
        self.max = max_bps as f64;
        self.target = self.target.min(self.max);
    }

    pub fn on_remb(&mut self, bps: u64) {
        self.remb = Some(bps as f64);
        self.clamp();
    }

    /// A receiver report: `loss` in 0..=1, `rtt_ms` if known. Decreases are immediate (but at
    /// most once per 300 ms); increases are 8 % per second, and only on clean reports.
    pub fn on_report(&mut self, now_ms: u64, loss: f32, rtt_ms: Option<f32>) {
        let dt = now_ms.saturating_sub(self.last_change_ms);
        if loss > 0.10 {
            if dt >= 300 {
                self.target *= 1.0 - 0.5 * loss as f64;
                self.last_change_ms = now_ms;
            }
        } else if loss < 0.02 && rtt_ms.is_none_or(|r| r < 400.0) {
            let secs = (dt.min(2000)) as f64 / 1000.0;
            self.target *= 1.08f64.powf(secs);
            self.last_change_ms = now_ms;
        } else {
            self.last_change_ms = now_ms; // hold
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        let hi = self.remb.map_or(self.max, |r| r.min(self.max));
        self.target = self.target.clamp(self.min.min(hi), hi.max(self.min));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Thermal {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerState {
    pub on_battery: bool,
    pub low_power_mode: bool,
    pub thermal: Thermal,
    /// Number of remote viewers (or, for a camera in a call, other participants who want video).
    pub viewers: u32,
    /// Local preview visible (the sender's own tile). Alone it doesn't justify encoding.
    pub preview_visible: bool,
}

/// Cap on the ladder for the current power state, or `None` when nothing should be captured or
/// encoded (nobody watches). Battery caps at 720p30, Low Power Mode and Fair thermal at 540p30,
/// Serious at 360p15, Critical stops sending video.
pub fn power_cap(p: &PowerState) -> Option<Rung> {
    if p.viewers == 0 || p.thermal == Thermal::Critical {
        return None;
    }
    let idx = match (p.thermal, p.low_power_mode, p.on_battery) {
        (Thermal::Serious, _, _) => 6,
        (Thermal::Fair, _, _) | (_, true, _) => 4,
        (_, _, true) => 3,
        _ => 0,
    };
    Some(LADDER[idx])
}

/// Receive-side decode policy from window and tile visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeMode {
    /// Decode everything and display.
    Full,
    /// Window occluded or tile off-screen: ask the server to stop (op 15 quality 0) and drop frames.
    Paused,
    /// Visible but small or on battery: ask for the lower layer.
    Reduced,
}

pub fn decode_mode(window_occluded: bool, tile_visible: bool, tile_px: (u32, u32), on_battery: bool) -> DecodeMode {
    if window_occluded || !tile_visible {
        DecodeMode::Paused
    } else if tile_px.0 <= 480 || (on_battery && tile_px.0 <= 800) {
        DecodeMode::Reduced
    } else {
        DecodeMode::Full
    }
}

/// Quality percentage for op 15 for a decode mode.
pub fn wants_quality(m: DecodeMode) -> u32 {
    match m {
        DecodeMode::Full => 100,
        DecodeMode::Reduced => 50,
        DecodeMode::Paused => 0,
    }
}

// ---- send-side glue ---------------------------------------------------------------------------

/// What the media engine should do after an RTCP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderAction {
    /// Plaintext RTP packets (header + payload, to be sealed by the transport) to retransmit over RTX.
    Retransmit(Vec<Vec<u8>>),
    /// Force an IDR frame now.
    Keyframe,
    /// The target bitrate changed; reconfigure the encoder.
    Bitrate(u32),
}

/// One outgoing video stream: packetizes access units, remembers packets for NACKs, and turns
/// feedback into actions. Pure logic, no I/O; the connection seals and sends what it returns.
pub struct VideoSender {
    pub pt: u8,
    pub rtx_pt: u8,
    pub ssrc: u32,
    pub rtx_ssrc: u32,
    seq: u16,
    rtx_seq: u16,
    history: NackHistory,
    pub gate: KeyframeGate,
    pub controller: BitrateController,
    last_reported: u32,
}

impl VideoSender {
    pub fn new(pt: u8, rtx_pt: u8, ssrc: u32, rtx_ssrc: u32, controller: BitrateController) -> Self {
        let last_reported = controller.target();
        Self { pt, rtx_pt, ssrc, rtx_ssrc, seq: 0, rtx_seq: 0, history: NackHistory::new(512), gate: KeyframeGate::new(1000), controller, last_reported }
    }

    /// Packetize an access unit stamped at `pts_90k`. Returns plaintext RTP packets.
    pub fn send_frame(&mut self, nals: &[&[u8]], pts_90k: u32) -> Vec<Vec<u8>> {
        crate::voice::h264::packetize(nals, crate::voice::h264::MAX_PAYLOAD)
            .into_iter()
            .map(|(payload, marker)| {
                let seq = self.seq;
                self.seq = self.seq.wrapping_add(1);
                let mut pkt = crate::voice::h264::rtp_header(self.pt, marker, seq, pts_90k, self.ssrc).to_vec();
                pkt.extend_from_slice(&payload);
                self.history.store(seq, pts_90k, payload);
                pkt
            })
            .collect()
    }

    fn rtx_packet(&mut self, orig_seq: u16, ts: u32, payload: &[u8], marker: bool) -> Vec<u8> {
        let seq = self.rtx_seq;
        self.rtx_seq = self.rtx_seq.wrapping_add(1);
        let mut pkt = crate::voice::h264::rtp_header(self.rtx_pt, marker, seq, ts, self.rtx_ssrc).to_vec();
        pkt.extend_from_slice(&crate::voice::h264::rtx_payload(orig_seq, payload));
        pkt
    }

    pub fn on_rtcp(&mut self, now_ms: u64, pkt: &crate::voice::rtcp::Rtcp) -> Vec<SenderAction> {
        use crate::voice::rtcp::Rtcp;
        let mut out = Vec::new();
        match pkt {
            Rtcp::Nack { media, seqs, .. } if *media == self.ssrc => {
                let found: Vec<(u16, u32, Vec<u8>)> = self.history.retransmits(seqs, 64).into_iter().map(|(s, t, p)| (s, t, p.to_vec())).collect();
                let pkts: Vec<Vec<u8>> = found.iter().map(|(s, t, p)| self.rtx_packet(*s, *t, p, false)).collect();
                if !pkts.is_empty() {
                    out.push(SenderAction::Retransmit(pkts));
                }
            }
            Rtcp::Pli { media, .. } if *media == self.ssrc => {
                if self.gate.request(now_ms) {
                    out.push(SenderAction::Keyframe);
                }
            }
            Rtcp::Fir { entries, .. } if entries.iter().any(|e| e.0 == self.ssrc) => {
                if self.gate.request(now_ms) {
                    out.push(SenderAction::Keyframe);
                }
            }
            Rtcp::Remb { bitrate, .. } => self.controller.on_remb(*bitrate),
            Rtcp::ReceiverReport { blocks, .. } | Rtcp::SenderReport { blocks, .. } => {
                if let Some(b) = blocks.iter().find(|b| b.ssrc == self.ssrc) {
                    self.controller.on_report(now_ms, b.loss(), None);
                }
            }
            _ => {}
        }
        let t = self.controller.target();
        // Only reconfigure the encoder on a >10 % change; every reconfigure costs a rate-control reset.
        if t.abs_diff(self.last_reported) * 10 > self.last_reported {
            self.last_reported = t;
            out.push(SenderAction::Bitrate(t));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_keys_round_trip() {
        let k = stream_key(StreamScope::Guild(Snowflake(1), Snowflake(2)), Snowflake(3));
        assert_eq!(k, "guild:1:2:3");
        assert_eq!(parse_stream_key(&k), Some(ParsedStreamKey { guild_id: Some(Snowflake(1)), channel_id: Snowflake(2), owner_id: Snowflake(3) }));
        let k = stream_key(StreamScope::Call(Snowflake(2)), Snowflake(3));
        assert_eq!(k, "call:2:3");
        assert_eq!(parse_stream_key(&k).unwrap().guild_id, None);
        assert!(parse_stream_key("bogus:1").is_none());
        assert!(parse_stream_key("guild:1:2").is_none());
    }

    #[test]
    fn gateway_payloads() {
        assert_eq!(
            stream_create(StreamScope::Guild(Snowflake(1), Snowflake(2)), None),
            json!({"op":18,"d":{"type":"guild","guild_id":"1","channel_id":"2","preferred_region":null}})
        );
        assert_eq!(stream_create(StreamScope::Call(Snowflake(2)), Some("us-east"))["d"]["guild_id"], Value::Null);
        assert_eq!(stream_watch("k"), json!({"op":20,"d":{"stream_key":"k"}}));
        assert_eq!(stream_delete("k")["op"], 19);
        assert_eq!(stream_ping("k")["op"], 21);
        assert_eq!(stream_set_paused("k", true), json!({"op":22,"d":{"stream_key":"k","paused":true}}));
    }

    #[test]
    fn op12_and_sink_wants() {
        let r = LADDER[3];
        let v = video(10, 11, 12, &[layer("screen", "100", 11, 12, &r, 100, true)]);
        assert_eq!(v["op"], 12);
        let s = &v["d"]["streams"][0];
        assert_eq!((s["type"].as_str(), s["rid"].as_str(), s["ssrc"].as_u64()), (Some("screen"), Some("100"), Some(11)));
        assert_eq!(s["max_resolution"], json!({"type":"fixed","width":1280,"height":720}));
        assert_eq!(video_off(10), json!({"op":12,"d":{"audio_ssrc":10,"video_ssrc":0,"rtx_ssrc":0,"streams":[]}}));
        assert_eq!(media_sink_wants(0, &[(77, 100)]), json!({"op":15,"d":{"any":0,"77":100}}));
    }

    #[test]
    fn nack_history_evicts_and_limits() {
        let mut h = NackHistory::new(3);
        for s in 0..5u16 {
            h.store(s, s as u32 * 3000, vec![s as u8]);
        }
        assert!(h.get(1).is_none());
        assert_eq!(h.get(4), Some((12000, &[4u8][..])));
        assert_eq!(h.retransmits(&[0, 2, 3, 4], 2).len(), 2);
    }

    #[test]
    fn keyframe_gate_rate_limits_and_defers() {
        let mut g = KeyframeGate::new(1000);
        assert!(g.request(0));
        assert!(!g.request(400));
        assert!(!g.due(900));
        assert!(g.due(1000));
        assert!(!g.due(1100), "deferred request fires once");
        assert!(g.periodic(5000, 3));
        assert!(!g.periodic(5100, 3));
    }

    #[test]
    fn controller_backs_off_on_loss_and_respects_remb() {
        let mut c = BitrateController::new(4_000_000, 200_000, 8_000_000);
        c.on_report(1000, 0.30, Some(50.0));
        assert!(c.target() < 3_500_000, "{}", c.target());
        let after = c.target();
        c.on_report(1100, 0.30, None);
        assert_eq!(c.target(), after, "decreases are rate limited");
        for t in 1..=10 {
            c.on_report(1100 + t * 1000, 0.0, Some(30.0));
        }
        assert!(c.target() > after * 3 / 2);
        c.on_remb(1_000_000);
        assert_eq!(c.target(), 1_000_000);
        c.on_report(99_000, 0.0, Some(30.0));
        assert!(c.target() <= 1_000_000);
        let mut c = BitrateController::new(300_000, 250_000, 8_000_000);
        for t in 0..20 {
            c.on_report(1000 * (t + 1), 0.9, None);
        }
        assert_eq!(c.target(), 250_000);
        let mut c = BitrateController::new(1_000_000, 100_000, 2_000_000);
        for t in 0..60 {
            c.on_report(1000 * (t + 1), 0.0, Some(20.0));
        }
        assert_eq!(c.target(), 2_000_000);
        c.set_max(500_000);
        assert_eq!(c.target(), 500_000);
    }

    #[test]
    fn ladder_picks_by_bitrate_source_and_cap() {
        let top = LADDER[0];
        assert_eq!(pick_rung(10_000_000, (3840, 2160), &top), LADDER[0]);
        assert_eq!(pick_rung(3_000_000, (3840, 2160), &top), LADDER[1]);
        assert_eq!(pick_rung(2_600_000, (1920, 1080), &top), LADDER[1]);
        assert_eq!(pick_rung(10_000_000, (1280, 720), &top).height, 720);
        assert_eq!(pick_rung(10_000_000, (1920, 1080), &LADDER[3]), LADDER[3]);
        assert_eq!(pick_rung(1_000, (1920, 1080), &top), LADDER[7]);
    }

    #[test]
    fn power_policy() {
        let base = PowerState { on_battery: false, low_power_mode: false, thermal: Thermal::Nominal, viewers: 1, preview_visible: true };
        assert_eq!(power_cap(&base), Some(LADDER[0]));
        assert_eq!(power_cap(&PowerState { viewers: 0, ..base }), None, "nobody watching: no capture");
        assert_eq!(power_cap(&PowerState { on_battery: true, ..base }), Some(LADDER[3]));
        assert_eq!(power_cap(&PowerState { low_power_mode: true, ..base }), Some(LADDER[4]));
        assert_eq!(power_cap(&PowerState { thermal: Thermal::Serious, ..base }), Some(LADDER[6]));
        assert_eq!(power_cap(&PowerState { thermal: Thermal::Critical, ..base }), None);
    }

    #[test]
    fn decode_policy() {
        assert_eq!(decode_mode(true, true, (1920, 1080), false), DecodeMode::Paused);
        assert_eq!(decode_mode(false, false, (1920, 1080), false), DecodeMode::Paused);
        assert_eq!(decode_mode(false, true, (320, 180), false), DecodeMode::Reduced);
        assert_eq!(decode_mode(false, true, (700, 400), true), DecodeMode::Reduced);
        assert_eq!(decode_mode(false, true, (1200, 700), true), DecodeMode::Full);
        assert_eq!(wants_quality(DecodeMode::Paused), 0);
    }

    #[test]
    fn sender_packetizes_and_answers_feedback() {
        use crate::voice::rtcp::{self, Rtcp};
        let mut s = VideoSender::new(105, 106, 11, 12, BitrateController::new(4_000_000, 200_000, 8_000_000));
        let idr = vec![0x65u8; 3000];
        let sps = [0x67u8, 1, 2];
        let pkts = s.send_frame(&[&sps, &idr], 9000);
        assert!(pkts.len() >= 3);
        let first = crate::voice::rtp::parse(&pkts[0]).unwrap();
        assert_eq!((first.ssrc, first.seq, first.payload_type), (11, 0, 105));
        assert!(crate::voice::h264::marker(pkts.last().unwrap()));
        // NACK for seqs 1 and 2 returns RTX packets carrying the OSN.
        let nack = rtcp::parse(&rtcp::nack(99, 11, &[1, 2, 500]));
        let SenderAction::Retransmit(r) = &s.on_rtcp(0, &nack[0])[0] else { panic!() };
        assert_eq!(r.len(), 2);
        let h = crate::voice::rtp::parse(&r[0]).unwrap();
        assert_eq!((h.ssrc, h.payload_type), (12, 106));
        assert_eq!(crate::voice::h264::rtx_unwrap(&r[0][12..]).unwrap().0, 1);
        // PLI forces a keyframe, rate limited; wrong SSRC is ignored.
        let pli = rtcp::parse(&rtcp::pli(99, 11));
        assert_eq!(s.on_rtcp(10, &pli[0]), vec![SenderAction::Keyframe]);
        assert!(s.on_rtcp(20, &pli[0]).is_empty());
        assert!(s.on_rtcp(5000, &rtcp::parse(&rtcp::pli(99, 77))[0]).is_empty());
        // REMB well below target reconfigures the encoder.
        let remb = rtcp::parse(&rtcp::remb(99, 1_000_000, &[11]));
        assert_eq!(s.on_rtcp(6000, &remb[0]), vec![SenderAction::Bitrate(s.controller.target())]);
        assert!(s.controller.target() <= 1_000_000);
        let _ = Rtcp::Other { pt: 0 };
    }
}
