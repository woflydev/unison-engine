//! Local audio settings: the shell reads its `voice.*` preferences and pushes them here. None of this
//! is sent to Discord (the official client keeps it per device too).

use unison_core::voice::params::params;

#[derive(uniffi::Record, Clone, Debug)]
pub struct AudioSettings {
    /// Core Audio device ids; 0 is the system default. Read when the audio unit opens (next join).
    pub input_device: u32,
    pub output_device: u32,
    /// 0..=200, percent. Input is also clamped to 100 by the settings pane.
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

#[uniffi::export]
pub fn set_audio_settings(s: AudioSettings) {
    let p = params();
    p.set_devices(s.input_device, s.output_device);
    p.set_input_gain(s.input_volume / 100.0);
    p.set_output_gain(s.output_volume / 100.0);
    p.set_input_mode_ptt(s.push_to_talk);
    p.set_auto_sensitivity(s.auto_sensitivity);
    p.set_threshold_db(s.threshold_db);
    p.set_processing(s.echo_cancellation, s.noise_suppression, s.automatic_gain_control);
}

/// Push-to-talk key state (key down / up), from the shell's hot-key monitor.
#[uniffi::export]
pub fn set_push_to_talk_down(down: bool) {
    params().set_ptt_down(down);
}

/// Per-user local volume (percent, 0..=200) and local mute for what this Mac plays.
#[uniffi::export]
pub fn set_voice_user_volume(user_id: String, volume: f32, muted: bool) {
    if let Ok(u) = user_id.parse::<u64>() {
        params().set_user(u, volume / 100.0, muted);
    }
}

/// The capture size and rate the current power state allows.
#[derive(uniffi::Record, Clone, Copy, Debug)]
pub struct VideoCap {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// Screen-share/camera capture cap for the power state the shell reports (battery, Low Power,
/// thermal 0 nominal .. 3 critical). `None`: capture nothing (nobody is watching, or too hot).
/// The ladder and thresholds are the core's; the shell only supplies the raw readings.
#[uniffi::export]
pub fn video_power_cap(viewers: u32, on_battery: bool, low_power_mode: bool, thermal: u8) -> Option<VideoCap> {
    use unison_core::voice::video::{power_cap, PowerState, Thermal};
    let thermal = match thermal {
        0 => Thermal::Nominal,
        1 => Thermal::Fair,
        2 => Thermal::Serious,
        _ => Thermal::Critical,
    };
    power_cap(&PowerState { on_battery, low_power_mode, thermal, viewers, preview_visible: true })
        .map(|r| VideoCap { width: r.width, height: r.height, fps: r.fps })
}
