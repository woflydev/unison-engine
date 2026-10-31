//! RTCP for the video path: parsing sender/receiver reports, generic NACK (RFC 4585), PLI, FIR
//! (RFC 5104), REMB and transport-wide congestion control feedback, and building the packets we
//! send. Operates on decrypted plaintext; the `_rtpsize` transport modes keep the 8-byte RTCP
//! header in the clear and encrypt the rest, which [`crate::voice::crypto`] undoes first.

const PT_SR: u8 = 200;
const PT_RR: u8 = 201;
const PT_RTPFB: u8 = 205;
const PT_PSFB: u8 = 206;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportBlock {
    pub ssrc: u32,
    /// Loss fraction in 1/256 units.
    pub fraction_lost: u8,
    pub cumulative_lost: u32,
    pub highest_seq: u32,
    pub jitter: u32,
    pub last_sr: u32,
    pub delay_since_sr: u32,
}

impl ReportBlock {
    pub fn loss(&self) -> f32 {
        self.fraction_lost as f32 / 256.0
    }

    /// Round-trip time in milliseconds given the compact NTP time of arrival (middle 32 bits).
    pub fn rtt_ms(&self, now_compact_ntp: u32) -> Option<f32> {
        if self.last_sr == 0 {
            return None;
        }
        let d = now_compact_ntp.wrapping_sub(self.last_sr).wrapping_sub(self.delay_since_sr);
        if d > 0x8000_0000 {
            return None;
        }
        Some(d as f32 * 1000.0 / 65536.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Rtcp {
    SenderReport { ssrc: u32, ntp: u64, rtp_ts: u32, packets: u32, octets: u32, blocks: Vec<ReportBlock> },
    ReceiverReport { ssrc: u32, blocks: Vec<ReportBlock> },
    Nack { sender: u32, media: u32, seqs: Vec<u16> },
    Pli { sender: u32, media: u32 },
    /// `(ssrc, command sequence number)` entries.
    Fir { sender: u32, entries: Vec<(u32, u8)> },
    Remb { sender: u32, bitrate: u64, ssrcs: Vec<u32> },
    TransportCc(TransportCc),
    Other { pt: u8 },
}

/// Transport-wide congestion control feedback (draft-holmer-rmcat-transport-wide-cc-extensions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportCc {
    pub sender: u32,
    pub media: u32,
    pub base_seq: u16,
    /// Reference time in microseconds (24-bit field × 64 ms).
    pub reference_us: i64,
    pub fb_count: u8,
    /// Per transport sequence number: arrival time in microseconds, or `None` if reported lost.
    pub arrivals: Vec<(u16, Option<i64>)>,
}

fn u32at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn blocks(b: &[u8], count: usize) -> Option<Vec<ReportBlock>> {
    (0..count)
        .map(|i| {
            let o = i * 24;
            let w = b.get(o..o + 24)?;
            Some(ReportBlock {
                ssrc: u32at(w, 0)?,
                fraction_lost: w[4],
                cumulative_lost: u32at(w, 4)? & 0x00ff_ffff,
                highest_seq: u32at(w, 8)?,
                jitter: u32at(w, 12)?,
                last_sr: u32at(w, 16)?,
                delay_since_sr: u32at(w, 20)?,
            })
        })
        .collect()
}

/// Parse a compound RTCP packet. Malformed trailing packets end parsing; earlier ones are kept.
pub fn parse(mut data: &[u8]) -> Vec<Rtcp> {
    let mut out = Vec::new();
    while data.len() >= 4 && data[0] >> 6 == 2 {
        let len = (u16::from_be_bytes([data[2], data[3]]) as usize + 1) * 4;
        if len > data.len() {
            break;
        }
        let (pkt, rest) = data.split_at(len);
        data = rest;
        let fmt = (pkt[0] & 0x1f) as usize;
        let pt = pkt[1];
        let body = &pkt[4..];
        let parsed = match pt {
            PT_SR => (|| {
                Some(Rtcp::SenderReport {
                    ssrc: u32at(body, 0)?,
                    ntp: (u32at(body, 4)? as u64) << 32 | u32at(body, 8)? as u64,
                    rtp_ts: u32at(body, 12)?,
                    packets: u32at(body, 16)?,
                    octets: u32at(body, 20)?,
                    blocks: blocks(body.get(24..)?, fmt)?,
                })
            })(),
            PT_RR => (|| Some(Rtcp::ReceiverReport { ssrc: u32at(body, 0)?, blocks: blocks(body.get(4..)?, fmt)? }))(),
            PT_RTPFB => parse_rtpfb(fmt, body),
            PT_PSFB => parse_psfb(fmt, body),
            _ => Some(Rtcp::Other { pt }),
        };
        match parsed {
            Some(p) => out.push(p),
            None => break,
        }
    }
    out
}

fn parse_rtpfb(fmt: usize, b: &[u8]) -> Option<Rtcp> {
    let (sender, media) = (u32at(b, 0)?, u32at(b, 4)?);
    match fmt {
        1 => {
            let mut seqs = Vec::new();
            for chunk in b.get(8..)?.chunks_exact(4) {
                let pid = u16::from_be_bytes([chunk[0], chunk[1]]);
                let blp = u16::from_be_bytes([chunk[2], chunk[3]]);
                seqs.push(pid);
                (0..16).filter(|i| blp >> i & 1 == 1).for_each(|i| seqs.push(pid.wrapping_add(i + 1)));
            }
            Some(Rtcp::Nack { sender, media, seqs })
        }
        15 => parse_tcc(sender, media, b.get(8..)?).map(Rtcp::TransportCc),
        _ => Some(Rtcp::Other { pt: PT_RTPFB }),
    }
}

fn parse_psfb(fmt: usize, b: &[u8]) -> Option<Rtcp> {
    let (sender, media) = (u32at(b, 0)?, u32at(b, 4)?);
    match fmt {
        1 => Some(Rtcp::Pli { sender, media }),
        4 => Some(Rtcp::Fir {
            sender,
            entries: b.get(8..)?.chunks_exact(8).map(|c| (u32::from_be_bytes([c[0], c[1], c[2], c[3]]), c[4])).collect(),
        }),
        15 => {
            let f = b.get(8..)?;
            if f.get(..4)? != b"REMB" {
                return Some(Rtcp::Other { pt: PT_PSFB });
            }
            let n = f[4] as usize;
            let exp = f[5] >> 2;
            let mantissa = ((f[5] & 3) as u64) << 16 | (f[6] as u64) << 8 | f[7] as u64;
            let ssrcs = f.get(8..8 + 4 * n)?.chunks_exact(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect();
            Some(Rtcp::Remb { sender, bitrate: mantissa << exp, ssrcs })
        }
        _ => Some(Rtcp::Other { pt: PT_PSFB }),
    }
}

fn parse_tcc(sender: u32, media: u32, b: &[u8]) -> Option<TransportCc> {
    let base_seq = u16::from_be_bytes(b.get(0..2)?.try_into().ok()?);
    let count = u16::from_be_bytes(b.get(2..4)?.try_into().ok()?) as usize;
    let rt = i32::from_be_bytes([0, *b.get(4)?, *b.get(5)?, *b.get(6)?]); // 24-bit, positive in practice
    let fb_count = *b.get(7)?;
    let mut i = 8;
    // Status chunks until `count` symbols are known. Symbols: 0 lost, 1 small delta, 2 large delta.
    let mut symbols: Vec<u8> = Vec::with_capacity(count);
    while symbols.len() < count {
        let c = u16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?);
        i += 2;
        let want = count - symbols.len();
        if c & 0x8000 == 0 {
            let (sym, run) = (((c >> 13) & 3) as u8, (c & 0x1fff) as usize);
            symbols.extend(std::iter::repeat_n(sym, run.min(want)));
        } else if c & 0x4000 == 0 {
            symbols.extend((0..14.min(want)).map(|k| ((c >> (13 - k)) & 1) as u8));
        } else {
            symbols.extend((0..7.min(want)).map(|k| ((c >> (12 - 2 * k)) & 3) as u8));
        }
    }
    let mut t = rt as i64 * 64_000;
    let mut arrivals = Vec::with_capacity(count);
    for (k, &s) in symbols.iter().enumerate() {
        let seq = base_seq.wrapping_add(k as u16);
        match s {
            0 => arrivals.push((seq, None)),
            1 => {
                t += *b.get(i)? as i64 * 250;
                i += 1;
                arrivals.push((seq, Some(t)));
            }
            _ => {
                t += i16::from_be_bytes(b.get(i..i + 2)?.try_into().ok()?) as i64 * 250;
                i += 2;
                arrivals.push((seq, Some(t)));
            }
        }
    }
    Some(TransportCc { sender, media, base_seq, reference_us: rt as i64 * 64_000, fb_count, arrivals })
}

fn header(fmt: u8, pt: u8, words: usize) -> [u8; 4] {
    let l = (words as u16).to_be_bytes();
    [0x80 | fmt, pt, l[0], l[1]]
}

/// Generic NACK for the given lost sequence numbers (sorted and de-duplicated here, packed into PID/BLP pairs).
pub fn nack(sender: u32, media: u32, lost: &[u16]) -> Vec<u8> {
    let mut s = lost.to_vec();
    s.sort_unstable();
    s.dedup();
    let mut pairs: Vec<(u16, u16)> = Vec::new();
    for q in s {
        match pairs.last_mut() {
            Some((pid, blp)) if q.wrapping_sub(*pid) >= 1 && q.wrapping_sub(*pid) <= 16 => *blp |= 1 << (q.wrapping_sub(*pid) - 1),
            _ => pairs.push((q, 0)),
        }
    }
    let mut v = header(1, PT_RTPFB, 2 + pairs.len()).to_vec();
    v.extend_from_slice(&sender.to_be_bytes());
    v.extend_from_slice(&media.to_be_bytes());
    for (pid, blp) in pairs {
        v.extend_from_slice(&pid.to_be_bytes());
        v.extend_from_slice(&blp.to_be_bytes());
    }
    v
}

pub fn pli(sender: u32, media: u32) -> Vec<u8> {
    let mut v = header(1, PT_PSFB, 2).to_vec();
    v.extend_from_slice(&sender.to_be_bytes());
    v.extend_from_slice(&media.to_be_bytes());
    v
}

pub fn fir(sender: u32, media: u32, seq: u8) -> Vec<u8> {
    let mut v = header(4, PT_PSFB, 4).to_vec();
    v.extend_from_slice(&sender.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(&media.to_be_bytes());
    v.extend_from_slice(&[seq, 0, 0, 0]);
    v
}

pub fn remb(sender: u32, bitrate: u64, ssrcs: &[u32]) -> Vec<u8> {
    let mut exp = 0u8;
    while bitrate >> exp > 0x3ffff {
        exp += 1;
    }
    let mantissa = (bitrate >> exp) as u32;
    let mut v = header(15, PT_PSFB, 4 + ssrcs.len()).to_vec();
    v.extend_from_slice(&sender.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"REMB");
    v.push(ssrcs.len() as u8);
    v.push(exp << 2 | (mantissa >> 16) as u8);
    v.extend_from_slice(&(mantissa as u16).to_be_bytes());
    for s in ssrcs {
        v.extend_from_slice(&s.to_be_bytes());
    }
    v
}

/// Receiver report with one block, for the streams we watch.
pub fn receiver_report(sender: u32, b: &ReportBlock) -> Vec<u8> {
    let mut v = header(1, PT_RR, 7).to_vec();
    v.extend_from_slice(&sender.to_be_bytes());
    v.extend_from_slice(&b.ssrc.to_be_bytes());
    v.push(b.fraction_lost);
    v.extend_from_slice(&b.cumulative_lost.to_be_bytes()[1..]);
    for w in [b.highest_seq, b.jitter, b.last_sr, b.delay_since_sr] {
        v.extend_from_slice(&w.to_be_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nack_round_trips_with_gaps_and_wrap() {
        let lost = [65534u16, 65535, 0, 3, 17, 18, 40, 1000];
        let p = nack(1, 2, &lost);
        let Rtcp::Nack { sender, media, seqs } = &parse(&p)[0] else { panic!() };
        assert_eq!((*sender, *media), (1, 2));
        let mut got = seqs.clone();
        got.sort_unstable();
        let mut want = lost.to_vec();
        want.sort_unstable();
        assert_eq!(got, want);
        // Sorting breaks the 65534/0 wrap into its own pair, which is still correct, just one pair longer.
        assert_eq!(p.len(), 12 + 4 * 5);
    }

    #[test]
    fn pli_and_fir() {
        assert_eq!(parse(&pli(5, 6)), vec![Rtcp::Pli { sender: 5, media: 6 }]);
        assert_eq!(parse(&fir(5, 6, 9)), vec![Rtcp::Fir { sender: 5, entries: vec![(6, 9)] }]);
    }

    #[test]
    fn remb_exponent_mantissa() {
        for bps in [30_000u64, 2_500_000, 8_000_000, 100_000_000] {
            let Rtcp::Remb { bitrate, ssrcs, .. } = parse(&remb(1, bps, &[7, 8])).remove(0) else { panic!() };
            assert_eq!(ssrcs, vec![7, 8]);
            assert!(bitrate <= bps && bps - bitrate <= bps / 100, "{bps} -> {bitrate}");
        }
    }

    #[test]
    fn compound_sr_rr_with_blocks() {
        let blk = ReportBlock { ssrc: 9, fraction_lost: 64, cumulative_lost: 123, highest_seq: 5000, jitter: 7, last_sr: 0x1000_0000, delay_since_sr: 0x0001_0000 };
        let mut c = receiver_report(3, &blk);
        c.extend(pli(3, 9));
        let r = parse(&c);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], Rtcp::ReceiverReport { ssrc: 3, blocks: vec![blk.clone()] });
        assert!((blk.loss() - 0.25).abs() < 1e-6);
        // 1.5 s after the SR was sent, minus 1 s of receiver delay: 0.5 s.
        let rtt = blk.rtt_ms(0x1000_0000 + 0x0001_0000 + 0x8000).unwrap();
        assert!((rtt - 500.0).abs() < 1.0);

        // Sender report built by hand.
        let mut sr = vec![0x80, 200, 0, 6];
        sr.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 6]);
        let Rtcp::SenderReport { ssrc, ntp, rtp_ts, packets, octets, blocks } = parse(&sr).remove(0) else { panic!() };
        assert_eq!((ssrc, ntp, rtp_ts, packets, octets, blocks.len()), (1, 2 << 32 | 3, 4, 5, 6, 0));
    }

    #[test]
    fn truncated_packets_are_ignored() {
        let p = pli(1, 2);
        assert!(parse(&p[..p.len() - 1]).is_empty());
        assert!(parse(&[]).is_empty());
        let mut c = pli(1, 2);
        c.extend_from_slice(&[0x81, 206, 0, 9, 0]);
        assert_eq!(parse(&c).len(), 1);
    }

    #[test]
    fn transport_cc_decodes_runs_and_vectors() {
        // 8 packets: seq 100..108. Run chunk: 2 received-small; status vector (2-bit): lost, small, large, small, lost.. ->
        let mut b = Vec::new();
        b.extend_from_slice(&100u16.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&[0, 0, 2, 7]); // reference = 2 * 64 ms, fb count 7
        b.extend_from_slice(&(0x0000u16 | 1 << 13 | 2).to_be_bytes()); // run: small x2
        // 2-bit vector of 6 symbols: lost, small, large, small, lost, small (7 slots, last unused = 0)
        let v: u16 = 0xC000 | (0 << 12) | (1 << 10) | (2 << 8) | (1 << 6) | (0 << 4) | (1 << 2);
        b.extend_from_slice(&v.to_be_bytes());
        b.extend_from_slice(&[4, 8]); // deltas for first two small: 1 ms, 2 ms
        // small(seq 101? order): symbols = S S L S Lg S Lt... decode handles order; supply deltas in order of received.
        // received after run: small(seq 103), large(seq 104), small(seq 105), small(seq 107)... count is 8 so 6 more symbols
        b.extend_from_slice(&[4]); // seq 103 (symbol idx 3 -> small)
        b.extend_from_slice(&(-8i16).to_be_bytes()); // large: -2 ms
        b.extend_from_slice(&[4, 4]);
        let mut p = header(15, PT_RTPFB, 2 + b.len().div_ceil(4)).to_vec();
        p.extend_from_slice(&1u32.to_be_bytes());
        p.extend_from_slice(&2u32.to_be_bytes());
        p.extend_from_slice(&b);
        while p.len() % 4 != 0 {
            p.push(0);
        }
        let n = p.len() / 4 - 1;
        p[2..4].copy_from_slice(&(n as u16).to_be_bytes());
        let Rtcp::TransportCc(t) = parse(&p).remove(0) else { panic!() };
        assert_eq!((t.base_seq, t.fb_count, t.reference_us), (100, 7, 128_000));
        assert_eq!(t.arrivals.len(), 8);
        assert_eq!(t.arrivals[0], (100, Some(129_000)));
        assert_eq!(t.arrivals[1], (101, Some(131_000)));
        assert_eq!(t.arrivals[2], (102, None));
        assert!(t.arrivals[3].1.is_some());
        assert_eq!(t.arrivals[4].1, Some(t.arrivals[3].1.unwrap() - 2_000));
        assert!(t.arrivals.iter().filter(|a| a.1.is_none()).count() >= 2);
    }
}
