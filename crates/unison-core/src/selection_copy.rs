//! What copying a selection across messages puts on the clipboard (UX.md 5c.8).
//!
//! The shell tracks the drag and hands over, for each message the selection touches, the author, the
//! time as it shows it, and the part of the text that is selected. The result is the same on every
//! platform: a message that starts a block (author's name shown) is preceded by `Author — time`, the
//! messages of one author that follow it are plain lines, and a system note is its own line.

/// One message the selection touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub author: String,
    /// The time as the shell shows it ("Today at 10:22").
    pub time_label: String,
    /// The selected text of the message (all of it for a message in the middle of the selection).
    pub text: String,
    /// The message continues the author's block above it: no name line.
    pub grouped: bool,
    /// A call, a join or a pin: its text is the whole line, with no name.
    pub system: bool,
}

/// The text to copy, or an empty string when nothing is selected.
pub fn copy_text(parts: &[Part]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for p in parts {
        if p.text.is_empty() {
            continue;
        }
        if !p.system && !p.grouped && !p.author.is_empty() {
            lines.push(format!("{} — {}", p.author, p.time_label));
        }
        lines.push(p.text.clone());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(author: &str, text: &str, grouped: bool) -> Part {
        Part { author: author.into(), time_label: "10:22".into(), text: text.into(), grouped, system: false }
    }

    #[test]
    fn a_block_of_one_author_gets_one_name_line() {
        let out = copy_text(&[p("Ana", "hi", false), p("Ana", "there", true), p("Bo", "yo", false)]);
        assert_eq!(out, "Ana — 10:22\nhi\nthere\nBo — 10:22\nyo");
    }

    #[test]
    fn a_partial_first_message_keeps_only_the_selected_text() {
        assert_eq!(copy_text(&[p("Ana", "llo", true), p("Ana", "next", true)]), "llo\nnext");
    }

    #[test]
    fn system_notes_and_empty_messages() {
        let sys = Part { author: "Ana".into(), time_label: "9:00".into(), text: "Ana started a call.".into(), grouped: false, system: true };
        assert_eq!(copy_text(&[p("Ana", "", false), sys, p("Bo", "hey", false)]), "Ana started a call.\nBo — 10:22\nhey");
        assert_eq!(copy_text(&[]), "");
    }
}
