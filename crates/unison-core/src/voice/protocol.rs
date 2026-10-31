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
}

/// Op 0. `streams` describes the one video stream a desktop client offers even when its
/// camera is off; **capture** pending (FIDELITY.md, voice).
pub fn identify(i: &IdentifyInput) -> Value {
    json!({ "op": op::IDENTIFY, "d": {
        "server_id": i.server_id.to_string(),
        "channel_id": i.channel_id.to_string(),
        "user_id": i.user_id.to_string(),
        "session_id": i.session_id,
        "token": i.token,
        "max_dave_protocol_version": i.max_dave_protocol_version,
        "video": true,
        "streams": [{ "type": "video", "rid": "100", "quality": 100 }],
    }})
}

/// Op 3 (v ≥ 4): a millisecond timestamp and the last server `seq` seen.
pub fn heartbeat(t_ms: u64, seq_ack: Option<i64>) -> Value {
    json!({ "op": op::HEARTBEAT, "d": { "t": t_ms, "seq_ack": seq_ack } })
}

pub fn resume(token: &str, session_id: &str, server_id: Id, channel_id: Id, seq_ack: Option<i64>) -> Value {
    json!({ "op": op::RESUME, "d": {
        "token": token,
        "session_id": session_id,
        "server_id": server_id.to_string(),
        "channel_id": channel_id.to_string(),
        "seq_ack": seq_ack,
    }})
}

/// The codecs a desktop client advertises (`priority` is `1000 × p`). **Capture** pending:
/// values follow public desktop traces until pinned from a recorded call.
pub fn codecs() -> Value {
    json!([
        { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120 },
        { "name": "AV1", "type": "video", "priority": 1000, "payload_type": 101, "rtx_payload_type": 102, "encode": false, "decode": false },
        { "name": "H265", "type": "video", "priority": 2000, "payload_type": 103, "rtx_payload_type": 104, "encode": false, "decode": false },
        { "name": "H264", "type": "video", "priority": 3000, "payload_type": 105, "rtx_payload_type": 106, "encode": true, "decode": true },
        { "name": "VP8", "type": "video", "priority": 4000, "payload_type": 107, "rtx_payload_type": 108, "encode": false, "decode": false },
        { "name": "VP9", "type": "video", "priority": 5000, "payload_type": 109, "rtx_payload_type": 110, "encode": false, "decode": false },
    ])
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
        });
        assert_eq!(
            v.to_string(),
            r#"{"op":0,"d":{"server_id":"1","channel_id":"2","user_id":"3","session_id":"s","token":"t","max_dave_protocol_version":1,"video":true,"streams":[{"type":"video","rid":"100","quality":100}]}}"#
        );
    }

    #[test]
    fn select_protocol_spreads_address_like_the_client() {
        let v = select_protocol("203.0.113.9", 5000, "aead_aes256_gcm_rtpsize", "uuid", &["fixed_keyframe_interval".into()]);
        let d = &v["d"];
        let keys: Vec<&str> = d.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["protocol", "data", "address", "port", "mode", "codecs", "rtc_connection_id", "experiments"]);
        assert_eq!(d["data"].to_string(), r#"{"address":"203.0.113.9","port":5000,"mode":"aead_aes256_gcm_rtpsize"}"#);
        assert_eq!(d["codecs"][0].to_string(), r#"{"name":"opus","type":"audio","priority":1000,"payload_type":120}"#);
    }

    #[test]
    fn heartbeat_and_speaking() {
        assert_eq!(heartbeat(5, None).to_string(), r#"{"op":3,"d":{"t":5,"seq_ack":null}}"#);
        assert_eq!(heartbeat(5, Some(9)).to_string(), r#"{"op":3,"d":{"t":5,"seq_ack":9}}"#);
        assert_eq!(speaking(1, 77).to_string(), r#"{"op":5,"d":{"speaking":1,"delay":0,"ssrc":77}}"#);
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
