//! UniFFI surface for the platform shells.
//!
//! IDs cross the boundary as strings. Queries read the store synchronously and
//! return only what is on screen; commands spawn on the core runtime and report
//! back through [`EventListener`]. Every write takes a [`UserGesture`], which the
//! shell may only supply from a UI event handler.

use std::path::PathBuf;

use std::sync::Arc;

use unison_core::engine::{Engine, EngineSink};

use unison_core::fidelity::HostInfo;

use unison_core::forum::{self, ForumQuery, SortOrder, TagMatch};

use unison_core::intent::{Gesture as CoreGesture, UserIntent};

use unison_core::model::{self, Channel, Id, Message, channel_type, message_type};

use unison_core::power::PowerState;

use unison_core::privacy::{self, Category, Policy};

use unison_core::remote_auth::RemoteAuthEvent;

use unison_core::rest::LocalFile;

use unison_core::session::{ConnectionState, SessionEvent};

use unison_core::store::{Delta, RailEntry, Store};

use unison_markdown::{Resolve, SpanKind};

mod accounts;
mod actions;
mod audio;
mod channels;
mod comfort;
mod composer;
mod dms;
mod forums;
mod interactions;
mod lists;
mod local;
mod media;
mod messages;
mod moderation;
mod nav;
mod people;
mod presence;
mod release;
mod profile;
mod admin;
mod saved;
mod search;
mod sends;
mod uploads;
mod slash;
mod stage;
mod settings;
mod setup;
mod shell_nav;
mod types;
mod voice;

pub use accounts::*;
pub use channels::*;
pub use composer::*;
pub use dms::*;
pub use forums::*;
pub use interactions::*;
pub use messages::*;
pub use nav::*;
pub use setup::*;
pub use people::*;
pub use profile::*;
pub use admin::*;
pub use saved::*;
pub use search::*;
pub use uploads::*;
pub use settings::*;
pub use types::*;
pub use voice::*;

uniffi::setup_scaffolding!();

fn id(s: &str) -> Id {
    s.parse().unwrap_or_default()
}

// ===== Callbacks =====

#[uniffi::export(with_foreign)]
pub trait EventListener: Send + Sync {
    fn on_deltas(&self, account: String, deltas: Vec<UiDelta>);
    fn on_connection(&self, account: String, state: ConnState);
    fn on_logged_out(&self, account: String);
    fn on_login(&self, event: LoginEvent);
    fn on_account_added(&self, account: AccountInfo, token: String);
    fn on_error(&self, account: String, message: String);
    fn on_captcha(&self, account: String, sitekey: Option<String>, rqdata: Option<String>);
    /// Pinned messages arrived for a channel the user asked about.
    fn on_pins(&self, account: String, channel_id: String, messages: Vec<MessageItem>);
    /// Result of `fetch_profile`.
    fn on_profile(&self, account: String, profile: ProfileItem);
    /// Favourite and frecent emoji loaded for the reaction picker (`fetch_reaction_emoji`).
    fn on_reaction_emoji(&self, account: String, emoji: ReactionEmojis);
    /// Result of `fetch_gifs`: trending (with category tiles) or search results for `query`.
    fn on_gifs(&self, account: String, query: String, categories: Vec<GifCategory>, gifs: Vec<GifItem>);
    /// Result of `fetch_own_profile`: the raw editable profile fields.
    fn on_own_profile(&self, account: String, profile: OwnProfile);
    /// A command produced a place to go (e.g. a DM was opened from the friends list).
    fn on_navigate(&self, account: String, guild_id: Option<String>, channel_id: String);
    /// A remote video access unit (Annex B H.264). `stream` is `call` for camera video or a stream key.
    fn on_remote_video(&self, account: String, stream: String, user_id: String, data: Vec<u8>, keyframe: bool, pts: u32);
    /// The encoder should react: a keyframe was requested, or the bitrate target changed (bits/s in `value`).
    /// `source` is `call` (camera) or the stream key (screen).
    fn on_video_control(&self, account: String, source: String, kind: VideoControlKind, value: u32);
}

#[derive(uniffi::Error, Debug, thiserror::Error)]
pub enum FfiError {
    #[error("{message}")]
    Failed { message: String },
}

impl From<unison_core::Error> for FfiError {
    fn from(e: unison_core::Error) -> Self {
        FfiError::Failed {
            message: e.user_message(),
        }
    }
}

// ===== Engine =====

struct Sink {
    listener: Arc<dyn EventListener>,
}

impl EngineSink for Sink {
    fn session_event(&self, account: Id, event: SessionEvent) {
        let a = account.to_string();
        match event {
            SessionEvent::Deltas(ds) => {
                let ui: Vec<UiDelta> = ds.iter().filter_map(ui_delta).collect();
                if !ui.is_empty() {
                    self.listener.on_deltas(a, ui);
                }
            }
            SessionEvent::Connection(c) => self.listener.on_connection(
                a,
                match c {
                    ConnectionState::Connecting => ConnState::Connecting,
                    ConnectionState::Connected => ConnState::Connected,
                    ConnectionState::Offline => ConnState::Offline,
                },
            ),
            SessionEvent::LoggedOut => self.listener.on_logged_out(a),
            SessionEvent::Captcha { sitekey, rqdata } => {
                self.listener.on_captcha(a, sitekey, rqdata)
            }
            SessionEvent::Error(m) => self.listener.on_error(a, m),
        }
    }

    fn login_event(&self, event: RemoteAuthEvent) {
        self.listener.on_login(match event {
            RemoteAuthEvent::Qr { url } => LoginEvent::Qr { url },
            RemoteAuthEvent::Scanned {
                user_id,
                username,
                avatar,
            } => LoginEvent::Scanned {
                username,
                avatar_url: avatar.map(|a| {
                    format!("https://cdn.discordapp.com/avatars/{user_id}/{a}.webp?size=128")
                }),
            },
            RemoteAuthEvent::LoggedIn { .. } => LoginEvent::LoggedIn,
            RemoteAuthEvent::Mfa { ticket, totp, sms, backup, webauthn, login_instance_id } => {
                LoginEvent::Mfa { ticket, totp, sms, backup, webauthn, login_instance_id }
            }
            RemoteAuthEvent::Captcha { service, sitekey, rqdata, rqtoken, session_id } => {
                LoginEvent::Captcha { service, sitekey, rqdata, rqtoken, session_id }
            }
            RemoteAuthEvent::VerifyEmail { message } => LoginEvent::VerifyEmail { message },
            RemoteAuthEvent::Cancelled => LoginEvent::Cancelled,
            RemoteAuthEvent::Failed { message } => LoginEvent::Failed { message },
        });
    }

    fn account_added(&self, user: model::User, token: String) {
        self.listener.on_account_added(account_info(&user), token);
    }
}

fn account_info(u: &model::User) -> AccountInfo {
    AccountInfo {
        id: u.id.to_string(),
        username: u.username.clone(),
        display_name: u.display_name().to_string(),
        avatar_url: u.avatar_url(128),
    }
}

#[derive(uniffi::Object)]
pub struct UnisonEngine {
    inner: Arc<Engine>,
    listener: Arc<dyn EventListener>,
    policy_path: PathBuf,
}

#[uniffi::export]
impl UnisonEngine {
    /// `languages` is `Locale.preferredLanguages`; `timezone` is `TimeZone.current.identifier`.
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        languages: Vec<String>,
        timezone: String,
        signed_profile: Option<String>,
        listener: Arc<dyn EventListener>,
    ) -> Result<Arc<Self>, FfiError> {
        init_log();
        let dir = PathBuf::from(&data_dir);
        let policy_path = dir.join("privacy.json");
        let policy: Policy = std::fs::read(&policy_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut host = HostInfo::detect();
        if !languages.is_empty() {
            host.languages = languages;
        }
        if !timezone.is_empty() {
            host.timezone = timezone;
        }
        let sink = Arc::new(Sink {
            listener: listener.clone(),
        });
        let inner = Engine::new(dir, host, sink, signed_profile, policy)?;
        Ok(Arc::new(Self {
            inner,
            listener,
            policy_path,
        }))
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

#[uniffi::export]
impl UnisonEngine {
    /// A script is driving the app: from now on no REST request is sent (see `rest::AUTOMATION`).
    pub fn set_automation(&self, on: bool) {
        unison_core::rest::set_automation(on);
    }
}

impl UnisonEngine {
    fn with_store<T>(&self, account_id: &str, f: impl FnOnce(&Store) -> T) -> Option<T> {
        let s = self.inner.session(id(account_id))?;
        let guard = s.store.read();
        Some(f(&guard))
    }

    fn spawn_write<F, Fut>(&self, account_id: String, gesture: UserGesture, f: F)
    where
        F: FnOnce(Arc<unison_core::session::Session>, UserIntent) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = unison_core::Result<()>> + Send + 'static,
    {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            match f(s, intent).await {
                Ok(()) | Err(unison_core::Error::Cancelled) => {}
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }
}

/// Opt-in diagnostics: `UNISON_LOG=/path/file` (filter via `UNISON_LOG_FILTER`, default `unison_core=debug`).
/// Nothing is logged unless the variable is set; logs never include tokens or message content.
fn init_log() {
    let Ok(path) = std::env::var("UNISON_LOG") else {
        return;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let filter = std::env::var("UNISON_LOG_FILTER")
        .unwrap_or_else(|_| "unison_core=debug,unison_ffi=debug".into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init();
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_prefers_prefix_and_word_starts() {
        let a = fuzzy("gen", "general").unwrap();
        let b = fuzzy("gen", "off-topic-gen").unwrap();
        let c = fuzzy("gen", "garden-news").unwrap();
        assert!(a > b && b > c);
        assert!(fuzzy("xyz", "general").is_none());
    }
}
