//! Local audio settings: the shell reads its `voice.*` preferences and pushes them here. None of this
//! is sent to Discord (the official client keeps it per device too).

use unison_core::voice::params::params;

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
