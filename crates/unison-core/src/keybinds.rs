//! The keyboard shortcuts the app ships, as data. A shell draws the list and binds the accelerators it
//! can; `keys` is platform-neutral (`mod` is Command on macOS and Control elsewhere) so a second shell
//! translates it instead of retyping the table. Custom bindings are not offered yet.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keybind {
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    /// Tokens joined with `+`: `mod`, `shift`, `alt`, `ctrl`, then one key (`up`, `[`, `1..9`, a letter).
    pub keys: &'static str,
}

const fn k(id: &'static str, group: &'static str, title: &'static str, keys: &'static str) -> Keybind {
    Keybind { id, group, title, keys }
}

pub const TABLE: &[Keybind] = &[
    k("search", "Navigation", "Search everywhere", "mod+k"),
    k("search_chat", "Navigation", "Search this chat", "mod+f"),
    k("inbox", "Navigation", "Inbox", "mod+shift+i"),
    k("dms", "Navigation", "Direct Messages", "mod+0"),
    k("prev_channel", "Navigation", "Previous channel", "alt+up"),
    k("next_channel", "Navigation", "Next channel", "alt+down"),
    k("prev_unread", "Navigation", "Previous unread channel", "alt+shift+up"),
    k("next_unread", "Navigation", "Next unread channel", "alt+shift+down"),
    k("prev_server", "Navigation", "Previous server", "mod+alt+up"),
    k("next_server", "Navigation", "Next server", "mod+alt+down"),
    k("back", "Navigation", "History back", "mod+["),
    k("forward", "Navigation", "History forward", "mod+]"),
    k("members", "Window", "Show or hide the member list", "mod+alt+m"),
    k("zoom_in", "Window", "Larger interface", "mod+="),
    k("zoom_out", "Window", "Smaller interface", "mod+-"),
    k("zoom_reset", "Window", "Default interface size", "mod+0"),
    k("settings", "Window", "Settings", "mod+,"),
    k("account", "Accounts", "Switch to account 1 to 9", "ctrl+1"),
    k("add_account", "Accounts", "Add account", "mod+shift+n"),
    k("mute", "Calls", "Mute or unmute", "mod+shift+m"),
    k("deafen", "Calls", "Deafen or undeafen", "mod+shift+d"),
];

/// Groups in display order.
pub fn groups() -> Vec<&'static str> {
    let mut g: Vec<&'static str> = Vec::new();
    for b in TABLE {
        if !g.contains(&b.group) {
            g.push(b.group);
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_keys_wellformed() {
        let mut ids: Vec<_> = TABLE.iter().map(|b| b.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TABLE.len());
        for b in TABLE {
            let parts: Vec<_> = b.keys.split('+').collect();
            assert!(parts.len() >= 2 || b.keys.len() == 1, "{}", b.id);
            assert!(!parts.last().unwrap().is_empty(), "{}", b.id);
        }
    }

    #[test]
    fn groups_keep_order() {
        assert_eq!(groups(), vec!["Navigation", "Window", "Accounts", "Calls"]);
    }
}
