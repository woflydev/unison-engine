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
