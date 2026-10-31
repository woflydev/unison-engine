//! Behavioural state that persists across launches (drafts, recent reactions, reply ping, call
//! volumes, account order). The rules and storage are the core's (`localstate.rs`); shells call this.
use super::*;

use unison_core::localstate::{RecentReaction, UserVoice};

#[derive(uniffi::Record, Clone, Debug)]
pub struct RecentEmoji {
    pub key: String,
    pub name: String,
    pub url: Option<String>,
}

fn to_ffi(r: RecentReaction) -> RecentEmoji {
    RecentEmoji { key: r.key, name: r.name, url: r.url }
}

#[uniffi::export]
impl UnisonEngine {
    pub fn draft(&self, account_id: String, channel_id: String) -> String {
        self.inner.local.draft(&account_id, &channel_id)
    }

    /// Keeps the draft in memory and writes it a couple of seconds after typing settles.
    pub fn set_draft(&self, account_id: String, channel_id: String, text: String) {
        if self.inner.local.set_draft(&account_id, &channel_id, &text) {
            self.inner.flush_drafts_later();
        }
    }

    /// Write pending drafts now (the shell calls this when the app is about to quit).
    pub fn flush_local(&self) {
        self.inner.local.flush_drafts();
    }

    pub fn recent_reactions(&self) -> Vec<RecentEmoji> {
        self.inner.local.recent_reactions().into_iter().map(to_ffi).collect()
    }

    pub fn record_reaction(&self, key: String, name: String, url: Option<String>) -> Vec<RecentEmoji> {
        self.inner.local.record_reaction(RecentReaction { key, name, url }).into_iter().map(to_ffi).collect()
    }

    pub fn reply_ping(&self) -> bool {
        self.inner.local.reply_ping()
    }

    pub fn set_reply_ping(&self, on: bool) {
        self.inner.local.set_reply_ping(on);
    }

    /// Local volume (percent) and mute for one person in calls.
    pub fn user_voice(&self, user_id: String) -> UserVoiceItem {
        let v = self.inner.local.user_voices().get(&user_id).copied().unwrap_or(UserVoice { volume: 100.0, muted: false });
        UserVoiceItem { volume: v.volume, muted: v.muted }
    }

    /// Store and apply a person's local volume/mute to the running voice engine.
    pub fn set_user_voice(&self, user_id: String, volume: f32, muted: bool) {
        self.inner.local.set_user_voice(&user_id, UserVoice { volume, muted });
        audio::set_voice_user_volume(user_id, volume, muted);
    }

    /// Apply every stored per-person setting (call at launch).
    pub fn restore_user_voices(&self) {
        for (id, v) in self.inner.local.user_voices() {
            audio::set_voice_user_volume(id, v.volume, v.muted);
        }
    }

    pub fn account_order(&self) -> Vec<String> {
        self.inner.local.account_order()
    }

    pub fn set_account_order(&self, ids: Vec<String>) {
        self.inner.local.set_account_order(&ids);
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct UserVoiceItem {
    pub volume: f32,
    pub muted: bool,
}
