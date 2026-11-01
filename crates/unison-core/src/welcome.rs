//! The welcome guide: when it shows and which steps it has. What each step looks like is the
//! shell's (`docs/WELCOME.md`).
//!
//! The guide has a revision. A first launch with no account plays all of it. After an update the
//! data folder (and the revision a person last saw) survive, so only steps added since then play,
//! as a short "what's new"; an update that adds no step shows nothing. Settings can ask for a
//! replay, which plays everything on the next launch. A person who already used Unison before the
//! guide existed is marked as having seen the current revision without being shown it.

use std::path::Path;

/// Bump when a step is added (give it `since: REVISION`).
pub const REVISION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Play the steps added after revision `from` (0 plays all), then record the current revision.
    Show { from: u32 },
    /// Someone who already uses Unison: record the current revision, show nothing.
    MarkSeen,
    Skip,
}

pub fn plan(seen: u32, replay: bool, has_accounts: bool, used_before: bool) -> Plan {
    if replay {
        return Plan::Show { from: 0 };
    }
    if seen == 0 {
        return if has_accounts || used_before { Plan::MarkSeen } else { Plan::Show { from: 0 } };
    }
    if seen < REVISION && steps(seen).len() > 1 {
        Plan::Show { from: seen }
    } else if seen < REVISION {
        Plan::MarkSeen
    } else {
        Plan::Skip
    }
}

/// Whether the data folder holds an account's cache from an earlier run.
pub fn used_before(data_dir: &Path) -> bool {
    std::fs::read_dir(data_dir)
        .map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("account-")))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Hello,
    Search,
    Appearance,
    /// What Unison holds back from Discord, with a switch for each.
    Privacy,
    /// The conveniences that are on from the start, and the keys.
    Features,
    /// The last screen: "Sign in" before the first account, "Done" after.
    Finish,
}

impl Step {
    /// The revision that introduced the step.
    fn since(self) -> u32 {
        1
    }
}

/// The steps to play, in order: all of them from revision 0, only newer ones after that, then
/// the closing screen. (Notifications are not a step: the system is asked after sign-in, when a
/// message can actually arrive.)
pub fn steps(from: u32) -> Vec<Step> {
    let mut all = vec![Step::Hello, Step::Search, Step::Appearance, Step::Privacy, Step::Features];
    all.retain(|s| s.since() > from);
    all.push(Step::Finish);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_shows_all_and_existing_users_are_marked() {
        assert_eq!(plan(0, false, false, false), Plan::Show { from: 0 });
        assert_eq!(plan(0, false, true, false), Plan::MarkSeen);
        assert_eq!(plan(0, false, false, true), Plan::MarkSeen);
        assert_eq!(plan(REVISION, false, true, true), Plan::Skip);
    }

    #[test]
    fn replay_plays_everything() {
        assert_eq!(plan(REVISION, true, true, true), Plan::Show { from: 0 });
    }

    #[test]
    fn a_revision_without_new_steps_shows_nothing() {
        // With REVISION 1 every step has since 1, so from 1 only the closing screen is left.
        assert_eq!(steps(REVISION), vec![Step::Finish]);
    }

    #[test]
    fn an_old_data_folder_counts_as_used() {
        let d = std::env::temp_dir().join(format!("unison-welcome-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(!used_before(&d));
        std::fs::write(d.join("account-1.sqlite"), b"").unwrap();
        assert!(used_before(&d));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_full_run_starts_and_ends_in_place() {
        assert_eq!(steps(0).first(), Some(&Step::Hello));
        assert_eq!(steps(0).last(), Some(&Step::Finish));
    }
}
