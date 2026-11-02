//! One-click "Update and restart": the parts of installing an update that every shell repeats.
//! The shell only does the OS work (mount the download, copy the app, verify its signature, swap the
//! bundle, relaunch); the rules and every sentence the person reads are here.
//!
//! - `check_candidate`: may the freshly copied app replace the running one (same app, the version the
//!   signed feed announced, same signer)?
//! - `install_route`: can this copy replace itself where it lives, or must it fall back to opening the
//!   installer?
//! - `restart_gate`: never restart in the middle of a call.
//! - `Stage`: the state shown by the pill, the button and the status line, with its sentences.

/// What the shell read from an app bundle's code signature.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeIdentity {
    /// Bundle identifier.
    pub identifier: String,
    /// Marketing version from the bundle.
    pub version: String,
    /// Developer team, when signed with a Developer ID or distribution identity.
    pub team_id: Option<String>,
    /// Signed ad hoc (no identity): what builds without a Developer ID are.
    pub ad_hoc: bool,
}

/// A heads-up shown next to "Update and Restart" when this build is signed ad hoc. Keychain trusts an
/// ad hoc app only as that exact binary, so the updated app has to ask for the saved sign-in once;
/// a Deny there leaves the person signed out. Builds signed by a team are trusted across updates.
/// The Keychain is macOS's, so no other platform shows it.
pub fn restart_keychain_note(running: &CodeIdentity) -> Option<&'static str> {
    (cfg!(target_os = "macos") && running.ad_hoc && running.team_id.is_none())
        .then_some("After the restart, macOS will ask if Unison can access your Keychain to sign you back in. Choose Always Allow, or sign in again with your phone.")
}

/// Why an update was not installed. Each has one plain sentence that says what to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    ChecksumMismatch,
    SignatureInvalid,
    WrongApp,
    WrongVersion,
    DifferentSigner,
    OsTooOld(String),
    Unreachable,
    InstallFailed,
    CallInProgress,
}

impl Refusal {
    pub fn text(&self) -> String {
        match self {
            Refusal::ChecksumMismatch => "The download did not match its signed checksum and was discarded. Try again, or download Unison from the website.".into(),
            Refusal::SignatureInvalid => "The new version's code signature is not valid, so it was not installed. Download it again from the website.".into(),
            Refusal::WrongApp => "The download is not Unison, so it was not installed.".into(),
            Refusal::WrongVersion => "The download is not the version the update server announced, so it was not installed.".into(),
            Refusal::DifferentSigner => "The new version is signed by someone other than the signer of this copy, so it was not installed. Download it from the website if you trust it.".into(),
            Refusal::OsTooOld(v) => format!("The new version needs {v} or later. This computer can't take it."),
            Refusal::Unreachable => "The update server did not answer. Try again in a moment.".into(),
            Refusal::InstallFailed => "Unison couldn't install the update, and this copy is unchanged. Try again, or download it from the website.".into(),
            Refusal::CallInProgress => "You're in a call. Press Restart Now when it ends.".into(),
        }
    }
}

/// May `candidate` (the freshly copied app) replace `running`? Same app and the version announced by
/// the signed feed, plus the signer rule: a copy signed by a team may only be replaced by the same
/// team (never downgraded to ad hoc or handed to another team); an ad hoc copy (developer and
/// un-notarised builds, which have no team to compare) may take any build the feed's own Ed25519
/// signature vouches for. The shell has already confirmed the signature is valid and unbroken.
pub fn check_candidate(running: &CodeIdentity, candidate: &CodeIdentity, expected_version: &str) -> Result<(), Refusal> {
    if running.identifier != candidate.identifier {
        return Err(Refusal::WrongApp);
    }
    if candidate.version.trim() != expected_version.trim() {
        return Err(Refusal::WrongVersion);
    }
    match (&running.team_id, &candidate.team_id) {
        (Some(a), Some(b)) if a == b => Ok(()),
        (Some(_), _) => Err(Refusal::DifferentSigner),
        (None, _) => Ok(()),
    }
}

/// How an install can go on this machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// Replace the running copy where it lives, then relaunch.
    InPlace,
    /// Fall back to opening the installer for the person, with the sentence to show.
    OpenInstaller(String),
}

/// Can this copy replace itself? `location_writable`: the app's folder and bundle are writable by the
/// person. `read_only_location`: running from a disk image, a read-only volume or a randomised
/// quarantine path, where replacing the bundle would change nothing the person keeps.
pub fn install_route(location_writable: bool, read_only_location: bool) -> Route {
    if read_only_location {
        Route::OpenInstaller("Unison is running from a disk image, so it can't update itself. The installer is opening: drag Unison into Applications.".into())
    } else if !location_writable {
        Route::OpenInstaller("Unison can't replace itself where it is installed. The installer is opening: drag Unison over the old copy.".into())
    } else {
        Route::InPlace
    }
}

/// Whether the restart may happen now. Never restart in the middle of a call.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    Now,
    WaitForCall(String),
}

pub fn restart_gate(in_call: bool) -> Gate {
    if in_call { Gate::WaitForCall(Refusal::CallInProgress.text()) } else { Gate::Now }
}

/// Download progress in 0...1, or `None` while the total is unknown.
pub fn progress_fraction(received: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| (received as f64 / total as f64).clamp(0.0, 1.0))
}

/// Where an update is, for every shell's pill, button and status line.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Idle,
    /// This build ships no update key, so there is no update channel.
    NotConfigured,
    Checking,
    UpToDate,
    Available { version: String },
    Downloading { received: u64, total: u64 },
    Verifying,
    Installing,
    /// Copied and verified; waiting for the person to restart (during a call, or by their choice).
    ReadyToRestart { version: String, waiting_for_call: bool },
    Restarting,
    Failed(String),
}

impl Stage {
    /// The one line shown beside the action. Empty when nothing needs saying.
    pub fn text(&self) -> String {
        match self {
            Stage::Idle => String::new(),
            Stage::NotConfigured => "This build has no update channel.".into(),
            Stage::Checking => "Checking for updates…".into(),
            Stage::UpToDate => "You're up to date.".into(),
            Stage::Available { version } => format!("Version {version} is available."),
            Stage::Downloading { received, total } => match progress_fraction(*received, *total) {
                Some(f) => format!("Downloading… {}%", (f * 100.0).floor() as u32),
                None => "Downloading…".into(),
            },
            Stage::Verifying => "Checking the download…".into(),
            Stage::Installing => "Installing…".into(),
            Stage::ReadyToRestart { version, waiting_for_call: true } => format!("Version {version} is ready. {}", Refusal::CallInProgress.text()),
            Stage::ReadyToRestart { version, waiting_for_call: false } => format!("Version {version} is ready. Restart to finish."),
            Stage::Restarting => "Restarting…".into(),
            Stage::Failed(m) => m.clone(),
        }
    }

    /// Label for the action button, or `None` when no action applies.
    pub fn action_label(&self) -> Option<&'static str> {
        match self {
            Stage::Available { .. } => Some("Update and Restart"),
            Stage::ReadyToRestart { .. } => Some("Restart Now"),
            Stage::Failed(_) => Some("Try Again"),
            _ => None,
        }
    }

    /// True while something is running that another click must not interrupt.
    pub fn busy(&self) -> bool {
        matches!(self, Stage::Checking | Stage::Downloading { .. } | Stage::Verifying | Stage::Installing | Stage::Restarting)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keychain_note_only_for_ad_hoc_builds() {
        let id = |ad_hoc: bool, team: Option<&str>| super::CodeIdentity { identifier: "app.unison.client".into(), version: "0.3.0".into(), team_id: team.map(Into::into), ad_hoc };
        // The Keychain is macOS's: other platforms never show the note.
        assert_eq!(super::restart_keychain_note(&id(true, None)).is_some(), cfg!(target_os = "macos"));
        assert!(super::restart_keychain_note(&id(false, Some("ABCDE12345"))).is_none());
    }

    use super::*;

    fn ident(team: Option<&str>, version: &str) -> CodeIdentity {
        CodeIdentity { identifier: "app.unison.client".into(), version: version.into(), team_id: team.map(Into::into), ad_hoc: team.is_none() }
    }

    #[test]
    fn signer_rules() {
        let run = ident(Some("TEAM1"), "0.2.0");
        assert!(check_candidate(&run, &ident(Some("TEAM1"), "0.3.0"), "0.3.0").is_ok());
        assert_eq!(check_candidate(&run, &ident(Some("TEAM2"), "0.3.0"), "0.3.0"), Err(Refusal::DifferentSigner));
        assert_eq!(check_candidate(&run, &ident(None, "0.3.0"), "0.3.0"), Err(Refusal::DifferentSigner), "no downgrade to ad hoc");
        assert_eq!(check_candidate(&run, &ident(Some("TEAM1"), "0.2.5"), "0.3.0"), Err(Refusal::WrongVersion));
        let mut other = ident(Some("TEAM1"), "0.3.0");
        other.identifier = "com.evil.app".into();
        assert_eq!(check_candidate(&run, &other, "0.3.0"), Err(Refusal::WrongApp));
        let adhoc = ident(None, "0.2.0");
        assert!(check_candidate(&adhoc, &ident(None, "0.3.0"), "0.3.0").is_ok());
        assert!(check_candidate(&adhoc, &ident(Some("TEAM1"), "0.3.0"), "0.3.0").is_ok(), "ad hoc to Developer ID");
    }

    #[test]
    fn routes_gates_and_text() {
        assert_eq!(install_route(true, false), Route::InPlace);
        assert!(matches!(install_route(false, false), Route::OpenInstaller(_)));
        assert!(matches!(install_route(true, true), Route::OpenInstaller(m) if m.contains("disk image")));
        assert_eq!(restart_gate(false), Gate::Now);
        assert!(matches!(restart_gate(true), Gate::WaitForCall(m) if m.contains("call")));
        assert_eq!(progress_fraction(5, 0), None);
        assert_eq!(progress_fraction(50, 200), Some(0.25));
        assert_eq!(progress_fraction(900, 200), Some(1.0));
        assert_eq!(Stage::Downloading { received: 50, total: 200 }.text(), "Downloading… 25%");
        assert_eq!(Stage::Available { version: "1.2.3".into() }.action_label(), Some("Update and Restart"));
        assert!(Stage::Installing.busy() && !Stage::Failed("x".into()).busy());
        assert!(Stage::ReadyToRestart { version: "1".into(), waiting_for_call: true }.text().contains("call"));
        for r in [
            Refusal::ChecksumMismatch,
            Refusal::SignatureInvalid,
            Refusal::WrongApp,
            Refusal::WrongVersion,
            Refusal::DifferentSigner,
            Refusal::Unreachable,
            Refusal::InstallFailed,
            Refusal::CallInProgress,
        ] {
            let t = r.text();
            assert!(t.ends_with('.') && !t.contains('\n'), "plain sentence: {t}");
        }
    }
}
