//! What a receiver owes the sender of a video stream: put packets back in order, ask for the ones
//! that went missing (NACK, answered over RTX), give up on a hole after a short wait so one lost
//! packet costs a keyframe request instead of a stall, and keep the numbers a receiver report needs.
//! Pure state driven by the caller's clock, so it is tested without sockets.

use std::collections::BTreeMap;

use crate::voice::rtcp::ReportBlock;

/// How long a hole may stay open before we skip it and ask for a keyframe.
pub const MAX_WAIT_MS: u64 = 160;
/// Gap between two NACKs for the same packet.
const RETRY_MS: u64 = 30;
const MAX_TRIES: u8 = 6;
/// A jump this large is a restarted stream, not loss: start over.
const RESET_GAP: i64 = 1000;
/// More than this many packets waiting behind a hole means the hole is not coming back.
const MAX_HELD: usize = 512;

pub struct Packet {
    pub payload: Vec<u8>,
    pub marker: bool,
    pub timestamp: u32,
}

struct Hole {
    since_ms: u64,
    last_nack_ms: Option<u64>,
    tries: u8,
}

/// Releases packets in sequence order. A packet that arrives late (after its hole was skipped) is dropped.
#[derive(Default)]
pub struct Reorder {
    next: Option<i64>,
    hi: i64,
    held: BTreeMap<i64, Packet>,
    holes: BTreeMap<i64, Hole>,
    /// Packets we gave up on, for the log.
    pub skipped: u32,
}

impl Reorder {
    fn unwrap(&self, seq: u16) -> i64 {
        self.hi + seq.wrapping_sub(self.hi as u16) as i16 as i64
    }

    /// Accept one packet (an original or an RTX copy); returns what can now be released, in order.
    pub fn push(&mut self, seq: u16, pkt: Packet, now_ms: u64) -> Vec<(u16, Packet)> {
        let Some(next) = self.next else {
            self.next = Some(seq as i64);
            self.hi = seq as i64;
            self.held.insert(seq as i64, pkt);
            return self.drain(now_ms);
        };
        let ext = self.unwrap(seq);
        if ext < next {
            return Vec::new(); // late or duplicate
        }
        if ext - self.hi > RESET_GAP {
            self.held.clear();
            self.holes.clear();
            self.next = Some(ext);
            self.hi = ext;
        } else if ext > self.hi {
            for m in self.hi + 1..ext {
                self.holes.entry(m).or_insert(Hole { since_ms: now_ms, last_nack_ms: None, tries: 0 });
            }
            self.hi = ext;
        }
        self.holes.remove(&ext);
        self.held.insert(ext, pkt);
        self.drain(now_ms)
    }

    /// Release anything a timeout has unblocked. Call regularly, also when no packet arrived.
    pub fn drain(&mut self, now_ms: u64) -> Vec<(u16, Packet)> {
        let mut out = Vec::new();
        loop {
            let Some(next) = self.next else { break };
            if let Some(p) = self.held.remove(&next) {
                out.push((next as u16, p));
                self.next = Some(next + 1);
                continue;
            }
            let Some((&first, _)) = self.held.iter().next() else { break };
            let waited = self.holes.get(&next).is_some_and(|h| now_ms.saturating_sub(h.since_ms) >= MAX_WAIT_MS);
            if waited || self.held.len() > MAX_HELD {
                self.skipped += (first - next) as u32;
                self.holes.retain(|&s, _| s >= first);
                self.next = Some(first);
                continue;
            }
            break;
        }
        out
    }

    /// Sequence numbers worth asking for again right now.
    pub fn due_nacks(&mut self, now_ms: u64) -> Vec<u16> {
        let mut v = Vec::new();
        for (&s, h) in self.holes.iter_mut() {
            if v.len() >= 64 {
                break;
            }
            if h.tries < MAX_TRIES && h.last_nack_ms.is_none_or(|t| now_ms.saturating_sub(t) >= RETRY_MS) {
                h.tries += 1;
                h.last_nack_ms = Some(now_ms);
                v.push(s as u16);
            }
        }
        v
    }
}

/// RFC 3550 receiver statistics for one SSRC.
#[derive(Default)]
pub struct RecvStats {
    base: Option<i64>,
    max: i64,
    received: u32,
    expected_prior: i64,
    received_prior: u32,
    jitter: f64,
    transit: Option<u32>,
    last_sr: u32,
    last_sr_ms: u64,
}

impl RecvStats {
    pub fn on_packet(&mut self, seq: u16, rtp_ts: u32, now_ms: u64, clock_khz: u64) {
        let ext = match self.base {
            None => {
                self.base = Some(seq as i64);
                self.max = seq as i64;
                seq as i64
            }
            Some(_) => self.max + seq.wrapping_sub(self.max as u16) as i16 as i64,
        };
        self.max = self.max.max(ext);
        self.received += 1;
        let transit = ((now_ms * clock_khz) as u32).wrapping_sub(rtp_ts);
        if let Some(prev) = self.transit {
            let d = transit.wrapping_sub(prev) as i32;
            self.jitter += ((d as f64).abs() - self.jitter) / 16.0;
        }
        self.transit = Some(transit);
    }

    /// Remember a sender report from this SSRC (its NTP time, middle 32 bits) for RTT.
    pub fn on_sender_report(&mut self, ntp: u64, now_ms: u64) {
        self.last_sr = (ntp >> 16) as u32;
        self.last_sr_ms = now_ms;
    }

    /// The report block for the interval since the last one.
    pub fn block(&mut self, ssrc: u32, now_ms: u64) -> Option<ReportBlock> {
        let base = self.base?;
        let expected = self.max - base + 1;
        let expected_interval = expected - self.expected_prior;
        let received_interval = self.received - self.received_prior;
        let lost_interval = expected_interval - received_interval as i64;
        self.expected_prior = expected;
        self.received_prior = self.received;
        let fraction_lost = if expected_interval <= 0 || lost_interval <= 0 { 0 } else { ((lost_interval << 8) / expected_interval).min(255) as u8 };
        let delay_since_sr = if self.last_sr == 0 { 0 } else { (now_ms.saturating_sub(self.last_sr_ms) * 65536 / 1000) as u32 };
        Some(ReportBlock {
            ssrc,
            fraction_lost,
            cumulative_lost: (expected - self.received as i64).clamp(0, 0x7f_ffff) as u32,
            highest_seq: self.max as u32,
            jitter: self.jitter as u32,
            last_sr: self.last_sr,
            delay_since_sr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(n: u8) -> Packet {
        Packet { payload: vec![n], marker: false, timestamp: 0 }
    }
    fn seqs(v: &[(u16, Packet)]) -> Vec<u16> {
        v.iter().map(|x| x.0).collect()
    }

    #[test]
    fn in_order_passes_straight_through() {
        let mut r = Reorder::default();
        assert_eq!(seqs(&r.push(10, p(0), 0)), vec![10]);
        assert_eq!(seqs(&r.push(11, p(0), 1)), vec![11]);
        assert!(r.push(11, p(0), 2).is_empty(), "a duplicate is dropped");
    }

    #[test]
    fn a_hole_is_nacked_and_filled_by_the_retransmission() {
        let mut r = Reorder::default();
        r.push(65534, p(0), 0);
        assert!(r.push(1, p(0), 5).is_empty(), "held behind the missing 65535 and 0");
        assert_eq!(r.due_nacks(5), vec![65535, 0]);
        assert!(r.due_nacks(10).is_empty(), "not again within the retry gap");
        assert_eq!(r.due_nacks(40), vec![65535, 0]);
        assert_eq!(seqs(&r.push(65535, p(0), 45)), vec![65535]);
        assert_eq!(seqs(&r.push(0, p(0), 46)), vec![0, 1], "the rest is released together");
        assert!(r.due_nacks(200).is_empty());
        assert_eq!(r.skipped, 0);
    }

    #[test]
    fn a_hole_that_never_fills_is_skipped_after_the_wait() {
        let mut r = Reorder::default();
        r.push(1, p(0), 0);
        r.push(3, p(0), 10);
        assert!(r.drain(10 + MAX_WAIT_MS - 1).is_empty());
        let out = r.drain(10 + MAX_WAIT_MS);
        assert_eq!(seqs(&out), vec![3]);
        assert_eq!(r.skipped, 1);
        assert!(r.push(2, p(0), 500).is_empty(), "the late copy is too late");
    }

    #[test]
    fn a_huge_jump_restarts_instead_of_nacking_thousands() {
        let mut r = Reorder::default();
        r.push(1, p(0), 0);
        assert_eq!(seqs(&r.push(5000, p(0), 1)), vec![5000]);
        assert!(r.due_nacks(2).is_empty());
    }

    #[test]
    fn report_block_counts_loss_per_interval() {
        let mut s = RecvStats::default();
        for (i, q) in [100u16, 101, 103, 104].iter().enumerate() {
            s.on_packet(*q, 0, i as u64 * 33, 90);
        }
        s.on_sender_report(0x0001_0002_0003_0004, 1000);
        let b = s.block(9, 1500).unwrap();
        assert_eq!((b.highest_seq, b.cumulative_lost), (104, 1));
        assert_eq!(b.fraction_lost, 51, "1 of 5 is 51/256");
        assert_eq!(b.last_sr, 0x0002_0003);
        assert_eq!(b.delay_since_sr, 500 * 65536 / 1000);
        s.on_packet(105, 0, 2000, 90);
        let b = s.block(9, 2000).unwrap();
        assert_eq!((b.fraction_lost, b.cumulative_lost), (0, 1), "a clean interval reports no loss");
    }
}
