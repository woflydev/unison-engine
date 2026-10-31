//! "Saved for later" messages and reminders. Kept on this device (`localstate`): Discord's own
//! synced list isn't used because its requests aren't captured yet. A reminder is a time; the shell
//! hands it to the OS scheduler, so nothing here runs a timer.

use serde::{Deserialize, Serialize};

use crate::model::Id;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub account: Id,
    pub guild: Option<Id>,
    pub channel: Id,
    pub message: Id,
    pub author: String,
    pub avatar_url: String,
    /// What the message said when it was saved, shortened.
    pub snippet: String,
    pub location: String,
    pub saved_ms: u64,
    pub remind_at_ms: Option<u64>,
}

/// A saved message's preview: the first line of text, or a note about attachments.
pub fn snippet(content: &str, attachments: usize) -> String {
    let text: String = content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect();
    match (text.is_empty(), attachments) {
        (false, _) => text,
        (true, 0) => String::new(),
        (true, 1) => "Attachment".into(),
        (true, n) => format!("{n} attachments"),
    }
}

/// When to remind, relative to now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delay {
    Minutes(u64),
    Hours(u64),
    /// Next calendar day at 9:00 local time.
    TomorrowMorning,
    /// The coming Monday at 9:00 local time.
    NextWeek,
}

pub const CHOICES: [(&str, Delay); 5] = [
    ("In 20 minutes", Delay::Minutes(20)),
    ("In 1 hour", Delay::Hours(1)),
    ("In 3 hours", Delay::Hours(3)),
    ("Tomorrow morning", Delay::TomorrowMorning),
    ("Next week", Delay::NextWeek),
];

const DAY_MS: u64 = 86_400_000;
const MORNING_MS: u64 = 9 * 3_600_000;

/// The time (unix ms) a delay lands on. `utc_offset_secs` is the user's offset from UTC.
pub fn remind_at(delay: Delay, now_ms: u64, utc_offset_secs: i32) -> u64 {
    let offset = utc_offset_secs as i64 * 1000;
    let local = now_ms as i64 + offset;
    let day = local.div_euclid(DAY_MS as i64);
    let at_morning = |days: i64| ((day + days) * DAY_MS as i64 + MORNING_MS as i64 - offset) as u64;
    match delay {
        Delay::Minutes(m) => now_ms + m * 60_000,
        Delay::Hours(h) => now_ms + h * 3_600_000,
        Delay::TomorrowMorning => at_morning(1),
        Delay::NextWeek => {
            // 1970-01-01 was a Thursday; Monday is 0.
            let weekday = (day + 3).rem_euclid(7);
            let ahead = match (7 - weekday) % 7 {
                0 => 7,
                n => n,
            };
            at_morning(ahead)
        }
    }
}

/// Newest saved first; one entry per message.
pub fn add(list: &mut Vec<Saved>, item: Saved) {
    list.retain(|s| !(s.account == item.account && s.message == item.message));
    list.insert(0, item);
}

pub fn remove(list: &mut Vec<Saved>, account: Id, message: Id) {
    list.retain(|s| !(s.account == account && s.message == message));
}

pub fn set_reminder(list: &mut [Saved], account: Id, message: Id, at: Option<u64>) -> bool {
    match list.iter_mut().find(|s| s.account == account && s.message == message) {
        Some(s) => {
            s.remind_at_ms = at;
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake as S;

    fn item(m: u64) -> Saved {
        Saved { account: S(1), guild: None, channel: S(2), message: S(m), author: "a".into(), avatar_url: String::new(), snippet: "hi".into(), location: String::new(), saved_ms: 0, remind_at_ms: None }
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet("  hello \n  world ", 0), "hello world");
        assert_eq!(snippet("", 1), "Attachment");
        assert_eq!(snippet("", 3), "3 attachments");
        assert_eq!(snippet(&"x".repeat(300), 0).len(), 200);
    }

    #[test]
    fn delays_land_on_local_mornings() {
        // 2024-01-03 (Wednesday) 15:30 UTC, user at UTC+10.
        let now = 1_704_295_800_000u64;
        let off = 10 * 3600;
        assert_eq!(remind_at(Delay::Minutes(20), now, off), now + 1_200_000);
        let tomorrow = remind_at(Delay::TomorrowMorning, now, off);
        // Locally it is Thursday 4 Jan 01:30, so tomorrow morning is 5 Jan 09:00 at UTC+10.
        assert_eq!(tomorrow, 1_704_409_200_000);
        let week = remind_at(Delay::NextWeek, now, off);
        // The coming Monday is 8 Jan, 09:00 local.
        assert_eq!(week, 1_704_668_400_000);
    }

    #[test]
    fn list_edits() {
        let mut l = vec![];
        add(&mut l, item(10));
        add(&mut l, item(11));
        add(&mut l, item(10));
        assert_eq!(l.iter().map(|s| s.message.0).collect::<Vec<_>>(), [10, 11]);
        assert!(set_reminder(&mut l, S(1), S(11), Some(5)));
        assert!(!set_reminder(&mut l, S(1), S(99), Some(5)));
        remove(&mut l, S(1), S(10));
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].remind_at_ms, Some(5));
    }
}
