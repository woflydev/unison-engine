//! Behavioural state that persists across launches (drafts, recent reactions, reply ping, call
//! volumes, account order). The rules and storage are the core's (`localstate.rs`); shells call this.
use super::*;

use unison_core::localstate::{RecentReaction, UserVoice};

/// How much the on-device search history holds, and what the chosen level allows.
#[derive(uniffi::Record, Clone, Debug)]
pub struct IndexUsage {
    pub bytes: u64,
    pub messages: u64,
    /// 0 when unlimited.
    pub budget_bytes: u64,
    pub per_channel: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct RecentEmoji {
    pub key: String,
    pub name: String,
    pub url: Option<String>,
}

fn to_ffi(r: RecentReaction) -> RecentEmoji {
    RecentEmoji { key: r.key, name: r.name, url: r.url }
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum WelcomeStep {
    Hello,
    Search,
    Appearance,
    Privacy,
    Features,
    Finish,
}

/// The guide's steps in order (`unison_core::welcome`); `from` is the revision already seen.
#[uniffi::export]
pub fn welcome_steps(from: u32) -> Vec<WelcomeStep> {
    use unison_core::welcome::Step as S;
    unison_core::welcome::steps(from)
        .into_iter()
        .map(|s| match s {
            S::Hello => WelcomeStep::Hello,
            S::Search => WelcomeStep::Search,
            S::Appearance => WelcomeStep::Appearance,
            S::Privacy => WelcomeStep::Privacy,
            S::Features => WelcomeStep::Features,
            S::Finish => WelcomeStep::Finish,
        })
        .collect()
}

#[uniffi::export]
impl UnisonEngine {
    /// The revision to start the guide after (0 plays everything), or nothing when it should not
    /// show now. Someone who already uses Unison is marked as having seen it here.
    pub fn welcome_plan(&self, has_accounts: bool) -> Option<u32> {
        use unison_core::welcome::{plan, used_before, Plan};
        let dir = self.policy_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let l = &self.inner.local;
        match plan(l.welcome_seen(), l.welcome_replay(), has_accounts, used_before(&dir)) {
            Plan::Show { from } => Some(from),
            Plan::MarkSeen => {
                l.set_welcome_seen();
                None
            }
            Plan::Skip => None,
        }
    }

    /// The guide finished or was skipped.
    pub fn finish_welcome(&self) {
        self.inner.local.set_welcome_seen();
    }

    /// Settings: show the whole guide on the next launch.
    pub fn replay_welcome_next_launch(&self) {
        self.inner.local.request_welcome_replay();
    }

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

    /// Search history level: 0 Standard, 1 Large, 2 Unlimited.
    pub fn index_level(&self) -> u8 {
        self.inner.local.index_level()
    }

    pub fn set_index_level(&self, level: u8) {
        self.inner.set_index_level(level);
    }

    pub fn index_usage(&self) -> IndexUsage {
        let (bytes, messages) = self.inner.index_usage();
        let (budget, per_channel) = unison_core::localstate::index_limits(self.inner.local.index_level());
        IndexUsage { bytes, messages, budget_bytes: budget, per_channel: per_channel as u32 }
    }

    /// Forget every stored message (what search finds on this device); Discord is untouched.
    pub fn clear_index(&self) {
        self.inner.clear_index();
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
