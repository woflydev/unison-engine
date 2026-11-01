//! RTP framing for the voice UDP socket, and the 74-byte IP discovery exchange.

/// Opus payload type Discord negotiates (`payload_type: 120`).
pub const OPUS_PT: u8 = 120;
/// 20 ms of 48 kHz audio.
pub const FRAME_SAMPLES: u32 = 960;
/// Opus "silence" frame, sent five times after speaking stops.
pub const SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// A 12-byte RTP header with no CSRCs or extension, as built for outgoing audio.
pub fn header(seq: u16, timestamp: u32, ssrc: u32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0] = 0x80;
    h[1] = OPUS_PT;
    h[2..4].copy_from_slice(&seq.to_be_bytes());
    h[4..8].copy_from_slice(&timestamp.to_be_bytes());
    h[8..12].copy_from_slice(&ssrc.to_be_bytes());
    h
}

/// Header-extension ids the official native module registers for audio (`discord::media::
/// RtpAudioExtensions` in `discord_voice.node`, 2026-11-01): 1 `urn:ietf:params:rtp-hdrext:ssrc-audio-level`,
/// 3 `abs-send-time`, 9 Discord's `speaker` (received only), 10/11/12 the sdes mid and stream ids
/// (unused here: there is no SDP). Outgoing audio carries the first two, as libwebrtc's sender does.
pub const EXT_AUDIO_LEVEL: u8 = 1;
pub const EXT_ABS_SEND_TIME: u8 = 3;

/// Video registers `abs-send-time` (3) and, for transport-cc, the transport-wide sequence number (5,
/// `draft-holmer-rmcat-transport-wide-cc-extensions-01`) among others (`discord::media::RtpVideoExtensions`).
/// Every outgoing video and RTX packet carries those two, as libwebrtc's egress does for registered
/// extensions; the others (toffset, orientation, playout-delay, content-type, timing, mid, rids) are
/// not attached (they need capture timing, rotation or an SDP mid we do not have).
pub const EXT_TRANSPORT_SEQ: u8 = 5;
/// `urn:3gpp:video-orientation` (CVO), id 4 in `RtpVideoExtensions`.
pub const EXT_VIDEO_ORIENTATION: u8 = 4;

/// The 4-byte extension header (profile 0xBEDE, 2 words) and 8-byte body for a video packet:
/// abs-send-time then the 16-bit transport-wide sequence number, one-byte form, zero padded.
pub fn video_extension_block(now_ms: u64, transport_seq: u16) -> [u8; 12] {
    let ast = abs_send_time(now_ms).to_be_bytes();
    let s = transport_seq.to_be_bytes();
    [0xBE, 0xDE, 0, 2, (EXT_ABS_SEND_TIME << 4) | 2, ast[1], ast[2], ast[3], (EXT_TRANSPORT_SEQ << 4) | 1, s[0], s[1], 0]
}

/// Split a plaintext RTP packet into what is authenticated (fixed header, CSRCs, extension header) and
/// what is sealed (extension body and payload), exactly as the receiver's `parse` reads it.
/// The data of extension `want` in a one-byte-header extension body, if present.
fn one_byte_ext(ext_body: &[u8], want: u8) -> Option<&[u8]> {
    let mut i = 0;
    while i < ext_body.len() {
        let b = ext_body[i];
        if b == 0 {
            i += 1; // padding
            continue;
        }
        let (id, len) = (b >> 4, (b & 0x0f) as usize + 1);
        if id == 15 {
            return None;
        }
        let data = ext_body.get(i + 1..i + 1 + len)?;
        if id == want {
            return Some(data);
        }
        i += 1 + len;
    }
    None
}

/// The transport-wide sequence number (id 5) from a one-byte-header extension body, if present.
pub fn transport_seq(ext_body: &[u8]) -> Option<u16> {
    one_byte_ext(ext_body, EXT_TRANSPORT_SEQ).filter(|d| d.len() == 2).map(|d| u16::from_be_bytes([d[0], d[1]]))
}

/// Clockwise rotation (0, 90, 180, 270) the receiver applies to show the picture upright, from
/// the video-orientation extension (low two bits; libwebrtc `ConvertCVOByteToVideoRotation`).
/// Phones send portrait video as landscape frames plus this.
pub fn video_rotation(ext_body: &[u8]) -> Option<u16> {
    one_byte_ext(ext_body, EXT_VIDEO_ORIENTATION).and_then(|d| d.first()).map(|b| (*b & 0x03) as u16 * 90)
}

pub fn seal_split(packet: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = parse(packet)?;
    Some((&packet[..i.aad_len], &packet[i.aad_len..]))
}

/// The 24-bit abs-send-time value for a monotonic millisecond clock (6.18 fixed point seconds).
pub fn abs_send_time(now_ms: u64) -> u32 {
    ((((now_ms << 18) + 500) / 1000) & 0x00ff_ffff) as u32
}

/// Outgoing audio with the one-byte header extensions: the 16 authenticated bytes (fixed header with
/// the X bit, profile 0xBEDE and a length of 2 words) and the 8-byte extension body, which is sealed
/// together with the payload (`_rtpsize` modes). `level_dbfs` becomes the -dBov magnitude (0 to 127)
/// and `voice` the V bit of the audio-level byte.
pub fn audio_header_with_extensions(seq: u16, timestamp: u32, ssrc: u32, level_dbfs: f32, voice: bool, now_ms: u64) -> ([u8; 16], [u8; 8]) {
    let mut h = [0u8; 16];
    h[..12].copy_from_slice(&header(seq, timestamp, ssrc));
    h[0] |= 0x10;
    h[12..14].copy_from_slice(&[0xBE, 0xDE]);
    h[14..16].copy_from_slice(&2u16.to_be_bytes());
    let level = (-level_dbfs).clamp(0.0, 127.0) as u8;
    let ast = abs_send_time(now_ms).to_be_bytes();
    let body = [(EXT_AUDIO_LEVEL << 4), (u8::from(voice) << 7) | level, (EXT_ABS_SEND_TIME << 4) | 2, ast[1], ast[2], ast[3], 0, 0];
    (h, body)
}

/// RTCP shares the socket; its second byte is a packet type in 200..=204 (72..=76 with the marker bit cleared).
pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 2 && (192..=223).contains(&packet[1])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming {
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload_type: u8,
    /// Bytes authenticated but not encrypted in the `_rtpsize` modes: the fixed header, CSRCs,
    /// and the 4-byte extension header (the extension body is encrypted).
    pub aad_len: usize,
    /// Length of the encrypted extension body that leads the plaintext, if any.
    pub ext_body_len: usize,
    /// The P bit: the decrypted payload ends in padding whose last byte is its length.
    pub padded: bool,
}

/// Remove RTP padding (RFC 3550 5.1) from a decrypted payload. Senders pad video for bandwidth
/// probes (often padding-only RTX packets); left in, the pad bytes sit where DAVE expects its
/// trailer and the frame fails to decrypt. `false` means the packet is malformed or only padding.
pub fn strip_padding(payload: &mut Vec<u8>, padded: bool) -> bool {
    if padded {
        let n = payload.last().copied().unwrap_or(0) as usize;
        if n == 0 || n > payload.len() {
            return false;
        }
        payload.truncate(payload.len() - n);
    }
    !payload.is_empty()
}

pub fn parse(packet: &[u8]) -> Option<Incoming> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let cc = (packet[0] & 0x0f) as usize;
    let has_ext = packet[0] & 0x10 != 0;
    let mut aad_len = 12 + 4 * cc;
    let mut ext_body_len = 0;
    if has_ext {
        let words = u16::from_be_bytes(packet.get(aad_len + 2..aad_len + 4)?.try_into().ok()?) as usize;
        aad_len += 4;
        ext_body_len = words * 4;
    }
    if packet.len() < aad_len {
        return None;
    }
    Some(Incoming {
        seq: u16::from_be_bytes([packet[2], packet[3]]),
        timestamp: u32::from_be_bytes(packet[4..8].try_into().ok()?),
        ssrc: u32::from_be_bytes(packet[8..12].try_into().ok()?),
        payload_type: packet[1] & 0x7f,
        aad_len,
        ext_body_len,
        padded: packet[0] & 0x20 != 0,
    })
}

/// IP discovery request: type 1, length 70, our SSRC, then 66 zero bytes.
pub fn discovery_request(ssrc: u32) -> [u8; 74] {
    let mut b = [0u8; 74];
    b[0..2].copy_from_slice(&1u16.to_be_bytes());
    b[2..4].copy_from_slice(&70u16.to_be_bytes());
    b[4..8].copy_from_slice(&ssrc.to_be_bytes());
    b
}

/// IP discovery response: our external address (NUL-terminated) and port.
pub fn discovery_response(b: &[u8]) -> Option<(String, u16)> {
    if b.len() < 74 || u16::from_be_bytes([b[0], b[1]]) != 2 {
        return None;
    }
    let addr = &b[8..72];
    let end = addr.iter().position(|&c| c == 0).unwrap_or(addr.len());
    let ip = std::str::from_utf8(&addr[..end]).ok()?.to_string();
    Some((ip, u16::from_be_bytes([b[72], b[73]])))
}

#[cfg(test)]
mod tests {
    #[test]
    fn video_rotation_reads_the_cvo_byte_next_to_other_extensions() {
        // abs-send-time (id 3, 3 bytes), CVO (id 4, 1 byte) = 90 degrees, padding.
        let ext = [0x32, 1, 2, 3, 0x40, 0x01, 0, 0];
        assert_eq!(super::video_rotation(&ext), Some(90));
        assert_eq!(super::video_rotation(&[0x40, 0x03, 0, 0]), Some(270));
        assert_eq!(super::video_rotation(&[0x51, 0, 7, 0]), None);
        assert_eq!(super::transport_seq(&[0x51, 0, 7, 0]), Some(7));
    }

    #[test]
    fn padding_is_stripped_and_padding_only_packets_are_dropped() {
        let mut p = vec![1, 2, 3, 0, 0, 3];
        assert!(super::strip_padding(&mut p, true));
        assert_eq!(p, [1, 2, 3]);
        let mut only = vec![0, 0, 0, 4];
        assert!(!super::strip_padding(&mut only, true));
        let mut bad = vec![1, 9];
        assert!(!super::strip_padding(&mut bad, true));
        let mut plain = vec![1, 2, 3];
        assert!(super::strip_padding(&mut plain, false));
        assert_eq!(plain, [1, 2, 3]);
    }

    use super::*;

    #[test]
    fn header_round_trips() {
        let h = header(513, 0x01020304, 0xAABBCCDD);
        let p = parse(&h).unwrap();
        assert_eq!((p.seq, p.timestamp, p.ssrc, p.payload_type, p.aad_len), (513, 0x01020304, 0xAABBCCDD, 120, 12));
        assert!(!is_rtcp(&h));
    }

    #[test]
    fn extension_header_is_aad_and_body_is_encrypted() {
        // X bit set, profile 0xBEDE, one 32-bit word of extension body.
        let mut p = header(1, 2, 3).to_vec();
        p[0] |= 0x10;
        p.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
        p.extend_from_slice(&[1, 2, 3, 4, 9, 9]);
        let i = parse(&p).unwrap();
        assert_eq!((i.aad_len, i.ext_body_len), (16, 4));
    }

    #[test]
    fn audio_extensions_are_authenticated_header_plus_sealed_body() {
        let (h, body) = audio_header_with_extensions(1, 960, 7, -30.4, true, 1000);
        let i = parse(&[&h[..], &body[..], &[0xAA][..]].concat()).unwrap();
        assert_eq!((i.aad_len, i.ext_body_len), (16, 8), "the parser sees the same split we seal");
        assert_eq!(&h[12..16], &[0xBE, 0xDE, 0, 2]);
        assert_eq!(h[0], 0x90);
        assert_eq!(body[0], 0x10, "id 1, one data byte");
        assert_eq!(body[1], 0x80 | 30, "voice bit and -dBov");
        assert_eq!(body[2], 0x32, "id 3, three data bytes");
        assert_eq!(abs_send_time(1000), 262144, "one second is 2^18");
        assert_eq!(&body[3..6], &abs_send_time(1000).to_be_bytes()[1..]);
        let (_, quiet) = audio_header_with_extensions(1, 2, 3, -200.0, false, 0);
        assert_eq!(quiet[1], 127);
    }

    #[test]
    fn video_extension_block_layout() {
        let b = video_extension_block(1000, 0x1234);
        assert_eq!(b, [0xBE, 0xDE, 0, 2, 0x32, 0x04, 0x00, 0x00, 0x51, 0x12, 0x34, 0]);
        let mut p = crate::voice::h264::rtp_header(105, true, 1, 2, 3).to_vec();
        p[0] |= 0x10;
        p.extend_from_slice(&b);
        p.extend_from_slice(&[9, 9, 9]);
        let (aad, rest) = seal_split(&p).unwrap();
        assert_eq!((aad.len(), rest.len()), (16, 8 + 3));
        assert!(seal_split(&[1, 2, 3]).is_none(), "a short or foreign packet is never split");
    }

    #[test]
    fn transport_seq_is_found_among_other_elements() {
        let b = video_extension_block(1000, 0x1234);
        assert_eq!(transport_seq(&b[4..]), Some(0x1234));
        assert_eq!(transport_seq(&[0x32, 1, 2, 3, 0, 0]), None);
        assert_eq!(transport_seq(&[]), None);
    }

    #[test]
    fn rtcp_is_recognised() {
        assert!(is_rtcp(&[0x81, 201, 0, 7]));
    }

    #[test]
    fn discovery() {
        let r = discovery_request(7);
        assert_eq!(&r[..8], &[0, 1, 0, 70, 0, 0, 0, 7]);
        let mut resp = [0u8; 74];
        resp[1] = 2;
        resp[3] = 70;
        resp[8..19].copy_from_slice(b"203.0.113.9");
        resp[72..74].copy_from_slice(&50004u16.to_be_bytes());
        assert_eq!(discovery_response(&resp), Some(("203.0.113.9".into(), 50004)));
    }
}
