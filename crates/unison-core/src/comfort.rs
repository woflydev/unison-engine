//! Small conveniences and the rules behind them: tracking-free links, the `@everyone` check,
//! notification keywords and quiet hours, and plain-text export. Everything
//! here is local: it adds no request and sends nothing by itself. Shells call it through the FFI
//! and only draw the result (`docs/ARCHITECTURE.md` §2).

use serde::{Deserialize, Serialize};

const MAX_KEYWORDS: usize = 20;

/// Minutes after local midnight, `end` exclusive; a window that wraps midnight has `end < start`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QuietHours {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Comfort {
    /// Strip tracking parameters from links pasted into the composer.
    pub clean_links: bool,
    /// Ask before sending a message that will ping `@everyone` or `@here`.
    pub confirm_everyone: bool,
    /// Remove GPS data from photos before they upload.
    pub strip_location: bool,
    /// Turn HEIC and HEIF photos into JPEG before they upload, as the web client does (Discord shows no preview of HEIC).
    pub convert_heic: bool,
    /// Words that raise a notification like a mention does (lowercase, whole words).
    pub keywords: Vec<String>,
    /// Trim a message that is over the length limit instead of refusing it: `Some(true)` always,
    /// `Some(false)` never, `None` follows the account (on without Nitro). The cut-off part stays in
    /// the composer; nothing is ever sent as a second message.
    pub trim_long: Option<bool>,
    /// No banners or sounds in this window; unread counts and badges still update.
    pub quiet: Option<QuietHours>,
    /// A click on a video opens it full size over the dimmed window (on, the default) instead of playing it
    /// in place inside the message.
    pub video_viewer: bool,
    /// Play HDR videos mapped to standard range, so the display does not brighten while they play. Off by default:
    /// HDR plays as made.
    pub video_sdr: bool,
}

impl Default for Comfort {
    fn default() -> Self {
        Self { clean_links: true, confirm_everyone: true, strip_location: true, convert_heic: true, keywords: vec![], trim_long: None, quiet: None, video_viewer: true, video_sdr: false }
    }
}

/// Longest keyword kept, in characters.
pub const MAX_KEYWORD_LEN: usize = 32;

/// One word from typed or pasted text: lowercase, without punctuation around it. `None` when
/// nothing usable is left (under two characters or over [`MAX_KEYWORD_LEN`]).
fn clean_keyword(raw: &str) -> Option<String> {
    let w = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let n = w.chars().count();
    (2..=MAX_KEYWORD_LEN).contains(&n).then_some(w)
}

/// Split free text (spaces, commas or new lines) into keywords: lowercase, one word each, two
/// letters or more, without duplicates, at most [`MAX_KEYWORDS`].
pub fn normalize_keywords(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    add_keywords(&mut out, raw);
    out
}

/// What [`add_keywords`] did, for the one line the shell may show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordAdd {
    /// At least one word was added.
    Added,
    /// Every word was already in the list.
    Duplicate,
    /// Nothing usable (empty, one letter, or longer than [`MAX_KEYWORD_LEN`]).
    Unusable,
    /// The list holds [`MAX_KEYWORDS`] already (or filled up before every word fit).
    Full,
}

/// Add the words in `raw` to `list` (see [`normalize_keywords`] for what counts as a word).
pub fn add_keywords(list: &mut Vec<String>, raw: &str) -> KeywordAdd {
    let (mut added, mut dup, mut full) = (false, false, false);
    for part in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let Some(w) = clean_keyword(part) else { continue };
        if list.contains(&w) {
            dup = true;
        } else if list.len() >= MAX_KEYWORDS {
            full = true;
        } else {
            list.push(w);
            added = true;
        }
    }
    match (added, full, dup) {
        (_, true, _) => KeywordAdd::Full,
        (true, _, _) => KeywordAdd::Added,
        (false, _, true) => KeywordAdd::Duplicate,
        _ => KeywordAdd::Unusable,
    }
}

/// The one plain sentence for a refused add (`None` when something was added).
pub fn keyword_note(r: KeywordAdd) -> Option<&'static str> {
    match r {
        KeywordAdd::Added => None,
        KeywordAdd::Duplicate => Some("Already in the list."),
        KeywordAdd::Unusable => Some("Use a word of 2 to 32 letters or digits."),
        KeywordAdd::Full => Some("You can keep up to 20 words."),
    }
}

pub fn remove_keyword(list: &mut Vec<String>, word: &str) {
    let w = word.to_lowercase();
    list.retain(|k| *k != w);
}

/// Whether `text` contains any keyword as a whole word (letters and digits delimit words, so
/// "rust" matches "Rust!" but not "trust").
pub fn keyword_hit(text: &str, keywords: &[String]) -> bool {
    if keywords.is_empty() || text.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    keywords.iter().any(|k| {
        let mut from = 0;
        while let Some(i) = lower[from..].find(k.as_str()) {
            let start = from + i;
            let end = start + k.len();
            let before = lower[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let after = lower[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
            if before && after {
                return true;
            }
            from = start + lower[start..].chars().next().map_or(1, char::len_utf8);
        }
        false
    })
}

pub fn is_quiet(q: &QuietHours, minute_of_day: u16) -> bool {
    if q.start == q.end {
        return false;
    }
    if q.start < q.end {
        (q.start..q.end).contains(&minute_of_day)
    } else {
        minute_of_day >= q.start || minute_of_day < q.end
    }
}

// ----- Links -----

const TRACKING: [&str; 17] = [
    "fbclid", "gclid", "gclsrc", "dclid", "msclkid", "igshid", "igsh", "mc_cid", "mc_eid", "_hsenc", "_hsmi", "yclid", "twclid",
    "ref_src", "ref_url", "wickedid", "vero_id",
];
/// Share ids that are tracking only on these hosts (`si` means something else elsewhere).
const SHARE_ID_HOSTS: [&str; 4] = ["youtube.com", "youtu.be", "music.youtube.com", "open.spotify.com"];

fn is_tracking(key: &str, host: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_")
        || TRACKING.contains(&k.as_str())
        || (k == "si" && SHARE_ID_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))))
}

fn clean_url(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches(['.', ',', ')', '!', ';', ':', '\'', '"']);
    let tail = &url[trimmed.len()..];
    let (rest, fragment) = match trimmed.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (trimmed, None),
    };
    let (base, query) = rest.split_once('?')?;
    let host = base.split("://").nth(1)?.split('/').next()?.rsplit('@').next()?.to_ascii_lowercase();
    let kept: Vec<&str> = query.split('&').filter(|p| !p.is_empty() && !is_tracking(p.split('=').next().unwrap_or(""), &host)).collect();
    if kept.len() == query.split('&').filter(|p| !p.is_empty()).count() {
        return None;
    }
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = fragment {
        out.push('#');
        out.push_str(f);
    }
    out.push_str(tail);
    Some(out)
}

/// `text` without tracking parameters on its links, and how many links changed. Code spans, code
/// blocks and `<angle-bracket>` links are left exactly as written.
pub fn clean_links(text: &str) -> (String, usize) {
    if !text.contains("http") {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut changed = 0;
    for (i, segment) in split_code(text).into_iter().enumerate() {
        // Odd segments are code.
        if i % 2 == 1 {
            out.push_str(segment);
            continue;
        }
        let mut rest = segment;
        while let Some(at) = rest.find("http") {
            let (head, tail) = rest.split_at(at);
            let is_link = tail.starts_with("http://") || tail.starts_with("https://");
            let wrapped = head.ends_with('<');
            let len = tail.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(tail.len());
            out.push_str(head);
            if is_link && !wrapped {
                match clean_url(&tail[..len]) {
                    Some(c) => {
                        out.push_str(&c);
                        changed += 1;
                    }
                    None => out.push_str(&tail[..len]),
                }
            } else {
                out.push_str(&tail[..len.max(4).min(tail.len())]);
                rest = &tail[len.max(4).min(tail.len())..];
                continue;
            }
            rest = &tail[len..];
        }
        out.push_str(rest);
    }
    (out, changed)
}

/// Alternating plain and code segments: even indexes are plain text, odd ones are code (fenced
/// blocks and inline spans, markers included).
fn split_code(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|b| **b == b'`').count();
            let fence = &text[i..i + run];
            if let Some(close) = text[i + run..].find(fence) {
                let end = i + run + close + run;
                out.push(&text[start..i]);
                out.push(&text[i..end]);
                start = end;
                i = end;
                continue;
            }
            i += run;
            continue;
        }
        i += 1;
    }
    out.push(&text[start..]);
    out
}

// ----- Sending -----

/// `@everyone` or `@here` written outside code, which is what pings a whole channel.
pub fn everyone_ping(text: &str) -> Option<&'static str> {
    split_code(text).into_iter().step_by(2).find_map(|plain| {
        let lower = plain.to_ascii_lowercase();
        ["@everyone", "@here"].into_iter().find(|w| {
            lower.match_indices(w).any(|(i, _)| {
                let before_ok = lower[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
                let after_ok = lower[i + w.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
                before_ok && after_ok
            })
        })
    })
}

/// Photo types whose location can be removed without re-encoding.
pub fn can_strip_location(content_type: &str) -> bool {
    matches!(content_type, "image/jpeg" | "image/heic" | "image/heif" | "image/tiff")
}

// ----- Long code blocks -----

/// A code block with more lines than this is shown cut down...
pub const COLLAPSE_OVER: u32 = 16;
/// ...to this many lines, followed by a note saying how many are hidden.
pub const COLLAPSE_KEEP: u32 = 10;

/// What [`collapse_code`] produced: the shortened text, where each input span went (`None` when
/// it lay entirely inside a hidden part), and the notes it inserted as `(start, len, hidden_lines)`
/// in UTF-16 units.
pub struct Collapsed {
    pub text: String,
    pub spans: Vec<Option<(u32, u32)>>,
    pub notes: Vec<(u32, u32, u32)>,
}

pub fn collapse_note(hidden: u32) -> String {
    format!("… {hidden} more line{}", if hidden == 1 { "" } else { "s" })
}

/// Cut long code blocks (`blocks` and `spans` are `(start, len)` in UTF-16 units over `text`) down
/// to [`COLLAPSE_KEEP`] lines plus a note. `None` when nothing is long enough. Spans are mapped
/// to the new text; a block that was cut keeps covering its note.
pub fn collapse_code(text: &str, blocks: &[(u32, u32)], spans: &[(u32, u32)]) -> Option<Collapsed> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    // (from, to, hidden): from the start of the first hidden line to the end of the block.
    let mut cuts: Vec<(u32, u32, u32)> = Vec::new();
    for &(start, len) in blocks {
        let (s, mut e) = (start as usize, ((start + len) as usize).min(units.len()));
        if s >= e {
            continue;
        }
        if units[e - 1] == 10 {
            e -= 1;
        }
        let newlines: Vec<usize> = (s..e).filter(|&i| units[i] == 10).collect();
        let total = newlines.len() as u32 + 1;
        if total <= COLLAPSE_OVER {
            continue;
        }
        let from = newlines[COLLAPSE_KEEP as usize - 1] + 1;
        cuts.push((from as u32, e as u32, total - COLLAPSE_KEEP));
    }
    if cuts.is_empty() {
        return None;
    }
    cuts.sort();
    let mut mapped: Vec<Option<(u32, u32)>> = spans.iter().map(|&s| Some(s)).collect();
    let mut notes: Vec<(u32, u32, u32)> = Vec::new();
    // Last cut first, so earlier offsets stay valid.
    for &(from, to, hidden) in cuts.iter().rev() {
        let note: Vec<u16> = collapse_note(hidden).encode_utf16().collect();
        let note_len = note.len() as u32;
        units.splice(from as usize..to as usize, note);
        let remap = |p: u32, is_end: bool| -> u32 {
            if p <= from {
                p
            } else if p >= to {
                p - (to - from) + note_len
            } else if is_end {
                from
            } else {
                from + note_len
            }
        };
        for m in mapped.iter_mut() {
            if let Some((s, l)) = *m {
                let (ns, ne) = (remap(s, false), remap(s + l, true));
                *m = (ne > ns).then(|| (ns, ne - ns));
            }
        }
        for n in notes.iter_mut() {
            n.0 = remap(n.0, false);
        }
        notes.push((from, note_len, hidden));
    }
    notes.sort();
    Some(Collapsed { text: String::from_utf16_lossy(&units), spans: mapped, notes })
}

// ----- Export -----

pub struct ExportRow {
    /// Already formatted by the caller (local date and time).
    pub when: String,
    pub author: String,
    pub content: String,
    pub attachments: Vec<String>,
}

/// A plain-text transcript, oldest first, in the shape most people would paste into a note.
pub fn export_text(title: &str, rows: &[ExportRow]) -> String {
    let mut out = format!("{title}\n{} messages\n", rows.len());
    for r in rows {
        out.push_str(&format!("\n[{}] {}\n", r.when, r.author));
        if !r.content.is_empty() {
            out.push_str(&r.content);
            out.push('\n');
        }
        for a in &r.attachments {
            out.push_str(&format!("(attachment) {a}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn videos_open_in_the_viewer_unless_turned_off() {
        assert!(Comfort::default().video_viewer);
        // Settings saved before the option existed keep the default.
        let old: Comfort = serde_json::from_str(r#"{"clean_links":false}"#).unwrap();
        assert!(old.video_viewer && !old.clean_links && !old.video_sdr);
    }

    use super::*;

    #[test]
    fn keywords_normalize_and_match_whole_words() {
        let k = normalize_keywords("Rust, rust\n a ,Zürich,  ");
        assert_eq!(k, vec!["rust", "zürich"]);
        assert!(keyword_hit("I love Rust!", &k));
        assert!(!keyword_hit("trust issues", &k));
        assert!(keyword_hit("in ZÜRICH today", &k));
        assert!(!keyword_hit("nothing", &[]));
        assert_eq!(normalize_keywords(&"ab,".repeat(5)).len(), 1);
        let many: String = (0..40).map(|i| format!("word{i},")).collect();
        assert_eq!(normalize_keywords(&many).len(), MAX_KEYWORDS);
    }

    #[test]
    fn keyword_editing_rules() {
        let mut l = vec![];
        assert_eq!(add_keywords(&mut l, "  Standup! "), KeywordAdd::Added);
        assert_eq!(l, vec!["standup"]);
        assert_eq!(add_keywords(&mut l, "STANDUP"), KeywordAdd::Duplicate);
        assert_eq!(add_keywords(&mut l, "a"), KeywordAdd::Unusable);
        assert_eq!(add_keywords(&mut l, "   "), KeywordAdd::Unusable);
        assert_eq!(add_keywords(&mut l, &"x".repeat(33)), KeywordAdd::Unusable);
        assert_eq!(add_keywords(&mut l, "my name, rust"), KeywordAdd::Added);
        assert_eq!(l, vec!["standup", "my", "name", "rust"]);
        remove_keyword(&mut l, "My");
        assert_eq!(l, vec!["standup", "name", "rust"]);
        for i in 0..30 {
            add_keywords(&mut l, &format!("word{i}"));
        }
        assert_eq!(l.len(), MAX_KEYWORDS);
        assert_eq!(add_keywords(&mut l, "another"), KeywordAdd::Full);
        assert!(keyword_note(KeywordAdd::Added).is_none() && keyword_note(KeywordAdd::Full).is_some());
        // Whole words and case are the matcher's job, on what is stored.
        assert!(keyword_hit("Standup at ten", &["standup".into()]));
        assert!(!keyword_hit("standups", &["standup".into()]));
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        let night = QuietHours { start: 22 * 60, end: 7 * 60 };
        assert!(is_quiet(&night, 23 * 60));
        assert!(is_quiet(&night, 3 * 60));
        assert!(!is_quiet(&night, 12 * 60));
        assert!(!is_quiet(&night, 7 * 60));
        let day = QuietHours { start: 9 * 60, end: 17 * 60 };
        assert!(is_quiet(&day, 9 * 60) && !is_quiet(&day, 17 * 60));
        assert!(!is_quiet(&QuietHours { start: 60, end: 60 }, 60));
    }

    #[test]
    fn links_lose_tracking_only() {
        let (t, n) = clean_links("see https://example.com/a?id=3&utm_source=x&fbclid=abc#top ok");
        assert_eq!(t, "see https://example.com/a?id=3#top ok");
        assert_eq!(n, 1);
        let (t, _) = clean_links("(https://example.com/?utm_medium=y).");
        assert_eq!(t, "(https://example.com/).");
        let (t, n) = clean_links("https://youtu.be/xyz?si=AAA&t=30 and https://example.com/?si=keep");
        assert_eq!(t, "https://youtu.be/xyz?t=30 and https://example.com/?si=keep");
        assert_eq!(n, 1);
    }

    #[test]
    fn links_in_code_or_brackets_are_untouched() {
        for s in ["`https://a.com/?utm_source=x`", "```\nhttps://a.com/?fbclid=1\n```", "<https://a.com/?utm_source=x>", "no links here", "https://a.com/?q=1"] {
            let (t, n) = clean_links(s);
            assert_eq!((t.as_str(), n), (s, 0), "{s}");
        }
        let (t, n) = clean_links("`x` https://a.com/?gclid=1 `y`");
        assert_eq!((t.as_str(), n), ("`x` https://a.com/ `y`", 1));
    }

    #[test]
    fn everyone_is_found_outside_code_only() {
        assert_eq!(everyone_ping("hey @everyone!"), Some("@everyone"));
        assert_eq!(everyone_ping("@Here now"), Some("@here"));
        assert_eq!(everyone_ping("`@everyone`"), None);
        assert_eq!(everyone_ping("mail me@everyone.com"), None);
        assert_eq!(everyone_ping("@everyones"), None);
        assert_eq!(everyone_ping("plain"), None);
    }

    #[test]
    fn location_stripping_covers_photo_types_only() {
        assert!(can_strip_location("image/jpeg") && !can_strip_location("image/png"));
    }

    #[test]
    fn export_lists_messages_in_order() {
        let rows = [
            ExportRow { when: "2026-10-30 09:00".into(), author: "ann".into(), content: "hi".into(), attachments: vec![] },
            ExportRow { when: "2026-10-30 09:01".into(), author: "bo".into(), content: String::new(), attachments: vec!["https://x/y.png".into()] },
        ];
        let t = export_text("#general", &rows);
        assert!(t.starts_with("#general\n2 messages\n"));
        assert!(t.find("ann").unwrap() < t.find("bo").unwrap());
        assert!(t.contains("(attachment) https://x/y.png"));
    }

    fn block(lines: u32) -> String {
        (1..=lines).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn long_code_blocks_are_cut_and_short_ones_are_not() {
        let short = format!("hi\n{}\nbye", block(16));
        let len = |s: &str| s.encode_utf16().count() as u32;
        assert!(collapse_code(&short, &[(3, len(&block(16)))], &[]).is_none(), "16 lines fit");

        let body = block(30);
        let text = format!("hi\n{body}\nbye");
        let start = 3;
        let blen = len(&body);
        // A bold span before the block, the block itself, one inside the hidden part, one after.
        let after = start + blen + 1;
        let spans = [(0, 2), (start, blen), (start + len(&block(20)), 3), (after, 3)];
        let c = collapse_code(&text, &[(start, blen)], &spans).unwrap();
        assert!(c.text.starts_with("hi\nline1\nline2"));
        assert!(c.text.contains("line10\n… 20 more lines\nbye"), "{}", c.text);
        assert!(!c.text.contains("line11"));
        assert_eq!(c.spans[0], Some((0, 2)));
        let (bs, bl) = c.spans[1].unwrap();
        assert_eq!(bs, 3);
        let units: Vec<u16> = c.text.encode_utf16().collect();
        assert_eq!(String::from_utf16_lossy(&units[(bs + bl) as usize..]), "\nbye", "the block now ends with its note");
        assert_eq!(c.spans[2], None, "inside the hidden part");
        let (as_, al) = c.spans[3].unwrap();
        assert_eq!(String::from_utf16_lossy(&units[as_ as usize..(as_ + al) as usize]), "bye");
        assert_eq!(c.notes.len(), 1);
        let (ns, nl, hidden) = c.notes[0];
        assert_eq!((String::from_utf16_lossy(&units[ns as usize..(ns + nl) as usize]).as_str(), hidden), ("… 20 more lines", 20));
    }

    #[test]
    fn several_blocks_and_astral_text_stay_aligned() {
        let a = block(20);
        let text = format!("😀 {a} x {a}");
        let len = |s: &str| s.encode_utf16().count() as u32;
        let (s1, s2) = (3, 3 + len(&a) + 3);
        let c = collapse_code(&text, &[(s1, len(&a)), (s2, len(&a))], &[(s2, len(&a))]).unwrap();
        assert_eq!(c.notes.len(), 2);
        assert_eq!(c.text.matches("… 10 more lines").count(), 2);
        let units: Vec<u16> = c.text.encode_utf16().collect();
        let (ss, sl) = c.spans[0].unwrap();
        assert!(String::from_utf16_lossy(&units[ss as usize..(ss + sl) as usize]).starts_with("line1\n"));
    }
}

