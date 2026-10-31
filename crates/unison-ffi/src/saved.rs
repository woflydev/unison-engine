//! Saved-for-later messages and reminders (rules in `unison_core::saved`; kept on this device).

use unison_core::saved::{self, Saved};

use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct SavedItem {
    pub account_id: String,
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub author: String,
    pub avatar_url: String,
    pub snippet: String,
    pub location: String,
    pub saved_ms: u64,
    /// When the reminder fires; the shell gives this to the OS scheduler.
    pub remind_at_ms: Option<u64>,
    /// A reminder time that has passed.
    pub reminder_due: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ReminderChoice {
    pub index: u8,
    pub label: String,
}

fn item(s: Saved, now: u64) -> SavedItem {
    SavedItem {
        account_id: s.account.to_string(),
        guild_id: s.guild.map(|g| g.to_string()),
        channel_id: s.channel.to_string(),
        message_id: s.message.to_string(),
        author: s.author,
        avatar_url: s.avatar_url,
        snippet: s.snippet,
        location: s.location,
        saved_ms: s.saved_ms,
        reminder_due: s.remind_at_ms.is_some_and(|t| t <= now),
        remind_at_ms: s.remind_at_ms,
    }
}

#[uniffi::export]
pub fn reminder_choices() -> Vec<ReminderChoice> {
    saved::CHOICES.iter().enumerate().map(|(i, (l, _))| ReminderChoice { index: i as u8, label: (*l).into() }).collect()
}

#[uniffi::export]
impl UnisonEngine {
    /// Everything saved, newest first, across accounts.
    pub fn saved_messages(&self) -> Vec<SavedItem> {
        let now = now_ms();
        self.inner
            .local
            .saved()
            .into_iter()
            .map(|mut s| {
                // Stored as the message said it (raw mentions): shown with names, resolved now so a
                // renamed role or a newly known member reads right, and old entries are fixed too.
                if let Some(p) = self.with_store(&s.account.to_string(), |st| messages::preview_text(st, s.guild, &s.snippet, 200)) {
                    s.snippet = p;
                }
                item(s, now)
            })
            .collect()
    }

    pub fn is_saved(&self, account_id: String, message_id: String) -> bool {
        let (a, m) = (id(&account_id), id(&message_id));
        self.inner.local.saved().iter().any(|s| s.account == a && s.message == m)
    }

    /// Save a loaded message for later (keeps its author and a preview, so it shows even when the
    /// channel isn't loaded). Does nothing for a message the engine doesn't have.
    pub fn save_message(&self, account_id: String, channel_id: String, message_id: String) {
        let (c, m) = (id(&channel_id), id(&message_id));
        let built = self.with_store(&account_id, |s| {
            let msg = s.history.get(&c)?.messages.iter().find(|x| x.id == m)?;
            Some(Saved {
                account: id(&account_id),
                guild: s.channels.get(&c).and_then(|ch| ch.guild_id),
                channel: c,
                message: m,
                author: msg.author.display_name().to_string(),
                avatar_url: msg.author.avatar_url(64),
                snippet: saved::snippet(&msg.content, msg.attachments.len()),
                location: messages::location(s, c),
                saved_ms: now_ms(),
                remind_at_ms: None,
            })
        });
        if let Some(Some(item)) = built {
            self.inner.local.edit_saved(|l| saved::add(l, item));
        }
    }

    pub fn unsave_message(&self, account_id: String, message_id: String) {
        let (a, m) = (id(&account_id), id(&message_id));
        self.inner.local.edit_saved(|l| saved::remove(l, a, m));
    }

    /// Remind about a message at one of `reminder_choices` (saving it first if needed). Returns the
    /// time so the shell can schedule the notification.
    pub fn remind_message(&self, account_id: String, channel_id: String, message_id: String, choice: u8, utc_offset_secs: i32) -> Option<u64> {
        let delay = saved::CHOICES.get(choice as usize)?.1;
        if !self.is_saved(account_id.clone(), message_id.clone()) {
            self.save_message(account_id.clone(), channel_id, message_id.clone());
        }
        let at = saved::remind_at(delay, now_ms(), utc_offset_secs);
        let (a, m) = (id(&account_id), id(&message_id));
        let mut ok = false;
        self.inner.local.edit_saved(|l| ok = saved::set_reminder(l, a, m, Some(at)));
        ok.then_some(at)
    }

    pub fn clear_reminder(&self, account_id: String, message_id: String) {
        let (a, m) = (id(&account_id), id(&message_id));
        self.inner.local.edit_saved(|l| {
            saved::set_reminder(l, a, m, None);
        });
    }
}
