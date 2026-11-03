//! Polls, message components, modals, forwards and thread chips on messages, plus the commands
//! that act on them (component interactions, poll votes, thread membership and creation).

use super::*;
use unison_core::interactions::{self as core_i, ComponentBlock};

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct PollAnswerItem {
    pub id: u32,
    pub text: String,
    pub emoji: Option<String>,
    pub count: u32,
    pub me: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct PollItem {
    pub question: String,
    pub answers: Vec<PollAnswerItem>,
    pub multiselect: bool,
    pub finalized: bool,
    /// Expiry as unix milliseconds, when the poll has one.
    pub expiry_ms: Option<u64>,
    pub total: u32,
}

/// One answer row of the poll form: text plus an optional emoji (`emoji_id` for a custom one).
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PollAnswerDraft {
    pub text: String,
    pub emoji_name: Option<String>,
    pub emoji_id: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PollDurationItem {
    pub hours: u32,
    pub label: String,
}

/// The poll form's limits, so the shell never carries a number of its own.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PollRules {
    pub max_question: u32,
    pub max_answer: u32,
    pub min_answers: u32,
    pub max_answers: u32,
    pub default_hours: u32,
    pub durations: Vec<PollDurationItem>,
}

fn poll_draft(question: String, answers: Vec<PollAnswerDraft>, hours: u32, multiselect: bool) -> unison_core::poll_create::PollDraft {
    use unison_core::poll_create::{AnswerDraft, AnswerEmoji, PollDraft};
    PollDraft {
        question,
        answers: answers
            .into_iter()
            .map(|a| AnswerDraft {
                text: a.text,
                emoji: a.emoji_name.map(|name| AnswerEmoji { id: a.emoji_id, name }),
            })
            .collect(),
        hours,
        multiselect,
    }
}

#[uniffi::export]
pub fn poll_rules() -> PollRules {
    use unison_core::poll_create as p;
    PollRules {
        max_question: p::MAX_QUESTION as u32,
        max_answer: p::MAX_ANSWER as u32,
        min_answers: p::MIN_ANSWERS as u32,
        max_answers: p::MAX_ANSWERS as u32,
        default_hours: p::DEFAULT_HOURS,
        durations: p::DURATIONS.iter().map(|(h, l)| PollDurationItem { hours: *h, label: (*l).into() }).collect(),
    }
}

/// Why the form cannot be sent yet (one plain sentence), or nothing when it can.
#[uniffi::export]
pub fn poll_refusal(question: String, answers: Vec<PollAnswerDraft>, hours: u32, multiselect: bool) -> Option<String> {
    unison_core::poll_create::refusal(&poll_draft(question, answers, hours, multiselect))
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct ComponentButton {
    pub style: u8,
    pub label: String,
    pub emoji: Option<String>,
    pub custom_id: Option<String>,
    pub url: Option<String>,
    pub disabled: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct ComponentOption {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
    pub selected: bool,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Hash)]
pub enum ComponentBlockItem {
    Buttons {
        buttons: Vec<ComponentButton>,
    },
    Select {
        component_type: u8,
        custom_id: String,
        placeholder: Option<String>,
        options: Vec<ComponentOption>,
        min_values: u32,
        max_values: u32,
        disabled: bool,
    },
    Text {
        text: RichText,
    },
    Separator,
    Media {
        items: Vec<ComponentMedia>,
    },
    Unsupported,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct ComponentMedia {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: Option<String>,
}

/// A forwarded message's content (`message_snapshots[0].message`).
#[derive(uniffi::Record, Clone, Debug)]
pub struct ForwardItem {
    pub content: RichText,
    pub attachments: Vec<AttachmentItem>,
    pub embeds: Vec<EmbedItem>,
    pub timestamp_ms: Option<u64>,
    /// Where it was forwarded from (channel and message), when the reference says.
    pub source_channel_id: Option<String>,
    pub source_message_id: Option<String>,
}

/// The thread a message started: a chip under the message.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Hash)]
pub struct ThreadChip {
    pub id: String,
    pub name: String,
    pub message_count: u32,
    pub joined: bool,
    pub archived: bool,
    pub unread: bool,
    /// When the thread last had a message (ms since the epoch, from the id of its newest message), 0 if unknown.
    pub active_ms: u64,
    /// Who said the newest message the account has loaded of this thread, and the start of it ("Sam: see you
    /// there"), mentions as names; empty when none is loaded (the entrance then shows only the count).
    pub preview: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ModalFieldItem {
    pub custom_id: String,
    pub label: String,
    pub placeholder: Option<String>,
    pub value: Option<String>,
    pub paragraph: bool,
    pub required: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ModalItem {
    pub title: String,
    pub fields: Vec<ModalFieldItem>,
}

fn ms_from_iso(s: &str) -> Option<u64> {
    chrono_lite(s)
}

/// `2025-01-02T03:04:05.678000+00:00` -> unix ms (UTC offsets other than 0 are applied).
fn chrono_lite(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time_end = rest.find(['+', 'Z', '-']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(time_end);
    let mut t = time.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec_f: f64 = t.next().unwrap_or("0").parse().ok()?;
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut secs = days * 86400 + h * 3600 + mi * 60;
    if let Some(sign) = zone.chars().next().filter(|c| *c == '+' || *c == '-') {
        let z = &zone[1..];
        let (zh, zm) = z.split_once(':').unwrap_or((z, "0"));
        let off = zh.parse::<i64>().ok()? * 3600 + zm.parse::<i64>().ok()? * 60;
        secs -= if sign == '+' { off } else { -off };
    }
    Some((secs as f64 * 1000.0 + sec_f * 1000.0) as u64)
}

pub(crate) fn poll_item(m: &Message) -> Option<PollItem> {
    let p = core_i::parse_poll(m.poll.as_ref()?)?;
    Some(PollItem {
        question: p.question,
        answers: p
            .answers
            .into_iter()
            .map(|a| PollAnswerItem { id: a.id, text: a.text, emoji: a.emoji, count: a.count, me: a.me })
            .collect(),
        multiselect: p.multiselect,
        finalized: p.finalized,
        expiry_ms: p.expiry.as_deref().and_then(ms_from_iso),
        total: p.total,
    })
}

pub(crate) fn component_items(s: &Store, guild: Option<Id>, m: &Message) -> Vec<ComponentBlockItem> {
    core_i::parse_components(&m.components)
        .into_iter()
        .map(|b| match b {
            ComponentBlock::Buttons(v) => ComponentBlockItem::Buttons {
                buttons: v
                    .into_iter()
                    .map(|b| ComponentButton {
                        style: b.style,
                        label: b.label,
                        emoji: b.emoji,
                        custom_id: b.custom_id,
                        url: b.url.map(|u| privacy::clean_outbound_url(&u)),
                        disabled: b.disabled,
                    })
                    .collect(),
            },
            ComponentBlock::Select(s) => ComponentBlockItem::Select {
                component_type: s.component_type,
                custom_id: s.custom_id,
                placeholder: s.placeholder,
                options: s
                    .options
                    .into_iter()
                    .map(|o| ComponentOption { label: o.label, value: o.value, description: o.description, selected: o.default })
                    .collect(),
                min_values: s.min_values,
                max_values: s.max_values,
                disabled: s.disabled,
            },
            ComponentBlock::Text(t) => ComponentBlockItem::Text { text: crate::messages::rich(s, guild, &t) },
            ComponentBlock::Separator => ComponentBlockItem::Separator,
            ComponentBlock::Media(v) => ComponentBlockItem::Media {
                items: v.into_iter().map(|m| ComponentMedia { url: m.url, width: m.width, height: m.height, alt: m.alt }).collect(),
            },
            ComponentBlock::Unsupported => ComponentBlockItem::Unsupported,
        })
        .collect()
}

pub(crate) fn forward_item(s: &Store, guild: Option<Id>, m: &Message) -> Option<ForwardItem> {
    let snap = m.message_snapshots.first()?;
    let inner: Message = serde_json::from_value(snap["message"].clone()).ok()?;
    let r = m.message_reference.as_ref();
    Some(ForwardItem {
        content: rich(s, guild, &inner.content),
        attachments: inner.attachments.iter().map(attachment_item).collect(),
        embeds: inner.embeds.iter().map(|e| embed_item(s, guild, e)).collect(),
        timestamp_ms: ms_from_iso(&inner.timestamp),
        source_channel_id: r.and_then(|r| r.channel_id).map(|c| c.to_string()),
        source_message_id: r.and_then(|r| r.message_id).map(|c| c.to_string()),
    })
}

pub(crate) fn thread_chip(s: &Store, m: &Message) -> Option<ThreadChip> {
    // The message's own `thread` is a snapshot; the live channel (member, archive state) wins.
    let t = m.thread.as_ref()?;
    let live = s.channels.get(&t.id).unwrap_or(t);
    Some(ThreadChip {
        id: live.id.to_string(),
        name: live.name.clone().unwrap_or_default(),
        message_count: live.message_count.unwrap_or(0),
        joined: live.member.is_some(),
        archived: live.is_archived(),
        unread: live.member.is_some() && s.is_unread(live) && !s.is_channel_muted(live),
        active_ms: live.last_message_id.map_or(0, |m| m.timestamp_ms()),
        // An archived thread is over: its newest message is not news, so the card does not show it.
        preview: if live.is_archived() { String::new() } else { s
            .history
            .get(&live.id)
            .and_then(|h| h.messages.iter().rev().find(|m| m.id.0 != 0))
            .map(|m| {
                let text = crate::messages::preview_text(s, live.guild_id, &m.content, 90);
                let who = s.display_name(live.guild_id, m.author.id);
                if text.is_empty() { format!("{who}: attachment") } else { format!("{who}: {text}") }
            })
            .unwrap_or_default() },
    })
}

#[uniffi::export]
impl UnisonEngine {
    /// Press a button, or send a select's choice (`values` is `Some`). Same request as the web
    /// client's component click.
    pub fn press_component(
        &self,
        account_id: String,
        channel_id: String,
        message_id: String,
        component_type: u8,
        custom_id: String,
        values: Option<Vec<String>>,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.component_interact(i, id(&channel_id), id(&message_id), component_type, custom_id, values).await
        });
    }

    /// Post a poll (the composer's + menu). The core re-checks the form and refuses with a sentence.
    pub fn create_poll(
        &self,
        account_id: String,
        channel_id: String,
        question: String,
        answers: Vec<PollAnswerDraft>,
        hours: u32,
        multiselect: bool,
        gesture: UserGesture,
    ) {
        let draft = poll_draft(question, answers, hours, multiselect);
        self.spawn_write(account_id, gesture, move |s, i| async move { s.send_poll(i, id(&channel_id), &draft).await });
    }

    /// Click a poll answer: votes, changes or clears the vote as the web client does.
    pub fn vote_poll(&self, account_id: String, channel_id: String, message_id: String, answer_id: u32, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.poll_vote(i, id(&channel_id), id(&message_id), answer_id).await
        });
    }

    /// The modal an application opened, until answered.
    pub fn pending_modal(&self, account_id: String) -> Option<ModalItem> {
        self.with_store(&account_id, |s| {
            s.modal.as_ref().map(|m| ModalItem {
                title: m.title.clone(),
                fields: m
                    .fields
                    .iter()
                    .map(|f| ModalFieldItem {
                        custom_id: f.custom_id.clone(),
                        label: f.label.clone(),
                        placeholder: f.placeholder.clone(),
                        value: f.value.clone(),
                        paragraph: f.paragraph,
                        required: f.required,
                    })
                    .collect(),
            })
        })
        .flatten()
    }

    /// Submit the open modal (`values` is `[(custom_id, text)]`) or cancel it with `None`.
    pub fn answer_modal(&self, account_id: String, values: Option<Vec<ModalValue>>, gesture: UserGesture) {
        let fields = values.map(|v| v.into_iter().map(|x| (x.custom_id, x.value)).collect());
        self.spawn_write(account_id, gesture, move |s, i| async move { s.answer_modal(i, fields).await });
    }

    /// Follow (`join`) or leave a thread.
    pub fn set_thread_membership(&self, account_id: String, thread_id: String, join: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.thread_membership(i, id(&thread_id), join).await
        });
    }

    /// Start a thread on a message (`message_id` set) or in the channel.
    pub fn create_thread(
        &self,
        account_id: String,
        channel_id: String,
        message_id: Option<String>,
        name: String,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.create_thread(i, id(&channel_id), message_id.as_deref().map(id), name).await.map(|_| ())
        });
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ModalValue {
    pub custom_id: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_to_ms() {
        assert_eq!(chrono_lite("1970-01-02T00:00:00+00:00"), Some(86_400_000));
        assert_eq!(chrono_lite("2024-03-01T12:00:00.500000+00:00"), Some(1_709_294_400_500));
        assert_eq!(chrono_lite("2024-03-01T14:00:00+02:00"), Some(1_709_294_400_000));
    }
}
