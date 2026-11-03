//! A message on its way out: which stage it is in, how many bytes of its files are through, how
//! fast, and how long is left. The session updates it as the upload runs and every shell draws the
//! same progress from it. It lives in memory only; nothing is persisted.

use std::time::{Duration, Instant};

use crate::model::Id;
use crate::snowflake::Snowflake;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// File bytes are going to Discord's storage.
    Uploading,
    /// Waiting for Discord to accept the message itself.
    Sending,
    /// Stopped: `error` says why (`None` when the user cancelled). Stays until dismissed so the
    /// shell can give the text and files back.
    Failed,
}

#[derive(Debug, Clone)]
pub struct FileProgress {
    pub name: String,
    pub total: u64,
    pub sent: u64,
}

#[derive(Debug, Clone)]
pub struct SendState {
    pub nonce: Snowflake,
    pub channel: Id,
    pub phase: Phase,
    pub files: Vec<FileProgress>,
    pub error: Option<String>,
    /// The message text, so a failed send can be shown and put back.
    pub text: String,
    /// Sending it again can work (a connection problem, not a refusal that needs an edit).
    pub can_retry: bool,
    /// Smoothed upload speed in bytes per second (0 until measured).
    rate: f64,
    sample_at: Instant,
    sample_sent: u64,
}

/// How often the speed is re-measured. Shorter windows make the ETA jump around.
const SAMPLE: Duration = Duration::from_millis(400);

/// Upload progress gathered between updates. A 256 KiB piece lands many times a second on a fast link; the
/// store is written, and the shells told, about ten times a second with what came in meanwhile.
#[derive(Debug)]
pub struct ProgressBatch {
    pending: Vec<u64>,
    last: Instant,
}

impl ProgressBatch {
    pub fn new(files: usize, now: Instant) -> Self {
        Self { pending: vec![0; files], last: now }
    }

    /// Note `bytes` through for file `index`. Returns what to apply when a tenth of a second has passed.
    pub fn push(&mut self, index: usize, bytes: u64, now: Instant) -> Option<Vec<u64>> {
        if let Some(p) = self.pending.get_mut(index) {
            *p += bytes;
        }
        if now.saturating_duration_since(self.last) < Duration::from_millis(100) {
            return None;
        }
        self.last = now;
        Some(self.pending.iter_mut().map(std::mem::take).collect())
    }
}

impl SendState {
    pub fn new(nonce: Snowflake, channel: Id, files: Vec<(String, u64)>, now: Instant) -> Self {
        let uploading = !files.is_empty();
        Self {
            nonce,
            channel,
            phase: if uploading { Phase::Uploading } else { Phase::Sending },
            files: files.into_iter().map(|(name, total)| FileProgress { name, total, sent: 0 }).collect(),
            error: None,
            text: String::new(),
            can_retry: false,
            rate: 0.0,
            sample_at: now,
            sample_sent: 0,
        }
    }

    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.total).sum()
    }

    pub fn sent(&self) -> u64 {
        self.files.iter().map(|f| f.sent).sum()
    }

    /// 0.0 to 1.0 of all file bytes (0 when there are no files).
    pub fn fraction(&self) -> f32 {
        match self.total() {
            0 => 0.0,
            t => (self.sent() as f64 / t as f64).min(1.0) as f32,
        }
    }

    /// The file being sent now: the first that isn't through.
    pub fn current(&self) -> Option<&FileProgress> {
        self.files.iter().find(|f| f.sent < f.total).or(self.files.last())
    }

    /// Bytes through for file `index`; re-measures the speed once a sample window has passed.
    pub fn add(&mut self, index: usize, bytes: u64, now: Instant) {
        if let Some(f) = self.files.get_mut(index) {
            f.sent = (f.sent + bytes).min(f.total);
        }
        let dt = now.saturating_duration_since(self.sample_at);
        if dt >= SAMPLE {
            let sent = self.sent();
            let instant = (sent - self.sample_sent) as f64 / dt.as_secs_f64();
            self.rate = if self.rate == 0.0 { instant } else { 0.7 * self.rate + 0.3 * instant };
            self.sample_at = now;
            self.sample_sent = sent;
        }
    }

    pub fn bytes_per_sec(&self) -> u64 {
        self.rate as u64
    }

    /// Seconds left at the current speed; `None` until there is a speed to go by or when not uploading.
    pub fn eta_secs(&self) -> Option<u32> {
        if self.phase != Phase::Uploading || self.rate < 1024.0 {
            return None;
        }
        Some(((self.total() - self.sent()) as f64 / self.rate).ceil() as u32)
    }

    /// All bytes are through: from here the wait is on Discord.
    pub fn finish_upload(&mut self) {
        for f in &mut self.files {
            f.sent = f.total;
        }
        self.phase = Phase::Sending;
    }

    pub fn fail(&mut self, error: Option<String>, can_retry: bool) {
        self.phase = Phase::Failed;
        self.error = error;
        self.can_retry = can_retry;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(files: &[u64]) -> (SendState, Instant) {
        let t0 = Instant::now();
        let f = files.iter().enumerate().map(|(i, s)| (format!("f{i}"), *s)).collect();
        (SendState::new(Snowflake(1), Snowflake(2), f, t0), t0)
    }

    #[test]
    fn progress_is_applied_in_tenths_of_a_second_without_losing_bytes() {
        let t0 = Instant::now();
        let mut b = ProgressBatch::new(2, t0);
        assert_eq!(b.push(0, 100, t0 + Duration::from_millis(10)), None);
        assert_eq!(b.push(1, 50, t0 + Duration::from_millis(60)), None);
        assert_eq!(b.push(0, 25, t0 + Duration::from_millis(120)), Some(vec![125, 50]));
        // What was handed out is not handed out again.
        assert_eq!(b.push(1, 5, t0 + Duration::from_millis(130)), None);
        assert_eq!(b.push(1, 5, t0 + Duration::from_millis(240)), Some(vec![0, 10]));
        // A file index that doesn't exist is ignored, not a panic.
        assert_eq!(b.push(9, 1, t0 + Duration::from_millis(400)), Some(vec![0, 0]));
    }

    #[test]
    fn text_only_sends_start_in_sending() {
        let (s, _) = state(&[]);
        assert_eq!(s.phase, Phase::Sending);
        assert_eq!((s.fraction(), s.eta_secs()), (0.0, None));
    }

    #[test]
    fn progress_adds_up_across_files_and_is_capped() {
        let (mut s, t0) = state(&[100, 300]);
        s.add(0, 100, t0);
        assert_eq!(s.fraction(), 0.25);
        assert_eq!(s.current().unwrap().name, "f1");
        s.add(1, 5000, t0);
        assert_eq!((s.sent(), s.fraction()), (400, 1.0), "never past the total");
        s.add(9, 10, t0); // an unknown file index is ignored
        assert_eq!(s.sent(), 400);
    }

    #[test]
    fn speed_and_eta_are_smoothed() {
        let (mut s, t0) = state(&[10_000_000]);
        s.add(0, 1_000_000, t0 + Duration::from_millis(100));
        assert_eq!(s.eta_secs(), None, "no speed yet: the window hasn't passed");
        s.add(0, 1_000_000, t0 + Duration::from_millis(500));
        // 2 MB in 0.5 s = 4 MB/s; 8 MB left = 2 s.
        assert_eq!(s.bytes_per_sec(), 4_000_000);
        assert_eq!(s.eta_secs(), Some(2));
        // A slow patch pulls the average down but not all the way.
        s.add(0, 100_000, t0 + Duration::from_millis(1000));
        assert!(s.bytes_per_sec() < 4_000_000 && s.bytes_per_sec() > 2_000_000, "{}", s.bytes_per_sec());
    }

    #[test]
    fn finishing_and_failing_change_the_phase() {
        let (mut s, t0) = state(&[1000]);
        s.add(0, 10, t0);
        s.finish_upload();
        assert_eq!((s.phase, s.fraction(), s.eta_secs()), (Phase::Sending, 1.0, None));
        s.fail(Some("nope".into()), true);
        assert_eq!((s.phase, s.error.as_deref(), s.can_retry), (Phase::Failed, Some("nope"), true));
        let (mut c, _) = state(&[1]);
        c.fail(None, false);
        assert!(c.phase == Phase::Failed && c.error.is_none(), "a cancel has no error text");
    }
}
