//! Voice gateway (`RTCControlSocket`) payloads, built exactly as the desktop client's
//! JavaScript builds them, and the messages it receives.
//!
//! Text frames are JSON `{op, d}` (server frames also carry `seq`). DAVE's MLS messages are
//! binary: server → client `[seq: u16][op: u8][payload]`, client → server `[op: u8][payload]`.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::Id;

/// `?v=9`; v10 only with the SFU-update experiment, which we don't opt into.
pub const VERSION: u8 = 9;

pub mod op {
    pub const IDENTIFY: u8 = 0;
    pub const SELECT_PROTOCOL: u8 = 1;
    pub const READY: u8 = 2;
    pub const HEARTBEAT: u8 = 3;
    pub const SESSION_DESCRIPTION: u8 = 4;
    pub const SPEAKING: u8 = 5;
    pub const HEARTBEAT_ACK: u8 = 6;
    pub const RESUME: u8 = 7;
    pub const HELLO: u8 = 8;
    pub const RESUMED: u8 = 9;
    pub const CLIENTS_CONNECT: u8 = 11;
    pub const VIDEO: u8 = 12;
    pub const CLIENT_DISCONNECT: u8 = 13;
    pub const SESSION_UPDATE: u8 = 14;
    pub const VOICE_BACKEND_VERSION: u8 = 16;
    pub const DAVE_PREPARE_TRANSITION: u8 = 21;
    pub const DAVE_EXECUTE_TRANSITION: u8 = 22;
    pub const DAVE_TRANSITION_READY: u8 = 23;
    pub const DAVE_PREPARE_EPOCH: u8 = 24;
    pub const DAVE_EXTERNAL_SENDER: u8 = 25;
    pub const DAVE_KEY_PACKAGE: u8 = 26;
    pub const DAVE_PROPOSALS: u8 = 27;
    pub const DAVE_COMMIT_WELCOME: u8 = 28;
    pub const DAVE_ANNOUNCE_COMMIT: u8 = 29;
    pub const DAVE_WELCOME: u8 = 30;
    pub const DAVE_INVALID_COMMIT_WELCOME: u8 = 31;
}

/// Speaking flags (`SpeakingFlags`): microphone voice.
pub const SPEAKING_VOICE: u32 = 1;

/// `wss://{host}/?v=9`, with port 443 (or 80) dropped as `RTCConnection.endpoint` does.
pub fn url(endpoint: &str) -> String {
    let host = endpoint.trim_start_matches("wss://").trim_end_matches('/');
    let host = match host.rsplit_once(':') {
        Some((h, "443" | "80")) => h,
        _ => host,
    };
    format!("wss://{host}/?v={VERSION}")
}

pub struct IdentifyInput<'a> {
    pub server_id: Id,
    pub channel_id: Id,
    pub user_id: Id,
    pub session_id: &'a str,
    pub token: &'a str,
    pub max_dave_protocol_version: u16,
    /// A Go Live connection (publisher or viewer): the official client's stream context describes its
    /// stream as `screen`, the call's as `video` (`getVideoStreamParameters(context)`).
    pub screen: bool,
}

/// Op 0. `streams` is what `getVideoStreamParameters` returns: one `{type, rid:"100", quality:100}`
/// entry, `screen` on a Go Live connection and `video` on the call (web bundle 2026-10-30,
/// `RTCControlSocket.identify`; undefined keys are not serialised).
pub fn identify(i: &IdentifyInput) -> Value {
    json!({ "op": op::IDENTIFY, "d": {
        "server_id": i.server_id.to_string(),
        "channel_id": i.channel_id.to_string(),
        "user_id": i.user_id.to_string(),
        "session_id": i.session_id,
        "token": i.token,
        "max_dave_protocol_version": i.max_dave_protocol_version,
        "video": true,
        "streams": [{ "type": if i.screen { "screen" } else { "video" }, "rid": "100", "quality": 100 }],
    }})
}

/// Op 3. From server version 8 the data is `{t, seq_ack}` with `-1` until a sequenced frame has
/// arrived; before that it is the bare timestamp (`RTCControlSocket.sendHeartbeat`).
pub fn heartbeat(t_ms: u64, seq_ack: Option<i64>, server_version: u8) -> Value {
    if server_version >= 8 {
        json!({ "op": op::HEARTBEAT, "d": { "t": t_ms, "seq_ack": seq_ack.unwrap_or(-1) } })
    } else {
        json!({ "op": op::HEARTBEAT, "d": t_ms })
    }
}

pub fn resume(token: &str, session_id: &str, server_id: Id, channel_id: Id, seq_ack: Option<i64>) -> Value {
    json!({ "op": op::RESUME, "d": {
        "token": token,
        "session_id": session_id,
        "server_id": server_id.to_string(),
        "channel_id": channel_id.to_string(),
        "seq_ack": seq_ack.unwrap_or(-1),
    }})
}

/// The codecs a desktop client advertises (`priority` is `1000 × p`). **Capture** pending:
/// values follow public desktop traces until pinned from a recorded call.
pub fn codecs() -> Value {
    json!([
        { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120 },
        video_codec(0, "AV1", false, false),
        video_codec(1, "H265", false, false),
        video_codec(2, "H264", true, true),
        video_codec(3, "VP8", false, false),
    ])
}

/// Video codec `i` of the list: priority `1000 × (i + 1)`, payload type `101 + 2i`, RTX one above
/// (`RTCConnection.initialize`: `priority: t + 1`, `payloadType: 101 + 2 * t`; `e$` multiplies the
/// priority by 1000).
fn video_codec(i: u8, name: &str, encode: bool, decode: bool) -> Value {
    let pt = VIDEO_PT_BASE + 2 * i;
    json!({ "name": name, "type": "video", "priority": 1000 * (i as u32 + 1), "payload_type": pt, "rtx_payload_type": pt + 1, "encode": encode, "decode": decode })
}

/// First video payload type; the codec at index `i` gets `101 + 2i` and RTX `102 + 2i`.
pub const VIDEO_PT_BASE: u8 = 101;
/// Where H.264 sits in [`codecs`]. The official desktop on a Mac signals AV1 decode
/// (`SIGNAL_AV1_DECODE`, `SIGNAL_AV1_HARDWARE_DECODE`) and has H265, so its list is AV1, H265, H264, VP8
/// (web bundle `setupMediaEngine`, `y`, `L`; `discord_voice.node` reports the codecs). We list all four
/// at the official numbers but say `encode`/`decode` false for what we cannot do, which is the one
/// deliberate difference.
pub const H264_INDEX: u8 = 2;

/// The experiments the client itself switches on (`RTCConnection._chooseExperiments`): not an echo
/// of the server's READY list. A desktop call with no remote-config extras sends only this one,
/// which we honour (the server's `keyframe_interval` drives periodic keyframes, `video.rs`).
pub fn client_experiments() -> Vec<String> {
    vec!["fixed_keyframe_interval".to_string()]
}

/// Op 1, `selectProtocol` for the UDP path: `data` holds the discovered address, and the same
/// fields are spread at the top level followed by `codecs`, `rtc_connection_id`, `experiments`.
pub fn select_protocol(address: &str, port: u16, mode: &str, rtc_connection_id: &str, experiments: &[String]) -> Value {
    json!({ "op": op::SELECT_PROTOCOL, "d": {
        "protocol": "udp",
        "data": { "address": address, "port": port, "mode": mode },
        "address": address,
        "port": port,
        "mode": mode,
        "codecs": codecs(),
        "rtc_connection_id": rtc_connection_id,
        "experiments": experiments,
    }})
}

/// Which speaking changes the desktop client sends as op 5 (`RTCConnection.shouldSendSpeaking`, web
/// bundle 2026-10-30): the first time it speaks (and again if the SSRC changes), and when the
/// priority-speaker bit flips. Starting and stopping to speak again is not sent: the server hears
/// it from the audio itself.
#[derive(Default)]
pub struct SpeakingGate {
    last: Option<(u32, u32)>, // (ssrc, status)
}

impl SpeakingGate {
    pub fn should_send(&mut self, status: u32, ssrc: u32) -> bool {
        const PRIORITY: u32 = 4;
        let (last_ssrc, last_status) = match self.last {
            Some((s, st)) => (Some(s), st),
            None => (None, 0),
        };
        let changed = last_ssrc != Some(ssrc) || (status & PRIORITY) != (last_status & PRIORITY);
        let ok = changed && (last_ssrc.is_some() || status != 0) && (last_ssrc == Some(ssrc) || status != 0);
        if ok {
            self.last = Some((ssrc, status));
        }
        ok
    }
}

/// Op 5. `delay` is always 0 for microphone audio.
pub fn speaking(flags: u32, ssrc: u32) -> Value {
    json!({ "op": op::SPEAKING, "d": { "speaking": flags, "delay": 0, "ssrc": ssrc } })
}

/// Op 16, sent right after READY on v ≥ 6.
pub fn backend_version() -> Value {
    json!({ "op": op::VOICE_BACKEND_VERSION, "d": {} })
}

pub fn transition_ready(transition_id: u16) -> Value {
    json!({ "op": op::DAVE_TRANSITION_READY, "d": { "transition_id": transition_id } })
}

pub fn invalid_commit_welcome(transition_id: u16) -> Value {
    json!({ "op": op::DAVE_INVALID_COMMIT_WELCOME, "d": { "transition_id": transition_id } })
}

/// A client → server binary frame.
pub fn binary(op: u8, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len() + 1);
    b.push(op);
    b.extend_from_slice(payload);
    b
}

// ----- Incoming -----

#[derive(Debug, Deserialize)]
pub struct Frame {
    pub op: u8,
    #[serde(default)]
    pub d: Value,
    #[serde(default)]
    pub seq: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub v: u8,
    pub heartbeat_interval: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ready {
    pub ssrc: u32,
    pub ip: String,
    pub port: u16,
    pub modes: Vec<String>,
    pub experiments: Vec<String>,
    pub streams: Vec<ReadyStream>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReadyStream {
    pub ssrc: u32,
    pub rtx_ssrc: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SessionDescription {
    pub mode: String,
    pub secret_key: Vec<u8>,
    pub audio_codec: Option<String>,
    pub media_session_id: Option<String>,
    pub dave_protocol_version: u16,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SpeakingEvent {
    pub user_id: Option<String>,
    pub ssrc: u32,
    pub speaking: Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Transition {
    pub transition_id: u16,
    pub protocol_version: u16,
    pub epoch: u64,
}

/// A server → client binary frame: `(seq, op, payload)`.
pub fn parse_binary(b: &[u8]) -> Option<(u16, u8, &[u8])> {
    if b.len() < 3 {
        return None;
    }
    Some((u16::from_be_bytes([b[0], b[1]]), b[2], &b[3..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    #[test]
    fn url_drops_default_port() {
        assert_eq!(url("c-sea07.discord.media:443"), "wss://c-sea07.discord.media/?v=9");
        assert_eq!(url("c-sea07.discord.media:2096"), "wss://c-sea07.discord.media:2096/?v=9");
    }

    #[test]
    fn identify_shape() {
        let v = identify(&IdentifyInput {
            server_id: Snowflake(1),
            channel_id: Snowflake(2),
            user_id: Snowflake(3),
            session_id: "s",
            token: "t",
            max_dave_protocol_version: 1,
            screen: false,
        });
        assert_eq!(
            v.to_string(),
            r#"{"op":0,"d":{"server_id":"1","channel_id":"2","user_id":"3","session_id":"s","token":"t","max_dave_protocol_version":1,"video":true,"streams":[{"type":"video","rid":"100","quality":100}]}}"#
        );
        let s = identify(&IdentifyInput { server_id: Snowflake(1), channel_id: Snowflake(2), user_id: Snowflake(3), session_id: "s", token: "t", max_dave_protocol_version: 1, screen: true });
        assert_eq!(s["d"]["streams"][0]["type"], "screen", "a Go Live connection describes its stream as screen");
    }

    #[test]
    fn codec_list_follows_the_official_numbering() {
        let c = codecs();
        let c = c.as_array().unwrap();
        // Audio first, then video at priority 1000 x (i + 1), payload 101 + 2i, RTX one above.
        for (i, v) in c[1..].iter().enumerate() {
            assert_eq!(v["priority"], 1000 * (i as u64 + 1));
            assert_eq!(v["payload_type"], 101 + 2 * i as u64);
            assert_eq!(v["rtx_payload_type"], 102 + 2 * i as u64);
        }
        let h264 = c.iter().find(|v| v["name"] == "H264").unwrap();
        assert_eq!(h264["payload_type"], crate::voice::stream::H264_PT);
        assert_eq!(h264["rtx_payload_type"], crate::voice::stream::H264_RTX_PT);
        assert!(c.iter().all(|v| v["name"] != "VP9"), "VP9 is not in the default official list");
        assert_eq!(h264["payload_type"], 105, "AV1 101, H265 103, H264 105, VP8 107 as on a Mac desktop");
    }

    #[test]
    fn select_protocol_spreads_address_like_the_client() {
        let v = select_protocol("203.0.113.9", 5000, "aead_aes256_gcm_rtpsize", "uuid", &client_experiments());
        let d = &v["d"];
        let keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["protocol", "data", "address", "port", "mode", "codecs", "rtc_connection_id", "experiments"]);
        assert_eq!(d["data"].to_string(), r#"{"address":"203.0.113.9","port":5000,"mode":"aead_aes256_gcm_rtpsize"}"#);
        assert_eq!(d["codecs"][0].to_string(), r#"{"name":"opus","type":"audio","priority":1000,"payload_type":120}"#);
    }

    #[test]
    fn heartbeat_and_speaking() {
        assert_eq!(heartbeat(5, None, 9).to_string(), r#"{"op":3,"d":{"t":5,"seq_ack":-1}}"#);
        assert_eq!(heartbeat(5, Some(9), 9).to_string(), r#"{"op":3,"d":{"t":5,"seq_ack":9}}"#);
        assert_eq!(heartbeat(5, Some(9), 7).to_string(), r#"{"op":3,"d":5}"#, "before v8 the data is the bare timestamp");
        assert_eq!(speaking(1, 77).to_string(), r#"{"op":5,"d":{"speaking":1,"delay":0,"ssrc":77}}"#);
    }

    #[test]
    fn speaking_is_sent_once_then_only_for_priority() {
        let mut g = SpeakingGate::default();
        assert!(!g.should_send(0, 77), "silence before the first word is not announced");
        assert!(g.should_send(1, 77), "the first word");
        assert!(!g.should_send(0, 77), "stopping is not sent");
        assert!(!g.should_send(1, 77), "starting again is not sent");
        assert!(g.should_send(1 | 4, 77), "priority speaker on");
        assert!(g.should_send(1, 77), "priority speaker off");
        assert!(g.should_send(1, 78), "a new SSRC is announced");
    }

    #[test]
    fn binary_frames() {
        assert_eq!(binary(26, &[1, 2]), vec![26, 1, 2]);
        assert_eq!(parse_binary(&[0, 5, 25, 9]), Some((5, 25, &[9u8][..])));
    }

    #[test]
    fn session_description_decodes_key() {
        let s: SessionDescription = serde_json::from_value(json!({"mode": "aead_aes256_gcm_rtpsize", "secret_key": [1, 2, 3], "dave_protocol_version": 1})).unwrap();
        assert_eq!(s.secret_key, vec![1, 2, 3]);
        assert_eq!(s.dave_protocol_version, 1);
    }
}
