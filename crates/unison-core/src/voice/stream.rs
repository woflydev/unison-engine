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
pub const H264_PT: u8 = 105;
pub const H264_RTX_PT: u8 = 106;

pub struct RemoteFrame {
    pub user: u64,
    pub annexb: Vec<u8>,
    pub keyframe: bool,
    pub pts: u32,
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
}

impl Media {
    pub fn send(&self, header: &[u8], payload: &[u8]) {
        let pkt = self.seal.lock().seal(header, payload);
        if let Some(p) = pkt {
            let _ = self.sock.send(&p);
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
    out: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>>,
    io: Mutex<Option<VideoIo>>,
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
        let (Some(want), true) = (*self.want.lock(), self.media.lock().is_some()) else { return };
        let ss = *self.ssrcs.lock();
        if ss.video == 0 {
            return;
        }
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
        let Some(media) = self.media() else { return };
        let packets = {
            let mut g = self.tx.lock();
            let Some(tx) = g.as_mut() else { return };
            let nals = h264::split_annexb(annexb);
            if nals.is_empty() {
                return;
            }
            tx.sender.send_frame(&nals, pts_90k)
        };
        for p in packets {
            media.send(&p[..12], &p[12..]);
        }
    }

    /// Handle a decrypted RTCP packet aimed at what we send.
    pub fn on_rtcp(&self, now_ms: u64, packets: &[Rtcp]) {
        let Some(media) = self.media() else { return };
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
                        media.send(&p[..12], &p[12..]);
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

/// Receive side of one remote video SSRC.
pub struct RemoteStream {
    dep: Depacketizer,
    last_pli: Option<std::time::Instant>,
}

impl RemoteStream {
    pub fn new() -> Self {
        Self { dep: Depacketizer::new(), last_pli: None }
    }

    /// Feed one decrypted RTP payload. Returns a finished frame, and whether to ask for a keyframe now.
    pub fn push(&mut self, seq: u16, payload: &[u8], marker: bool, now: std::time::Instant) -> (Option<h264::Frame>, bool) {
        let frame = self.dep.push(seq, payload, marker);
        let want = self.dep.needs_keyframe && self.last_pli.is_none_or(|t| now.duration_since(t) > Duration::from_millis(700));
        if want {
            self.last_pli = Some(now);
        }
        (frame, want)
    }
}

/// Decrypt an RTCP packet of the `_rtpsize` modes into plaintext (header + body).
pub fn open_rtcp(open: &Aead, pkt: &[u8]) -> Option<Vec<u8>> {
    let fake = rtp::Incoming { seq: 0, timestamp: 0, ssrc: 0, payload_type: 0, aad_len: 8, ext_body_len: 0 };
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
        st.attach(Arc::new(Media { sock: a, seal }), tx, None);
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
