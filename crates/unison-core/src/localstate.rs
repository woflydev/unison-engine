//! Behaviour the user expects to survive a restart that is not part of any one account: drafts,
//! recent reactions, the reply-ping default, per-person call volume and the account order. Kept in
//! `local.sqlite` next to the account caches so every shell gets the same behaviour; shells keep
//! only UI-only preferences (theme, density, pane widths) in their own settings system.

use std::collections::HashMap;
use std::path::Path;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::Result;

const RECENT_MAX: usize = 24;

/// What each search-history level allows per account: room on disk in bytes (0: no limit) and
/// messages kept for each channel. Messages are stored compressed, so Standard holds roughly
/// 200,000 of them.
pub fn index_limits(level: u8) -> (u64, i64) {
    match level {
        0 => (200 << 20, 500),
        1 => (1 << 30, 2000),
        _ => (0, 10_000),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentReaction {
    /// Unicode emoji, or `name:id` for a custom one.
    pub key: String,
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UserVoice {
    /// Percent, 0..=200.
    pub volume: f32,
    pub muted: bool,
}

/// Calls, the parts that are not device-specific.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoicePrefs {
    /// Percent; the input slider stops at 100.
    pub input_volume: f32,
    pub output_volume: f32,
    pub push_to_talk: bool,
    pub auto_sensitivity: bool,
    /// dBFS, used when auto sensitivity is off.
    pub threshold_db: f32,
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub automatic_gain_control: bool,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        Self {
            input_volume: 100.0,
            output_volume: 100.0,
            push_to_talk: false,
            auto_sensitivity: true,
            threshold_db: -50.0,
            echo_cancellation: true,
            noise_suppression: true,
            automatic_gain_control: true,
        }
    }
}

impl VoicePrefs {
    pub fn clamped(&self) -> Self {
        Self {
            input_volume: self.input_volume.clamp(0.0, 100.0),
            output_volume: self.output_volume.clamp(0.0, 200.0),
            threshold_db: self.threshold_db.clamp(-80.0, -20.0),
            ..*self
        }
    }
}

#[derive(Default)]
struct Drafts {
    map: HashMap<String, String>,
    loaded: bool,
    dirty: bool,
    flush_pending: bool,
}

pub struct LocalState {
    db: Cache,
    drafts: Mutex<Drafts>,
    /// Places opened, for the search surface's recents and ranking (loaded on first use).
    visits: Mutex<Option<crate::find::Recents>>,
}

impl LocalState {
    pub fn open(dir: &Path) -> Result<Self> {
        Ok(Self { db: Cache::open(&dir.join("local.sqlite"))?, drafts: Mutex::new(Drafts::default()), visits: Mutex::new(None) })
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        self.db.get(key).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn put<T: Serialize>(&self, key: &str, v: &T) {
        if let Ok(b) = serde_json::to_vec(v) {
            let _ = self.db.put(key, &b);
        }
    }

    // ----- Drafts -----

    fn with_drafts<R>(&self, f: impl FnOnce(&mut Drafts) -> R) -> R {
        let mut d = self.drafts.lock();
        if !d.loaded {
            d.map = self.get("drafts").unwrap_or_default();
            d.loaded = true;
        }
        f(&mut d)
    }

    pub fn draft(&self, account: &str, channel: &str) -> String {
        self.with_drafts(|d| d.map.get(&format!("{account}:{channel}")).cloned().unwrap_or_default())
    }

    /// Update a draft in memory. Returns true when the caller should schedule `flush_drafts` (a
    /// short while later), so typing never writes to disk per keystroke.
    pub fn set_draft(&self, account: &str, channel: &str, text: &str) -> bool {
        self.with_drafts(|d| {
            let k = format!("{account}:{channel}");
            if text.is_empty() {
                d.dirty |= d.map.remove(&k).is_some();
            } else if d.map.get(&k).map(String::as_str) != Some(text) {
                d.map.insert(k, text.to_string());
                d.dirty = true;
            }
            let schedule = d.dirty && !d.flush_pending;
            d.flush_pending |= schedule;
            schedule
        })
    }

    pub fn flush_drafts(&self) {
        let snapshot = self.with_drafts(|d| {
            d.flush_pending = false;
            std::mem::take(&mut d.dirty).then(|| d.map.clone())
        });
        if let Some(m) = snapshot {
            self.put("drafts", &m);
        }
    }

    // ----- Places opened (search recents and ranking) -----

    fn with_visits<R>(&self, f: impl FnOnce(&mut crate::find::Recents) -> R) -> R {
        let mut g = self.visits.lock();
        let r = g.get_or_insert_with(|| self.get("visits").unwrap_or_default());
        f(r)
    }

    /// Note that a channel was opened (by any means: click, search, link, history).
    pub fn record_visit(&self, account: &str, guild: Option<&str>, channel: &str, now_ms: u64) {
        let snapshot = self.with_visits(|r| {
            r.record(account, guild, channel, now_ms);
            r.clone()
        });
        self.put("visits", &snapshot);
    }

    pub fn recents(&self) -> crate::find::Recents {
        self.with_visits(|r| r.clone())
    }

    // ----- Reactions -----

    pub fn recent_reactions(&self) -> Vec<RecentReaction> {
        self.get("recent_reactions").unwrap_or_default()
    }

    /// Put `r` first, drop its earlier entry, keep at most 24.
    pub fn record_reaction(&self, r: RecentReaction) -> Vec<RecentReaction> {
        let mut list = self.recent_reactions();
        list.retain(|x| x.key != r.key);
        list.insert(0, r);
        list.truncate(RECENT_MAX);
        self.put("recent_reactions", &list);
        list
    }

    // ----- Composer -----

    /// Whether a reply pings its author (default on).
    // ----- Saved messages -----

    pub fn saved(&self) -> Vec<crate::saved::Saved> {
        self.get("saved_messages").unwrap_or_default()
    }

    pub fn edit_saved(&self, f: impl FnOnce(&mut Vec<crate::saved::Saved>)) {
        let mut all = self.saved();
        f(&mut all);
        self.put("saved_messages", &all);
    }

    /// Folders the user left open in the rail, by folder id.
    pub fn open_folders(&self) -> Vec<i64> {
        self.get("open_folders").unwrap_or_default()
    }

    pub fn set_folder_open(&self, id: i64, open: bool) {
        let mut all = self.open_folders();
        all.retain(|x| *x != id);
        if open {
            all.push(id);
        }
        self.put("open_folders", &all);
    }

    /// `None` follows the account (on for free accounts, off with Nitro).
    pub fn compress_uploads(&self) -> Option<bool> {
        self.get("compress_uploads")
    }

    pub fn set_compress_uploads(&self, choice: Option<bool>) {
        self.put("compress_uploads", &choice);
    }

    pub fn comfort(&self) -> crate::comfort::Comfort {
        self.get("comfort").unwrap_or_default()
    }

    pub fn set_comfort(&self, c: &crate::comfort::Comfort) {
        self.put("comfort", c);
    }

    /// The welcome guide was shown (or is not for this person); see `welcome.rs`.
    pub fn welcome_seen(&self) -> u32 {
        self.get("welcome_rev").unwrap_or(0)
    }

    /// Record the current revision as seen and clear any replay request.
    pub fn set_welcome_seen(&self) {
        self.put("welcome_rev", &crate::welcome::REVISION);
        self.put("welcome_replay", &false);
    }

    pub fn welcome_replay(&self) -> bool {
        self.get("welcome_replay").unwrap_or(false)
    }

    /// Settings asked to see the guide again: it plays on the next launch.
    pub fn request_welcome_replay(&self) {
        self.put("welcome_replay", &true);
    }

    pub fn reply_ping(&self) -> bool {
        self.get("reply_ping").unwrap_or(true)
    }

    pub fn set_reply_ping(&self, on: bool) {
        self.put("reply_ping", &on);
    }

    // ----- Search history (the on-device message index) -----

    /// 0 Standard, 1 Large, 2 Unlimited.
    pub fn index_level(&self) -> u8 {
        self.get::<u8>("index_level").unwrap_or(0).min(2)
    }

    pub fn set_index_level(&self, level: u8) {
        self.put("index_level", &level.min(2));
    }

    // ----- Calls -----

    pub fn user_voices(&self) -> HashMap<String, UserVoice> {
        self.get("user_voice").unwrap_or_default()
    }

    /// Store a person's local volume and mute; the default (100, unmuted) is not stored.
    pub fn set_user_voice(&self, user: &str, v: UserVoice) {
        let mut all = self.user_voices();
        if v.volume == 100.0 && !v.muted {
            all.remove(user);
        } else {
            all.insert(user.to_string(), v);
        }
        self.put("user_voice", &all);
    }

    /// Volumes, input mode and processing for calls (devices and the push-to-talk key are per-OS, so the shell keeps them).
    pub fn voice_prefs(&self) -> VoicePrefs {
        self.get("voice_prefs").unwrap_or_default()
    }

    pub fn set_voice_prefs(&self, p: &VoicePrefs) {
        self.put("voice_prefs", &p.clamped());
    }

    /// Use native calls (otherwise Discord's web view).
    pub fn native_voice(&self) -> bool {
        self.get("native_voice").unwrap_or(true)
    }

    pub fn set_native_voice(&self, on: bool) {
        self.put("native_voice", &on);
    }

    // ----- Updates -----

    /// Update checks at launch (on unless the person turned them off) and the beta channel (off).
    pub fn update_prefs(&self) -> crate::update::UpdatePrefs {
        self.get("update_prefs").unwrap_or_default()
    }

    pub fn set_update_prefs(&self, p: &crate::update::UpdatePrefs) {
        self.put("update_prefs", p);
    }

    /// The last signed feed the updater fetched, kept so the "update available" badge is there at the
    /// next launch (and every one after) until the person updates, not only on the day of the check.
    /// It is verified again before it is used, like a fresh one.
    pub fn cached_appcast(&self) -> Option<String> {
        self.get("update_feed")
    }

    pub fn set_cached_appcast(&self, signed_json: &str) {
        self.put("update_feed", &signed_json.to_string());
    }

    // ----- Accounts -----

    pub fn account_order(&self) -> Vec<String> {
        self.get("account_order").unwrap_or_default()
    }

    pub fn set_account_order(&self, ids: &[String]) {
        if self.account_order() != ids {
            self.put("account_order", &ids);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> LocalState {
        LocalState { db: Cache::in_memory().unwrap(), drafts: Mutex::new(Drafts::default()), visits: Mutex::new(None) }
    }

    #[test]
    fn drafts_flush_once_and_persist() {
        let s = state();
        assert!(s.set_draft("a", "c", "hel"));
        assert!(!s.set_draft("a", "c", "hello"), "a flush is already pending");
        assert_eq!(s.draft("a", "c"), "hello");
        s.flush_drafts();
        assert!(s.set_draft("a", "c", ""), "clearing marks it dirty again");
        s.flush_drafts();
        assert_eq!(s.draft("a", "c"), "");
        assert!(!s.set_draft("a", "c", ""), "nothing changed");
    }

    #[test]
    fn recent_reactions_are_deduped_and_capped() {
        let s = state();
        for i in 0..30 {
            s.record_reaction(RecentReaction { key: format!("k{i}"), name: format!("n{i}"), url: None });
        }
        let r = s.record_reaction(RecentReaction { key: "k5".into(), name: "n5".into(), url: None });
        assert_eq!(r.len(), RECENT_MAX);
        assert_eq!(r[0].key, "k5");
        assert_eq!(r.iter().filter(|x| x.key == "k5").count(), 1);
    }

    #[test]
    fn defaults_are_not_stored() {
        let s = state();
        s.set_user_voice("1", UserVoice { volume: 50.0, muted: false });
        s.set_user_voice("2", UserVoice { volume: 100.0, muted: true });
        s.set_user_voice("1", UserVoice { volume: 100.0, muted: false });
        let all = s.user_voices();
        assert_eq!(all.len(), 1);
        assert!(all["2"].muted);
        assert!(s.reply_ping());
    }
}
