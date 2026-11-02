//! Which new messages arrive with motion (docs/UX.md 4.10). One rule for every shell.
//!
//! A message that lands while you read the end of a channel fades in and the transcript glides up by its
//! height. The same applies to messages that were missed and are fetched a moment after a channel opens
//! (the unread ones): they arrive the same way instead of popping in. A whole screen of them is not an
//! arrival any more, it is a page: that swaps in, because animating many rows is noise and costs frames.

/// At most this many new rows animate in together ...
pub const MAX_ROWS: u32 = 12;
/// ... and only when together they are no taller than this many viewports.
pub const MAX_SCREENS: f64 = 0.8;

/// Do `rows` new rows, `screens` viewport heights tall in all, arrive with motion?
pub fn animates(rows: u32, screens: f64) -> bool {
    rows >= 1 && rows <= MAX_ROWS && screens <= MAX_SCREENS
}

#[cfg(test)]
mod tests {
    use super::animates;

    #[test]
    fn a_few_missed_messages_arrive_with_motion() {
        assert!(animates(1, 0.05));
        assert!(animates(6, 0.4));
        assert!(animates(12, 0.8));
    }

    #[test]
    fn a_page_swaps_in() {
        assert!(!animates(13, 0.3), "too many rows");
        assert!(!animates(5, 1.5), "taller than the viewport");
        assert!(!animates(0, 0.0), "nothing arrived");
    }
}
