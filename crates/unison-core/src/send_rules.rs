//! What a message may be before it is sent: its length, its files, and what to do when it doesn't
//! fit. Limits come from the official client (web build 655747): 2000 characters, 4000 with the
//! increased-length perk (Nitro, `premium_type` 2 only; Basic and Classic keep 2000), counted in
//! UTF-16 units (`String.length`), at most 10 files, each under the upload limit.
//!
//! Trimming is one message only: the part that fits is sent and the rest stays in the composer for
//! the user to send, edit or drop with their own gesture. Nothing here ever sends a second message.

use crate::upload_policy;

pub const BASE_CHARS: u32 = 2000;
pub const NITRO_CHARS: u32 = 4000;
pub const MAX_FILES: usize = 10;
/// Most file bytes one message may carry in total (the web client's `getMaxTotalAttachmentSize`;
/// an experiment raises it to 1 GiB, which we don't assume).
pub const MAX_TOTAL_BYTES: u64 = 500 * 1024 * 1024;
/// Room kept for the closing fence added to a trimmed code block.
const FENCE_CLOSE: &str = "\n```";

pub fn char_limit(premium_type: Option<u8>) -> u32 {
    if premium_type == Some(2) { NITRO_CHARS } else { BASE_CHARS }
}

/// The length Discord counts: UTF-16 code units.
pub fn char_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// `None` follows the account: trimming is on when the limit is the base 2000 (no Nitro), off with
/// Nitro, where overlong text is simply refused.
pub fn trim_enabled(choice: Option<bool>, premium_type: Option<u8>) -> bool {
    choice.unwrap_or(char_limit(premium_type) == BASE_CHARS)
}

/// The counter appears once the remainder is a tenth of the limit or less (the official rule).
pub fn counter_visible(len: u32, limit: u32) -> bool {
    limit.saturating_sub(len) <= limit / 10
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trimmed {
    /// What goes out now, at most `limit` units.
    pub send: String,
    /// What stays in the composer (empty when nothing was cut).
    pub rest: String,
    /// Characters left behind (UTF-16 units of `rest`).
    pub removed: u32,
}

/// Cut `text` to `limit` units at the friendliest boundary near the end: a paragraph break, a line
/// break, a sentence end, a space, else the last whole character. Never inside a `<@id>`-style
/// token. A code block that is open at the cut is closed in `send` and reopened in `rest`.
pub fn trim_to_limit(text: &str, limit: u32) -> Trimmed {
    let text = text.trim();
    if char_len(text) <= limit {
        return Trimmed { send: text.to_string(), rest: String::new(), removed: 0 };
    }
    let mut room = limit;
    let mut cut = best_cut(text, room);
    let open_fence = |cut: usize| fence_state(&text[..cut]);
    if open_fence(cut).is_some() {
        room = limit.saturating_sub(char_len(FENCE_CLOSE));
        cut = best_cut(text, room);
    }
    // Cutting back may have closed or reopened a block; trust the final position.
    let fence = open_fence(cut);
    let mut send = text[..cut].trim_end().to_string();
    let mut rest = text[cut..].trim_start().to_string();
    if let Some(lang) = fence {
        send.push_str(FENCE_CLOSE);
        rest = format!("```{lang}\n{rest}");
    }
    if send.is_empty() {
        // One unbroken run longer than the limit: take whole characters.
        let hard = hard_cut(text, limit);
        send = text[..hard].to_string();
        rest = text[hard..].to_string();
    }
    let removed = char_len(&rest);
    Trimmed { send, rest, removed }
}

/// Byte index of the largest prefix of `text` within `max` UTF-16 units.
fn hard_cut(text: &str, max: u32) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        units += c.len_utf16() as u32;
        if units > max {
            return i;
        }
    }
    text.len()
}

fn best_cut(text: &str, max: u32) -> usize {
    let hard = hard_cut(text, max);
    // Look back at most a tenth of the limit for something nicer than the hard cut.
    let window = (max / 10).max(20) as usize;
    let lo = text[..hard].char_indices().rev().nth(window).map_or(0, |(i, _)| i);
    let zone = &text[lo..hard];
    let after = |needle: &str| zone.rfind(needle).map(|i| lo + i + needle.len());
    let mut cut = after("\n\n")
        .or_else(|| after("\n"))
        .or_else(|| [". ", "! ", "? "].iter().filter_map(|n| after(n)).max())
        .or_else(|| after(" "))
        .unwrap_or(hard);
    // Never split a token like <@123>, <#123>, <:name:123> or <t:123:R>.
    if let Some(open) = text[..cut].rfind('<') {
        let token = &text[open..cut];
        if !token.contains('>') && !token.contains(char::is_whitespace) && token.len() > 1 {
            cut = open;
        }
    }
    if cut == 0 { hard } else { cut }
}

/// The language of the code block still open at the end of `s` (empty string for none), or `None`
/// when every fence is closed.
fn fence_state(s: &str) -> Option<String> {
    let mut open: Option<String> = None;
    let mut rest = s;
    while let Some(i) = rest.find("```") {
        let after = &rest[i + 3..];
        match open {
            None => {
                let lang: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '+' || *c == '-' || *c == '_').collect();
                open = Some(lang);
            }
            Some(_) => open = None,
        }
        rest = after;
    }
    open
}

// ----- Checking a send -----

#[derive(Debug, Clone, PartialEq)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
    pub content_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub premium_type: Option<u8>,
    pub boost_tier: Option<u8>,
    /// The saved choice for trimming (`None` = follow the account).
    pub trim_choice: Option<bool>,
    /// Whether oversized photos and videos will be shrunk before sending.
    pub compress: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Send exactly this.
    Send,
    /// Send `send`; `rest` stays in the composer.
    Trim(Trimmed),
    /// Don't send; say why.
    Blocked(String),
}

fn size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB { format!("{:.1} MB", bytes as f64 / MB).replace(".0 MB", " MB") } else { format!("{} KB", (bytes / 1024).max(1)) }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Decide what happens to a message with `text` and `files`. Files that are over the limit pass
/// when the plan will shrink them (`upload_policy::plan`); the shell shrinks them before sending
/// and the session checks the result again.
pub fn check(text: &str, files: &[FileMeta], acct: &Account) -> Verdict {
    if files.len() > MAX_FILES {
        let over = files.len() - MAX_FILES;
        return Verdict::Blocked(format!("You can attach up to {MAX_FILES} files to one message. Remove {}.", plural(over, "file", "files")));
    }
    let limit = upload_policy::limit_bytes(acct.premium_type, acct.boost_tier);
    if let Some(f) = files.iter().find(|f| f.size == 0) {
        return Verdict::Blocked(format!("{} is empty, so there is nothing to send. Remove it.", f.name));
    }
    for f in files {
        if f.size <= limit {
            continue;
        }
        let shrinkable = !matches!(upload_policy::plan(true, limit, f.size, &f.content_type), upload_policy::Plan::Keep);
        let head = format!("{} is {}; the limit here is {}.", f.name, size(f.size), size(limit));
        if shrinkable && acct.compress {
            continue;
        }
        return Verdict::Blocked(if shrinkable {
            format!("{head} Turn on compression in Settings > Uploads & Extras, or pick a smaller file.")
        } else {
            format!("{head} Pick a smaller file.")
        });
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    if total > MAX_TOTAL_BYTES {
        return Verdict::Blocked(format!("These files add up to {}; one message can carry up to {}. Send some in another message.", size(total), size(MAX_TOTAL_BYTES)));
    }
    let chars = char_limit(acct.premium_type);
    let len = char_len(text.trim());
    if len > chars {
        if trim_enabled(acct.trim_choice, acct.premium_type) {
            return Verdict::Trim(trim_to_limit(text, chars));
        }
        return Verdict::Blocked(format!("Your message is {len} characters and the limit is {chars}. Remove {}.", plural((len - chars) as usize, "character", "characters")));
    }
    Verdict::Send
}

/// The same checks on files that are already as small as they will get (after shrinking), so the
/// session never sends something Discord is certain to refuse.
pub fn check_final(files: &[(String, u64)], premium_type: Option<u8>, boost_tier: Option<u8>) -> Result<(), String> {
    let limit = upload_policy::limit_bytes(premium_type, boost_tier);
    let total: u64 = files.iter().map(|(_, s)| *s).sum();
    if total > MAX_TOTAL_BYTES {
        return Err(format!("These files add up to {}; one message can carry up to {}. Nothing was sent.", size(total), size(MAX_TOTAL_BYTES)));
    }
    match files.iter().find(|(_, s)| *s > limit) {
        Some((name, s)) => Err(format!("{name} is {}; the limit here is {}. It can't be made small enough, so nothing was sent.", size(*s), size(limit))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(premium: Option<u8>) -> Account {
        Account { premium_type: premium, boost_tier: None, trim_choice: None, compress: true }
    }

    fn long(n: usize) -> String {
        "word ".repeat(n / 5 + 1)[..n].to_string()
    }

    #[test]
    fn limits_follow_nitro_only() {
        assert_eq!(char_limit(None), 2000);
        assert_eq!(char_limit(Some(1)), 2000, "Classic");
        assert_eq!(char_limit(Some(3)), 2000, "Basic");
        assert_eq!(char_limit(Some(2)), 4000);
        assert!(trim_enabled(None, None) && !trim_enabled(None, Some(2)));
        assert!(!trim_enabled(Some(false), None) && trim_enabled(Some(true), Some(2)));
    }

    #[test]
    fn length_counts_utf16_units() {
        assert_eq!(char_len("abc"), 3);
        assert_eq!(char_len("😀"), 2);
        assert_eq!(char_len("é"), 1);
        assert!(!counter_visible(1700, 2000));
        assert!(counter_visible(1800, 2000) && counter_visible(2500, 2000));
    }

    #[test]
    fn fits_means_untouched() {
        let t = trim_to_limit("  hello  ", 2000);
        assert_eq!((t.send.as_str(), t.rest.as_str(), t.removed), ("hello", "", 0));
        let exact = "a".repeat(2000);
        assert_eq!(trim_to_limit(&exact, 2000).rest, "");
    }

    #[test]
    fn trims_at_a_paragraph_then_a_line_then_a_space() {
        let head = long(1900);
        let text = format!("{head}\n\n{}", long(300));
        let t = trim_to_limit(&text, 2000);
        assert_eq!(t.send, head.trim_end());
        assert!(t.rest.starts_with("word") && !t.rest.starts_with('\n'));
        assert_eq!(format!("{}\n\n{}", t.send, t.rest).replace(' ', ""), text.trim().replace(' ', ""), "nothing is lost");
        assert!(char_len(&t.send) <= 2000);

        let by_line = format!("{head}\n{}", long(300));
        assert_eq!(trim_to_limit(&by_line, 2000).send, head.trim_end());

        let t = trim_to_limit(&long(2500), 2000);
        assert!(char_len(&t.send) <= 2000 && t.send.ends_with("word"), "cut at a space, not mid-word: {:?}", &t.send[t.send.len() - 8..]);
        assert_eq!(t.removed, char_len(&t.rest));
    }

    #[test]
    fn an_unbroken_run_is_cut_on_a_character_boundary() {
        let text = "😀".repeat(1500); // 3000 units
        let t = trim_to_limit(&text, 2000);
        assert_eq!(char_len(&t.send), 2000);
        assert_eq!(t.send.chars().count(), 1000);
        assert_eq!(char_len(&t.rest), 1000);
    }

    #[test]
    fn mentions_and_emoji_tokens_are_not_split() {
        let head = "x".repeat(1990);
        let text = format!("{head}<@123456789012345678> and more words after it");
        let t = trim_to_limit(&text, 2000);
        assert_eq!(t.send, head, "cut before the mention");
        assert!(t.rest.starts_with("<@123456789012345678>"));
        let text = format!("{head}<:blob:123456789> tail words here");
        let t = trim_to_limit(&text, 2000);
        assert!(!t.send.contains('<'));
    }

    #[test]
    fn an_open_code_block_is_closed_and_reopened() {
        let code = (0..400).map(|i| format!("let v{i} = {i};")).collect::<Vec<_>>().join("\n");
        let text = format!("intro\n```rust\n{code}\n```\nafter");
        let t = trim_to_limit(&text, 2000);
        assert!(char_len(&t.send) <= 2000);
        assert!(t.send.ends_with("\n```"), "closed: {:?}", &t.send[t.send.len().saturating_sub(12)..]);
        assert_eq!(t.send.matches("```").count() % 2, 0, "balanced");
        assert!(t.rest.starts_with("```rust\n"), "reopened with its language");
        assert!(t.rest.ends_with("after"));
        // A cut after the block has closed adds nothing.
        let text = format!("```\nshort\n```\n{}", long(2200));
        let t = trim_to_limit(&text, 2000);
        assert_eq!(t.send.matches("```").count(), 2);
        assert!(!t.rest.starts_with("```"));
    }

    #[test]
    fn check_applies_trim_by_account() {
        let text = "a ".repeat(1150);
        assert!(matches!(check(&text, &[], &acct(None)), Verdict::Trim(_)), "free: automatic trim");
        assert_eq!(check(&text, &[], &acct(Some(2))), Verdict::Send, "Nitro fits 4000");
        let big = "a".repeat(4300);
        match check(&big, &[], &acct(Some(2))) {
            Verdict::Blocked(m) => assert!(m.contains("4300") && m.contains("limit is 4000") && m.contains("300 characters"), "{m}"),
            v => panic!("{v:?}"),
        }
        let mut off = acct(None);
        off.trim_choice = Some(false);
        match check(&text, &[], &off) {
            Verdict::Blocked(m) => assert!(m.contains("2299") && m.contains("limit is 2000"), "{m}"),
            v => panic!("{v:?}"),
        }
    }

    fn file(name: &str, mb: u64, ct: &str) -> FileMeta {
        FileMeta { name: name.into(), size: mb * 1024 * 1024, content_type: ct.into() }
    }

    #[test]
    fn files_are_checked_before_anything_is_sent() {
        assert_eq!(check("", &[file("a.png", 3, "image/png")], &acct(None)), Verdict::Send);
        // Shrinkable and compression on: allowed through.
        assert_eq!(check("", &[file("a.mp4", 50, "video/mp4")], &acct(None)), Verdict::Send);
        // Compression off: say how to fix it.
        let mut off = acct(None);
        off.compress = false;
        match check("", &[file("a.mp4", 50, "video/mp4")], &off) {
            Verdict::Blocked(m) => assert!(m.contains("a.mp4 is 50 MB") && m.contains("limit here is 10 MB") && m.contains("compression"), "{m}"),
            v => panic!("{v:?}"),
        }
        // Not shrinkable at all.
        match check("", &[file("a.zip", 50, "application/zip")], &acct(None)) {
            Verdict::Blocked(m) => assert!(m.contains("Pick a smaller file") && !m.contains("compression"), "{m}"),
            v => panic!("{v:?}"),
        }
        // With Nitro the same zip fits.
        assert_eq!(check("", &[file("a.zip", 50, "application/zip")], &acct(Some(2))), Verdict::Send);
        let many: Vec<FileMeta> = (0..12).map(|i| file(&format!("{i}.png"), 1, "image/png")).collect();
        match check("", &many, &acct(None)) {
            Verdict::Blocked(m) => assert!(m.contains("up to 10 files") && m.contains("Remove 2 files"), "{m}"),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn empty_files_and_oversized_totals_are_refused() {
        let empty = FileMeta { name: "z.txt".into(), size: 0, content_type: "text/plain".into() };
        match check("", &[empty], &acct(None)) {
            Verdict::Blocked(m) => assert!(m.contains("z.txt is empty"), "{m}"),
            v => panic!("{v:?}"),
        }
        // Ten 60 MB files fit one by one with Nitro but not together.
        let many: Vec<FileMeta> = (0..10).map(|i| file(&format!("{i}.zip"), 60, "application/zip")).collect();
        match check("", &many, &acct(Some(2))) {
            Verdict::Blocked(m) => assert!(m.contains("add up to 600 MB") && m.contains("up to 500 MB"), "{m}"),
            v => panic!("{v:?}"),
        }
        let names: Vec<(String, u64)> = (0..10).map(|i| (format!("{i}.zip"), 60 * 1024 * 1024)).collect();
        assert!(check_final(&names, Some(2), None).unwrap_err().contains("Nothing was sent"));
        assert!(check_final(&names[..8], Some(2), None).is_ok(), "480 MB is within the cap");
    }

    #[test]
    fn final_check_catches_what_shrinking_could_not_fix() {
        assert!(check_final(&[("a".into(), 5 * 1024 * 1024)], None, None).is_ok());
        let e = check_final(&[("a.mp4".into(), 30 * 1024 * 1024)], None, None).unwrap_err();
        assert!(e.contains("a.mp4 is 30 MB") && e.contains("nothing was sent"), "{e}");
        assert!(check_final(&[("a.mp4".into(), 30 * 1024 * 1024)], Some(2), None).is_ok());
        // A boosted server raises the limit for everyone in it.
        assert!(check_final(&[("a".into(), 40 * 1024 * 1024)], None, Some(2)).is_ok());
    }
}

/// Most destinations one forward can go to, as the official picker allows. Each is its own request,
/// sent in turn (`FORWARD_GAP` apart), never all at once.
pub const MAX_FORWARD_TARGETS: usize = 5;
pub const FORWARD_GAP: std::time::Duration = std::time::Duration::from_millis(700);
