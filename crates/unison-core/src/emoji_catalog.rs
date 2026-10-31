//! Discord's unicode emoji catalog: categories, aliases and skin-tone variants, for the picker and
//! `:name:` autocomplete. Generated from the web client by `tools/emoji/catalog.py`.

use std::collections::HashMap;
use std::sync::LazyLock;

pub struct UnicodeEmoji {
    pub category: &'static str,
    /// The primary name first, then aliases.
    pub names: Vec<&'static str>,
    pub emoji: &'static str,
    /// Skin-tone variants (5, or 25 for two-person emoji); empty when there are none.
    pub tones: Vec<&'static str>,
}

/// Discord's picker categories, in its order: id, title.
pub const CATEGORIES: [(&str, &str); 8] = [
    ("people", "People"),
    ("nature", "Nature"),
    ("food", "Food & Drink"),
    ("activity", "Activities"),
    ("travel", "Travel & Places"),
    ("objects", "Objects"),
    ("symbols", "Symbols"),
    ("flags", "Flags"),
];

static ALL: LazyLock<Vec<UnicodeEmoji>> = LazyLock::new(|| {
    include_str!("../data/emoji_catalog.tsv")
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let category = f.next()?;
            let names: Vec<&str> = f.next()?.split(',').collect();
            let emoji = f.next()?;
            let tones = f.next().filter(|t| !t.is_empty()).map(|t| t.split(',').collect()).unwrap_or_default();
            Some(UnicodeEmoji { category, names, emoji, tones })
        })
        .collect()
});

static BY_NAME: LazyLock<HashMap<&'static str, usize>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for (i, e) in ALL.iter().enumerate() {
        for n in &e.names {
            m.entry(*n).or_insert(i);
        }
    }
    m
});

pub fn all() -> &'static [UnicodeEmoji] {
    &ALL
}

/// The emoji for an exact `:name:` (any alias), e.g. "smile" → "😄".
pub fn by_name(name: &str) -> Option<&'static UnicodeEmoji> {
    BY_NAME.get(name).map(|&i| &ALL[i])
}

/// How well `query` matches `name` (lower is better): exact, prefix, word prefix, substring.
pub fn match_score(query: &str, name: &str) -> Option<u32> {
    if name == query {
        return Some(0);
    }
    if name.starts_with(query) {
        return Some(1);
    }
    if name.match_indices(query).any(|(i, _)| name[..i].ends_with(['_', ' ', '-', '.'])) {
        return Some(2);
    }
    name.contains(query).then_some(3)
}

/// Unicode emoji matching `query` (case-insensitive), best first. `rank` breaks ties (lower
/// first), e.g. position in the user's frequent list.
pub fn search(query: &str, limit: usize, rank: &dyn Fn(&str) -> usize) -> Vec<&'static UnicodeEmoji> {
    let q = query.to_lowercase();
    let mut hits: Vec<(u32, usize, usize, usize, &UnicodeEmoji)> = ALL
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let best = e.names.iter().filter_map(|n| match_score(&q, n)).min()?;
            Some((best, rank(e.names[0]), e.names[0].len(), i, e))
        })
        .collect();
    hits.sort_by_key(|h| (h.0, h.1, h.2, h.3));
    hits.into_iter().take(limit).map(|h| h.4).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads() {
        assert!(all().len() > 1800);
        let wave = by_name("waving_hand").unwrap();
        assert_eq!(wave.emoji, "👋");
        assert_eq!(wave.tones.len(), 5);
        assert_eq!(by_name("thumbsup").unwrap().emoji, by_name("+1").unwrap().emoji);
    }

    #[test]
    fn ranking() {
        let no = |_: &str| 0usize;
        let r = search("smile", 5, &no);
        assert_eq!(r[0].names[0], "smile");
        let r = search("thumbs", 5, &no);
        assert_eq!(r[0].emoji, "👍");
        // Prefix beats word prefix beats substring.
        assert_eq!(match_score("fire", "fire_engine"), Some(1));
        assert_eq!(match_score("engine", "fire_engine"), Some(2));
        assert_eq!(match_score("ire", "fire_engine"), Some(3));
        assert_eq!(match_score("zzz", "fire"), None);
        // Frequency breaks ties between equally good matches.
        let fav = |n: &str| if n == "smirk" { 0 } else { 9 };
        let r = search("smi", 40, &fav);
        let a = r.iter().position(|e| e.names[0] == "smirk").unwrap();
        let b = r.iter().position(|e| e.names[0] == "smile").unwrap();
        assert!(a < b);
    }
}
