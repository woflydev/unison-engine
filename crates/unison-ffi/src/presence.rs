use super::*;
use unison_core::activity::{self, Activity};

/// One thing a person is doing: a custom status, a game, a song. All text is ready to draw.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ActivityItem {
    pub kind: u8,
    /// "Playing", "Listening to", ... empty for a custom status.
    pub verb: String,
    pub name: String,
    pub details: String,
    pub state: String,
    /// Unicode emoji of a custom status.
    pub emoji: Option<String>,
    /// Image of a custom emoji (when `emoji` is `None`).
    pub emoji_url: Option<String>,
    pub image_url: Option<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

pub(crate) fn item(a: &Activity) -> ActivityItem {
    let custom_emoji = a.emoji_id.is_some();
    ActivityItem {
        kind: a.kind,
        verb: a.verb().to_string(),
        name: a.name.clone(),
        details: a.details.clone(),
        state: a.state.clone(),
        emoji: (!custom_emoji && !a.emoji_name.is_empty()).then(|| a.emoji_name.clone()),
        emoji_url: a.emoji_url(),
        image_url: a.image_url(),
        start_ms: a.start_ms,
        end_ms: a.end_ms,
    }
}

/// What a member or DM row shows under the name: the custom status, else the primary activity.
pub(crate) fn row_activity(list: &[Activity]) -> Option<ActivityItem> {
    let c = activity::custom(list).filter(|c| !c.state.is_empty());
    c.or_else(|| activity::primary(list)).map(item)
}

#[uniffi::export]
impl UnisonEngine {
    /// Everything a person is doing right now, custom status first (the profile card).
    pub fn user_activities(&self, account_id: String, user_id: String) -> Vec<ActivityItem> {
        self.with_store(&account_id, |s| {
            let mut v: Vec<&Activity> = s.user_activities(id(&user_id)).iter().collect();
            v.sort_by_key(|a| a.kind != activity::CUSTOM);
            v.into_iter().map(item).collect()
        })
        .unwrap_or_default()
    }
}
