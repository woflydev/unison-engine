//! What to do about the microphone, given the OS permission state. Every shell has the same three
//! states and needs the same decision and the same plain sentences, so they live here; the shell
//! only reads its OS status, calls [`plan`], and draws or acts on the answer.

/// The OS permission, as every platform reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicStatus {
    NotDetermined,
    Authorized,
    Denied,
    /// Blocked by policy; the person cannot change it themselves.
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPlan {
    /// Capture may start.
    Proceed,
    /// Ask the OS now; start only if the answer is yes, and only from a person's next click.
    Ask,
    /// Do not touch the microphone; show [`blocked_note`].
    Blocked,
}

pub fn plan(status: MicStatus) -> MicPlan {
    match status {
        MicStatus::Authorized => MicPlan::Proceed,
        MicStatus::NotDetermined => MicPlan::Ask,
        MicStatus::Denied | MicStatus::Restricted => MicPlan::Blocked,
    }
}

/// One line for a microphone that cannot be used.
pub fn blocked_note(status: MicStatus) -> &'static str {
    match status {
        MicStatus::Restricted => "Your Mac doesn't allow microphone use for Unison.",
        _ => "Unison can't use the microphone. Allow it in System Settings › Privacy & Security › Microphone.",
    }
}

/// One line after the person answered the OS prompt.
pub fn answer_note(allowed: bool, joining: bool) -> &'static str {
    match (allowed, joining) {
        (true, true) => "Microphone allowed. Click the channel again to join.",
        (true, false) => "Microphone allowed.",
        (false, _) => "Unison can't use the microphone. You can allow it in System Settings › Privacy & Security.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_follows_permission() {
        assert_eq!(plan(MicStatus::Authorized), MicPlan::Proceed);
        assert_eq!(plan(MicStatus::NotDetermined), MicPlan::Ask);
        assert_eq!(plan(MicStatus::Denied), MicPlan::Blocked);
        assert_eq!(plan(MicStatus::Restricted), MicPlan::Blocked);
    }

    #[test]
    fn notes_are_one_plain_line() {
        for s in [MicStatus::Denied, MicStatus::Restricted] {
            assert!(!blocked_note(s).contains('\n'));
        }
        assert_ne!(blocked_note(MicStatus::Restricted), blocked_note(MicStatus::Denied));
        assert!(answer_note(true, true).contains("join"));
        assert!(!answer_note(true, false).contains("join"));
        assert!(answer_note(false, true).contains("System Settings"));
    }
}
