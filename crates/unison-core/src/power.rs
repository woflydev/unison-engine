//! One power state drives batching and work everywhere in the core.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// Focused and interacting.
    Active,
    /// On screen but idle.
    Visible,
    /// Hidden, minimised or fully covered.
    Background,
    /// On battery with Low Power Mode or a thermal warning.
    LowPower,
}

impl PowerState {
    /// How long deltas may be coalesced before being pushed to the UI.
    /// `None` means hold them until the window becomes visible again.
    pub fn delta_flush(self) -> Option<Duration> {
        match self {
            PowerState::Active => Some(Duration::from_millis(8)),
            PowerState::Visible => Some(Duration::from_millis(100)),
            PowerState::LowPower => Some(Duration::from_millis(250)),
            PowerState::Background => None,
        }
    }

    /// Whether typing/presence churn should reach the UI at all.
    pub fn wants_ephemeral(self) -> bool {
        matches!(self, PowerState::Active | PowerState::Visible)
    }

    /// Debounce for writing the cache to disk.
    pub fn cache_flush(self) -> Duration {
        match self {
            PowerState::Active | PowerState::Visible => Duration::from_secs(5),
            PowerState::LowPower => Duration::from_secs(15),
            PowerState::Background => Duration::from_secs(60),
        }
    }
}

/// Coalesces deltas and decides when they may reach the UI. Pure (the caller passes the clock),
/// so the power-state matrix is unit-tested without a runtime.
#[derive(Debug)]
pub struct Batcher<T: PartialEq> {
    pending: Vec<T>,
    flush_at: Option<Instant>,
}

impl<T: PartialEq> Default for Batcher<T> {
    fn default() -> Self {
        Self { pending: Vec::new(), flush_at: None }
    }
}

impl<T: PartialEq> Batcher<T> {
    /// Queue `items`; typing/presence churn (`ephemeral`) is dropped when the state does not want it.
    pub fn push(&mut self, now: Instant, power: PowerState, items: impl IntoIterator<Item = T>, ephemeral: impl Fn(&T) -> bool) {
        let wants = power.wants_ephemeral();
        for d in items {
            if !wants && ephemeral(&d) {
                continue;
            }
            if !self.pending.contains(&d) {
                self.pending.push(d);
            }
        }
        if self.flush_at.is_none() && !self.pending.is_empty() {
            if let Some(after) = power.delta_flush() {
                self.flush_at = Some(now + after);
            }
        }
    }

    /// When the next flush is due; `None` means nothing may wake the process for this.
    pub fn deadline(&self) -> Option<Instant> {
        self.flush_at
    }

    /// The batch to emit if the flush deadline has passed.
    pub fn take_due(&mut self, now: Instant) -> Option<Vec<T>> {
        match self.flush_at {
            Some(at) if at <= now => {
                self.flush_at = None;
                (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
            }
            _ => None,
        }
    }

    /// A power change: hidden cancels any armed timer (no wake-up while backgrounded); becoming
    /// visible delivers everything held at once and disarms the timer.
    pub fn on_power(&mut self, power: PowerState) -> Option<Vec<T>> {
        self.flush_at = None;
        if power.delta_flush().is_none() || self.pending.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.pending))
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Lets the first update of a key through at once and holds the rest to one per `window`: a busy
/// message in a big server gets dozens of reactions a second, and rebuilding its row for each is work
/// nobody can see. Pure (the caller passes the clock), like `Batcher`.
#[derive(Debug)]
pub struct Throttle<K: Eq + Hash + Clone> {
    window: Duration,
    last: HashMap<K, Instant>,
    held: Vec<K>,
}

impl<K: Eq + Hash + Clone> Throttle<K> {
    pub fn new(window: Duration) -> Self {
        Self { window, last: HashMap::new(), held: Vec::new() }
    }

    /// True when `key` may go out now (its first update, or the window has passed). Otherwise it is held
    /// and comes back from `take_due` once, however many updates arrive meanwhile.
    pub fn offer(&mut self, now: Instant, key: K) -> bool {
        if self.last.len() > 512 {
            let window = self.window;
            self.last.retain(|_, t| now.duration_since(*t) < window);
        }
        match self.last.get(&key) {
            Some(t) if now.duration_since(*t) < self.window => {
                if !self.held.contains(&key) {
                    self.held.push(key);
                }
                false
            }
            _ => {
                self.held.retain(|k| k != &key);
                self.last.insert(key, now);
                true
            }
        }
    }

    /// When the next held key is due; `None` means no timer is needed.
    pub fn deadline(&self) -> Option<Instant> {
        self.held.iter().filter_map(|k| self.last.get(k)).map(|t| *t + self.window).min()
    }

    /// The held keys whose window has passed (each counts as sent now).
    pub fn take_due(&mut self, now: Instant) -> Vec<K> {
        let (due, still): (Vec<K>, Vec<K>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|k| self.last.get(k).is_none_or(|t| *t + self.window <= now));
        self.held = still;
        for k in &due {
            self.last.insert(k.clone(), now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [PowerState; 4] = [PowerState::Active, PowerState::Visible, PowerState::LowPower, PowerState::Background];
    fn eph(d: &&str) -> bool {
        d.starts_with("typing")
    }

    #[test]
    fn flush_cadence_matrix() {
        let t0 = Instant::now();
        for (p, ms) in [(PowerState::Active, 8), (PowerState::Visible, 100), (PowerState::LowPower, 250)] {
            let mut b = Batcher::default();
            b.push(t0, p, ["msg"], eph);
            assert_eq!(b.deadline(), Some(t0 + Duration::from_millis(ms)), "{p:?}");
            assert!(b.take_due(t0 + Duration::from_millis(ms - 1)).is_none(), "{p:?} early");
            assert_eq!(b.take_due(t0 + Duration::from_millis(ms)), Some(vec!["msg"]), "{p:?}");
            assert_eq!(b.deadline(), None, "{p:?} disarms after flush");
        }
    }

    #[test]
    fn coalesces_within_one_window() {
        let t0 = Instant::now();
        let mut b = Batcher::default();
        b.push(t0, PowerState::Visible, ["a", "b"], eph);
        b.push(t0 + Duration::from_millis(50), PowerState::Visible, ["a", "c"], eph);
        // A later push does not slide the deadline.
        assert_eq!(b.deadline(), Some(t0 + Duration::from_millis(100)));
        assert_eq!(b.take_due(t0 + Duration::from_millis(100)), Some(vec!["a", "b", "c"]));
    }

    #[test]
    fn background_never_arms_a_timer() {
        let t0 = Instant::now();
        let mut b = Batcher::default();
        for i in 0..1000 {
            b.push(t0 + Duration::from_secs(i), PowerState::Background, ["msg", "typing"], eph);
            assert_eq!(b.deadline(), None, "background must not wake the process");
        }
        assert_eq!(b.len(), 1, "ephemeral dropped, duplicates coalesced");
        assert!(b.take_due(t0 + Duration::from_secs(10_000)).is_none());
    }

    #[test]
    fn going_background_cancels_an_armed_timer() {
        let t0 = Instant::now();
        let mut b = Batcher::default();
        b.push(t0, PowerState::Visible, ["msg"], eph);
        assert!(b.deadline().is_some());
        assert!(b.on_power(PowerState::Background).is_none());
        assert_eq!(b.deadline(), None);
        assert_eq!(b.len(), 1, "held, not lost");
        // Back on screen: everything held is delivered at once and no stale timer remains.
        assert_eq!(b.on_power(PowerState::Active), Some(vec!["msg"]));
        assert_eq!(b.deadline(), None);
    }

    #[test]
    fn ephemeral_only_in_visible_states() {
        let t0 = Instant::now();
        for p in ALL {
            let mut b = Batcher::default();
            b.push(t0, p, ["typing"], eph);
            assert_eq!(b.len(), usize::from(p.wants_ephemeral()), "{p:?}");
        }
    }

    #[test]
    fn cache_flush_backs_off_with_power() {
        assert!(PowerState::Active.cache_flush() <= PowerState::LowPower.cache_flush());
        assert!(PowerState::LowPower.cache_flush() < PowerState::Background.cache_flush());
        assert_eq!(PowerState::Visible.cache_flush(), PowerState::Active.cache_flush());
    }

    #[test]
    fn only_background_holds_deltas() {
        for p in ALL {
            assert_eq!(p.delta_flush().is_none(), p == PowerState::Background, "{p:?}");
        }
    }

    #[test]
    fn throttle_sends_first_at_once_then_one_per_window() {
        let t0 = Instant::now();
        let w = Duration::from_millis(250);
        let mut t = Throttle::new(w);
        assert!(t.offer(t0, "a"), "first goes out at once");
        assert!(!t.offer(t0 + Duration::from_millis(10), "a"));
        assert!(!t.offer(t0 + Duration::from_millis(20), "a"));
        assert!(t.offer(t0 + Duration::from_millis(20), "b"), "another key is independent");
        assert_eq!(t.deadline(), Some(t0 + w));
        assert!(t.take_due(t0 + Duration::from_millis(249)).is_empty());
        assert_eq!(t.take_due(t0 + w), vec!["a"], "one trailing update for any number held");
        assert_eq!(t.deadline(), None);
        assert!(!t.offer(t0 + w + Duration::from_millis(5), "a"), "the trailing send starts a new window");
        assert!(t.offer(t0 + w * 3, "a"), "a quiet key goes out at once again");
    }
}
