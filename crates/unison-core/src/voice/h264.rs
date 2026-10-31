//! H.264 over RTP (RFC 6184, packetization-mode 1): Annex B / AVCC handling, single-NAL, STAP-A
//! and FU-A packetization, RTX framing (RFC 4588), and the matching depacketizer.

/// Video RTP clock rate.
pub const CLOCK_RATE: u32 = 90_000;
/// Payload budget per packet; leaves room for the 12-byte header, extensions and the AEAD tag.
pub const MAX_PAYLOAD: usize = 1100;

pub const NAL_STAP_A: u8 = 24;
pub const NAL_FU_A: u8 = 28;
pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;

/// Split an Annex B stream (3- or 4-byte start codes) into NAL units without the start codes.
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new(); // (start_code_pos, payload_pos)
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &(_, p)) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |s| s.0);
        // A 4-byte start code leaves one trailing zero on the previous NAL; trailing zeros are never valid NAL bytes.
        while end > p && data[end - 1] == 0 {
            end -= 1;
        }
        if end > p {
            out.push(&data[p..end]);
        }
    }
    out
}

/// Split a length-prefixed (AVCC / VideoToolbox) sample into NAL units.
pub fn split_avcc(data: &[u8], len_size: usize) -> Option<Vec<&[u8]>> {
    if !(1..=4).contains(&len_size) {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let hdr = data.get(i..i + len_size)?;
        let n = hdr.iter().fold(0usize, |a, &b| (a << 8) | b as usize);
        i += len_size;
        out.push(data.get(i..i + n)?);
        i += n;
    }
    Some(out)
}

/// Annex B bytes for a list of NALs (4-byte start codes), as a decoder-side reassembly output.
pub fn to_annexb(nals: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::with_capacity(nals.iter().map(|n| n.len() + 4).sum());
    for n in nals {
        v.extend_from_slice(&[0, 0, 0, 1]);
        v.extend_from_slice(n);
    }
    v
}

pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1f)
}

pub fn is_keyframe(nals: &[&[u8]]) -> bool {
    nals.iter().any(|n| nal_type(n) == NAL_IDR)
}

fn flush(out: &mut Vec<Vec<u8>>, agg: &mut Vec<&[u8]>, agg_len: &mut usize) {
    match agg.len() {
        0 => {}
        1 => out.push(agg[0].to_vec()),
        _ => {
            // F bit is the OR of members', NRI the max.
            let f = agg.iter().fold(0, |a, n| a | (n[0] & 0x80));
            let nri = agg.iter().map(|n| n[0] & 0x60).max().unwrap_or(0);
            let mut p = vec![f | nri | NAL_STAP_A];
            for n in agg.iter() {
                p.extend_from_slice(&(n.len() as u16).to_be_bytes());
                p.extend_from_slice(n);
            }
            out.push(p);
        }
    }
    agg.clear();
    *agg_len = 1;
}

/// Packetize one access unit. Returns `(payload, marker)` pairs; the marker is set on the last.
/// Parameter sets and SEI are aggregated into STAP-A when two or more fit together.
pub fn packetize(nals: &[&[u8]], max: usize) -> Vec<(Vec<u8>, bool)> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut agg: Vec<&[u8]> = Vec::new();
    let mut agg_len = 1usize;

    for &nal in nals.iter().filter(|n| !n.is_empty()) {
        if nal.len() < max {
            if agg_len + 2 + nal.len() > max {
                flush(&mut out, &mut agg, &mut agg_len);
            }
            agg_len += 2 + nal.len();
            agg.push(nal);
            // Only parameter sets, SEI and delimiters aggregate; slices go out alone to keep latency flat.
            if !matches!(nal_type(nal), NAL_SPS | NAL_PPS | 6 | 9) {
                flush(&mut out, &mut agg, &mut agg_len);
            }
        } else {
            flush(&mut out, &mut agg, &mut agg_len);
            let (hdr, body) = (nal[0], &nal[1..]);
            let chunk = max - 2;
            let n = body.len().div_ceil(chunk);
            for (k, c) in body.chunks(chunk).enumerate() {
                let mut ind = hdr & 0x1f;
                if k == 0 {
                    ind |= 0x80;
                }
                if k + 1 == n {
                    ind |= 0x40;
                }
                let mut p = Vec::with_capacity(2 + c.len());
                p.push(hdr & 0xe0 | NAL_FU_A);
                p.push(ind);
                p.extend_from_slice(c);
                out.push(p);
            }
        }
    }
    flush(&mut out, &mut agg, &mut agg_len);
    let last = out.len().saturating_sub(1);
    out.into_iter().enumerate().map(|(i, p)| (p, i == last)).collect()
}

/// RTP header for video: marker bit, dynamic payload type, no extension.
pub fn rtp_header(pt: u8, marker: bool, seq: u16, timestamp: u32, ssrc: u32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0] = 0x80;
    h[1] = (pt & 0x7f) | if marker { 0x80 } else { 0 };
    h[2..4].copy_from_slice(&seq.to_be_bytes());
    h[4..8].copy_from_slice(&timestamp.to_be_bytes());
    h[8..12].copy_from_slice(&ssrc.to_be_bytes());
    h
}

pub fn marker(packet: &[u8]) -> bool {
    packet.get(1).is_some_and(|b| b & 0x80 != 0)
}

/// RTX payload: the original sequence number (OSN) followed by the original payload.
pub fn rtx_payload(orig_seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + payload.len());
    v.extend_from_slice(&orig_seq.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

/// Undo [`rtx_payload`]: `(original seq, original payload)`.
pub fn rtx_unwrap(payload: &[u8]) -> Option<(u16, &[u8])> {
    Some((u16::from_be_bytes(payload.get(..2)?.try_into().ok()?), &payload[2..]))
}

/// Reassembles access units from in-order payloads. A sequence gap discards the frame in flight
/// and sets [`Depacketizer::needs_keyframe`] until the next IDR, so a PLI should be sent.
#[derive(Default)]
pub struct Depacketizer {
    nals: Vec<Vec<u8>>,
    fu: Option<Vec<u8>>,
    last_seq: Option<u16>,
    broken: bool,
    pub needs_keyframe: bool,
}

/// A finished access unit.
pub struct Frame {
    pub annexb: Vec<u8>,
    pub keyframe: bool,
}

impl Depacketizer {
    pub fn new() -> Self {
        Self { needs_keyframe: true, ..Default::default() }
    }

    pub fn push(&mut self, seq: u16, payload: &[u8], marker: bool) -> Option<Frame> {
        if let Some(l) = self.last_seq {
            if seq != l.wrapping_add(1) {
                self.nals.clear();
                self.fu = None;
                self.broken = true;
                self.needs_keyframe = true;
            }
        }
        self.last_seq = Some(seq);
        match nal_type(payload) {
            1..=23 => self.nals.push(payload.to_vec()),
            NAL_STAP_A => {
                let mut i = 1;
                while i + 2 <= payload.len() {
                    let n = u16::from_be_bytes([payload[i], payload[i + 1]]) as usize;
                    i += 2;
                    match payload.get(i..i + n) {
                        Some(nal) if !nal.is_empty() => self.nals.push(nal.to_vec()),
                        _ => {
                            self.broken = true;
                            break;
                        }
                    }
                    i += n;
                }
            }
            NAL_FU_A if payload.len() >= 2 => {
                let ind = payload[1];
                if ind & 0x80 != 0 {
                    let mut v = vec![payload[0] & 0xe0 | ind & 0x1f];
                    v.extend_from_slice(&payload[2..]);
                    self.fu = Some(v);
                } else if let Some(f) = self.fu.as_mut() {
                    f.extend_from_slice(&payload[2..]);
                } else {
                    self.broken = true;
                }
                if ind & 0x40 != 0 {
                    if let Some(f) = self.fu.take() {
                        self.nals.push(f);
                    }
                }
            }
            _ => self.broken = true,
        }
        if !marker {
            return None;
        }
        let nals = std::mem::take(&mut self.nals);
        let broken = std::mem::take(&mut self.broken);
        self.fu = None;
        if broken || nals.is_empty() {
            self.needs_keyframe = true;
            return None;
        }
        let refs: Vec<&[u8]> = nals.iter().map(|n| n.as_slice()).collect();
        let keyframe = is_keyframe(&refs);
        if keyframe {
            self.needs_keyframe = false;
        } else if self.needs_keyframe {
            return None; // can't decode a P frame without its reference
        }
        Some(Frame { annexb: to_annexb(&refs), keyframe })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(t: u8, len: usize, fill: u8) -> Vec<u8> {
        let mut v = vec![0x60 | t];
        v.extend((0..len).map(|i| fill.wrapping_add(i as u8) | 1));
        v
    }

    fn roundtrip(nals: &[Vec<u8>], max: usize) -> (Vec<(Vec<u8>, bool)>, Option<Frame>) {
        let refs: Vec<&[u8]> = nals.iter().map(|n| n.as_slice()).collect();
        let pk = packetize(&refs, max);
        let mut d = Depacketizer::new();
        let mut frame = None;
        for (i, (p, m)) in pk.iter().enumerate() {
            frame = d.push(i as u16, p, *m);
        }
        (pk, frame)
    }

    #[test]
    fn annexb_split_handles_both_start_codes() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let n = split_annexb(&data);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
    }

    #[test]
    fn avcc_split() {
        let data = [0, 0, 0, 2, 0x67, 1, 0, 0, 0, 1, 0x65];
        assert_eq!(split_avcc(&data, 4).unwrap(), vec![&[0x67, 1][..], &[0x65][..]]);
        assert!(split_avcc(&[0, 0, 0, 9, 1], 4).is_none());
    }

    #[test]
    fn parameter_sets_aggregate_into_stap_a_and_idr_fragments() {
        let nals = vec![nal(7, 20, 1), nal(8, 4, 2), nal(5, 3000, 3)];
        let (pk, frame) = roundtrip(&nals, 1100);
        assert_eq!(nal_type(&pk[0].0), NAL_STAP_A);
        assert!(pk[1..].iter().all(|(p, _)| nal_type(p) == NAL_FU_A));
        assert!(pk.iter().all(|(p, _)| p.len() <= 1100));
        assert_eq!(pk.iter().filter(|(_, m)| *m).count(), 1);
        assert!(pk.last().unwrap().1);
        let f = frame.expect("frame");
        assert!(f.keyframe);
        let refs: Vec<&[u8]> = nals.iter().map(|n| n.as_slice()).collect();
        assert_eq!(f.annexb, to_annexb(&refs));
    }

    #[test]
    fn small_slice_is_single_nal() {
        let nals = vec![nal(5, 50, 4)];
        let (pk, frame) = roundtrip(&nals, 1100);
        assert_eq!(pk.len(), 1);
        assert_eq!(pk[0].0, nals[0]);
        assert!(frame.unwrap().keyframe);
    }

    #[test]
    fn fu_a_boundary_sizes_round_trip() {
        for len in [1097, 1098, 1099, 1100, 2197, 2198, 2199, 5000] {
            let nals = vec![nal(5, len, 7)];
            let (pk, frame) = roundtrip(&nals, 1100);
            assert!(pk.iter().all(|(p, _)| p.len() <= 1100), "len {len}");
            let refs: Vec<&[u8]> = nals.iter().map(|n| n.as_slice()).collect();
            assert_eq!(frame.unwrap().annexb, to_annexb(&refs), "len {len}");
        }
    }

    #[test]
    fn loss_drops_frame_and_demands_keyframe() {
        let idr = nal(5, 3000, 1);
        let p = nal(1, 3000, 2);
        let mut d = Depacketizer::new();
        let mut seq = 0u16;
        let mut feed = |d: &mut Depacketizer, n: &Vec<u8>, skip: Option<usize>| {
            let mut out = None;
            for (i, (pl, m)) in packetize(&[n.as_slice()], 1100).into_iter().enumerate() {
                if skip == Some(i) {
                    seq = seq.wrapping_add(1);
                    continue;
                }
                out = d.push(seq, &pl, m);
                seq = seq.wrapping_add(1);
            }
            out
        };
        assert!(feed(&mut d, &idr, None).is_some());
        assert!(!d.needs_keyframe);
        assert!(feed(&mut d, &p, Some(1)).is_none());
        assert!(d.needs_keyframe);
        assert!(feed(&mut d, &p, None).is_none(), "P frame without reference is withheld");
        assert!(feed(&mut d, &idr, None).is_some());
        assert!(!d.needs_keyframe);
        assert!(feed(&mut d, &p, None).is_some());
    }

    #[test]
    fn sequence_wraps() {
        let mut d = Depacketizer::new();
        assert!(d.push(65535, &nal(5, 10, 1), true).is_some());
        assert!(d.push(0, &nal(1, 10, 1), true).is_some());
    }

    #[test]
    fn rtx_round_trip_and_header() {
        let w = rtx_payload(0xBEEF, &[1, 2, 3]);
        assert_eq!(rtx_unwrap(&w), Some((0xBEEF, &[1u8, 2, 3][..])));
        let h = rtp_header(105, true, 7, 90_000, 42);
        assert!(marker(&h));
        assert_eq!(crate::voice::rtp::parse(&h).unwrap().payload_type, 105);
    }
}
