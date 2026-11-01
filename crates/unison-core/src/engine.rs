//! All accounts, one runtime, one power state.
//!
//! Every signed-in account stays connected ("hot"), so switching is a pointer
//! swap. Only the active account delivers deltas at full cadence; the others
//! batch theirs and only matter for rail badges and the unified inbox.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};

use crate::cache::Cache;
use crate::fidelity::{ClientIdentity, HostInfo};
use crate::model::{Id, User};
use crate::power::PowerState;
use crate::privacy::Policy;
use crate::profile::{self, IdentityProfile};
use crate::remote_auth::{self, RemoteAuthEvent};
use crate::error::CaptchaSolution;
use crate::rest::{LoginOutcome, Rest};
use crate::session::{Session, SessionEvent};
use crate::store::Delta;
use crate::transport::Transport;
use crate::{Error, Result};

const INBOX_CAPACITY: usize = 200;

pub trait EngineSink: Send + Sync {
    fn session_event(&self, account: Id, event: SessionEvent);
    fn login_event(&self, event: RemoteAuthEvent);
    /// A new account finished signing in. The shell stores `token` in the Keychain.
    fn account_added(&self, user: User, token: String);
}

#[derive(Debug, Clone)]
pub struct InboxEntry {
    pub account: Id,
    pub guild: Option<Id>,
    pub channel: Id,
    pub message: Id,
    pub author: String,
    pub preview: String,
}

pub struct Engine {
    pub rt: tokio::runtime::Runtime,
    data_dir: PathBuf,
    pub local: Arc<crate::localstate::LocalState>,
    pub profile: IdentityProfile,
    host: RwLock<HostInfo>,
    pub policy: Arc<RwLock<Policy>>,
    sessions: RwLock<Vec<Arc<Session>>>,
    active: RwLock<Option<Id>>,
    power: RwLock<PowerState>,
    sink: Arc<dyn EngineSink>,
    inbox: Arc<Mutex<VecDeque<InboxEntry>>>,
    login: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The unauthenticated client of an in-progress password sign-in (keeps cookies and fingerprint).
    pw_rest: Mutex<Option<Arc<Rest>>>,
}

impl Engine {
    pub fn new(
        data_dir: PathBuf,
        host: HostInfo,
        sink: Arc<dyn EngineSink>,
        signed_profile: Option<String>,
        policy: Policy,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&data_dir).map_err(|e| Error::Other(e.to_string()))?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("unison-core")
            .on_thread_start(lower_thread_qos)
            .enable_all()
            .build()
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok(Arc::new(Self {
            rt,
            local: Arc::new(crate::localstate::LocalState::open(&data_dir)?),
            data_dir,
            profile: profile::select(signed_profile.as_deref()),
            host: RwLock::new(host),
            policy: Arc::new(RwLock::new(policy)),
            sessions: RwLock::new(Vec::new()),
            active: RwLock::new(None),
            power: RwLock::new(PowerState::Active),
            sink,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            login: Mutex::new(None),
            pw_rest: Mutex::new(None),
        }))
    }

    /// Write pending drafts a couple of seconds from now (a timer exists only while one is pending).
    pub fn flush_drafts_later(&self) {
        let local = self.local.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            local.flush_drafts();
        });
    }

    fn new_rest(&self, saved_cookies: Option<&str>) -> Result<Arc<Rest>> {
        let _guard = self.rt.enter();
        let transport = Arc::new(Transport::new(saved_cookies)?);
        let identity = Arc::new(ClientIdentity::new(self.profile.clone(), self.host.read().clone()));
        Ok(Arc::new(Rest::new(transport, identity, self.policy.clone())))
    }

    fn cache_for(&self, user: Id) -> Result<Arc<Cache>> {
        let cache = Arc::new(Cache::open(&self.data_dir.join(format!("account-{user}.sqlite")))?);
        let (bytes, per_channel) = crate::localstate::index_limits(self.local.index_level());
        cache.set_limits(bytes, per_channel);
        Ok(cache)
    }

    /// Change how much message history is kept for search, for every account at once.
    pub fn set_index_level(&self, level: u8) {
        self.local.set_index_level(level);
        let (bytes, per_channel) = crate::localstate::index_limits(self.local.index_level());
        for s in self.sessions_snapshot() {
            s.cache.set_limits(bytes, per_channel);
            let _ = s.cache.enforce_budget();
        }
    }

    /// Bytes and messages the search history holds across the signed-in accounts.
    pub fn index_usage(&self) -> (u64, u64) {
        self.sessions_snapshot().iter().fold((0, 0), |(b, m), s| (b + s.cache.used_bytes().unwrap_or(0), m + s.cache.message_count().unwrap_or(0)))
    }

    pub fn clear_index(&self) {
        for s in self.sessions_snapshot() {
            let _ = s.cache.clear_messages();
        }
    }

    // ----- Accounts -----

    /// Begin QR login. Events go to [`EngineSink::login_event`].
    pub fn start_login(self: &Arc<Self>) -> Result<()> {
        self.cancel_login();
        // Called from the shell's thread: spawn onto the core runtime, not the caller's (none).
        let _guard = self.rt.enter();
        let rest = self.new_rest(None)?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = remote_auth::start(rest, tx);
        let me = self.clone();
        self.rt.spawn(async move {
            while let Some(ev) = rx.recv().await {
                if let RemoteAuthEvent::LoggedIn { token } = &ev {
                    let token = token.clone();
                    let me2 = me.clone();
                    tokio::spawn(async move {
                        if let Err(e) = me2.add_account(token).await {
                            me2.sink.login_event(RemoteAuthEvent::Failed { message: e.to_string() });
                        }
                    });
                }
                me.sink.login_event(ev);
            }
        });
        *self.login.lock() = Some(handle);
        Ok(())
    }

    pub fn cancel_login(&self) {
        if let Some(h) = self.login.lock().take() {
            h.abort();
        }
        *self.pw_rest.lock() = None;
    }

    /// Password sign-in, step 1. Stops the QR flow; events arrive through [`EngineSink::login_event`].
    pub fn login_password(self: &Arc<Self>, login: String, password: String, captcha: Option<CaptchaSolution>) -> Result<()> {
        self.cancel_login();
        let _guard = self.rt.enter();
        let rest = self.new_rest(None)?;
        *self.pw_rest.lock() = Some(rest.clone());
        let me = self.clone();
        self.rt.spawn(async move {
            // The login page loads experiments first; the response sets the fingerprint and cookies.
            let _ = rest.fetch_fingerprint().await;
            let out = rest.login_password(&login, &password, captcha).await;
            me.finish_login_step(out).await;
        });
        Ok(())
    }

    /// Password retry after a captcha: same client, same credentials.
    pub fn login_password_retry(self: &Arc<Self>, login: String, password: String, captcha: CaptchaSolution) -> Result<()> {
        let Some(rest) = self.pw_rest.lock().clone() else { return self.login_password(login, password, Some(captcha)) };
        let me = self.clone();
        self.rt.spawn(async move {
            let out = rest.login_password(&login, &password, Some(captcha)).await;
            me.finish_login_step(out).await;
        });
        Ok(())
    }

    /// Second factor: `kind` is `totp`, `sms`, `backup` or `webauthn`.
    pub fn login_mfa(self: &Arc<Self>, kind: String, code: String, ticket: String, instance: Option<String>, captcha: Option<CaptchaSolution>) -> Result<()> {
        let Some(rest) = self.pw_rest.lock().clone() else { return Err(Error::Other("no sign-in in progress".into())) };
        let me = self.clone();
        self.rt.spawn(async move {
            let out = rest.login_mfa(&kind, &code, &ticket, instance.as_deref(), captcha).await;
            me.finish_login_step(out).await;
        });
        Ok(())
    }

    pub fn login_sms_send(self: &Arc<Self>, ticket: String) -> Result<()> {
        let Some(rest) = self.pw_rest.lock().clone() else { return Err(Error::Other("no sign-in in progress".into())) };
        let me = self.clone();
        self.rt.spawn(async move {
            if let Err(e) = rest.login_sms_send(&ticket).await {
                me.sink.login_event(RemoteAuthEvent::Failed { message: e.to_string() });
            }
        });
        Ok(())
    }

    async fn finish_login_step(self: &Arc<Self>, out: Result<LoginOutcome>) {
        use crate::rest::LoginOutcome as O;
        let ev = match out {
            Err(e) => RemoteAuthEvent::Failed { message: e.to_string() },
            Ok(O::Token(token)) => {
                *self.pw_rest.lock() = None;
                match self.add_account(token.clone()).await {
                    Ok(()) => RemoteAuthEvent::LoggedIn { token },
                    Err(e) => RemoteAuthEvent::Failed { message: e.to_string() },
                }
            }
            Ok(O::Mfa(m)) => RemoteAuthEvent::Mfa {
                ticket: m.ticket, totp: m.totp, sms: m.sms, backup: m.backup, webauthn: m.webauthn,
                login_instance_id: m.login_instance_id,
            },
            Ok(O::Captcha(c)) => RemoteAuthEvent::Captcha {
                service: c.captcha_service, sitekey: c.captcha_sitekey, rqdata: c.captcha_rqdata,
                rqtoken: c.captcha_rqtoken, session_id: c.captcha_session_id,
            },
            Ok(O::VerifyEmail(message)) => RemoteAuthEvent::VerifyEmail { message },
            Ok(O::PhoneVerification) => RemoteAuthEvent::Failed {
                message: "Discord needs to verify a phone number for this account. Do that on discord.com, then sign in here.".into(),
            },
            Ok(O::Blocked(message)) | Ok(O::Failed(message)) => RemoteAuthEvent::Failed { message },
        };
        self.sink.login_event(ev);
    }

    /// Validate a fresh token, then bring the account online.
    async fn add_account(self: &Arc<Self>, token: String) -> Result<()> {
        let rest = self.new_rest(None)?;
        rest.set_token(Some(token.clone()));
        let user: User = rest.get("/users/@me").await?;
        if self.session(user.id).is_some() {
            return Ok(());
        }
        let cache = self.cache_for(user.id)?;
        let session = self.make_session(user.id, rest, cache)?;
        session.store.write().notify_keywords = self.local.comfort().keywords;
        session.start(token.clone());
        self.sessions.write().push(session);
        if self.active.read().is_none() {
            self.set_active(user.id);
        }
        self.sink.account_added(user, token);
        Ok(())
    }

    /// Restore an account from the Keychain at launch: paints from cache with no network wait.
    pub fn restore_account(self: &Arc<Self>, user: Id, token: String) -> Result<()> {
        if self.session(user).is_some() {
            return Ok(());
        }
        let cache = self.cache_for(user)?;
        let cookies = cache.get_string("cookies")?;
        let rest = self.new_rest(cookies.as_deref())?;
        let session = self.make_session(user, rest, cache)?;
        session.store.write().notify_keywords = self.local.comfort().keywords;
        {
            // One live gateway session per account is normal, but opening them all in the same
            // instant at launch is not: the first connects now, each later one 5 s after the
            // previous plus up to 3 s of jitter. The cache paints every account at once.
            let others = self.sessions.read().len() as u64;
            let delay = if others == 0 {
                Duration::ZERO
            } else {
                Duration::from_millis(5_000 * others + fastrand::u64(0..3_000))
            };
            let _g = self.rt.enter();
            session.start_after(token, delay);
        }
        self.sessions.write().push(session);
        if self.active.read().is_none() {
            self.set_active(user);
        }
        Ok(())
    }

    /// Open an account from its cache alone, with no token and no connection (dev checks only).
    /// The session has no token and no gateway; the FFI also switches REST off for the process.
    pub fn open_cached_account(self: &Arc<Self>, user: Id) -> Result<()> {
        if self.session(user).is_some() {
            return Ok(());
        }
        let cache = self.cache_for(user)?;
        let rest = self.new_rest(None)?;
        let session = self.make_session(user, rest, cache)?;
        session.start_offline();
        self.sessions.write().push(session);
        if self.active.read().is_none() {
            self.set_active(user);
        }
        Ok(())
    }

    fn make_session(self: &Arc<Self>, user: Id, rest: Arc<Rest>, cache: Arc<Cache>) -> Result<Arc<Session>> {
        let weak = Arc::downgrade(self);
        let emit = Arc::new(move |ev: SessionEvent| {
            let Some(engine) = weak.upgrade() else { return };
            if let SessionEvent::Deltas(ds) = &ev {
                engine.collect_inbox(user, ds);
            }
            engine.sink.session_event(user, ev);
        });
        let s = Session::new(user, rest, cache, emit)?;
        Ok(s)
    }

    pub fn remove_account(&self, user: Id) {
        let removed: Vec<Arc<Session>> = {
            let mut s = self.sessions.write();
            let (gone, keep): (Vec<_>, Vec<_>) = s.drain(..).partition(|x| x.user_id == user);
            *s = keep;
            gone
        };
        for s in removed {
            s.shutdown();
        }
        let mut active = self.active.write();
        if *active == Some(user) {
            *active = self.sessions.read().first().map(|s| s.user_id);
        }
        drop(active);
        self.apply_power();
    }

    pub fn accounts(&self) -> Vec<Id> {
        self.sessions.read().iter().map(|s| s.user_id).collect()
    }

    pub fn session(&self, user: Id) -> Option<Arc<Session>> {
        self.sessions.read().iter().find(|s| s.user_id == user).cloned()
    }

    /// One call at a time across every signed-in account, as in one official client: a call joined
    /// (or started, or accepted) on `joining` first leaves the call any other account is in. Called
    /// only from those join gestures, so it never acts on its own.
    pub fn leave_other_calls(&self, joining: Id) {
        for s in self.sessions_snapshot() {
            if s.user_id != joining && s.in_call() {
                tracing::debug!(target: "unison_core::voice", left = s.user_id.0, joining = joining.0, "one call at a time: leaving the other account's call");
                s.hang_up();
            }
        }
    }

    pub fn sessions_snapshot(&self) -> Vec<Arc<Session>> {
        self.sessions.read().clone()
    }

    pub fn active(&self) -> Option<Arc<Session>> {
        let id = (*self.active.read())?;
        self.session(id)
    }

    pub fn active_id(&self) -> Option<Id> {
        *self.active.read()
    }

    pub fn set_active(&self, user: Id) {
        *self.active.write() = Some(user);
        self.apply_power();
    }

    // ----- Power -----

    pub fn set_power(&self, p: PowerState) {
        *self.power.write() = p;
        self.apply_power();
    }

    fn apply_power(&self) {
        let p = *self.power.read();
        let active = *self.active.read();
        for s in self.sessions.read().iter() {
            let state = if Some(s.user_id) == active {
                p
            } else if p == PowerState::Background {
                PowerState::Background
            } else {
                PowerState::LowPower
            };
            s.set_power(state);
        }
    }

    pub fn set_languages(&self, languages: Vec<String>, timezone: String) {
        let mut h = self.host.write();
        if !languages.is_empty() {
            h.languages = languages;
        }
        if !timezone.is_empty() {
            h.timezone = timezone;
        }
    }

    // ----- Unified inbox -----

    fn collect_inbox(&self, account: Id, deltas: &[Delta]) {
        let Some(session) = self.session(account) else { return };
        let store = session.store.read();
        let mut inbox = self.inbox.lock();
        for d in deltas {
            let Delta::Mentioned { channel, message } = d else { continue };
            let Some(m) = store.find_message(*channel, *message) else { continue };
            let guild = store.channels.get(channel).and_then(|c| c.guild_id);
            inbox.push_front(InboxEntry {
                account,
                guild,
                channel: *channel,
                message: *message,
                author: store.display_name(guild, m.author.id),
                // Raw on purpose (mentions still unresolved): the FFI resolves names and shortens it.
                preview: m.content.chars().take(600).collect(),
            });
        }
        inbox.truncate(INBOX_CAPACITY);
    }

    pub fn inbox(&self) -> Vec<InboxEntry> {
        self.inbox.lock().iter().cloned().collect()
    }

    pub fn clear_inbox(&self) {
        self.inbox.lock().clear();
    }

    pub fn shutdown(&self) {
        for s in self.sessions.read().iter() {
            s.shutdown();
        }
    }
}

/// Core threads run at utility QoS on macOS so the scheduler prefers efficiency cores.
fn lower_thread_qos() {
    #[cfg(target_vendor = "apple")]
    // SAFETY: sets the calling thread's QoS class; no pointers involved.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;

    struct NoSink;
    impl EngineSink for NoSink {
        fn session_event(&self, _: Id, _: SessionEvent) {}
        fn login_event(&self, _: RemoteAuthEvent) {}
        fn account_added(&self, _: User, _: String) {}
    }

    /// Dev checks open an account from its cache alone: a session exists and is active, with no
    /// token (so no request could be authorised) and no gateway.
    #[test]
    fn a_cached_account_opens_offline() {
        let dir = std::env::temp_dir().join(format!("unison-offline-{}", std::process::id()));
        let engine = Engine::new(dir.clone(), HostInfo::detect(), Arc::new(NoSink), None, Policy::default()).unwrap();
        let user: Id = crate::snowflake::Snowflake(42);
        engine.open_cached_account(user).unwrap();
        assert!(engine.session(user).is_some());
        assert_eq!(engine.active_id(), Some(user));
        engine.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}
