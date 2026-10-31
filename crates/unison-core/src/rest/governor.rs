//! A human-rate ceiling on everything Unison writes to Discord.
//!
//! Every write already needs a real gesture (`UserIntent`). This is the second line: even a bug,
//! a runaway loop or a held key can't turn gestures into a burst that looks like spam or a
//! self-bot. The limits sit well above what a person does by hand, so normal use never meets them;
//! over a limit the write is refused with a plain sentence and nothing is sent.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use reqwest::Method;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Message,
    FriendRequest,
    Join,
    OpenDm,
    Ring,
    Other,
}

/// (kind it applies to, or `None` for every write; window; most writes allowed in it)
const LIMITS: [(Option<Kind>, Duration, usize); 7] = [
    (None, Duration::from_secs(10), 14),
    (None, Duration::from_secs(300), 90),
    (Some(Kind::Message), Duration::from_secs(10), 6),
    (Some(Kind::FriendRequest), Duration::from_secs(600), 5),
    (Some(Kind::Join), Duration::from_secs(600), 3),
    (Some(Kind::OpenDm), Duration::from_secs(600), 5),
    (Some(Kind::Ring), Duration::from_secs(300), 4),
];

fn kind(method: &Method, path: &str) -> Kind {
    let path = path.split('?').next().unwrap_or(path);
    if method != Method::POST {
        return Kind::Other;
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["channels", _, "messages"] => Kind::Message,
        ["users", "@me", "relationships"] => Kind::FriendRequest,
        ["users", "@me", "channels"] => Kind::OpenDm,
        ["invites", _] => Kind::Join,
        ["channels", _, "call", "ring"] => Kind::Ring,
        _ => Kind::Other,
    }
}

#[derive(Default)]
pub struct Governor {
    seen: VecDeque<(Instant, Kind)>,
}

impl Governor {
    /// Records the write and returns `Ok`, or returns how long to wait and records nothing.
    pub fn check(&mut self, now: Instant, method: &Method, path: &str) -> Result<(), Duration> {
        let k = kind(method, path);
        let longest = LIMITS.iter().map(|l| l.1).max().unwrap_or_default();
        while self.seen.front().is_some_and(|(t, _)| now.duration_since(*t) > longest) {
            self.seen.pop_front();
        }
        let mut wait = Duration::ZERO;
        for (only, window, max) in LIMITS {
            if only.is_some_and(|o| o != k) {
                continue;
            }
            let hits: Vec<Instant> = self.seen.iter().filter(|(t, kk)| now.duration_since(*t) <= window && only.is_none_or(|o| o == *kk)).map(|(t, _)| *t).collect();
            if hits.len() >= max {
                // The oldest write in the window leaving it frees a slot.
                let free_at = hits[hits.len() - max] + window;
                wait = wait.max(free_at.saturating_duration_since(now));
            }
        }
        if wait > Duration::ZERO {
            return Err(wait.max(Duration::from_secs(1)));
        }
        self.seen.push_back((now, k));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_never_meet_the_limits() {
        let mut g = Governor::default();
        let t = Instant::now();
        // A message a second for ten seconds is already fast typing; reactions and edits between.
        for i in 0..5 {
            assert!(g.check(t + Duration::from_secs(i * 2), &Method::POST, "/channels/1/messages").is_ok());
            assert!(g.check(t + Duration::from_secs(i * 2), &Method::PUT, "/channels/1/messages/2/reactions/x/@me").is_ok());
        }
    }

    #[test]
    fn a_burst_of_messages_is_stopped_and_frees_up() {
        let mut g = Governor::default();
        let t = Instant::now();
        for _ in 0..6 {
            assert!(g.check(t, &Method::POST, "/channels/1/messages").is_ok());
        }
        let wait = g.check(t, &Method::POST, "/channels/1/messages").unwrap_err();
        assert!(wait >= Duration::from_secs(1) && wait <= Duration::from_secs(10));
        assert!(g.check(t + Duration::from_secs(11), &Method::POST, "/channels/1/messages").is_ok());
    }

    #[test]
    fn friend_requests_joins_and_new_dms_have_their_own_low_limits() {
        let t = Instant::now();
        let mut g = Governor::default();
        for i in 0..5 {
            assert!(g.check(t + Duration::from_secs(i * 3), &Method::POST, "/users/@me/relationships").is_ok());
        }
        assert!(g.check(t + Duration::from_secs(20), &Method::POST, "/users/@me/relationships").is_err());
        let mut g = Governor::default();
        for i in 0..3 {
            assert!(g.check(t + Duration::from_secs(i * 3), &Method::POST, "/invites/abc").is_ok());
        }
        assert!(g.check(t + Duration::from_secs(20), &Method::POST, "/invites/abc").is_err());
        // Other writes are unaffected by the join limit.
        assert!(g.check(t + Duration::from_secs(21), &Method::PUT, "/channels/1/pins/2").is_ok());
    }

    #[test]
    fn a_refused_write_is_not_counted() {
        let mut g = Governor::default();
        let t = Instant::now();
        for _ in 0..14 {
            let _ = g.check(t, &Method::PATCH, "/channels/1");
        }
        assert!(g.check(t, &Method::PATCH, "/channels/1").is_err());
        assert_eq!(g.seen.len(), 14);
    }
}
