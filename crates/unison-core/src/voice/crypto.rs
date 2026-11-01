//! Transport encryption for voice RTP: the `_rtpsize` AEAD modes.
//!
//! The RTP header (plus CSRCs and the 4-byte extension header) is authenticated but sent in
//! the clear; everything after it is encrypted. A 32-bit big-endian counter goes in the first
//! four bytes of the nonce (the rest is zero) and is appended to the packet after the tag.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce as GcmNonce};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::voice::rtp::Incoming;

/// Modes we support, in the order we prefer them.
pub const MODES: [&str; 2] = ["aead_aes256_gcm_rtpsize", "aead_xchacha20_poly1305_rtpsize"];

/// The first mode we support from the server's offer, in our preference order.
pub fn choose_mode(offered: &[String]) -> Option<&'static str> {
    MODES.into_iter().find(|m| offered.iter().any(|o| o == m))
}

pub enum Cipher {
    Aes(Box<Aes256Gcm>),
    XChaCha(Box<XChaCha20Poly1305>),
}

pub struct Transport {
    cipher: Cipher,
    counter: u32,
}

impl Transport {
    pub fn new(mode: &str, key: &[u8]) -> Option<Self> {
        let cipher = match mode {
            "aead_aes256_gcm_rtpsize" => Cipher::Aes(Box::new(Aes256Gcm::new_from_slice(key).ok()?)),
            "aead_xchacha20_poly1305_rtpsize" => Cipher::XChaCha(Box::new(XChaCha20Poly1305::new_from_slice(key).ok()?)),
            _ => return None,
        };
        Some(Self { cipher, counter: 0 })
    }

    /// `header` is sent in the clear; `payload` is encrypted. Returns the full UDP datagram.
    pub fn seal(&mut self, header: &[u8], payload: &[u8]) -> Option<Vec<u8>> {
        let n = self.counter;
        self.counter = self.counter.wrapping_add(1);
        let msg = Payload { msg: payload, aad: header };
        let sealed = match &self.cipher {
            Cipher::Aes(c) => c.encrypt(GcmNonce::from_slice(&nonce12(n)), msg).ok()?,
            Cipher::XChaCha(c) => c.encrypt(XNonce::from_slice(&nonce24(n)), msg).ok()?,
        };
        let mut out = Vec::with_capacity(header.len() + sealed.len() + 4);
        out.extend_from_slice(header);
        out.extend_from_slice(&sealed);
        out.extend_from_slice(&n.to_be_bytes());
        Some(out)
    }

    /// Decrypt an incoming RTP packet; returns the media payload with any extension body removed.
    pub fn open(&self, packet: &[u8], rtp: &Incoming) -> Option<Vec<u8>> {
        self.open_parts(packet, rtp).map(|(_, payload)| payload)
    }

    /// Decrypt an incoming RTP packet into its header-extension body and its media payload.
    pub fn open_parts(&self, packet: &[u8], rtp: &Incoming) -> Option<(Vec<u8>, Vec<u8>)> {
        if packet.len() < rtp.aad_len + 16 + 4 {
            return None;
        }
        let (body, suffix) = packet.split_at(packet.len() - 4);
        let n = u32::from_be_bytes(suffix.try_into().ok()?);
        let msg = Payload { msg: &body[rtp.aad_len..], aad: &body[..rtp.aad_len] };
        let mut plain = match &self.cipher {
            Cipher::Aes(c) => c.decrypt(GcmNonce::from_slice(&nonce12(n)), msg).ok()?,
            Cipher::XChaCha(c) => c.decrypt(XNonce::from_slice(&nonce24(n)), msg).ok()?,
        };
        if plain.len() < rtp.ext_body_len {
            return None;
        }
        let payload = plain.split_off(rtp.ext_body_len);
        Some((plain, payload))
    }
}

fn nonce12(n: u32) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[..4].copy_from_slice(&n.to_be_bytes());
    b
}

fn nonce24(n: u32) -> [u8; 24] {
    let mut b = [0u8; 24];
    b[..4].copy_from_slice(&n.to_be_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::rtp;

    #[test]
    fn prefers_aes_gcm() {
        let offered = vec!["aead_xchacha20_poly1305_rtpsize".to_string(), "aead_aes256_gcm_rtpsize".to_string(), "xsalsa20_poly1305".to_string()];
        assert_eq!(choose_mode(&offered), Some("aead_aes256_gcm_rtpsize"));
        assert_eq!(choose_mode(&["xsalsa20_poly1305".to_string()]), None);
    }

    #[test]
    fn both_modes_round_trip_and_reject_tampering() {
        for mode in MODES {
            let key = [7u8; 32];
            let mut tx = Transport::new(mode, &key).unwrap();
            let rx = Transport::new(mode, &key).unwrap();
            let h = rtp::header(10, 960, 42);
            let pkt = tx.seal(&h, b"opus-frame").unwrap();
            assert_eq!(&pkt[..12], &h, "header in the clear");
            assert_eq!(&pkt[pkt.len() - 4..], &[0, 0, 0, 0], "nonce counter appended");
            let parsed = rtp::parse(&pkt).unwrap();
            assert_eq!(rx.open(&pkt, &parsed).unwrap(), b"opus-frame");
            let second = tx.seal(&h, b"x").unwrap();
            assert_eq!(&second[second.len() - 4..], &[0, 0, 0, 1]);
            let mut bad = pkt.clone();
            bad[3] ^= 1; // header is authenticated
            assert!(rx.open(&bad, &rtp::parse(&bad).unwrap()).is_none());
        }
    }

    #[test]
    fn strips_encrypted_extension_body() {
        let key = [1u8; 32];
        let mut tx = Transport::new(MODES[0], &key).unwrap();
        let mut h = rtp::header(1, 2, 3).to_vec();
        h[0] |= 0x10;
        h.extend_from_slice(&[0xBE, 0xDE, 0x00, 0x01]);
        let pkt = tx.seal(&h, &[0x10, 0x7f, 0, 0, b'o', b'p']).unwrap();
        let parsed = rtp::parse(&pkt).unwrap();
        assert_eq!(Transport::new(MODES[0], &key).unwrap().open(&pkt, &parsed).unwrap(), b"op");
    }
}
