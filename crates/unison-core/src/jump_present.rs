//! When "Jump to present" is offered (docs/UX.md 5.6). One rule for every shell.
//!
//! The control exists to get a reader back from somewhere genuinely far from the newest message.
//! It must not appear for a short scroll-up, nor after following a reply to a message that was
//! sent a little while ago. A shell measures what it can see (how much of the loaded transcript
//! lies below the viewport, in viewport heights and in messages) and asks `should_show`; the
//! core adds what only it knows (is the window the newest page, how far in time is the newest
//! loaded message from the channel's latest one).
//!
//! Distance is counted in *screens* (viewport heights): the loaded part below the viewport, plus,
//! for a window that is not the newest page, a tail estimate for the part that is not loaded (at
//! least one screen, one more for every `TAIL_MS_PER_SCREEN` of time). Far means `SHOW_SCREENS`
//! or more, or `SHOW_MESSAGES` messages below. Once shown it stays until the reader is clearly
//! back (`HIDE_SCREENS`, `HIDE_MESSAGES`), so it does not flicker around the threshold.

/// Offered at this distance (viewport heights) ...
pub const SHOW_SCREENS: f32 = 3.0;
/// ... or this many loaded messages below the viewport.
pub const SHOW_MESSAGES: u32 = 80;
/// Once shown, hidden again only below this distance (hysteresis).
pub const HIDE_SCREENS: f32 = 1.5;
pub const HIDE_MESSAGES: u32 = 40;
/// A detached window's unloaded tail counts one screen per this much time between its newest
/// message and the channel's latest one (a quiet channel gap is far in time but few messages).
pub const TAIL_MS_PER_SCREEN: u64 = 5 * 60 * 1000;
/// Upper bound for the tail estimate, so one very old gap does not dominate arithmetic.
pub const TAIL_MAX_SCREENS: f32 = 50.0;

/// What a shell reports about the reader's place in the loaded transcript.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// The loaded window ends at the channel's newest message.
    pub at_latest: bool,
    /// Loaded content below the viewport's bottom edge, in viewport heights (0 at the bottom).
    pub screens_below: f32,
    /// Loaded messages below the viewport.
    pub messages_below: u32,
    /// Time between the newest loaded message and the channel's latest one (0 when unknown or
    /// when the window is the newest page).
    pub tail_ms: u64,
    /// The control is showing right now (for the hysteresis).
    pub shown: bool,
}

/// Distance from the present in screens, loaded part plus the estimated unloaded tail.
pub fn distance_screens(r: &Reading) -> f32 {
    let tail = if r.at_latest {
        0.0
    } else {
        (1.0 + r.tail_ms as f32 / TAIL_MS_PER_SCREEN as f32).min(TAIL_MAX_SCREENS)
    };
    r.screens_below.max(0.0) + tail
}

/// Should "Jump to present" be on screen?
pub fn should_show(r: &Reading) -> bool {
    let screens = distance_screens(r);
    if r.shown {
        screens >= HIDE_SCREENS || r.messages_below >= HIDE_MESSAGES
    } else {
        screens >= SHOW_SCREENS || r.messages_below >= SHOW_MESSAGES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_latest(screens: f32, msgs: u32, shown: bool) -> Reading {
        Reading { at_latest: true, screens_below: screens, messages_below: msgs, tail_ms: 0, shown }
    }

    fn detached(screens: f32, msgs: u32, tail_ms: u64, shown: bool) -> Reading {
        Reading { at_latest: false, screens_below: screens, messages_below: msgs, tail_ms, shown }
    }

    #[test]
    fn short_scroll_up_never_shows() {
        // A few messages, or a screen or two, above the bottom of the newest page.
        assert!(!should_show(&at_latest(0.0, 0, false)));
        assert!(!should_show(&at_latest(0.4, 6, false)));
        assert!(!should_show(&at_latest(2.0, 30, false)));
        assert!(!should_show(&at_latest(2.9, 79, false)));
    }

    #[test]
    fn far_up_shows() {
        assert!(should_show(&at_latest(3.0, 40, false)));
        assert!(should_show(&at_latest(8.0, 120, false)));
        // Many short messages: the count alone is far.
        assert!(should_show(&at_latest(2.0, 80, false)));
    }

    #[test]
    fn reply_to_a_recent_message_stays_quiet() {
        // The page around a message sent minutes ago: its window is not the newest page, but the
        // missing tail is about a minute, and the target is mid-screen.
        assert!(!should_show(&detached(0.8, 12, 60_000, false)));
        assert!(!should_show(&detached(1.0, 20, 4 * 60_000, false)));
    }

    #[test]
    fn old_window_shows_even_at_its_end() {
        // A message from days ago: the window is far in time from the channel's latest.
        assert!(should_show(&detached(0.0, 0, 3 * 24 * 3_600_000, false)));
        assert!(should_show(&detached(0.5, 5, 2 * 3_600_000, false)));
    }

    #[test]
    fn hysteresis_does_not_flicker() {
        // Between the two thresholds the previous state is kept.
        assert!(should_show(&at_latest(2.0, 50, true)));
        assert!(!should_show(&at_latest(2.0, 50, false)));
        assert!(should_show(&at_latest(1.5, 10, true)));
        // Clearly back: hides.
        assert!(!should_show(&at_latest(1.4, 39, true)));
        assert!(!should_show(&at_latest(0.0, 0, true)));
    }

    #[test]
    fn tail_is_capped_and_never_negative() {
        let r = detached(-1.0, 0, u64::MAX, false);
        assert!(distance_screens(&r) <= TAIL_MAX_SCREENS);
        assert!(distance_screens(&detached(-3.0, 0, 0, false)) >= 0.0);
    }

    #[test]
    fn at_latest_ignores_the_tail() {
        let r = Reading { at_latest: true, screens_below: 0.5, messages_below: 3, tail_ms: u64::MAX, shown: false };
        assert!(!should_show(&r));
    }
}
