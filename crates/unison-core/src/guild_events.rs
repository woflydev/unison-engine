//! Scheduled events and stage instances of a guild.

use serde::{Deserialize, Serialize};

use crate::model::Id;

pub mod status {
    pub const SCHEDULED: u8 = 1;
    pub const ACTIVE: u8 = 2;
    pub const COMPLETED: u8 = 3;
    pub const CANCELED: u8 = 4;
}

pub mod entity {
    pub const STAGE: u8 = 1;
    pub const VOICE: u8 = 2;
    pub const EXTERNAL: u8 = 3;
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduledEvent {
    pub id: Id,
    pub guild_id: Id,
    pub channel_id: Option<Id>,
    pub creator_id: Option<Id>,
    pub name: String,
    pub description: Option<String>,
    pub scheduled_start_time: String,
    pub scheduled_end_time: Option<String>,
    pub status: u8,
    pub entity_type: u8,
    pub entity_metadata: Option<EventMetadata>,
    pub user_count: Option<u32>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventMetadata {
    pub location: Option<String>,
}

impl ScheduledEvent {
    pub fn start_ms(&self) -> u64 {
        crate::timefmt::iso_to_ms(&self.scheduled_start_time).unwrap_or(0)
    }
    pub fn end_ms(&self) -> Option<u64> {
        self.scheduled_end_time.as_deref().and_then(crate::timefmt::iso_to_ms)
    }
    pub fn is_live(&self) -> bool {
        self.status == status::ACTIVE
    }
    /// Shown in the list: not finished and not canceled.
    pub fn is_open(&self) -> bool {
        matches!(self.status, status::SCHEDULED | status::ACTIVE)
    }
    pub fn image_url(&self) -> Option<String> {
        self.image.as_ref().map(|h| format!("https://cdn.discordapp.com/guild-events/{}/{}.png?size=512", self.id, h))
    }
}

impl crate::store::Store {
    /// May I schedule events here?
    pub fn can_create_events(&self, guild: Id) -> bool {
        self.guild_permissions(guild) & (crate::store::perm::MANAGE_EVENTS | CREATE_EVENTS) != 0
    }

    /// May I edit, start, end or delete this event? Manage Events covers any; Create Events only my own.
    pub fn can_manage_event(&self, e: &ScheduledEvent) -> bool {
        let p = self.guild_permissions(e.guild_id);
        p & crate::store::perm::MANAGE_EVENTS != 0 || (p & CREATE_EVENTS != 0 && e.creator_id.is_some() && e.creator_id == self.my_id())
    }
}

/// Live events first (soonest start), then upcoming by start time.
pub fn sorted(list: &[ScheduledEvent]) -> Vec<&ScheduledEvent> {
    let mut v: Vec<&ScheduledEvent> = list.iter().filter(|e| e.is_open()).collect();
    v.sort_by_key(|e| (!e.is_live(), e.start_ms()));
    v
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StageInstance {
    pub id: Id,
    pub guild_id: Id,
    pub channel_id: Id,
    pub topic: String,
    pub privacy_level: u8,
}

/// Permission bits for events (`Manage Events` edits anyone's; `Create Events` only your own).
pub const CREATE_EVENTS: u64 = 1 << 44;

pub const NAME_MAX: usize = 100;
pub const DESCRIPTION_MAX: usize = 1000;
pub const LOCATION_MAX: usize = 100;
/// An event must start at least this far ahead when it is created or moved.
pub const MIN_LEAD_MS: u64 = 60_000;

/// What the event form holds. `check` says why it can't be saved yet; `body` is the request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventDraft {
    pub name: String,
    pub description: String,
    /// `entity::STAGE`, `VOICE` or `EXTERNAL`.
    pub kind: u8,
    pub channel_id: Option<Id>,
    pub location: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

impl EventDraft {
    /// `None` when the draft can be saved, otherwise one sentence for the person. `start_changed` is
    /// false when editing an event and leaving its start alone (a running event's start is in the past).
    pub fn check(&self, now_ms: u64, start_changed: bool) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Give the event a name.".into());
        }
        if self.name.trim().chars().count() > NAME_MAX {
            return Some(format!("The name can be {NAME_MAX} characters at most."));
        }
        if self.description.chars().count() > DESCRIPTION_MAX {
            return Some(format!("The description can be {DESCRIPTION_MAX} characters at most."));
        }
        match self.kind {
            entity::STAGE | entity::VOICE => {
                if self.channel_id.is_none() {
                    return Some("Pick the channel the event takes place in.".into());
                }
            }
            entity::EXTERNAL => {
                if self.location.trim().is_empty() {
                    return Some("Say where the event takes place.".into());
                }
                if self.location.trim().chars().count() > LOCATION_MAX {
                    return Some(format!("The location can be {LOCATION_MAX} characters at most."));
                }
                if self.end_ms.is_none() {
                    return Some("Events outside Discord need an end time.".into());
                }
            }
            _ => return Some("Pick what kind of event this is.".into()),
        }
        if start_changed && self.start_ms < now_ms + MIN_LEAD_MS {
            return Some("The event has to start in the future.".into());
        }
        if self.end_ms.is_some_and(|e| e <= self.start_ms) {
            return Some("The event has to end after it starts.".into());
        }
        None
    }

    /// `POST`/`PATCH /guilds/{g}/scheduled-events[/{id}]`. `image` is a data URI, `Some(Null)` removes it.
    pub fn body(&self, image: Option<serde_json::Value>) -> serde_json::Value {
        use serde_json::json;
        let desc = self.description.trim();
        let external = self.kind == entity::EXTERNAL;
        let mut b = json!({
            "name": self.name.trim(),
            "description": if desc.is_empty() { serde_json::Value::Null } else { desc.into() },
            "privacy_level": 2,
            "entity_type": self.kind,
            "channel_id": if external { serde_json::Value::Null } else { self.channel_id.map(|c| c.to_string()).into() },
            "entity_metadata": if external { json!({ "location": self.location.trim() }) } else { serde_json::Value::Null },
            "scheduled_start_time": crate::timefmt::ms_to_iso(self.start_ms),
            "scheduled_end_time": self.end_ms.map(crate::timefmt::ms_to_iso),
        });
        if let Some(i) = image {
            b["image"] = i;
        }
        b
    }
}

/// Where an event may go next, by the rules of the API: scheduled -> active or canceled, active -> completed.
pub fn next_status(current: u8, wanted: u8) -> bool {
    matches!((current, wanted), (status::SCHEDULED, status::ACTIVE) | (status::SCHEDULED, status::CANCELED) | (status::ACTIVE, status::COMPLETED))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    fn ev(id: u64, st: u8, start: &str) -> ScheduledEvent {
        ScheduledEvent { id: Snowflake(id), status: st, scheduled_start_time: start.into(), ..Default::default() }
    }

    #[test]
    fn live_first_then_by_start_and_hides_finished() {
        let l = vec![
            ev(1, status::SCHEDULED, "2026-11-02T10:00:00Z"),
            ev(2, status::SCHEDULED, "2026-11-01T10:00:00Z"),
            ev(3, status::ACTIVE, "2026-11-05T10:00:00Z"),
            ev(4, status::COMPLETED, "2026-10-01T10:00:00Z"),
            ev(5, status::CANCELED, "2026-10-02T10:00:00Z"),
        ];
        assert_eq!(sorted(&l).iter().map(|e| e.id.0).collect::<Vec<_>>(), vec![3, 2, 1]);
    }

    fn draft() -> EventDraft {
        EventDraft { name: "Movie night".into(), kind: entity::VOICE, channel_id: Some(Snowflake(5)), start_ms: 10_000_000, ..Default::default() }
    }

    #[test]
    fn a_draft_says_what_is_missing() {
        let now = 1_000_000;
        assert_eq!(draft().check(now, true), None);
        assert!(EventDraft { name: "  ".into(), ..draft() }.check(now, true).is_some());
        assert!(EventDraft { channel_id: None, ..draft() }.check(now, true).unwrap().contains("channel"));
        assert!(EventDraft { start_ms: now + 1000, ..draft() }.check(now, true).unwrap().contains("future"));
        assert_eq!(EventDraft { start_ms: now - 5, ..draft() }.check(now, false), None, "a running event keeps its start");
        assert!(EventDraft { end_ms: Some(draft().start_ms), ..draft() }.check(now, true).unwrap().contains("after"));
        let ext = EventDraft { kind: entity::EXTERNAL, channel_id: None, location: "Park".into(), ..draft() };
        assert!(ext.check(now, true).unwrap().contains("end time"));
        assert_eq!(EventDraft { end_ms: Some(ext.start_ms + 1), ..ext }.check(now, true), None);
    }

    #[test]
    fn the_body_matches_the_event_kind() {
        let b = draft().body(None);
        assert_eq!((b["entity_type"].as_u64(), b["channel_id"].as_str(), b["privacy_level"].as_u64()), (Some(2), Some("5"), Some(2)));
        assert!(b["entity_metadata"].is_null() && b["description"].is_null() && b.get("image").is_none());
        let ext = EventDraft { kind: entity::EXTERNAL, location: " Park ".into(), end_ms: Some(20_000_000), description: "bring snacks".into(), ..draft() }.body(Some(serde_json::Value::Null));
        assert!(ext["channel_id"].is_null() && ext["image"].is_null());
        assert_eq!((ext["entity_metadata"]["location"].as_str(), ext["description"].as_str()), (Some("Park"), Some("bring snacks")));
    }

    #[test]
    fn status_moves_one_way() {
        assert!(next_status(status::SCHEDULED, status::ACTIVE));
        assert!(next_status(status::ACTIVE, status::COMPLETED));
        assert!(!next_status(status::ACTIVE, status::CANCELED));
        assert!(!next_status(status::COMPLETED, status::ACTIVE));
    }
}
