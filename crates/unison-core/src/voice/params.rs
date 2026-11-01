//! User-chosen audio settings shared with the real-time threads: devices, volumes, input mode and
//! sensitivity, processing toggles, and per-user volume. These are local to this Mac (the official
//! client never syncs them); the shell reads its `voice.*` preferences and calls the setters.
//!
//! Everything the audio callbacks read is an atomic, so changes apply live without locks. Device and
//! processing choices are read when the audio unit opens (the next join).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use parking_lot::Mutex;

pub struct AudioParams {
    /// Microphone gain, f32 bits (1.0 = unity).
    input_gain: AtomicU32,
    /// Master playout gain, f32 bits (0..2).
    output_gain: AtomicU32,
    ptt_mode: AtomicBool,
    ptt_down: AtomicBool,
    auto_sensitivity: AtomicBool,
    /// Manual voice threshold in dBFS, f32 bits.
    threshold_db: AtomicU32,
    echo: AtomicBool,
    noise: AtomicBool,
    agc: AtomicBool,
    /// Core Audio device ids, 0 = system default.
    input_device: AtomicU32,
    output_device: AtomicU32,
    users: Mutex<HashMap<u64, UserAudio>>,
    /// Bumped on any per-user change so receive threads can skip the lock.
    users_version: AtomicU32,
}

#[derive(Clone, Copy)]
pub struct UserAudio {
    pub volume: f32,
    pub muted: bool,
}

pub fn params() -> &'static AudioParams {
    static P: OnceLock<AudioParams> = OnceLock::new();
    P.get_or_init(|| AudioParams {
        input_gain: AtomicU32::new(1.0f32.to_bits()),
        output_gain: AtomicU32::new(1.0f32.to_bits()),
        ptt_mode: AtomicBool::new(false),
        ptt_down: AtomicBool::new(false),
        auto_sensitivity: AtomicBool::new(true),
        threshold_db: AtomicU32::new((-50.0f32).to_bits()),
        echo: AtomicBool::new(true),
        noise: AtomicBool::new(true),
        agc: AtomicBool::new(true),
        input_device: AtomicU32::new(0),
        output_device: AtomicU32::new(0),
        users: Mutex::new(HashMap::new()),
        users_version: AtomicU32::new(0),
    })
}

fn f(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

impl AudioParams {
    pub fn set_input_gain(&self, g: f32) {
        self.input_gain.store(g.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }
    pub fn input_gain(&self) -> f32 {
        f(&self.input_gain)
    }
    pub fn set_output_gain(&self, g: f32) {
        self.output_gain.store(g.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }
    pub fn output_gain(&self) -> f32 {
        f(&self.output_gain)
    }
    pub fn set_input_mode_ptt(&self, ptt: bool) {
        self.ptt_mode.store(ptt, Ordering::Relaxed);
    }
    pub fn set_ptt_down(&self, down: bool) {
        self.ptt_down.store(down, Ordering::Relaxed);
    }
    pub fn set_auto_sensitivity(&self, auto: bool) {
        self.auto_sensitivity.store(auto, Ordering::Relaxed);
    }
    pub fn set_threshold_db(&self, db: f32) {
        self.threshold_db.store(db.clamp(-100.0, 0.0).to_bits(), Ordering::Relaxed);
    }
    pub fn set_processing(&self, echo: bool, noise: bool, agc: bool) {
        self.echo.store(echo, Ordering::Relaxed);
        self.noise.store(noise, Ordering::Relaxed);
        self.agc.store(agc, Ordering::Relaxed);
    }
    pub fn processing(&self) -> (bool, bool, bool) {
        (self.echo.load(Ordering::Relaxed), self.noise.load(Ordering::Relaxed), self.agc.load(Ordering::Relaxed))
    }
    pub fn set_devices(&self, input: u32, output: u32) {
        self.input_device.store(input, Ordering::Relaxed);
        self.output_device.store(output, Ordering::Relaxed);
    }
    pub fn devices(&self) -> (u32, u32) {
        (self.input_device.load(Ordering::Relaxed), self.output_device.load(Ordering::Relaxed))
    }

    /// Per-user local volume (1.0 = 100 %) and local mute: only what this client plays.
    pub fn set_user(&self, user: u64, volume: f32, muted: bool) {
        let mut u = self.users.lock();
        if (volume - 1.0).abs() < 1e-3 && !muted {
            u.remove(&user);
        } else {
            u.insert(user, UserAudio { volume: volume.clamp(0.0, 2.0), muted });
        }
        self.users_version.fetch_add(1, Ordering::Relaxed);
    }
    pub fn user_gain(&self, user: u64) -> f32 {
        match self.users.lock().get(&user) {
            Some(a) if a.muted => 0.0,
            Some(a) => a.volume,
            None => 1.0,
        }
    }
    pub fn users_version(&self) -> u32 {
        self.users_version.load(Ordering::Relaxed)
    }

    /// Decide whether a 20 ms frame at `level_db` is voice, for the send loop.
    pub fn gate(&self, g: &mut Gate, level_db: f32) -> bool {
        g.update(
            level_db,
            self.ptt_mode.load(Ordering::Relaxed),
            self.ptt_down.load(Ordering::Relaxed),
            self.auto_sensitivity.load(Ordering::Relaxed),
            f(&self.threshold_db),
        )
    }
}

/// Frames of silence before speaking ends (about 260 ms). The official default is `vadTrailing` 25
/// (web bundle `MediaEngineStore`, native `CreateDefaultVadSettings` 0x19), counted in WebRTC's 10 ms
/// frames (an inference: the native gate runs per `OnPreEncode` audio frame), so about 250 ms; our
/// frames are 20 ms. `vadLeading` 5 (50 ms) is not modelled.
pub const HANG_FRAMES: u32 = 13;
/// Auto sensitivity keeps the threshold this far above the tracked noise floor.
const AUTO_MARGIN_DB: f32 = 12.0;
const AUTO_MIN_DB: f32 = -60.0;
const AUTO_MAX_DB: f32 = -30.0;

/// Voice gate state: hang time and the adaptive noise floor.
pub struct Gate {
    hang: u32,
    floor_db: f32,
}

impl Default for Gate {
    fn default() -> Self {
        Self { hang: 0, floor_db: -70.0 }
    }
}

impl Gate {
    pub fn update(&mut self, level_db: f32, ptt: bool, ptt_down: bool, auto: bool, manual_db: f32) -> bool {
        if ptt {
            self.hang = if ptt_down { 2 } else { self.hang.saturating_sub(1) };
            return self.hang > 0;
        }
        // Noise floor: falls fast to quieter frames, creeps up slowly (so speech doesn't raise it).
        self.floor_db = if level_db < self.floor_db { level_db } else { self.floor_db + (level_db - self.floor_db) * 0.002 };
        let threshold = if auto { (self.floor_db + AUTO_MARGIN_DB).clamp(AUTO_MIN_DB, AUTO_MAX_DB) } else { manual_db };
        if level_db > threshold {
            self.hang = HANG_FRAMES;
        } else {
            self.hang = self.hang.saturating_sub(1);
        }
        self.hang > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_threshold_and_hang() {
        let mut g = Gate::default();
        assert!(!g.update(-60.0, false, false, false, -50.0));
        assert!(g.update(-40.0, false, false, false, -50.0));
        for _ in 0..HANG_FRAMES - 1 {
            assert!(g.update(-80.0, false, false, false, -50.0));
        }
        assert!(!g.update(-80.0, false, false, false, -50.0));
    }

    #[test]
    fn ptt_ignores_level() {
        let mut g = Gate::default();
        assert!(!g.update(-10.0, true, false, true, -50.0), "loud but key up");
        assert!(g.update(-90.0, true, true, true, -50.0), "silent but key down");
        assert!(g.update(-90.0, true, false, true, -50.0), "short release tail");
        assert!(!g.update(-90.0, true, false, true, -50.0));
    }

    #[test]
    fn auto_adapts_to_noise_floor() {
        let mut g = Gate::default();
        for _ in 0..3000 {
            g.update(-55.0, false, false, true, -50.0);
        }
        assert!(!g.update(-50.0, false, false, true, -50.0), "hum just above floor is not voice");
        assert!(g.update(-30.0, false, false, true, -50.0), "speech is");
    }

    #[test]
    fn per_user_volume_and_mute() {
        let p = params();
        assert_eq!(p.user_gain(99), 1.0);
        let v = p.users_version();
        p.set_user(99, 0.5, false);
        assert_eq!(p.user_gain(99), 0.5);
        p.set_user(99, 0.5, true);
        assert_eq!(p.user_gain(99), 0.0);
        p.set_user(99, 1.0, false);
        assert_eq!(p.user_gain(99), 1.0);
        assert!(p.users_version() > v);
    }
}
