//! Behavioural preferences the shell used to keep in `UserDefaults`: update checks and calls.

use unison_core::localstate::VoicePrefs;

use crate::UnisonEngine;

#[derive(uniffi::Record, Clone, Debug)]
pub struct UpdaterPrefsItem {
    pub auto_check: bool,
    pub beta: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct VoicePrefsItem {
    pub input_volume: f32,
    pub output_volume: f32,
    pub push_to_talk: bool,
    pub auto_sensitivity: bool,
    pub threshold_db: f32,
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub automatic_gain_control: bool,
}

impl From<VoicePrefs> for VoicePrefsItem {
    fn from(p: VoicePrefs) -> Self {
        Self {
            input_volume: p.input_volume,
            output_volume: p.output_volume,
            push_to_talk: p.push_to_talk,
            auto_sensitivity: p.auto_sensitivity,
            threshold_db: p.threshold_db,
            echo_cancellation: p.echo_cancellation,
            noise_suppression: p.noise_suppression,
            automatic_gain_control: p.automatic_gain_control,
        }
    }
}

impl From<VoicePrefsItem> for VoicePrefs {
    fn from(p: VoicePrefsItem) -> Self {
        Self {
            input_volume: p.input_volume,
            output_volume: p.output_volume,
            push_to_talk: p.push_to_talk,
            auto_sensitivity: p.auto_sensitivity,
            threshold_db: p.threshold_db,
            echo_cancellation: p.echo_cancellation,
            noise_suppression: p.noise_suppression,
            automatic_gain_control: p.automatic_gain_control,
        }
    }
}

/// Core Audio device ids; 0 is the system default. Read when the audio unit opens (next join).
#[uniffi::export]
pub fn set_audio_devices(input_device: u32, output_device: u32) {
    unison_core::voice::params::params().set_devices(input_device, output_device);
}

fn apply(p: &VoicePrefs) {
    let params = unison_core::voice::params::params();
    params.set_input_gain(p.input_volume / 100.0);
    params.set_output_gain(p.output_volume / 100.0);
    params.set_input_mode_ptt(p.push_to_talk);
    params.set_auto_sensitivity(p.auto_sensitivity);
    params.set_threshold_db(p.threshold_db);
    params.set_processing(p.echo_cancellation, p.noise_suppression, p.automatic_gain_control);
}

#[uniffi::export]
impl UnisonEngine {
    pub fn voice_prefs(&self) -> VoicePrefsItem {
        self.inner.local.voice_prefs().into()
    }

    /// Stores the choices and applies them to the audio engine at once.
    pub fn set_voice_prefs(&self, prefs: VoicePrefsItem) {
        let p = VoicePrefs::from(prefs).clamped();
        self.inner.local.set_voice_prefs(&p);
        apply(&p);
    }

    /// Apply the stored call choices (call at launch).
    pub fn restore_voice_prefs(&self) {
        apply(&self.inner.local.voice_prefs());
    }

    pub fn native_voice(&self) -> bool {
        self.inner.local.native_voice()
    }

    pub fn set_native_voice(&self, on: bool) {
        self.inner.local.set_native_voice(on);
    }

    pub fn updater_prefs(&self) -> UpdaterPrefsItem {
        let p = self.inner.local.update_prefs();
        UpdaterPrefsItem { auto_check: p.auto_check, beta: p.beta }
    }

    pub fn set_updater_prefs(&self, prefs: UpdaterPrefsItem) {
        let mut p = self.inner.local.update_prefs();
        p.auto_check = prefs.auto_check;
        p.beta = prefs.beta;
        self.inner.local.set_update_prefs(&p);
    }

    /// `stable` or `beta`, for the feed URL and the offer check.
    pub fn update_channel(&self) -> String {
        self.inner.local.update_prefs().channel().into()
    }

    /// Whether the launch check should run now (on, and a day since the last).
    pub fn update_check_due(&self) -> bool {
        self.inner.local.update_prefs().check_due(now_ms())
    }

    /// Note that the feed was just asked.
    pub fn record_update_check(&self) {
        let mut p = self.inner.local.update_prefs();
        p.last_check_ms = now_ms();
        self.inner.local.set_update_prefs(&p);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
