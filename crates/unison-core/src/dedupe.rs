//! Request de-duplication: the same read is never sent twice at once, nor again while the
//! answer is still fresh (docs/ARCHITECTURE.md rule 10). The official client keeps results in
//! memory and asks again only when they go stale; this is that rule, in one place.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Dedupe {
    fresh: Mutex<HashMap<String, Instant>>,
    inflight: Mutex<HashSet<String>>,
}

impl Dedupe {
    /// True when the caller should send the request: nothing for `key` is in flight and the last
    /// success is older than `ttl`. Pair every `true` with `finish`.
    pub fn begin(&self, key: &str, ttl: Duration) -> bool {
        if self.fresh.lock().get(key).is_some_and(|t| t.elapsed() < ttl) {
            return false;
        }
        self.inflight.lock().insert(key.to_string())
    }

    /// The request ended. A success starts the freshness window; a failure allows an immediate retry.
    pub fn finish(&self, key: &str, ok: bool) {
        self.inflight.lock().remove(key);
        if ok {
            let mut f = self.fresh.lock();
            f.insert(key.to_string(), Instant::now());
            if f.len() > 512 {
                f.retain(|_, t| t.elapsed() < Duration::from_secs(600));
            }
        }
    }

    /// Forget a key (the data it covered changed, e.g. you saved the note).
    pub fn forget(&self, key: &str) {
        self.fresh.lock().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_and_fresh_requests_are_dropped() {
        let d = Dedupe::default();
        let ttl = Duration::from_secs(60);
        assert!(d.begin("a", ttl));
        assert!(!d.begin("a", ttl), "in flight");
        d.finish("a", true);
        assert!(!d.begin("a", ttl), "fresh");
        assert!(d.begin("b", ttl), "other keys are independent");
    }

    #[test]
    fn failures_retry_and_forget_reopens() {
        let d = Dedupe::default();
        let ttl = Duration::from_secs(60);
        assert!(d.begin("a", ttl));
        d.finish("a", false);
        assert!(d.begin("a", ttl));
        d.finish("a", true);
        d.forget("a");
        assert!(d.begin("a", ttl));
    }
}
