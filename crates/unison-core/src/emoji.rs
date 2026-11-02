//! Discord's names for unicode emoji, for reading favourites and frecency (which key unicode
//! emoji by name). Generated from the web client by `tools/emoji/extract.py`.

use std::collections::HashMap;
use std::sync::LazyLock;

static NAMES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    include_str!("../data/emoji_names.tsv").lines().filter_map(|l| l.split_once('\t')).collect()
});

/// The emoji for a Discord emoji name ("thumbsup" → "👍"), if it's a unicode emoji we know.
pub fn by_name(name: &str) -> Option<&'static str> {
    NAMES.get(name).copied()
}

/// The first name of each unicode emoji, with the variation selector left out of both sides so "❤" and "❤️"
/// are one emoji.
static SHORTCODES: LazyLock<HashMap<String, &'static str>> = LazyLock::new(|| {
    let mut out = HashMap::new();
    for (name, emoji) in include_str!("../data/emoji_names.tsv").lines().filter_map(|l| l.split_once('\t')) {
        out.entry(emoji.replace('\u{fe0f}', "")).or_insert(name);
    }
    out
});

/// Discord's name for a unicode emoji ("💀" → "skull"), for writing it as `:skull:`.
pub fn shortcode(emoji: &str) -> Option<&'static str> {
    SHORTCODES.get(&emoji.replace('\u{fe0f}', "")).copied()
}

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert_eq!(super::by_name("thumbsup"), Some("👍"));
        assert_eq!(super::by_name("thumbsup_tone1"), Some("👍🏻"));
        assert_eq!(super::by_name("123456789"), None);
    }

    #[test]
    fn shortcodes_go_back() {
        assert_eq!(super::shortcode("💀"), Some("skull"));
        assert_eq!(super::shortcode("👍"), Some("thumbsup"));
        assert_eq!(super::shortcode("❤"), super::shortcode("❤️"));
        assert_eq!(super::shortcode("not an emoji"), None);
    }
}
