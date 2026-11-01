//! Update verification and the "Update and restart" rules for a shell (see `unison_core::update`
//! and `unison_core::update_install`). The shell does the OS part; every decision and sentence is here.

use super::*;
use unison_core::update::{self, UPDATE_PUBLIC_KEY};
use unison_core::update_install as install;

#[derive(uniffi::Record, Clone, Debug)]
pub struct UpdateOffer {
    pub version: String,
    pub build: u64,
    /// The download for this OS and architecture.
    pub url: String,
    pub sha256: String,
    pub notes: String,
    pub min_os: String,
    /// Bytes of the download, 0 when unknown.
    pub size: u64,
    /// `stable` or `beta`.
    pub channel: String,
}

/// The running build as the shell knows it.
#[derive(uniffi::Record, Clone, Debug)]
pub struct UpdatePlatform {
    /// `macos`, `windows` or `linux`.
    pub os: String,
    /// `arm64` or `x86_64`.
    pub arch: String,
    /// Dotted OS version, e.g. `26.0.1`.
    pub os_version: String,
}

/// What the shell read from an app bundle's code signature.
#[derive(uniffi::Record, Clone, Debug)]
pub struct CodeIdentity {
    pub identifier: String,
    pub version: String,
    pub team_id: Option<String>,
    pub ad_hoc: bool,
}

/// How an update can be installed on this machine.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum InstallRoute {
    InPlace,
    /// Open the installer instead; show this sentence.
    OpenInstaller { message: String },
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum RestartGate {
    Now,
    /// A call is active; show this sentence and wait.
    WaitForCall { message: String },
}

/// Where an update is. The shell stores one of these and draws it with `update_stage_*`.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum UpdateStage {
    Idle,
    NotConfigured,
    Checking,
    UpToDate,
    Available { version: String },
    Downloading { received: u64, total: u64 },
    Verifying,
    Installing,
    ReadyToRestart { version: String, waiting_for_call: bool },
    Restarting,
    Failed { message: String },
}

/// Problems the shell hits that need the shared sentence.
#[derive(uniffi::Enum, Clone, Copy, Debug)]
pub enum UpdateProblem {
    ChecksumMismatch,
    SignatureInvalid,
    Unreachable,
    InstallFailed,
}

/// True when this build ships an update public key (otherwise there is no update channel).
#[uniffi::export]
pub fn update_channel_configured() -> bool {
    !UPDATE_PUBLIC_KEY.trim().is_empty()
}

/// The feed to ask for a channel (`stable` or `beta`), derived from the configured base URL.
#[uniffi::export]
pub fn update_feed_url(base: String, channel: String) -> String {
    update::feed_url(&base, &channel)
}

/// Verify a signed appcast and pick the download for `platform`. `Ok(None)` means there is nothing
/// to install: not newer, another channel, or no build for this platform.
#[uniffi::export]
pub fn verify_appcast(
    signed_json: String,
    current_version: String,
    current_build: u64,
    platform: UpdatePlatform,
    channel: String,
) -> Result<Option<UpdateOffer>, FfiError> {
    let p = update::Platform { os: platform.os, arch: platform.arch, os_version: platform.os_version };
    let Some(o) = update::evaluate(&signed_json, UPDATE_PUBLIC_KEY, &p, &current_version, current_build, &channel)? else {
        return Ok(None);
    };
    Ok(Some(UpdateOffer {
        version: o.version,
        build: o.build,
        url: o.url,
        sha256: o.sha256,
        notes: o.notes,
        min_os: String::new(),
        size: o.size,
        channel: o.channel,
    }))
}

#[uniffi::export]
pub fn update_digest_matches(sha256_hex: String, digest: Vec<u8>) -> bool {
    update::digest_matches(&sha256_hex, &digest)
}

/// May the freshly copied app replace the running one? The error message is the sentence to show.
#[uniffi::export]
pub fn update_check_candidate(running: CodeIdentity, candidate: CodeIdentity, expected_version: String) -> Result<(), FfiError> {
    let id = |c: CodeIdentity| install::CodeIdentity { identifier: c.identifier, version: c.version, team_id: c.team_id, ad_hoc: c.ad_hoc };
    install::check_candidate(&id(running), &id(candidate), &expected_version).map_err(|r| FfiError::Failed { message: r.text() })
}

#[uniffi::export]
pub fn update_install_route(location_writable: bool, read_only_location: bool) -> InstallRoute {
    match install::install_route(location_writable, read_only_location) {
        install::Route::InPlace => InstallRoute::InPlace,
        install::Route::OpenInstaller(message) => InstallRoute::OpenInstaller { message },
    }
}

/// The sentence to show beside "Update and Restart" for this build, if any (an ad hoc signed build
/// makes macOS ask for the saved sign-in once after the update).
#[uniffi::export]
pub fn update_restart_note(running: CodeIdentity) -> Option<String> {
    let id = install::CodeIdentity { identifier: running.identifier, version: running.version, team_id: running.team_id, ad_hoc: running.ad_hoc };
    install::restart_keychain_note(&id).map(str::to_string)
}

#[uniffi::export]
pub fn update_restart_gate(in_call: bool) -> RestartGate {
    match install::restart_gate(in_call) {
        install::Gate::Now => RestartGate::Now,
        install::Gate::WaitForCall(message) => RestartGate::WaitForCall { message },
    }
}

#[uniffi::export]
pub fn update_problem_text(problem: UpdateProblem) -> String {
    match problem {
        UpdateProblem::ChecksumMismatch => install::Refusal::ChecksumMismatch,
        UpdateProblem::SignatureInvalid => install::Refusal::SignatureInvalid,
        UpdateProblem::Unreachable => install::Refusal::Unreachable,
        UpdateProblem::InstallFailed => install::Refusal::InstallFailed,
    }
    .text()
}

fn stage(s: UpdateStage) -> install::Stage {
    match s {
        UpdateStage::Idle => install::Stage::Idle,
        UpdateStage::NotConfigured => install::Stage::NotConfigured,
        UpdateStage::Checking => install::Stage::Checking,
        UpdateStage::UpToDate => install::Stage::UpToDate,
        UpdateStage::Available { version } => install::Stage::Available { version },
        UpdateStage::Downloading { received, total } => install::Stage::Downloading { received, total },
        UpdateStage::Verifying => install::Stage::Verifying,
        UpdateStage::Installing => install::Stage::Installing,
        UpdateStage::ReadyToRestart { version, waiting_for_call } => install::Stage::ReadyToRestart { version, waiting_for_call },
        UpdateStage::Restarting => install::Stage::Restarting,
        UpdateStage::Failed { message } => install::Stage::Failed(message),
    }
}

/// The status line for a stage (empty when nothing needs saying).
#[uniffi::export]
pub fn update_stage_text(stage_value: UpdateStage) -> String {
    stage(stage_value).text()
}

/// The action button's label for a stage, if it has an action.
#[uniffi::export]
pub fn update_stage_action(stage_value: UpdateStage) -> Option<String> {
    stage(stage_value).action_label().map(str::to_string)
}

/// True while a stage is running and another click must not interrupt it.
#[uniffi::export]
pub fn update_stage_busy(stage_value: UpdateStage) -> bool {
    stage(stage_value).busy()
}

/// Download progress in 0...1, or nil while the total is unknown.
#[uniffi::export]
pub fn update_progress(received: u64, total: u64) -> Option<f64> {
    install::progress_fraction(received, total)
}
