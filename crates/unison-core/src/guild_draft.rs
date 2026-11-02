//! Unsaved per-server notification settings (Settings, Notifications). The rules for keeping a
//! `PATCH /users/@me/guilds/{id}/settings` body aside, laying it over the saved settings for
//! display and dropping what no longer differs, so every shell shows and saves the same thing.
use serde_json::{Map, Value, json};

use crate::model::{ChannelOverride, UserGuildSettings};

/// Fold `newer` into `into`: top-level keys replace, `channel_overrides` merges per channel.
pub fn merge(into: &mut Map<String, Value>, newer: &Value) {
    let Some(newer) = newer.as_object() else { return };
    for (k, v) in newer {
        if k == "channel_overrides" {
            let slot = into.entry(k.clone()).or_insert_with(|| json!({}));
            if let (Some(slot), Some(v)) = (slot.as_object_mut(), v.as_object()) {
                for (channel, over) in v {
                    let one = slot.entry(channel.clone()).or_insert_with(|| json!({}));
                    if let (Some(one), Some(over)) = (one.as_object_mut(), over.as_object()) {
                        one.extend(over.iter().map(|(a, b)| (a.clone(), b.clone())));
                    }
                }
            }
        } else {
            into.insert(k.clone(), v.clone());
        }
    }
}

fn override_of<'a>(base: &'a UserGuildSettings, channel: &str) -> Option<&'a ChannelOverride> {
    base.channel_overrides.iter().find(|o| o.channel_id.to_string() == channel)
}

/// Drop everything the saved settings already say. True when nothing is left.
pub fn prune(body: &mut Map<String, Value>, base: &UserGuildSettings) -> bool {
    if body.get("muted").and_then(Value::as_bool) == Some(base.muted) {
        body.remove("muted");
        body.remove("mute_config");
    }
    if body.get("message_notifications").and_then(Value::as_u64) == Some(u64::from(base.message_notifications)) {
        body.remove("message_notifications");
    }
    for (key, saved) in [("suppress_everyone", base.suppress_everyone), ("suppress_roles", base.suppress_roles)] {
        if body.get(key).and_then(Value::as_bool) == Some(saved) {
            body.remove(key);
        }
    }
    if let Some(overs) = body.get_mut("channel_overrides").and_then(Value::as_object_mut) {
        overs.retain(|channel, over| {
            let Some(o) = over.as_object_mut() else { return false };
            // A channel with no override of its own reads as unmuted, level 3 (the server's).
            let (muted, level) = override_of(base, channel).map_or((false, 3), |s| (s.muted, s.message_notifications));
            if o.get("muted").and_then(Value::as_bool) == Some(muted) {
                o.remove("muted");
                o.remove("mute_config");
            }
            if o.get("message_notifications").and_then(Value::as_u64) == Some(u64::from(level)) {
                o.remove("message_notifications");
            }
            !o.is_empty()
        });
        if overs.is_empty() {
            body.remove("channel_overrides");
        }
    }
    body.is_empty()
}

/// The saved settings with the unsaved body laid over them.
pub fn apply(base: &UserGuildSettings, body: &Map<String, Value>) -> UserGuildSettings {
    let mut s = base.clone();
    if let Some(v) = body.get("muted").and_then(Value::as_bool) {
        s.muted = v;
    }
    if let Some(v) = body.get("message_notifications").and_then(Value::as_u64) {
        s.message_notifications = v.min(3) as u8;
    }
    if let Some(v) = body.get("suppress_everyone").and_then(Value::as_bool) {
        s.suppress_everyone = v;
    }
    if let Some(v) = body.get("suppress_roles").and_then(Value::as_bool) {
        s.suppress_roles = v;
    }
    if let Some(overs) = body.get("channel_overrides").and_then(Value::as_object) {
        for (channel, over) in overs {
            let Ok(id) = channel.parse::<u64>().map(crate::snowflake::Snowflake) else { continue };
            let at = match s.channel_overrides.iter().position(|o| o.channel_id == id) {
                Some(i) => i,
                None => {
                    s.channel_overrides.push(ChannelOverride { channel_id: id, message_notifications: 3, ..Default::default() });
                    s.channel_overrides.len() - 1
                }
            };
            let o = &mut s.channel_overrides[at];
            if let Some(v) = over.get("muted").and_then(Value::as_bool) {
                o.muted = v;
            }
            if let Some(v) = over.get("message_notifications").and_then(Value::as_u64) {
                o.message_notifications = v.min(3) as u8;
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    fn base() -> UserGuildSettings {
        UserGuildSettings {
            message_notifications: 1,
            channel_overrides: vec![ChannelOverride { channel_id: Snowflake(9), muted: true, message_notifications: 3, collapsed: false }],
            ..Default::default()
        }
    }

    #[test]
    fn staged_changes_show_over_the_saved_ones_and_vanish_when_undone() {
        let mut body = Map::new();
        merge(&mut body, &json!({ "message_notifications": 0 }));
        merge(&mut body, &json!({ "suppress_roles": true }));
        merge(&mut body, &json!({ "channel_overrides": { "9": { "muted": false, "mute_config": null } } }));
        let shown = apply(&base(), &body);
        assert_eq!(shown.message_notifications, 0);
        assert!(shown.suppress_roles && !shown.channel_overrides[0].muted);
        assert!(!prune(&mut body, &base()), "three real changes");
        merge(&mut body, &json!({ "message_notifications": 1 }));
        merge(&mut body, &json!({ "suppress_roles": false }));
        merge(&mut body, &json!({ "channel_overrides": { "9": { "muted": true } } }));
        assert!(prune(&mut body, &base()), "all back to what is saved: nothing to send");
    }

    #[test]
    fn a_channel_without_an_override_reads_as_the_servers() {
        let mut body = Map::new();
        merge(&mut body, &json!({ "channel_overrides": { "5": { "message_notifications": 3 } } }));
        assert!(prune(&mut body, &base()), "level 3 on a channel with none is no change");
        merge(&mut body, &json!({ "channel_overrides": { "5": { "muted": true } } }));
        assert_eq!(apply(&base(), &body).channel_overrides.len(), 2);
    }
}
