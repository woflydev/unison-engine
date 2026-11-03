//! Messages on their way out: progress for the bar above the composer, cancel, and the failed
//! ones the shell hands back to the composer. State and rules are the core's (`send_state.rs`).
use super::*;

use unison_core::send_state::Phase;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum SendPhase {
    Uploading,
    Sending,
    Failed,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SendItem {
    pub nonce: String,
    pub phase: SendPhase,
    /// 0.0 to 1.0 of the file bytes.
    pub fraction: f32,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub file_count: u32,
    /// The file going up now.
    pub current_name: String,
    pub bytes_per_sec: u64,
    pub eta_secs: Option<u32>,
    /// Why it failed; `None` when the user cancelled.
    pub error: Option<String>,
    /// The message text, for the failed-send card.
    pub text: String,
    /// Sending it again can work.
    pub can_retry: bool,
}

/// A failed or cancelled message handed back to be edited.
#[derive(uniffi::Record, Clone, Debug)]
pub struct TakenBackItem {
    pub text: String,
    pub files: Vec<FileUpload>,
    pub reply_to: Option<String>,
    pub mention_reply: bool,
}

#[uniffi::export]
impl UnisonEngine {
    /// The sends in this channel that are still going or have failed, oldest first.
    pub fn sends(&self, account_id: String, channel_id: String) -> Vec<SendItem> {
        self.with_store(&account_id, |s| {
            s.sends
                .iter()
                .filter(|x| x.channel == id(&channel_id))
                .map(|x| SendItem {
                    nonce: x.nonce.to_string(),
                    phase: match x.phase {
                        Phase::Uploading => SendPhase::Uploading,
                        Phase::Sending => SendPhase::Sending,
                        Phase::Failed => SendPhase::Failed,
                    },
                    fraction: x.fraction(),
                    sent_bytes: x.sent(),
                    total_bytes: x.total(),
                    file_count: x.files.len() as u32,
                    current_name: x.current().map(|f| f.name.clone()).unwrap_or_default(),
                    bytes_per_sec: x.bytes_per_sec(),
                    eta_secs: x.eta_secs(),
                    error: x.error.clone(),
                    text: x.text.clone(),
                    can_retry: x.can_retry,
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Stop a message that is still uploading (a button press).
    pub fn cancel_send(&self, account_id: String, nonce: String, _gesture: UserGesture) {
        if let Some(s) = self.inner.session(id(&account_id)) {
            s.cancel_send(id(&nonce));
        }
    }

    /// Send a failed message again as it was (a button press).
    pub fn retry_send(&self, account_id: String, nonce: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        let (intent, listener) = (gesture.intent(), self.listener.clone());
        self.inner.rt.spawn(async move {
            match s.retry_send(intent, id(&nonce)).await {
                Ok(()) | Err(unison_core::Error::Cancelled) => {}
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }

    /// Take a failed or cancelled message back to edit it: its text, files and reply, no longer tracked.
    pub fn take_back_send(&self, account_id: String, nonce: String) -> Option<TakenBackItem> {
        let r = self.inner.session(id(&account_id))?.take_back_send(id(&nonce))?;
        Some(TakenBackItem {
            text: r.content,
            files: r
                .files
                .into_iter()
                .map(|f| FileUpload { filename: f.filename, content_type: f.content_type, bytes: f.bytes.to_vec(), spoiler: f.spoiler, description: f.description, voice_secs: f.voice.as_ref().map(|v| v.duration_secs), voice_waveform: f.voice.map(|v| v.waveform), original_content_type: f.original.as_ref().map(|o| o.content_type.clone()), original_md5: f.original.map(|o| o.md5) })
                .collect(),
            reply_to: r.reply_to.map(|r| r.to_string()),
            mention_reply: r.mention_reply,
        })
    }

    /// Forget a failed send.
    pub fn dismiss_send(&self, account_id: String, nonce: String) {
        if let Some(s) = self.inner.session(id(&account_id)) {
            s.dismiss_send(id(&nonce));
        }
    }
}
