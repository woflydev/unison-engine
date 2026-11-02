//! When an animated sticker may move. The setting is Discord's `animate_stickers` (0 always, 1 on
//! interaction, 2 never); Reduce Motion from the OS beats it, and nothing moves out of sight.

pub fn should_play(setting: u32, reduce_motion: bool, visible: bool, hovered: bool) -> bool {
    if reduce_motion || !visible {
        return false;
    }
    match setting {
        0 => true,
        1 => hovered,
        _ => false,
    }
}

/// Whether an animated custom emoji moves now: the "Animated emoji" setting, Reduce Motion, on screen and
/// the app in front (nothing moves in a window nobody is looking at).
pub fn emoji_should_play(setting: bool, reduce_motion: bool, visible: bool, app_active: bool) -> bool {
    setting && !reduce_motion && visible && app_active
}

#[cfg(test)]
mod tests {
    use super::{emoji_should_play, should_play};

    #[test]
    fn emoji_move_only_when_allowed_seen_and_in_front() {
        assert!(emoji_should_play(true, false, true, true));
        assert!(!emoji_should_play(false, false, true, true));
        assert!(!emoji_should_play(true, true, true, true));
        assert!(!emoji_should_play(true, false, false, true));
        assert!(!emoji_should_play(true, false, true, false));
    }

    #[test]
    fn setting_visibility_and_reduce_motion() {
        assert!(should_play(0, false, true, false));
        assert!(!should_play(0, false, false, false));
        assert!(!should_play(0, true, true, true));
        assert!(!should_play(1, false, true, false));
        assert!(should_play(1, false, true, true));
        assert!(!should_play(2, false, true, true));
    }
}
