//! Small conveniences: link cleaning, the `@everyone` check, notification keywords and quiet
//! hours, friend nicknames and transcript export. Rules live in `unison_core::comfort`.
use super::*;

use unison_core::comfort::{self, Comfort, QuietHours};
use unison_core::send_rules;

#[derive(uniffi::Record, Clone, Debug)]
pub struct ComfortItem {
    pub clean_links: bool,
    pub confirm_everyone: bool,
    pub strip_location: bool,
    /// Videos open full size over the dimmed window; off plays them in place.
    pub video_viewer: bool,
    /// HDR videos play in standard range (no brightening of the display).
    pub video_sdr: bool,
    /// Trim an overlong message (the same three-way choice as compression).
    pub trim_choice: crate::uploads::CompressChoice,
    /// Comma-separated. Read-only: the list is edited with `add_keywords` / `remove_keyword`.
    pub keywords: String,
    /// The notification words, in the order they were added (one chip each).
    pub keyword_list: Vec<String>,
    pub quiet_enabled: bool,
    /// Minutes after local midnight.
    pub quiet_start: u16,
    pub quiet_end: u16,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CleanedLinks {
    pub text: String,
    pub changed: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FileMetaItem {
    pub name: String,
    pub size: u64,
    pub content_type: String,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum SendAction {
    /// Send the text as it is.
    Send,
    /// Send `text` (the part that fits) and put `rest` back in the composer.
    Trim,
    /// Ask first (`note` says what will happen), then send `text`.
    Confirm,
    /// Don't send; `note` says why and what to do.
    Blocked,
}

/// What the composer should do with a message about to be sent.
#[derive(uniffi::Record, Clone, Debug)]
pub struct SendPlanItem {
    pub action: SendAction,
    pub text: String,
    pub rest: String,
    pub note: Option<String>,
}

/// The character counter next to the composer.
#[derive(uniffi::Record, Clone, Debug)]
pub struct CounterItem {
    pub len: u32,
    pub limit: u32,
    pub visible: bool,
    pub over: bool,
}

fn to_item(c: &Comfort) -> ComfortItem {
    let q = c.quiet.unwrap_or(QuietHours { start: 22 * 60, end: 7 * 60 });
    ComfortItem {
        clean_links: c.clean_links,
        confirm_everyone: c.confirm_everyone,
        strip_location: c.strip_location,
        video_viewer: c.video_viewer,
        video_sdr: c.video_sdr,
        trim_choice: match c.trim_long {
            None => crate::uploads::CompressChoice::Automatic,
            Some(true) => crate::uploads::CompressChoice::On,
            Some(false) => crate::uploads::CompressChoice::Off,
        },
        keywords: c.keywords.join(", "),
        keyword_list: c.keywords.clone(),
        quiet_enabled: c.quiet.is_some(),
        quiet_start: q.start,
        quiet_end: q.end,
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct KeywordsResult {
    pub list: Vec<String>,
    pub note: Option<String>,
}

impl UnisonEngine {
    /// Persist the list and apply it to every open account at once (no restart).
    fn save_keywords(&self, c: &Comfort) {
        self.inner.local.set_comfort(c);
        for s in self.inner.sessions_snapshot() {
            s.store.write().notify_keywords = c.keywords.clone();
        }
    }
}

#[uniffi::export]
impl UnisonEngine {
    pub fn comfort(&self) -> ComfortItem {
        to_item(&self.inner.local.comfort())
    }

    /// Add the words in `text` to the notification list (several at once when pasted). `note` is the
    /// one plain line to show when nothing was added.
    pub fn add_keywords(&self, text: String) -> KeywordsResult {
        let mut c = self.inner.local.comfort();
        let r = comfort::add_keywords(&mut c.keywords, &text);
        self.save_keywords(&c);
        KeywordsResult { list: c.keywords, note: comfort::keyword_note(r).map(str::to_string) }
    }

    pub fn remove_keyword(&self, word: String) -> Vec<String> {
        let mut c = self.inner.local.comfort();
        comfort::remove_keyword(&mut c.keywords, &word);
        self.save_keywords(&c);
        c.keywords
    }

    pub fn set_comfort(&self, item: ComfortItem) {
        // The keyword list is edited only through add/remove, so a stale item never overwrites it.
        let keywords = self.inner.local.comfort().keywords;
        let c = Comfort {
            clean_links: item.clean_links,
            confirm_everyone: item.confirm_everyone,
            strip_location: item.strip_location,
            video_viewer: item.video_viewer,
            video_sdr: item.video_sdr,
            trim_long: match item.trim_choice {
                crate::uploads::CompressChoice::Automatic => None,
                crate::uploads::CompressChoice::On => Some(true),
                crate::uploads::CompressChoice::Off => Some(false),
            },
            keywords,
            quiet: item.quiet_enabled.then_some(QuietHours { start: item.quiet_start.min(1439), end: item.quiet_end.min(1439) }),
        };
        self.inner.local.set_comfort(&c);
        for s in self.inner.sessions_snapshot() {
            s.store.write().notify_keywords = c.keywords.clone();
        }
    }

    /// Tracking parameters removed from pasted text (nothing changes when the option is off).
    pub fn clean_pasted_links(&self, text: String) -> CleanedLinks {
        if !self.inner.local.comfort().clean_links {
            return CleanedLinks { text, changed: 0 };
        }
        let (text, n) = comfort::clean_links(&text);
        CleanedLinks { text, changed: n as u32 }
    }

    /// Whether a banner may show now. `minute_of_day` is the shell's local time.
    pub fn banners_allowed(&self, minute_of_day: u16) -> bool {
        self.inner.local.comfort().quiet.is_none_or(|q| !comfort::is_quiet(&q, minute_of_day))
    }

    /// Decide what happens to a message before it is sent: too long (trim or refuse), too many or
    /// too large files, or a ping to a whole channel (confirm). Local only; nothing is requested.
    pub fn send_plan(&self, account_id: String, channel_id: String, text: String, files: Vec<FileMetaItem>) -> SendPlanItem {
        let c = self.inner.local.comfort();
        let acct = self.with_store(&account_id, |s| {
            let tier = s.channels.get(&id(&channel_id)).and_then(|c| c.guild_id).and_then(|g| s.guilds.get(&g)).map(|g| g.inline.premium_tier);
            send_rules::Account {
                premium_type: s.me.as_ref().and_then(|u| u.premium_type),
                boost_tier: tier,
                trim_choice: c.trim_long,
                compress: unison_core::upload_policy::effective(self.inner.local.compress_uploads(), s.me.as_ref().and_then(|u| u.premium_type)),
            }
        });
        let Some(acct) = acct else { return SendPlanItem { action: SendAction::Send, text, rest: String::new(), note: None } };
        let metas: Vec<send_rules::FileMeta> = files.into_iter().map(|f| send_rules::FileMeta { name: f.name, size: f.size, content_type: f.content_type }).collect();
        let (action, text, rest, note) = match send_rules::check(&text, &metas, &acct) {
            send_rules::Verdict::Blocked(m) => (SendAction::Blocked, text, String::new(), Some(m)),
            send_rules::Verdict::Trim(t) => {
                let note = format!("Sent the first {} characters. The last {} are still in the box.", send_rules::char_len(&t.send), t.removed);
                (SendAction::Trim, t.send, t.rest, Some(note))
            }
            send_rules::Verdict::Send => (SendAction::Send, text, String::new(), None),
        };
        // A ping check only applies to what will really be sent.
        if action != SendAction::Blocked && c.confirm_everyone {
            if let Some(word) = comfort::everyone_ping(&text) {
                let ask = self
                    .with_store(&account_id, |s| {
                        let ch = s.channels.get(&id(&channel_id))?;
                        let g = ch.guild_id?;
                        // Without the permission the text is just text: nothing is pinged.
                        if s.permissions(ch) & (1 << 17) == 0 {
                            return None;
                        }
                        let who = s.guilds.get(&g).and_then(|g| g.member_count).map(|n| format!("about {n} people")).unwrap_or_else(|| "everyone here".into());
                        Some(format!("{word} will notify {who} in #{}.", ch.name.clone().unwrap_or_default()))
                    })
                    .flatten();
                if let Some(ask) = ask {
                    return SendPlanItem { action: SendAction::Confirm, text, rest, note: Some(ask) };
                }
            }
        }
        SendPlanItem { action, text, rest, note }
    }

    /// The length counter for text in the composer (the official rule decides when it shows).
    pub fn counter(&self, account_id: String, text: String) -> CounterItem {
        let limit = send_rules::char_limit(self.with_store(&account_id, |s| s.me.as_ref().and_then(|u| u.premium_type)).flatten());
        let len = send_rules::char_len(text.trim());
        CounterItem { len, limit, visible: send_rules::counter_visible(len, limit), over: len > limit }
    }

    /// Open a message's long code blocks (local view state).
    pub fn expand_code(&self, account_id: String, channel_id: String, message_id: String) {
        if let Some(s) = self.inner.session(id(&account_id)) {
            s.expand_code(id(&channel_id), id(&message_id));
        }
    }

    pub fn friend_nickname(&self, account_id: String, user_id: String) -> Option<String> {
        self.with_store(&account_id, |s| s.friend_nicks.get(&id(&user_id)).cloned()).flatten()
    }

    /// Only you see it. Empty clears it.
    pub fn set_friend_nickname(&self, account_id: String, user_id: String, nickname: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.set_friend_nickname(i, id(&user_id), &nickname).await });
    }

    /// The messages loaded for a channel as plain text, oldest first (UTC times). Nothing is fetched.
    pub fn export_channel(&self, account_id: String, channel_id: String) -> Option<String> {
        self.with_store(&account_id, |s| {
            let c = id(&channel_id);
            let ch = s.channels.get(&c)?;
            let guild = ch.guild_id;
            let title = match guild {
                Some(_) => format!("#{}", ch.name.clone().unwrap_or_default()),
                None => dms::dm_item(s, ch).name,
            };
            let rows: Vec<comfort::ExportRow> = s
                .history
                .get(&c)?
                .messages
                .iter()
                .filter(|m| m.id.0 != 0)
                .map(|m| comfort::ExportRow {
                    when: format!("{} UTC", m.timestamp.chars().take(16).collect::<String>().replace('T', " ")),
                    author: s.display_name(guild, m.author.id),
                    content: m.content.clone(),
                    attachments: m.attachments.iter().map(|a| a.url.clone()).collect(),
                })
                .collect();
            Some(comfort::export_text(&title, &rows))
        })
        .flatten()
    }
}
