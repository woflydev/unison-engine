//! Video on a voice connection: the shared transport sealer, the send path (DAVE, packetize, seal),
//! and the receive path (RTCP, depacketize, decrypt). One of these lives in each connection's
//! `Shared`: the call connection carries camera video; every Go Live stream, ours or one we
//! watch, is a separate voice connection of its own with the same machinery and no audio.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;

use crate::voice::crypto::Transport as Aead;
use crate::voice::h264::{self, Depacketizer};
use crate::voice::rtcp::{self, Rtcp};
use crate::voice::recv;
use crate::voice::rtp;
use crate::voice::video::{self, BitrateController, LADDER, SenderAction, VideoSender};

/// What a voice connection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The channel call: audio, plus camera video.
    Call,
    /// Our own Go Live stream: video only, send.
    StreamPublisher,
    /// Someone else's Go Live stream: video only, receive.
    StreamViewer,
}

impl Role {
    pub fn has_audio(self) -> bool {
        self == Role::Call
    }
}

/// H.264 payload types we advertise in op 1 (`protocol::codecs`).
pub const H264_PT: u8 = super::protocol::VIDEO_PT_BASE + 2 * super::protocol::H264_INDEX;
pub const H264_RTX_PT: u8 = H264_PT + 1;

pub struct RemoteFrame {
    pub user: u64,
    pub annexb: Vec<u8>,
    pub keyframe: bool,
    pub pts: u32,
    /// Clockwise degrees to rotate for display (video-orientation extension).
    pub rotation: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoControl {
    Keyframe,
    Bitrate(u32),
}

/// Where decoded-side data goes: remote access units out, encoder control (PLI, bitrate) out.
#[derive(Clone)]
pub struct VideoIo {
    pub frames: Arc<dyn Fn(RemoteFrame) + Send + Sync>,
    pub control: Arc<dyn Fn(VideoControl) + Send + Sync>,
}

impl std::fmt::Debug for VideoIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VideoIo")
    }
}

/// What the app asked to send.
#[derive(Debug, Clone, Copy)]
pub struct VideoWant {
    /// `"video"` (camera) or `"screen"` (Go Live).
    pub kind: &'static str,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Ssrcs {
    pub audio: u32,
    pub video: u32,
    pub rtx: u32,
}

/// The connection's media socket and its one sealer. Audio, video and RTCP all draw nonces from
/// the same counter: two sealers under one key would reuse nonces.
pub struct Media {
    pub sock: UdpSocket,
    pub seal: Arc<Mutex<Aead>>,
    /// The RTCP CNAME of this connection (random, as libwebrtc generates it).
    pub cname: String,
}

impl Media {
    pub fn send(&self, header: &[u8], payload: &[u8]) {
        let pkt = self.seal.lock().seal(header, payload);
        if let Some(p) = pkt {
            let _ = self.sock.send(&p);
        }
    }

    /// A plaintext RTP packet whose header may carry extensions: the authenticated part stays in the
    /// clear, the rest is sealed. A packet that does not parse is dropped, never sent.
    pub fn send_packet(&self, packet: &[u8]) {
        if let Some((aad, rest)) = rtp::seal_split(packet) {
            self.send(aad, rest);
        }
    }

    /// An RTCP packet: the 8-byte header stays in the clear, the rest is sealed.
    pub fn send_rtcp(&self, plain: &[u8]) {
        if plain.len() > 8 {
            self.send(&plain[..8], &plain[8..]);
        }
    }
}

struct Tx {
    sender: VideoSender,
    kind: &'static str,
}

#[derive(Default)]
pub struct VideoState {
    pub ssrcs: Mutex<Ssrcs>,
    media: Mutex<Option<Arc<Media>>>,
    tx: Mutex<Option<Tx>>,
    want: Mutex<Option<VideoWant>>,
    /// Video and RTX SSRC → user, from op 12.
    pub users: Mutex<HashMap<u32, u64>>,
    /// RTX SSRC → the video SSRC it repairs, from op 12.
    pub rtx_of: Mutex<HashMap<u32, u32>>,
    out: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>>,
    io: Mutex<Option<VideoIo>>,
    /// Cadence of the periodic sender report for our video stream (1 s).
    sr_clock: Mutex<rtcp::Cadence>,
    /// What happened to the frames we were handed, summarised every 5 s (see [`TxLog`]).
    txlog: Mutex<TxLog>,
}

/// Counters behind the camera/screen diagnostics: why a frame did not become a packet, and how
/// much congestion feedback arrived, printed as one line every 5 seconds instead of one per packet.
#[derive(Default)]
pub struct TxLog {
    frames: u32,
    sent_frames: u32,
    no_sender: u32,
    dave_refused: u32,
    empty: u32,
    tcc: u32,
    next_ms: u64,
    first_logged: bool,
}

impl TxLog {
    const EVERY_MS: u64 = 5000;
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl VideoState {
    pub fn attach(&self, media: Arc<Media>, out: tokio::sync::mpsc::UnboundedSender<Value>, io: Option<VideoIo>) {
        *self.media.lock() = Some(media);
        *self.out.lock() = Some(out);
        *self.io.lock() = io;
        self.apply();
    }

    pub fn out_sender(&self) -> Option<tokio::sync::mpsc::UnboundedSender<Value>> {
        self.out.lock().clone()
    }

    pub fn media(&self) -> Option<Arc<Media>> {
        self.media.lock().clone()
    }

    pub fn io(&self) -> Option<VideoIo> {
        self.io.lock().clone()
    }

    /// Begin (or change) sending. Waits for the media path if it isn't up yet.
    pub fn start(&self, want: VideoWant) {
        let ss = *self.ssrcs.lock();
        tracing::debug!(target: "unison_core::voice", kind = want.kind, width = want.width, height = want.height, fps = want.fps, media_ready = self.media.lock().is_some(), video_ssrc = ss.video, "video start requested");
        *self.want.lock() = Some(want);
        self.apply();
    }

    pub fn stop(&self) {
        *self.want.lock() = None;
        let had = self.tx.lock().take().is_some();
        if had {
            let audio = self.ssrcs.lock().audio;
            if let Some(out) = self.out.lock().as_ref() {
                let _ = out.send(video::video_off(audio));
            }
        }
    }

    fn apply(&self) {
        let (Some(want), true) = (*self.want.lock(), self.media.lock().is_some()) else {
            tracing::debug!(target: "unison_core::voice", "video sender waits: no media path yet or nothing requested");
            return;
        };
        let ss = *self.ssrcs.lock();
        if ss.video == 0 {
            tracing::debug!(target: "unison_core::voice", "video sender waits: the server gave no video ssrc");
            return;
        }
        tracing::debug!(target: "unison_core::voice", kind = want.kind, video_ssrc = ss.video, rtx_ssrc = ss.rtx, "video sender created, op 12 sent");
        let cap = LADDER.iter().find(|r| r.width <= want.width.max(320) && r.height <= want.height.max(180) && r.fps <= want.fps.max(15)).copied().unwrap_or(LADDER[LADDER.len() - 1]);
        let ctl = BitrateController::new((cap.min_bps + cap.max_bps) / 2, cap.min_bps / 2, cap.max_bps);
        let sender = VideoSender::new(H264_PT, H264_RTX_PT, ss.video, ss.rtx, ctl);
        let layer = video::layer(want.kind, "100", ss.video, ss.rtx, &cap, 100, true);
        *self.tx.lock() = Some(Tx { sender, kind: want.kind });
        if let Some(out) = self.out.lock().as_ref() {
            let _ = out.send(video::video(ss.audio, ss.video, ss.rtx, &[layer]));
        }
        // A viewer can only start on a keyframe: the first one goes out as soon as the stream is announced.
        if let Some(io) = self.io() {
            (io.control)(VideoControl::Keyframe);
        }
    }

    pub fn sending(&self) -> bool {
        self.tx.lock().is_some()
    }

    /// Packetize, seal and send one access unit (Annex B, already DAVE-encrypted when the call is).
    pub fn push(&self, annexb: &[u8], pts_90k: u32) {
        let Some(media) = self.media() else {
            self.txlog.lock().no_sender += 1;
            return;
        };
        let packets = {
            let mut g = self.tx.lock();
            let Some(tx) = g.as_mut() else {
                self.txlog.lock().no_sender += 1;
                return;
            };
            let nals = h264::split_annexb(annexb);
            if nals.is_empty() {
                self.txlog.lock().empty += 1;
                return;
            }
            tx.sender.send_frame(&nals, pts_90k, now_ms())
        };
        {
            let mut l = self.txlog.lock();
            l.sent_frames += 1;
            if !l.first_logged {
                l.first_logged = true;
                tracing::debug!(target: "unison_core::voice", packets = packets.len(), bytes = annexb.len(), "first video frame packetized and sent");
            }
        }
        for p in packets {
            media.send_packet(&p);
        }
        self.maybe_sender_report(&media);
        self.log_summary();
    }

    /// A frame arrived while no sender exists (the camera is on in the app but the engine is not sending).
    pub fn note_idle(&self) {
        self.txlog.lock().no_sender += 1;
        self.log_summary();
    }

    /// A frame was handed to us but never reached [`VideoState::push`] because DAVE refused it.
    pub fn note_dave_refused(&self) {
        let mut l = self.txlog.lock();
        l.frames += 1;
        l.dave_refused += 1;
        drop(l);
        self.log_summary();
    }

    /// One line every 5 s while video is active: frames in, sent, and why the rest were not.
    fn log_summary(&self) {
        let now = now_ms();
        let mut l = self.txlog.lock();
        if l.next_ms == 0 {
            l.next_ms = now + TxLog::EVERY_MS;
            return;
        }
        if now < l.next_ms {
            return;
        }
        l.next_ms = now + TxLog::EVERY_MS;
        let stats = self.tx.lock().as_ref().map(|t| t.sender.stats);
        tracing::debug!(
            target: "unison_core::voice",
            frames_sent = l.sent_frames,
            dave_refused = l.dave_refused,
            no_sender = l.no_sender,
            empty = l.empty,
            packets = stats.map_or(0, |s| s.packets),
            octets = stats.map_or(0, |s| s.octets),
            transport_cc_feedback = l.tcc,
            "video tx, last 5 s"
        );
        l.sent_frames = 0;
        l.dave_refused = 0;
        l.no_sender = 0;
        l.empty = 0;
        l.tcc = 0;
    }

    /// The periodic sender report and CNAME for our video stream, once a second while we send, like
    /// libwebrtc. Sealed the way every RTCP packet is (`Media::send_rtcp`).
    fn maybe_sender_report(&self, media: &Media) {
        let now = now_ms();
        if !self.sr_clock.lock().due(now) {
            return;
        }
        let (ssrc, stats) = {
            let g = self.tx.lock();
            let Some(tx) = g.as_ref() else { return };
            (tx.sender.ssrc, tx.sender.stats)
        };
        if stats.packets == 0 {
            return;
        }
        let pkt = rtcp::sender_compound(ssrc, now, stats.rtp_ts_at(now, 90), stats.packets, stats.octets, &media.cname);
        media.send_rtcp(&pkt);
        tracing::debug!(target: "unison_core::voice", packets = stats.packets, octets = stats.octets, "rtcp sender report sent (video)");
    }

    /// Handle a decrypted RTCP packet aimed at what we send.
    pub fn on_rtcp(&self, now_ms: u64, packets: &[Rtcp]) {
        let Some(media) = self.media() else { return };
        for p in packets {
            if let Rtcp::TransportCc(t) = p {
                // Accepted and ignored: our rate control follows REMB and loss reports. Counted so a
                // test call can see the server sends it (it only does once we send extension id 5).
                let _ = t;
                self.txlog.lock().tcc += 1;
            }
        }
        let mut actions = Vec::new();
        if let Some(tx) = self.tx.lock().as_mut() {
            for p in packets {
                actions.extend(tx.sender.on_rtcp(now_ms, p));
            }
        }
        let io = self.io();
        for a in actions {
            match a {
                SenderAction::Retransmit(pkts) => {
                    for p in pkts {
                        media.send_packet(&p);
                    }
                }
                SenderAction::Keyframe => {
                    if let Some(io) = &io {
                        (io.control)(VideoControl::Keyframe);
                    }
                }
                SenderAction::Bitrate(b) => {
                    if let Some(io) = &io {
                        (io.control)(VideoControl::Bitrate(b));
                    }
                }
            }
        }
    }

    pub fn kind(&self) -> Option<&'static str> {
        self.tx.lock().as_ref().map(|t| t.kind)
    }
}

/// Counters for the once-a-second receive summary of one remote video SSRC.
#[derive(Default, Clone, Copy)]
pub struct RxCount {
    pub packets: u32,
    pub recovered: u32,
    pub nacked: u32,
    pub frames: u32,
    pub plis: u32,
    /// Complete frames DAVE would not decrypt.
    pub dave_failed: u32,
    /// Complete frames with no app sink to go to (video hooks missing).
    pub no_sink: u32,
}

/// What the receive loop should do after feeding or polling a remote stream.
#[derive(Default)]
pub struct RxOut {
    /// Finished access units with the RTP timestamp of their last packet.
    pub frames: Vec<(h264::Frame, u32)>,
    /// Packets to ask the sender to repeat (RTCP NACK).
    pub nacks: Vec<u16>,
    /// Ask for a new keyframe now (RTCP PLI).
    pub pli: bool,
}

/// Receive side of one remote video SSRC: reorder, ask for what is missing, depacketize.
pub struct RemoteStream {
    dep: Depacketizer,
    reorder: recv::Reorder,
    pub stats: recv::RecvStats,
    last_pli: Option<std::time::Instant>,
    pub count: RxCount,
    skipped_seen: u32,
    /// Last rotation the sender signalled; libwebrtc sends it on frames, not every packet.
    pub rotation: u16,
}

impl RemoteStream {
    pub fn new() -> Self {
        Self { dep: Depacketizer::new(), reorder: recv::Reorder::default(), stats: recv::RecvStats::default(), last_pli: None, count: RxCount::default(), skipped_seen: 0, rotation: 0 }
    }

    /// Feed one decrypted payload. `via_rtx` marks a retransmission: it fills a hole but is not
    /// counted as received, so the reports we send still show the loss the network caused.
    pub fn push(&mut self, seq: u16, pkt: recv::Packet, via_rtx: bool, now_ms: u64, now: std::time::Instant) -> RxOut {
        if via_rtx {
            self.count.recovered += 1;
        } else {
            self.count.packets += 1;
            self.stats.on_packet(seq, pkt.timestamp, now_ms, 90);
        }
        let released = self.reorder.push(seq, pkt, now_ms);
        self.finish(released, now_ms, now)
    }

    /// Drop P frames until the next keyframe and ask for one on the next poll.
    pub fn want_keyframe(&mut self) {
        self.dep.needs_keyframe = true;
        self.last_pli = None;
    }

    /// Timeouts and retries; call regularly.
    pub fn poll(&mut self, now_ms: u64, now: std::time::Instant) -> RxOut {
        let released = self.reorder.drain(now_ms);
        self.finish(released, now_ms, now)
    }

    fn finish(&mut self, released: Vec<(u16, recv::Packet)>, now_ms: u64, now: std::time::Instant) -> RxOut {
        let mut out = RxOut::default();
        for (seq, p) in released {
            if let Some(f) = self.dep.push(seq, &p.payload, p.marker) {
                self.count.frames += 1;
                out.frames.push((f, p.timestamp));
            }
        }
        out.nacks = self.reorder.due_nacks(now_ms);
        self.count.nacked += out.nacks.len() as u32;
        out.pli = self.dep.needs_keyframe && self.last_pli.is_none_or(|t| now.duration_since(t) > Duration::from_millis(700));
        if out.pli {
            self.last_pli = Some(now);
            self.count.plis += 1;
        }
        out
    }

    /// Packets given up on since the last call.
    pub fn take_skipped(&mut self) -> u32 {
        let n = self.reorder.skipped - self.skipped_seen;
        self.skipped_seen = self.reorder.skipped;
        n
    }
}

/// Decrypt an RTCP packet of the `_rtpsize` modes into plaintext (header + body).
pub fn open_rtcp(open: &Aead, pkt: &[u8]) -> Option<Vec<u8>> {
    let fake = rtp::Incoming { seq: 0, timestamp: 0, ssrc: 0, payload_type: 0, aad_len: 8, ext_body_len: 0, padded: false };
    let body = open.open(pkt, &fake)?;
    let mut plain = pkt[..8].to_vec();
    plain.extend_from_slice(&body);
    Some(plain)
}

pub fn pli(sender: u32, media: u32) -> Vec<u8> {
    rtcp::pli(sender, media)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_round_trips_through_the_sealed_transport() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        a.connect(b.local_addr().unwrap()).unwrap();
        b.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let key = [5u8; 32];
        let seal = Arc::new(Mutex::new(Aead::new("aead_aes256_gcm_rtpsize", &key).unwrap()));
        let open = Aead::new("aead_aes256_gcm_rtpsize", &key).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let st = VideoState::default();
        *st.ssrcs.lock() = Ssrcs { audio: 10, video: 11, rtx: 12 };
        st.start(VideoWant { kind: "screen", width: 1280, height: 720, fps: 30 });
        st.attach(Arc::new(Media { sock: a, seal, cname: "testcname0123456".into() }), tx, None);
        let op12 = rx.try_recv().unwrap();
        assert_eq!(op12["op"], 12);
        assert_eq!(op12["d"]["video_ssrc"], 11);
        // An IDR big enough to fragment, behind SPS and PPS.
        let mut frame = vec![0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5, 0, 0, 0, 1, 0x65];
        frame.extend((0..3000).map(|i| (i % 250) as u8 + 1));
        st.push(&frame, 9000);
        let mut dep = Depacketizer::new();
        let mut buf = [0u8; 2048];
        let mut got = None;
        while let Ok(n) = b.recv(&mut buf) {
            let h = rtp::parse(&buf[..n]).unwrap();
            assert_eq!((h.ssrc, h.payload_type), (11, H264_PT));
            assert_eq!((h.aad_len, h.ext_body_len), (16, 8), "video carries abs-send-time and the transport-wide sequence");
            let payload = open.open(&buf[..n], &h).unwrap();
            if let Some(f) = dep.push(h.seq, &payload, buf[1] & 0x80 != 0) {
                got = Some(f);
            }
        }
        let f = got.expect("a complete frame");
        assert!(f.keyframe);
        assert_eq!(f.annexb, frame);
    }
}
