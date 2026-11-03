//! Microphone permission: the shell reads the OS status and asks the core what to do (rules and
//! sentences in `unison_core::mic_access`).

use unison_core::mic_access::{self, MicStatus as Core};

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum MicPermission {
    NotDetermined,
    Authorized,
    Denied,
    Restricted,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum MicAction {
    /// Capture may start.
    Proceed,
    /// Ask the OS; start only on yes.
    Ask,
    /// Don't touch the microphone; show `note`.
    Blocked,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MicPlanItem {
    pub action: MicAction,
    /// One plain line for `Blocked`.
    pub note: Option<String>,
}

fn core(p: MicPermission) -> Core {
    match p {
        MicPermission::NotDetermined => Core::NotDetermined,
        MicPermission::Authorized => Core::Authorized,
        MicPermission::Denied => Core::Denied,
        MicPermission::Restricted => Core::Restricted,
    }
}

#[uniffi::export]
pub fn mic_plan(status: MicPermission) -> MicPlanItem {
    let s = core(status);
    match mic_access::plan(s) {
        mic_access::MicPlan::Proceed => MicPlanItem { action: MicAction::Proceed, note: None },
        mic_access::MicPlan::Ask => MicPlanItem { action: MicAction::Ask, note: None },
        mic_access::MicPlan::Blocked => MicPlanItem { action: MicAction::Blocked, note: Some(mic_access::blocked_note(s).into()) },
    }
}

/// The line to show after the person answered the OS prompt.
#[uniffi::export]
pub fn mic_answer_note(allowed: bool, joining: bool) -> String {
    mic_access::answer_note(allowed, joining).into()
}

/// Records one voice message. The shell pushes microphone samples (48 kHz, mono, `f32`) as they
/// arrive; the core encodes, builds the Ogg file and the waveform, and stops accepting audio at
/// the length limit. See `unison_core::voice_note`.
#[derive(uniffi::Object)]
pub struct VoiceNoteRecorder {
    note: parking_lot::Mutex<Option<unison_core::voice_note::VoiceNote>>,
}

#[uniffi::export]
impl VoiceNoteRecorder {
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { note: parking_lot::Mutex::new(unison_core::voice_note::VoiceNote::new()) })
    }

    /// Add samples; returns their loudness (0...1) for the live meter.
    pub fn push(&self, samples: Vec<f32>) -> f32 {
        self.note.lock().as_mut().map(|n| n.push(&samples)).unwrap_or(0.0)
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.note.lock().as_ref().map(|n| n.elapsed_secs()).unwrap_or(0.0)
    }

    /// The length limit was reached: stop recording.
    pub fn full(&self) -> bool {
        self.note.lock().as_ref().map(|n| n.full()).unwrap_or(false)
    }

    /// The finished file, ready to send; nothing when it was too short. The recorder is spent.
    pub fn finish(&self) -> Option<crate::FileUpload> {
        let f = self.note.lock().take()?.finish()?;
        Some(crate::FileUpload {
            filename: "voice-message.ogg".into(),
            content_type: "audio/ogg".into(),
            bytes: f.bytes,
            spoiler: false,
            description: None,
            voice_secs: Some(f.duration_secs),
            voice_waveform: Some(f.waveform),
            original_content_type: None,
            original_md5: None,
        })
    }
}

/// Limits the recorder bar shows (seconds).
#[uniffi::export]
pub fn voice_note_limits() -> Vec<f64> {
    vec![unison_core::voice_note::MIN_SECS, unison_core::voice_note::MAX_SECS]
}
