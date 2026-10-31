//! Op 37 (`GUILD_SUBSCRIPTIONS_BULK`) bookkeeping, shaped like the official client's
//! guild subscription store.
//!
//! The official client never resends what a guild is already subscribed to. Each flush
//! carries only what changed per guild: `typing`, `activities` and `threads` once, the
//! first time the guild is viewed, and the `channels` map whenever a channel's ranges change.
//! The channel map is an LRU of the five most recently viewed channels, most recent first,
//! each with its member-list ranges (`[[0, 99]]` by default). Payloads over 15 360 bytes
//! of JSON are split, as `updateGuildSubscriptions` does.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::model::Id;

/// `max: 5` in the official channel-subscription LRU.
const MAX_CHANNELS: usize = 5;
/// `updateGuildSubscriptions` starts a new op 37 once this much JSON is queued.
const MAX_PAYLOAD: usize = 15_360;

pub const DEFAULT_RANGES: &[(u32, u32)] = &[(0, 99)];

#[derive(Debug, Default)]
pub struct Subscriptions {
    featured: HashSet<Id>,
    /// Most recent first.
    channels: HashMap<Id, Vec<(Id, Vec<(u32, u32)>)>>,
    /// Per guild, in the order fields were queued.
    pending: Vec<(Id, Map<String, Value>)>,
}

impl Subscriptions {
    /// A new gateway session starts with nothing subscribed.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Typing, activities and threads for a guild being viewed. Queues nothing if already sent.
    pub fn guild(&mut self, guild: Id) {
        if self.featured.insert(guild) {
            let p = self.pending_for(guild);
            p.insert("typing".into(), Value::Bool(true));
            p.insert("activities".into(), Value::Bool(true));
            p.insert("threads".into(), Value::Bool(true));
        }
    }

    /// A channel's member-list ranges. Queues the whole channel map only when they changed.
    pub fn channel(&mut self, guild: Id, channel: Id, ranges: &[(u32, u32)]) {
        let lru = self.channels.entry(guild).or_default();
        if lru.first().is_some_and(|(c, r)| *c == channel && r == ranges) {
            return;
        }
        let unchanged = lru.iter().any(|(c, r)| *c == channel && r == ranges);
        lru.retain(|(c, _)| *c != channel);
        lru.insert(0, (channel, ranges.to_vec()));
        lru.truncate(MAX_CHANNELS);
        // Revisiting a channel with the same ranges only refreshes its recency, like `lru.set`
        // of an equal value: the official store reports no change and sends nothing.
        if unchanged {
            return;
        }
        let map: Map<String, Value> = lru
            .iter()
            .map(|(c, r)| (c.to_string(), Value::Array(r.iter().map(|(a, b)| json!([a, b])).collect())))
            .collect();
        self.pending_for(guild).insert("channels".into(), Value::Object(map));
    }

    fn pending_for(&mut self, guild: Id) -> &mut Map<String, Value> {
        let i = match self.pending.iter().position(|(g, _)| *g == guild) {
            Some(i) => i,
            None => {
                self.pending.push((guild, Map::new()));
                self.pending.len() - 1
            }
        };
        &mut self.pending[i].1
    }

    /// Everything queued, as op 37 `d` payloads (usually one).
    pub fn flush(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        let mut batch = Map::new();
        let mut size = 0;
        for (guild, fields) in std::mem::take(&mut self.pending) {
            let key = guild.to_string();
            let value = Value::Object(fields);
            let len = json!([key, value]).to_string().len();
            if size + len > MAX_PAYLOAD && !batch.is_empty() {
                out.push(json!({ "subscriptions": Value::Object(std::mem::take(&mut batch)) }));
                size = 0;
            }
            batch.insert(key, value);
            size += len;
        }
        if !batch.is_empty() {
            out.push(json!({ "subscriptions": Value::Object(batch) }));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    #[test]
    fn first_view_sends_features_then_channel() {
        let mut s = Subscriptions::default();
        s.guild(Snowflake(1));
        s.channel(Snowflake(1), Snowflake(10), DEFAULT_RANGES);
        let out = s.flush();
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            r#"[{"subscriptions":{"1":{"typing":true,"activities":true,"threads":true,"channels":{"10":[[0,99]]}}}}]"#
        );
        // Same guild, same channel: nothing to send.
        s.guild(Snowflake(1));
        s.channel(Snowflake(1), Snowflake(10), DEFAULT_RANGES);
        assert!(s.flush().is_empty());
    }

    #[test]
    fn channel_map_is_recent_first_and_capped() {
        let mut s = Subscriptions::default();
        for c in 10..17 {
            s.channel(Snowflake(1), Snowflake(c), DEFAULT_RANGES);
        }
        let out = s.flush();
        let chans = out[0]["subscriptions"]["1"]["channels"].as_object().unwrap();
        assert_eq!(chans.keys().cloned().collect::<Vec<_>>(), ["16", "15", "14", "13", "12"]);
        // Going back to a channel still in the map only reorders it locally.
        s.channel(Snowflake(1), Snowflake(14), DEFAULT_RANGES);
        assert!(s.flush().is_empty());
        // Wider ranges are a change.
        s.channel(Snowflake(1), Snowflake(14), &[(0, 99), (100, 199)]);
        let out = s.flush();
        let chans = out[0]["subscriptions"]["1"]["channels"].as_object().unwrap();
        assert_eq!(chans.keys().next().map(String::as_str), Some("14"));
    }

    #[test]
    fn forum_only_subscribes_features() {
        let mut s = Subscriptions::default();
        s.guild(Snowflake(1));
        assert_eq!(
            serde_json::to_string(&s.flush()).unwrap(),
            r#"[{"subscriptions":{"1":{"typing":true,"activities":true,"threads":true}}}]"#
        );
    }

    #[test]
    fn reset_resends() {
        let mut s = Subscriptions::default();
        s.guild(Snowflake(1));
        s.flush();
        s.reset();
        s.guild(Snowflake(1));
        assert_eq!(s.flush().len(), 1);
    }
}
