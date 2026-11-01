//! RTCP for the video path: parsing sender/receiver reports, generic NACK (RFC 4585), PLI, FIR
//! (RFC 5104), REMB and transport-wide congestion control feedback, and building the packets we
//! send. Operates on decrypted plaintext; the `_rtpsize` transport modes keep the 8-byte RTCP
//! header in the clear and encrypt the rest, which [`crate::voice::crypto`] undoes first.

const PT_SR: u8 = 200;
const PT_RR: u8 = 201;
const PT_SDES: u8 = 202;
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

/// Seconds between 1900 and 1970, for NTP timestamps.
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// The 64-bit NTP timestamp (seconds, fraction) of a Unix time in milliseconds.
pub fn ntp_from_unix_ms(ms: u64) -> (u32, u32) {
    let secs = ms / 1000 + NTP_UNIX_OFFSET;
    let frac = (((ms % 1000) << 32) / 1000) as u32;
    (secs as u32, frac)
}

/// A sender report with no report blocks (28 bytes): who we are, what time it is, and how much we sent.
pub fn sender_report(ssrc: u32, unix_ms: u64, rtp_ts: u32, packets: u32, octets: u32) -> Vec<u8> {
    let (sec, frac) = ntp_from_unix_ms(unix_ms);
    let mut v = header(0, PT_SR, 6).to_vec();
    for w in [ssrc, sec, frac, rtp_ts, packets, octets] {
        v.extend_from_slice(&w.to_be_bytes());
    }
    v
}

/// An SDES packet with one chunk holding the CNAME (item 1), terminated and padded to 32 bits.
pub fn sdes_cname(ssrc: u32, cname: &str) -> Vec<u8> {
    let text = &cname.as_bytes()[..cname.len().min(255)];
    let mut chunk = ssrc.to_be_bytes().to_vec();
    chunk.push(1);
    chunk.push(text.len() as u8);
    chunk.extend_from_slice(text);
    chunk.push(0);
    while chunk.len() % 4 != 0 {
        chunk.push(0);
    }
    let mut v = header(1, PT_SDES, chunk.len() / 4).to_vec();
    v.extend_from_slice(&chunk);
    v
}

/// What libwebrtc sends for a stream we are sending: a sender report followed by the SDES CNAME.
pub fn sender_compound(ssrc: u32, unix_ms: u64, rtp_ts: u32, packets: u32, octets: u32, cname: &str) -> Vec<u8> {
    let mut v = sender_report(ssrc, unix_ms, rtp_ts, packets, octets);
    v.extend_from_slice(&sdes_cname(ssrc, cname));
    v
}

/// 16 random letters and digits, as libwebrtc's default CNAME.
pub fn random_cname() -> String {
    (0..16).map(|_| fastrand::alphanumeric()).collect()
}

/// Collects arrival times of packets that carry a transport-wide sequence number and writes them
/// as transport-wide congestion control feedback (the same layout `parse_tcc` reads). Symbols are
/// written as two-bit status vectors, which is always valid; size matters little at 10 reports a second.
#[derive(Default)]
pub struct TccRecorder {
    media: u32,
    hi: Option<i64>,
    seen: std::collections::BTreeMap<i64, i64>,
    fb_count: u8,
}

impl TccRecorder {
    /// Note one packet: its transport-wide sequence number and arrival time in microseconds.
    pub fn record(&mut self, media_ssrc: u32, seq: u16, arrival_us: i64) {
        if self.seen.is_empty() {
            self.media = media_ssrc;
        }
        let hi = self.hi.unwrap_or(seq as i64);
        let ext = hi + seq.wrapping_sub(hi as u16) as i16 as i64;
        self.hi = Some(hi.max(ext));
        self.seen.entry(ext).or_insert(arrival_us);
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// The feedback packet for everything recorded since the last call; `None` when nothing arrived.
    pub fn build(&mut self, sender: u32) -> Option<Vec<u8>> {
        let last = *self.seen.keys().next_back()?;
        // A report covers at most 8000 sequence numbers: older ones are not worth reporting.
        let base = (*self.seen.keys().next()?).max(last - 8000);
        let count = (last - base + 1) as usize;
        let first_us = *self.seen.range(base..).next()?.1;
        let reference = first_us.div_euclid(64_000);
        let mut t_q = reference * 64_000;
        let mut symbols = Vec::with_capacity(count);
        let mut deltas: Vec<u8> = Vec::new();
        for k in 0..count as i64 {
            match self.seen.get(&(base + k)) {
                None => symbols.push(0u8),
                Some(&a) => {
                    let d = ((a - t_q) as f64 / 250.0).round() as i64;
                    if (0..=255).contains(&d) {
                        symbols.push(1);
                        deltas.push(d as u8);
                        t_q += d * 250;
                    } else {
                        let d = d.clamp(i16::MIN as i64, i16::MAX as i64);
                        symbols.push(2);
                        deltas.extend_from_slice(&(d as i16).to_be_bytes());
                        t_q += d * 250;
                    }
                }
            }
        }
        let mut body = Vec::new();
        body.extend_from_slice(&(base as u16).to_be_bytes());
        body.extend_from_slice(&(count as u16).to_be_bytes());
        body.extend_from_slice(&(((reference & 0xff_ffff) as u32) << 8 | self.fb_count as u32).to_be_bytes());
        for chunk in symbols.chunks(7) {
            let mut c = 0xC000u16;
            for (k, s) in chunk.iter().enumerate() {
                c |= (*s as u16) << (12 - 2 * k);
            }
            body.extend_from_slice(&c.to_be_bytes());
        }
        body.extend_from_slice(&deltas);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        let mut v = header(15, PT_RTPFB, (12 + body.len()) / 4 - 1).to_vec();
        v.extend_from_slice(&sender.to_be_bytes());
        v.extend_from_slice(&self.media.to_be_bytes());
        v.extend_from_slice(&body);
        self.fb_count = self.fb_count.wrapping_add(1);
        self.seen.clear();
        Some(v)
    }
}

/// Report cadence: libwebrtc's defaults are 1 s for video and 5 s for audio (its
/// `kDefaultVideoReportInterval` and `kDefaultAudioReportInterval`; an inference, the module's
/// own values were not decoded). Driven by the caller's clock so it can be tested with a fake one.
#[derive(Debug, Clone, Copy)]
pub struct Cadence {
    interval_ms: u64,
    next: Option<u64>,
}

impl Cadence {
    pub const VIDEO_MS: u64 = 1000;
    pub const AUDIO_MS: u64 = 5000;

    pub fn new(interval_ms: u64) -> Self {
        Self { interval_ms, next: None }
    }

    /// True once per interval; the first call only starts the clock (nothing is sent the instant a
    /// stream starts).
    pub fn due(&mut self, now_ms: u64) -> bool {
        match self.next {
            None => {
                self.next = Some(now_ms + self.interval_ms);
                false
            }
            Some(t) if now_ms >= t => {
                self.next = Some(now_ms + self.interval_ms);
                true
            }
            _ => false,
        }
    }
}

impl Default for Cadence {
    fn default() -> Self {
        Self::new(Self::VIDEO_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_cc_feedback_round_trips_with_losses_and_wrap() {
        let mut r = TccRecorder::default();
        // 65534, 65535, (0 lost), 1 across the wrap; the second arrives out of order; one gap of 300 ms.
        r.record(77, 65535, 1_100_000);
        r.record(77, 65534, 1_000_000);
        r.record(77, 1, 1_400_000);
        let fb = r.build(5).unwrap();
        assert_eq!(fb.len() % 4, 0);
        assert_eq!(u16::from_be_bytes([fb[2], fb[3]]) as usize, fb.len() / 4 - 1);
        let Rtcp::TransportCc(t) = &parse(&fb)[0] else { panic!("not transport-cc") };
        assert_eq!((t.sender, t.media, t.base_seq, t.fb_count), (5, 77, 65534, 0));
        let seqs: Vec<u16> = t.arrivals.iter().map(|a| a.0).collect();
        assert_eq!(seqs, vec![65534, 65535, 0, 1]);
        let at: Vec<Option<i64>> = t.arrivals.iter().map(|a| a.1).collect();
        assert_eq!(at, vec![Some(1_000_000), Some(1_100_000), None, Some(1_400_000)]);
        assert!(r.build(5).is_none(), "nothing new, nothing sent");
        r.record(77, 2, 1_500_000);
        let Rtcp::TransportCc(t) = &parse(&r.build(5).unwrap())[0] else { panic!() };
        assert_eq!((t.fb_count, t.base_seq), (1, 2));
    }

    #[test]
    fn sender_report_and_sdes_are_byte_exact() {
        // 2020-01-01T00:00:00.500Z.
        let sr = sender_report(0x01020304, 1_577_836_800_500, 0x11223344, 7, 900);
        assert_eq!(sr.len(), 28);
        assert_eq!(&sr[..8], &[0x80, 200, 0, 6, 1, 2, 3, 4]);
        assert_eq!(u32::from_be_bytes(sr[8..12].try_into().unwrap()), 1_577_836_800 + 2_208_988_800);
        assert_eq!(u32::from_be_bytes(sr[12..16].try_into().unwrap()), 0x8000_0000, "half a second");
        assert_eq!(&sr[16..28], &[0x11, 0x22, 0x33, 0x44, 0, 0, 0, 7, 0, 0, 3, 0x84]);
        let sd = sdes_cname(0x01020304, "abcde");
        assert_eq!(sd, [0x81, 202, 0, 3, 1, 2, 3, 4, 1, 5, b'a', b'b', b'c', b'd', b'e', 0, 0, 0, 0, 0][..sd.len()].to_vec());
        assert_eq!(sd.len() % 4, 0);
        assert_eq!(u16::from_be_bytes([sd[2], sd[3]]) as usize * 4 + 4, sd.len());
        let c = sender_compound(1, 0, 0, 0, 0, "x");
        assert_eq!(c.len(), 28 + sdes_cname(1, "x").len());
        // Our own parser takes both halves.
        assert_eq!(parse(&c).len(), 2);
    }

    #[test]
    fn cadence_fires_once_per_interval_on_a_fake_clock() {
        let mut c = Cadence::new(1000);
        assert!(!c.due(5000), "starting the clock sends nothing");
        assert!(!c.due(5999));
        assert!(c.due(6000));
        assert!(!c.due(6500));
        assert!(c.due(8000), "a late tick fires once, not twice");
        assert!(!c.due(8001));
        assert_eq!(random_cname().len(), 16);
    }

    #[test]
    fn garbage_never_panics_the_parser() {
        let mut x = 0x1234_5678u32;
        for n in 0..2000usize {
            let len = n % 90;
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = parse(&junk);
            // And a plausible header with random contents behind it.
            let mut p = vec![0x8f, 205, 0, (len / 4) as u8];
            p.extend_from_slice(&junk);
            let _ = parse(&p);
        }
    }

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
