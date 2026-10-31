//! Anti-automation by construction.
//!
//! Every state-changing API call takes a [`UserIntent`], and the only way to get
//! one is [`UserIntent::from_gesture`], which the FFI layer calls from UI event
//! handlers. Intents are single-use (not `Clone`) and expire quickly, so no
//! background task, timer or loop can perform writes on the user's behalf.

use std::time::{Duration, Instant};

use crate::{Error, Result};

const LIFETIME: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Click,
    Key,
    Drop,
    MenuItem,
    NotificationReply,
}

#[derive(Debug)]
pub struct UserIntent {
    gesture: Gesture,
    at: Instant,
    _private: (),
}

impl UserIntent {
    /// Only the platform shell's UI event handlers should call this.
    pub fn from_gesture(gesture: Gesture) -> Self {
        Self { gesture, at: Instant::now(), _private: () }
    }

    pub fn gesture(&self) -> Gesture {
        self.gesture
    }

    /// One gesture that drives two requests (upload then send). Both halves keep the
    /// original timestamp, so splitting never extends how long the gesture is valid.
    pub fn split(self) -> (Self, Self) {
        (Self { gesture: self.gesture, at: self.at, _private: () }, self)
    }

    /// Consume the intent; fails if it is stale.
    pub fn redeem(self) -> Result<()> {
        if self.at.elapsed() <= LIFETIME { Ok(()) } else { Err(Error::NoIntent) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_intent_redeems() {
        assert!(UserIntent::from_gesture(Gesture::Key).redeem().is_ok());
    }

    #[test]
    fn stale_intent_is_refused() {
        let i = UserIntent { gesture: Gesture::Click, at: Instant::now() - LIFETIME * 2, _private: () };
        assert!(matches!(i.redeem(), Err(Error::NoIntent)));
    }
}
