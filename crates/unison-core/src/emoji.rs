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

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert_eq!(super::by_name("thumbsup"), Some("👍"));
        assert_eq!(super::by_name("thumbsup_tone1"), Some("👍🏻"));
        assert_eq!(super::by_name("123456789"), None);
    }
}
