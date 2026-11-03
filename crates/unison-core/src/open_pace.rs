//! Human cadence for the "latest page" reads that opening a channel starts (ARCHITECTURE 5b.3).
//!
//! Two fetches for different channels are never closer than `GAP`. A read that comes sooner waits
//! out only what is left of the gap, not a whole new one, so clicking through uncached channels
//! costs one gap in all and not one gap per click. A preload (the press that precedes a click)
//! never waits: it is skipped when the gap has not passed, and the click's own open takes over.

use std::time::{Duration, Instant};

pub const GAP: Duration = Duration::from_millis(400);

#[derive(Debug, Default)]
pub struct OpenPacer {
    last_fetch: Option<Instant>,
}

impl OpenPacer {
    /// How long a fetch started at `now` must wait for the gap since the previous one.
    pub fn wait(&self, now: Instant) -> Duration {
        self.last_fetch.map_or(Duration::ZERO, |t| (t + GAP).saturating_duration_since(now))
    }

    /// A fetch was sent at `now`.
    pub fn started(&mut self, now: Instant) {
        self.last_fetch = Some(now);
    }

    /// A preload may start now: nothing to wait for.
    pub fn may_preload(&self, now: Instant) -> bool {
        self.wait(now).is_zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fetch_and_fetch_after_a_pause_do_not_wait() {
        let t0 = Instant::now();
        let mut p = OpenPacer::default();
        assert_eq!(p.wait(t0), Duration::ZERO);
        p.started(t0);
        assert_eq!(p.wait(t0 + GAP), Duration::ZERO);
        assert_eq!(p.wait(t0 + GAP * 3), Duration::ZERO);
    }

    #[test]
    fn a_fast_second_open_waits_only_for_the_rest_of_the_gap() {
        let t0 = Instant::now();
        let mut p = OpenPacer::default();
        p.started(t0);
        assert_eq!(p.wait(t0 + Duration::from_millis(300)), Duration::from_millis(100));
        assert_eq!(p.wait(t0 + Duration::from_millis(10)), Duration::from_millis(390));
    }

    #[test]
    fn fetches_are_never_closer_than_the_gap() {
        // Clicks 300 ms apart: each fetch lands one full gap after the previous.
        let t0 = Instant::now();
        let mut p = OpenPacer::default();
        let mut sent = Vec::new();
        for i in 0..5u32 {
            let click = t0 + Duration::from_millis(300) * i;
            let at = click + p.wait(click);
            p.started(at);
            sent.push(at);
        }
        for w in sent.windows(2) {
            assert!(w[1] - w[0] >= GAP, "{:?}", sent);
        }
    }

    #[test]
    fn a_preload_is_skipped_inside_the_gap() {
        let t0 = Instant::now();
        let mut p = OpenPacer::default();
        assert!(p.may_preload(t0));
        p.started(t0);
        assert!(!p.may_preload(t0 + Duration::from_millis(100)));
        assert!(p.may_preload(t0 + GAP));
    }
}
