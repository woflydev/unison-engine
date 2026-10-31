//! Where the user is and where they have been: back/forward history, the channel remembered per
//! server, the last place to reopen on launch, and the rules for stepping to the previous/next
//! channel or server. Shells call this; they never keep their own copy (docs/ARCHITECTURE.md).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::Id;

const HISTORY_CAP: usize = 60;

/// A place: a channel, and its server (`None` for DMs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    pub guild: Option<Id>,
    pub channel: Id,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NavState {
    #[serde(skip)]
    history: Vec<Stop>,
    #[serde(skip)]
    index: usize,
    /// Channel last open in each server, keyed by the server id (`"@me"` for DMs).
    last_in: HashMap<String, Id>,
    last: Option<Stop>,
}

fn key(guild: Option<Id>) -> String {
    guild.map_or_else(|| "@me".to_string(), |g| g.to_string())
}

impl NavState {
    /// Note a visit. `from_history` is a back/forward move that already repositioned the cursor.
    pub fn visit(&mut self, stop: Stop, from_history: bool) {
        self.last_in.insert(key(stop.guild), stop.channel);
        self.last = Some(stop);
        if from_history || self.history.get(self.index) == Some(&stop) {
            return;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.index + 1);
        }
        self.history.push(stop);
        if self.history.len() > HISTORY_CAP {
            self.history.remove(0);
        }
        self.index = self.history.len() - 1;
    }

    /// Move the cursor by `delta` (-1 back, 1 forward) and return where to go.
    pub fn travel(&mut self, delta: i32) -> Option<Stop> {
        let i = self.index as i64 + delta as i64;
        let stop = *self.history.get(usize::try_from(i).ok()?)?;
        self.index = i as usize;
        Some(stop)
    }

    pub fn can_travel(&self, delta: i32) -> bool {
        let i = self.index as i64 + delta as i64;
        i >= 0 && (i as usize) < self.history.len()
    }

    pub fn remembered(&self, guild: Option<Id>) -> Option<Id> {
        self.last_in.get(&key(guild)).copied()
    }

    pub fn forget(&mut self, guild: Option<Id>) {
        self.last_in.remove(&key(guild));
    }

    pub fn last(&self) -> Option<Stop> {
        self.last
    }
}

/// The next (`delta` 1) or previous (-1) entry of `order` after `current`, wrapping at the ends;
/// with `unread_only`, the next one that is unread. `None` when there is nowhere to go.
pub fn step<T: Copy + PartialEq>(order: &[(T, bool)], current: Option<T>, delta: i32, unread_only: bool) -> Option<T> {
    let n = order.len() as i64;
    if n == 0 {
        return None;
    }
    let cur = current
        .and_then(|c| order.iter().position(|(id, _)| *id == c))
        .map_or(if delta > 0 { -1 } else { n }, |i| i as i64);
    for s in 1..=n {
        let i = (cur + delta as i64 * s).rem_euclid(n) as usize;
        let (id, unread) = order[i];
        if !unread_only || unread {
            return (Some(id) != current).then_some(id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    fn stop(c: u64) -> Stop {
        Stop { guild: None, channel: Snowflake(c) }
    }

    #[test]
    fn history_goes_back_and_forward_and_truncates() {
        let mut n = NavState::default();
        for c in 1..=3 {
            n.visit(stop(c), false);
        }
        assert_eq!(n.travel(-1), Some(stop(2)));
        n.visit(stop(2), true);
        assert_eq!(n.travel(-1), Some(stop(1)));
        assert_eq!(n.travel(-1), None);
        n.visit(stop(9), false); // a new visit drops the forward branch
        assert!(!n.can_travel(1));
        assert_eq!(n.remembered(None), Some(Snowflake(9)));
    }

    #[test]
    fn step_wraps_and_skips_read() {
        let order = [(1, false), (2, true), (3, false), (4, true)];
        assert_eq!(step(&order, Some(1), 1, false), Some(2));
        assert_eq!(step(&order, Some(4), 1, false), Some(1));
        assert_eq!(step(&order, Some(1), -1, false), Some(4));
        assert_eq!(step(&order, Some(2), 1, true), Some(4));
        assert_eq!(step(&order, Some(4), 1, true), Some(2));
        assert_eq!(step(&order, None, 1, false), Some(1));
        assert_eq!(step(&[(1, false)], Some(1), 1, true), None);
        assert_eq!(step::<i32>(&[], None, 1, false), None);
    }
}
