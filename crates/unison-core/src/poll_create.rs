//! Creating a poll: the limits, the one-line refusals and the request body. The shell draws the
//! form and asks this module what is allowed; nothing here talks to Discord.
//!
//! Limits are the web client's poll modal: a question up to 300 characters, two to ten answers of
//! up to 55 characters, a duration from a fixed list, "allow multiple answers". Creating a poll
//! needs the Send Polls permission in a server (always allowed in a DM).

use serde_json::{json, Value};

use crate::snowflake::Snowflake;
use crate::store::perm;

pub const MAX_QUESTION: usize = 300;
pub const MAX_ANSWER: usize = 55;
pub const MIN_ANSWERS: usize = 2;
pub const MAX_ANSWERS: usize = 10;
/// How long a poll runs when the person does not pick (the web client's default: one day).
pub const DEFAULT_HOURS: u32 = 24;

/// Durations offered, in hours, with the label the form shows.
pub const DURATIONS: [(u32, &str); 7] = [
    (1, "1 hour"),
    (4, "4 hours"),
    (8, "8 hours"),
    (24, "24 hours"),
    (72, "3 days"),
    (168, "1 week"),
    (336, "2 weeks"),
];

/// Send Polls (`1 << 49`).
pub const SEND_POLLS: u64 = 1 << 49;

/// May this person start a poll here? `guild` is false in a DM and a group DM.
pub fn can_create(in_guild: bool, perms: u64) -> bool {
    if !in_guild {
        return true;
    }
    perms & perm::ADMINISTRATOR != 0 || perms & SEND_POLLS != 0 && perms & perm::SEND_MESSAGES != 0
}

/// An answer's optional emoji: a custom one has an id, a Unicode one only its glyph as `name`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnswerEmoji {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnswerDraft {
    pub text: String,
    pub emoji: Option<AnswerEmoji>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollDraft {
    pub question: String,
    pub answers: Vec<AnswerDraft>,
    pub hours: u32,
    pub multiselect: bool,
}

fn chars(s: &str) -> usize {
    s.trim().chars().count()
}

/// Answers with something in them, in order. Blank rows are ignored, like the web form.
pub fn filled(answers: &[AnswerDraft]) -> Vec<&AnswerDraft> {
    answers.iter().filter(|a| !a.text.trim().is_empty()).collect()
}

/// Why this poll cannot be sent yet, in one plain sentence; `None` when it can.
pub fn refusal(d: &PollDraft) -> Option<String> {
    if chars(&d.question) == 0 {
        return Some("Ask a question".into());
    }
    if chars(&d.question) > MAX_QUESTION {
        return Some(format!("The question can be at most {MAX_QUESTION} characters"));
    }
    let answers = filled(&d.answers);
    if answers.len() < MIN_ANSWERS {
        return Some(format!("Add at least {MIN_ANSWERS} answers"));
    }
    if answers.len() > MAX_ANSWERS {
        return Some(format!("A poll can have at most {MAX_ANSWERS} answers"));
    }
    if answers.iter().any(|a| chars(&a.text) > MAX_ANSWER) {
        return Some(format!("An answer can be at most {MAX_ANSWER} characters"));
    }
    if !DURATIONS.iter().any(|(h, _)| *h == d.hours) {
        return Some("Pick how long the poll runs".into());
    }
    None
}

/// `POST /channels/{id}/messages` body for a poll (`capture`: key order and `layout_type` follow
/// the public API object; the web client's modal posts the same `poll` with an empty `content`).
pub fn body(nonce: Snowflake, d: &PollDraft) -> Value {
    let answers: Vec<Value> = filled(&d.answers)
        .into_iter()
        .map(|a| {
            let mut media = serde_json::Map::new();
            media.insert("text".into(), a.text.trim().into());
            if let Some(e) = &a.emoji {
                media.insert(
                    "emoji".into(),
                    match &e.id {
                        Some(id) => json!({ "id": id, "name": e.name }),
                        None => json!({ "name": e.name }),
                    },
                );
            }
            json!({ "poll_media": Value::Object(media) })
        })
        .collect();
    json!({
        "mobile_network_type": "unknown",
        "content": "",
        "nonce": nonce.to_string(),
        "tts": false,
        "flags": 0,
        "poll": {
            "question": { "text": d.question.trim() },
            "answers": answers,
            "allow_multiselect": d.multiselect,
            "duration": d.hours,
            "layout_type": 1,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(q: &str, a: &[&str]) -> PollDraft {
        PollDraft {
            question: q.into(),
            answers: a.iter().map(|t| AnswerDraft { text: (*t).into(), emoji: None }).collect(),
            hours: DEFAULT_HOURS,
            multiselect: false,
        }
    }

    #[test]
    fn needs_a_question_and_two_answers() {
        assert!(refusal(&draft("", &["a", "b"])).is_some());
        assert!(refusal(&draft("Q", &["a", ""])).is_some());
        assert_eq!(refusal(&draft("Q", &["a", "b", ""])), None);
    }

    #[test]
    fn limits_are_counted_in_characters() {
        assert!(refusal(&draft(&"x".repeat(301), &["a", "b"])).is_some());
        assert_eq!(refusal(&draft(&"é".repeat(300), &["a", "b"])), None);
        assert!(refusal(&draft("Q", &[&"y".repeat(56), "b"])).is_some());
        let eleven: Vec<&str> = vec!["a"; 11];
        assert!(refusal(&draft("Q", &eleven)).is_some());
    }

    #[test]
    fn duration_must_be_offered() {
        let mut d = draft("Q", &["a", "b"]);
        d.hours = 5;
        assert!(refusal(&d).is_some());
    }

    #[test]
    fn body_skips_blank_answers_and_carries_emoji() {
        let mut d = draft(" Q ", &["a", " ", "b"]);
        d.answers[0].emoji = Some(AnswerEmoji { id: Some("9".into()), name: "yes".into() });
        d.answers[2].emoji = Some(AnswerEmoji { id: None, name: "👍".into() });
        d.multiselect = true;
        let b = body(Snowflake(5), &d);
        assert_eq!(b["content"], "");
        assert_eq!(b["poll"]["question"]["text"], "Q");
        assert_eq!(b["poll"]["answers"].as_array().unwrap().len(), 2);
        assert_eq!(b["poll"]["answers"][0]["poll_media"]["emoji"], json!({"id": "9", "name": "yes"}));
        assert_eq!(b["poll"]["answers"][1]["poll_media"]["emoji"], json!({"name": "👍"}));
        assert_eq!(b["poll"]["duration"], 24);
        assert_eq!(b["poll"]["allow_multiselect"], true);
    }

    #[test]
    fn permission_rule() {
        assert!(can_create(false, 0));
        assert!(!can_create(true, perm::SEND_MESSAGES));
        assert!(can_create(true, perm::SEND_MESSAGES | SEND_POLLS));
        assert!(!can_create(true, SEND_POLLS));
        assert!(can_create(true, perm::ADMINISTRATOR));
    }
}
