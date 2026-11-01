//! The one search surface's rules (⌘K and ⌘F): how text is folded for matching, how a place is
//! scored against what was typed, what "recent" means, where the match is highlighted, and how
//! much of a long message is shown. Pure, so every shell ranks and marks the same way; the
//! stores it reads from are passed in by the caller (`unison-ffi/src/find.rs`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ----- Folding -----

/// Lower-case one character and strip its diacritic when it has one, so "Zoë" is found by "zoe"
/// and "ÉCOLE" by "ecole". Always one character in, one character out, which keeps match offsets
/// in the folded text equal to offsets in the original.
pub fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let l = c.to_lowercase().next().unwrap_or(c);
    match l {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

pub fn fold(s: &str) -> String {
    s.chars().map(fold_char).collect()
}

/// The words typed, folded. Filter tokens (`from:x`) are not words; the caller removes them first.
pub fn terms(q: &str) -> Vec<String> {
    q.split_whitespace().map(fold).filter(|t| t.chars().any(char::is_alphanumeric)).collect()
}

fn is_sep(c: char) -> bool {
    !c.is_alphanumeric()
}

// ----- Scoring a place -----

/// Match tiers are 200 apart so frecency and unread cues only break ties within a tier.
const EXACT: i32 = 1000;
const PREFIX: i32 = 800;
const WORD: i32 = 600;
const SUBSTR: i32 = 400;
const FUZZY: i32 = 100;
/// A near miss with one typo; below a clean substring, around a loose subsequence.
const TYPO: i32 = 190;

/// How well folded `q` matches folded `s`: exact, prefix, a word's start, anywhere, or the
/// letters in order. `None` when it does not match at all.
pub fn match_score(q: &str, s: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    if s.is_empty() {
        return None;
    }
    if q == s {
        return Some(EXACT);
    }
    let len_pen = (s.chars().count() as i32 / 6).min(40);
    if s.starts_with(q) {
        return Some(PREFIX - len_pen);
    }
    // Word starts: after any separator.
    let mut word = 0;
    let mut prev_sep = true;
    for (i, c) in s.char_indices() {
        if prev_sep && !is_sep(c) {
            if s[i..].starts_with(q) {
                return Some(WORD - word * 20 - len_pen);
            }
            word += 1;
        }
        prev_sep = is_sep(c);
    }
    if let Some(p) = s.find(q) {
        return Some(SUBSTR - (s[..p].chars().count() as i32).min(100) / 2 - len_pen);
    }
    let seq = (q.chars().count() >= 3).then(|| subsequence(q, s).map(|b| FUZZY + b - len_pen)).flatten();
    let typo = typo_score(q, s).map(|t| t - len_pen);
    seq.into_iter().chain(typo).max()
}

/// Edit distance (insert, delete, substitute, swap two neighbours) of `a` and `b`, or `None` when
/// it exceeds `max`.
pub fn edit_distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let (n, m) = (a.len(), b.len());
    let mut prev2: Vec<usize> = vec![0; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[m] <= max).then_some(prev[m])
}

/// A near miss: `q` is one typo away (two for long words) from the start of a word of `s`
/// ("genral" for "general", "samul" for "samuel"). Only for words of four letters or more.
fn typo_score(q: &str, s: &str) -> Option<i32> {
    let qc: Vec<char> = q.chars().collect();
    if qc.len() < 4 {
        return None;
    }
    let max = if qc.len() >= 8 { 2 } else { 1 };
    let sc: Vec<char> = s.chars().collect();
    let mut best: Option<i32> = None;
    let (mut word, mut i) = (0, 0);
    while i < sc.len() {
        if is_sep(sc[i]) {
            i += 1;
            continue;
        }
        let end = sc[i..].iter().position(|&c| is_sep(c)).map_or(sc.len(), |p| i + p);
        let w = &sc[i..end];
        for len in qc.len().saturating_sub(1)..=qc.len() + 1 {
            let part = &w[..len.min(w.len())];
            if let Some(d) = edit_distance(&qc, part, max) {
                let v = TYPO - 30 * d as i32 - word * 5;
                best = Some(best.map_or(v, |b| b.max(v)));
            }
        }
        word += 1;
        i = end;
    }
    best
}

/// The letters of `q` in order inside `s`, scored by how tight and how early the run is (0..~90).
fn subsequence(q: &str, s: &str) -> Option<i32> {
    let sc: Vec<char> = s.chars().collect();
    let mut at = 0usize;
    let mut score = 0i32;
    let mut last: Option<usize> = None;
    for qc in q.chars() {
        let found = sc[at..].iter().position(|&c| c == qc)?;
        let pos = at + found;
        score += 2;
        if pos == 0 || is_sep(sc[pos - 1]) {
            score += 6;
        }
        match last {
            Some(p) if pos == p + 1 => score += 5,
            Some(p) => score -= ((pos - p - 1) as i32).min(4),
            None => score -= (pos as i32 / 2).min(4),
        }
        last = Some(pos);
        at = pos + 1;
    }
    Some(score.clamp(0, 90))
}

/// A place scored for the words typed: every word must match the title or (halved) its context
/// (the server it is in), and the score is the average. `None` when any word matches neither.
pub fn score_place(terms: &[String], title_f: &str, context_f: &str) -> Option<i32> {
    if terms.is_empty() {
        return Some(0);
    }
    let mut total = 0;
    for t in terms {
        let in_title = match_score(t, title_f);
        let in_ctx = match_score(t, context_f).map(|s| s / 2);
        total += in_title.into_iter().chain(in_ctx).max()?;
    }
    Some(total / terms.len() as i32)
}

/// Added to a place's score when it belongs to the active account: larger than any match tier
/// plus frecency and unread cues, so other accounts' places only follow all of the active one's.
pub const ACTIVE_ACCOUNT_FIRST: i32 = 10_000;

/// Orders `items` best first with the active account's before every other account's; within each
/// group, by `key` descending (a message id or time: newest first). Stable.
pub fn sort_active_first<T>(items: &mut [T], is_active: impl Fn(&T) -> bool, key: impl Fn(&T) -> u64) {
    items.sort_by_key(|t| std::cmp::Reverse((is_active(t), key(t))));
}

// ----- Frecency: recent and frequent places -----

/// One place the person opened, with how often and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub account: String,
    pub channel: String,
    pub guild: Option<String>,
    pub count: u32,
    pub last_ms: u64,
}

const RECENTS_CAP: usize = 200;
const HALF_LIFE_MS: f64 = 3.0 * 86_400_000.0;

/// Points (0..~110) for a place opened `count` times, last `age_ms` ago: frequency grows with
/// the log of the count, and recency halves every three days.
pub fn frecency(count: u32, age_ms: u64) -> i32 {
    let freq = 14.0 * (1.0 + count as f64).ln();
    let recent = 60.0 * 0.5f64.powf(age_ms as f64 / HALF_LIFE_MS);
    (freq + recent) as i32
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Recents {
    pub visits: Vec<Visit>,
}

impl Recents {
    pub fn record(&mut self, account: &str, guild: Option<&str>, channel: &str, now_ms: u64) {
        match self.visits.iter_mut().find(|v| v.account == account && v.channel == channel) {
            Some(v) => {
                v.count = v.count.saturating_add(1);
                v.last_ms = now_ms;
                v.guild = guild.map(str::to_string);
            }
            None => self.visits.push(Visit { account: account.into(), channel: channel.into(), guild: guild.map(str::to_string), count: 1, last_ms: now_ms }),
        }
        if self.visits.len() > RECENTS_CAP {
            self.visits.sort_by_key(|v| std::cmp::Reverse(frecency(v.count, now_ms.saturating_sub(v.last_ms))));
            self.visits.truncate(RECENTS_CAP);
        }
    }

    pub fn points(&self, account: &str, channel: &str, now_ms: u64) -> i32 {
        self.visits.iter().find(|v| v.account == account && v.channel == channel).map_or(0, |v| frecency(v.count, now_ms.saturating_sub(v.last_ms)))
    }

    /// Most recently opened first (what the empty query lists); frequency only breaks ties
    /// between places opened within the same minute.
    pub fn latest(&self, n: usize) -> Vec<&Visit> {
        let mut v: Vec<&Visit> = self.visits.iter().collect();
        v.sort_by_key(|v| (std::cmp::Reverse(v.last_ms / 60_000), std::cmp::Reverse(v.count)));
        v.truncate(n);
        v
    }
}

// ----- Highlighting and excerpts -----

/// Where the typed words occur in `text`, as UTF-16 `(start, length)` pairs (what the shells'
/// string types count in). A word marks where it starts a word, falling back to anywhere in the
/// text; overlapping marks merge.
pub fn marks(text: &str, terms: &[String]) -> Vec<(u32, u32)> {
    if terms.is_empty() || text.is_empty() {
        return vec![];
    }
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().copied().map(fold_char).collect();
    let mut u16_at = Vec::with_capacity(chars.len() + 1);
    let mut n = 0u32;
    for c in &chars {
        u16_at.push(n);
        n += c.len_utf16() as u32;
    }
    u16_at.push(n);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for t in terms {
        let tc: Vec<char> = t.chars().collect();
        if tc.is_empty() || tc.len() > folded.len() {
            continue;
        }
        let mut word_hits = Vec::new();
        let mut any_hits = Vec::new();
        for i in 0..=folded.len() - tc.len() {
            if folded[i..i + tc.len()] == tc[..] {
                any_hits.push(i);
                if i == 0 || is_sep(folded[i - 1]) {
                    word_hits.push(i);
                }
            }
        }
        let hits = if word_hits.is_empty() { any_hits } else { word_hits };
        spans.extend(hits.into_iter().take(8).map(|i| (i, i + tc.len())));
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in spans {
        match merged.last_mut() {
            Some(l) if s <= l.1 => l.1 = l.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged.into_iter().map(|(s, e)| (u16_at[s], u16_at[e] - u16_at[s])).collect()
}

/// At most `max` characters of `text`, starting a little before the first match so what was
/// searched for is on screen, with an ellipsis where text was cut.
pub fn excerpt(text: &str, terms: &[String], max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let folded: Vec<char> = chars.iter().copied().map(fold_char).collect();
    let first = terms
        .iter()
        .filter_map(|t| {
            let tc: Vec<char> = t.chars().collect();
            (0..=folded.len().saturating_sub(tc.len())).find(|&i| !tc.is_empty() && folded[i..i + tc.len()] == tc[..])
        })
        .min()
        .unwrap_or(0);
    let mut start = first.saturating_sub(max / 4);
    // Begin on a word.
    while start > 0 && start < chars.len() && !is_sep(chars[start - 1]) {
        start += 1;
        if start >= first {
            start = first;
            break;
        }
    }
    let end = (start + max).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

// ----- Remote result cache (freshness window) -----

/// The last few Discord search pages, kept for a short window so the same query (reopening the
/// palette, stepping back to a scope) is never asked twice while its answer is fresh
/// (ARCHITECTURE rule 10).
pub struct PageCache<V> {
    entries: VecDeque<(String, Instant, V)>,
}

impl<V: Clone> Default for PageCache<V> {
    fn default() -> Self {
        Self { entries: VecDeque::new() }
    }
}

impl<V: Clone> PageCache<V> {
    pub const CAP: usize = 24;

    pub fn get(&self, key: &str, ttl: Duration) -> Option<V> {
        self.entries.iter().find(|(k, t, _)| k == key && t.elapsed() < ttl).map(|(_, _, v)| v.clone())
    }

    pub fn put(&mut self, key: String, v: V) {
        self.entries.retain(|(k, _, _)| *k != key);
        self.entries.push_back((key, Instant::now(), v));
        while self.entries.len() > Self::CAP {
            self.entries.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(q: &str, s: &str) -> Option<i32> {
        match_score(&fold(q), &fold(s))
    }

    #[test]
    fn folds_diacritics_and_case_one_for_one() {
        assert_eq!(fold("ÉCOLE Zoë Łódź"), "ecole zoe lodz");
        assert_eq!(fold("Привет"), "привет");
        assert_eq!("Zoë".chars().count(), fold("Zoë").chars().count());
    }

    #[test]
    fn tiers_rank_exact_prefix_word_substring_fuzzy() {
        let exact = sc("general", "general").unwrap();
        let prefix = sc("gen", "general").unwrap();
        let word = sc("chat", "off-topic chat").unwrap();
        let sub = sc("ner", "general").unwrap();
        let fuzzy = sc("gnl", "general").unwrap();
        assert!(exact > prefix && prefix > word && word > sub && sub > fuzzy, "{exact} {prefix} {word} {sub} {fuzzy}");
        assert!(sc("xyz", "general").is_none());
        assert!(sc("gl", "general").is_none() && sc("z", "general").is_none(), "two letters never match by subsequence");
    }

    #[test]
    fn small_typos_still_match_and_rank_below_clean_hits() {
        assert!(sc("genral", "general").is_some(), "a missing letter");
        assert!(sc("genreal", "general").is_some(), "swapped neighbours");
        assert!(sc("samul", "samuel_k").is_some(), "a missing letter inside a username");
        assert!(sc("gneral", "general").unwrap() < sc("gener", "general").unwrap());
        assert!(sc("zebra", "general").is_none() && sc("abc", "abd").is_none(), "short words never typo-match");
        assert!(sc("harry", "hary the wizard").is_some());
        assert_eq!(edit_distance(&"kitten".chars().collect::<Vec<_>>(), &"sitting".chars().collect::<Vec<_>>(), 2), None);
    }

    #[test]
    fn earlier_words_and_shorter_names_win() {
        assert!(sc("chat", "chat room").unwrap() > sc("chat", "room chat").unwrap());
        assert!(sc("dev", "dev").unwrap() > sc("dev", "devops").unwrap());
        assert!(sc("dev", "devops").unwrap() > sc("dev", "development-announcements").unwrap());
    }

    #[test]
    fn every_word_must_match_title_or_context() {
        let t = terms("unison gen");
        assert!(score_place(&t, "general", "unison hq").is_some());
        assert!(score_place(&t, "random", "unison hq").is_none());
        // A title hit outranks the same word only in the context.
        let a = score_place(&terms("hq"), "hq", "other").unwrap();
        let b = score_place(&terms("hq"), "general", "hq").unwrap();
        assert!(a > b);
    }

    #[test]
    fn frecency_favours_recent_and_frequent() {
        let day = 86_400_000;
        assert!(frecency(1, 0) > frecency(1, 10 * day));
        assert!(frecency(20, 5 * day) > frecency(1, 5 * day));
        assert!(frecency(1, 0) < 130 && frecency(500, 0) < 200, "stays within a tier");
    }

    #[test]
    fn recents_count_and_cap() {
        let mut r = Recents::default();
        r.record("a", Some("g"), "c1", 1000);
        r.record("a", Some("g"), "c1", 2000);
        r.record("a", None, "c2", 3_000_000);
        assert_eq!(r.visits.len(), 2);
        assert_eq!(r.latest(5)[0].channel, "c2");
        assert_eq!(r.visits.iter().find(|v| v.channel == "c1").unwrap().count, 2);
        for i in 0..400 {
            r.record("a", None, &format!("x{i}"), 5_000_000 + i);
        }
        assert!(r.visits.len() <= RECENTS_CAP);
    }

    #[test]
    fn marks_are_utf16_and_word_anchored() {
        let t = terms("caf bat");
        // "😀 Café battery": emoji is 2 UTF-16 units.
        let m = marks("😀 Café battery", &t);
        assert_eq!(m, vec![(3, 3), (8, 3)]);
        assert!(marks("hello", &terms("zzz")).is_empty());
        // Falls back to a mid-word hit when no word starts with the term.
        assert_eq!(marks("unbelievable", &terms("liev")), vec![(4, 4)]);
        // Overlaps merge.
        assert_eq!(marks("battery", &terms("bat batt")), vec![(0, 4)]);
    }

    #[test]
    fn excerpt_keeps_the_match_in_view() {
        let long = format!("{} needle {}", "lorem ".repeat(60), "ipsum ".repeat(60));
        let e = excerpt(&long, &terms("needle"), 120);
        assert!(e.contains("needle") && e.starts_with('…') && e.ends_with('…'));
        assert!(e.chars().count() <= 122);
        assert_eq!(excerpt("short", &terms("short"), 120), "short");
    }

    #[test]
    fn active_account_ranks_above_other_accounts() {
        // An exact title match elsewhere still loses to a loose one in the active account.
        let (exact, loose) = (match_score("dev", "dev").unwrap() + 110 + 15, match_score("dev", "d e v tools").unwrap_or(FUZZY));
        assert!(loose + ACTIVE_ACCOUNT_FIRST > exact);
        let mut m = vec![("b", false, 9), ("a", true, 1), ("c", true, 5), ("d", false, 20)];
        sort_active_first(&mut m, |t| t.1, |t| t.2);
        assert_eq!(m.iter().map(|t| t.0).collect::<String>(), "cadb");
    }

    #[test]
    fn page_cache_expires_and_caps() {
        let mut c: PageCache<u32> = PageCache::default();
        c.put("a".into(), 1);
        assert_eq!(c.get("a", Duration::from_secs(30)), Some(1));
        assert_eq!(c.get("a", Duration::ZERO), None);
        for i in 0..40 {
            c.put(format!("k{i}"), i);
        }
        assert_eq!(c.get("a", Duration::from_secs(30)), None);
        assert!(c.entries.len() <= PageCache::<u32>::CAP);
    }
}
