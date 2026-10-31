//! DAVE end-to-end encryption, driven by the voice gateway's DAVE opcodes.
//!
//! MLS group state and frame crypto come from `davey`; this module maps gateway messages onto
//! it and returns what to send back. Transition handling follows daveprotocol.com: commits and
//! welcomes for a non-zero transition are acknowledged with op 23 and take effect on op 22.

use std::borrow::Cow;
use std::collections::HashSet;
use std::num::NonZeroU16;

use davey::{DaveSession, MediaType, ProposalsOperationType};
use serde_json::Value;

use crate::voice::protocol::{self, op};

/// The DAVE version we can speak (`max_dave_protocol_version`).
pub const MAX_VERSION: u16 = davey::DAVE_PROTOCOL_VERSION;

/// What to send on the voice gateway.
#[derive(Debug, PartialEq)]
pub enum Out {
    Json(Value),
    Binary(Vec<u8>),
}

pub struct Dave {
    user_id: u64,
    channel_id: u64,
    session: Option<DaveSession>,
    version: u16,
    /// Users in the call (op 11 / 13), plus us: the only members a commit may add.
    users: HashSet<u64>,
    /// A transition to protocol 0 announced by op 21, applied on op 22.
    pending_downgrade: Option<u16>,
}

impl Dave {
    pub fn new(user_id: u64, channel_id: u64) -> Self {
        Self { user_id, channel_id, session: None, version: 0, users: HashSet::from([user_id]), pending_downgrade: None }
    }

    pub fn version(&self) -> u16 {
        self.version
    }

    /// True once frames are end-to-end encrypted.
    pub fn is_active(&self) -> bool {
        self.session.as_ref().is_some_and(DaveSession::is_ready)
    }

    /// The code people compare to verify the call (shown in the call panel).
    pub fn privacy_code(&self) -> Option<String> {
        self.session.as_ref().and_then(|s| s.voice_privacy_code()).filter(|c| !c.is_empty()).map(str::to_string)
    }

    pub fn clients_connect(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.users.extend(ids);
    }

    pub fn client_disconnect(&mut self, id: u64) {
        self.users.remove(&id);
    }

    /// Op 4: the session's protocol version. A fresh MLS session waits for the external sender.
    pub fn on_session_description(&mut self, version: u16) {
        self.version = version;
        if version > 0 {
            self.session = NonZeroU16::new(version).and_then(|v| DaveSession::new(v, self.user_id, self.channel_id, None).ok());
        } else {
            self.session = None;
        }
    }

    /// Binary op 25 → our key package (op 26).
    pub fn on_external_sender(&mut self, payload: &[u8]) -> Vec<Out> {
        let Some(s) = self.session.as_mut() else { return vec![] };
        if s.set_external_sender(payload).is_err() {
            return vec![];
        }
        key_package(s)
    }

    /// Binary op 27 → commit (+ welcome) as op 28, when the proposals add someone.
    pub fn on_proposals(&mut self, payload: &[u8]) -> Vec<Out> {
        let (Some(s), Some((&kind, proposals))) = (self.session.as_mut(), payload.split_first()) else { return vec![] };
        let kind = if kind == 1 { ProposalsOperationType::REVOKE } else { ProposalsOperationType::APPEND };
        let expected: Vec<u64> = self.users.iter().copied().collect();
        match s.process_proposals(kind, proposals, Some(&expected)) {
            Ok(Some(cw)) => {
                let mut b = cw.commit;
                if let Some(w) = cw.welcome {
                    b.extend_from_slice(&w);
                }
                vec![Out::Binary(protocol::binary(op::DAVE_COMMIT_WELCOME, &b))]
            }
            _ => vec![],
        }
    }

    /// Binary op 29 `[transition_id: u16][commit]`.
    pub fn on_announce_commit(&mut self, payload: &[u8]) -> Vec<Out> {
        let Some((tid, commit)) = split_transition(payload) else { return vec![] };
        let ok = self.session.as_mut().is_some_and(|s| s.process_commit(commit).is_ok());
        self.after_group_change(tid, ok)
    }

    /// Binary op 30 `[transition_id: u16][welcome]`.
    pub fn on_welcome(&mut self, payload: &[u8]) -> Vec<Out> {
        let Some((tid, welcome)) = split_transition(payload) else { return vec![] };
        let ok = self.session.as_mut().is_some_and(|s| s.process_welcome(welcome).is_ok());
        self.after_group_change(tid, ok)
    }

    fn after_group_change(&mut self, tid: u16, ok: bool) -> Vec<Out> {
        if ok {
            return if tid != 0 { vec![Out::Json(protocol::transition_ready(tid))] } else { vec![] };
        }
        // Tell the server, then rejoin the group with a fresh key package.
        let mut out = vec![Out::Json(protocol::invalid_commit_welcome(tid))];
        if let Some(s) = self.session.as_mut() {
            if s.reset().is_ok() {
                if let Some(v) = NonZeroU16::new(self.version) {
                    let _ = s.reinit(v, self.user_id, self.channel_id, None);
                }
            }
            out.extend(key_package(s));
        }
        out
    }

    /// JSON op 21. A downgrade to 0 is acknowledged and applied on op 22.
    pub fn on_prepare_transition(&mut self, tid: u16, version: u16) -> Vec<Out> {
        if version == 0 {
            self.pending_downgrade = Some(tid);
            if let Some(s) = self.session.as_mut() {
                s.set_passthrough_mode(true, None);
            }
        }
        vec![Out::Json(protocol::transition_ready(tid))]
    }

    /// JSON op 22.
    pub fn on_execute_transition(&mut self, tid: u16) {
        if self.pending_downgrade == Some(tid) {
            self.pending_downgrade = None;
            self.version = 0;
            self.session = None;
        }
    }

    /// JSON op 24. Epoch 1 means a new group: start over with a new key package.
    pub fn on_prepare_epoch(&mut self, epoch: u64, version: u16) -> Vec<Out> {
        if epoch != 1 {
            return vec![];
        }
        self.version = version;
        let Some(v) = NonZeroU16::new(version) else { return vec![] };
        match self.session.as_mut() {
            Some(s) => {
                let _ = s.reinit(v, self.user_id, self.channel_id, None);
            }
            None => self.session = DaveSession::new(v, self.user_id, self.channel_id, None).ok(),
        }
        // The key package goes out once the external sender arrives for the new group.
        vec![]
    }

    /// Encrypt an outgoing Opus frame. Passes it through unchanged while DAVE is off.
    pub fn encrypt_opus<'a>(&mut self, frame: &'a [u8]) -> Option<Cow<'a, [u8]>> {
        match self.session.as_mut() {
            Some(s) if s.is_ready() => s.encrypt_opus(frame).ok(),
            Some(_) => None,
            None => Some(Cow::Borrowed(frame)),
        }
    }

    /// Encrypt an outgoing H.264 access unit (Annex B). Passes through while DAVE is off.
    pub fn encrypt_video<'a>(&mut self, frame: &'a [u8]) -> Option<Cow<'a, [u8]>> {
        match self.session.as_mut() {
            Some(s) if s.is_ready() => s.encrypt(MediaType::VIDEO, davey::Codec::H264, frame).ok(),
            Some(_) => None,
            None => Some(Cow::Borrowed(frame)),
        }
    }

    /// Decrypt an incoming video frame from `user`.
    pub fn decrypt_video(&mut self, user: u64, frame: &[u8]) -> Option<Vec<u8>> {
        match self.session.as_mut() {
            Some(s) if s.is_ready() || s.can_passthrough(user) => s.decrypt(user, MediaType::VIDEO, frame).ok(),
            Some(_) => None,
            None => Some(frame.to_vec()),
        }
    }

    /// Decrypt an incoming frame from `user`.
    pub fn decrypt(&mut self, user: u64, frame: &[u8]) -> Option<Vec<u8>> {
        match self.session.as_mut() {
            Some(s) if s.is_ready() || s.can_passthrough(user) => s.decrypt(user, MediaType::AUDIO, frame).ok(),
            Some(_) => None,
            None => Some(frame.to_vec()),
        }
    }
}

fn key_package(s: &mut DaveSession) -> Vec<Out> {
    match s.create_key_package() {
        Ok(kp) => vec![Out::Binary(protocol::binary(op::DAVE_KEY_PACKAGE, &kp))],
        Err(_) => vec![],
    }
}

fn split_transition(payload: &[u8]) -> Option<(u16, &[u8])> {
    (payload.len() >= 2).then(|| (u16::from_be_bytes([payload[0], payload[1]]), &payload[2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_dave_frames_pass_through() {
        let mut d = Dave::new(1, 2);
        d.on_session_description(0);
        assert_eq!(d.encrypt_opus(b"abc").unwrap().as_ref(), b"abc");
        assert_eq!(d.decrypt(5, b"xyz").unwrap(), b"xyz");
        assert!(!d.is_active());
    }

    #[test]
    fn dave_session_holds_audio_until_ready() {
        let mut d = Dave::new(1, 2);
        d.on_session_description(1);
        assert_eq!(d.version(), 1);
        // No group yet: nothing may leave unencrypted.
        assert!(d.encrypt_opus(b"abc").is_none());
    }

    #[test]
    fn downgrade_is_acknowledged_then_applied() {
        let mut d = Dave::new(1, 2);
        d.on_session_description(1);
        assert_eq!(d.on_prepare_transition(7, 0), vec![Out::Json(protocol::transition_ready(7))]);
        d.on_execute_transition(7);
        assert_eq!(d.version(), 0);
        assert_eq!(d.encrypt_opus(b"a").unwrap().as_ref(), b"a");
    }

    #[test]
    fn bad_commit_reports_invalid() {
        let mut d = Dave::new(1, 2);
        d.on_session_description(1);
        let out = d.on_announce_commit(&[0, 9, 1, 2, 3]);
        assert_eq!(out.first(), Some(&Out::Json(protocol::invalid_commit_welcome(9))));
    }
}
