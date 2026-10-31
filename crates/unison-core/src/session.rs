//! One signed-in account: gateway + REST + store + cache, and the commands the UI issues.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::cache::{Cache, SearchHit};
use crate::fidelity::{AppState, ClientIdentity};
use crate::forum::{self, ForumQuery};
use crate::gateway::{self, GatewayConfig, GatewayEvent, GatewayHandle};
use crate::intent::UserIntent;
use crate::model::{Channel, Id, Message};
use crate::power::{Batcher, PowerState};
use crate::rest::{LocalFile, NewForumPost, OutgoingMessage, Rest};
use crate::snowflake::Snowflake;
use crate::store::{Delta, Store, perm};
use crate::subscriptions::{DEFAULT_RANGES, Subscriptions};
use crate::voice::connection::{self as voice, Status as VoiceStatus, VoiceEvent, VoiceHandle};
use crate::transport::Method;
use crate::{Error, Result};

mod commands_ext;
pub mod streams;
mod nav;

/// What a session reports upward. The engine tags it with the account id.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Deltas(Vec<Delta>),
    Connection(ConnectionState),
    /// Token rejected (4004 or 401): the account must sign in again.
    LoggedOut,
    /// A write needs a captcha; the UI shows it and retries.
    Captcha { sitekey: Option<String>, rqdata: Option<String> },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Offline,
}

mod suggest_ext;

/// How long the rail layout waits for further edits before it is written.
const FOLDER_WRITE_DELAY: Duration = Duration::from_millis(1200);

pub type Emit = Arc<dyn Fn(SessionEvent) + Send + Sync>;

/// Opens closer together than this count as one burst (see `open_channel`).
const OPEN_BURST: Duration = Duration::from_millis(400);
/// A channel already showing its latest page is not fetched again for this long.
const REFETCH_MIN: Duration = Duration::from_secs(30);

pub struct Session {
    pub user_id: Id,
    pub rest: Arc<Rest>,
    pub store: Arc<RwLock<Store>>,
    pub cache: Arc<Cache>,
    identity: Arc<ClientIdentity>,
    gateway: Mutex<Option<GatewayHandle>>,
    emit: Emit,
    power_tx: watch::Sender<PowerState>,
    selected: Mutex<Option<(Option<Id>, Id)>>,
    /// When a channel was last opened, so a burst of switches fetches only where it stops.
    last_open: Mutex<Instant>,
    /// Reads already sent or answered (ARCHITECTURE rule 10).
    pub reads: crate::dedupe::Dedupe,
    profiles: Mutex<std::collections::HashMap<(Id, Option<Id>), (Instant, serde_json::Value)>>,
    pub nav: Mutex<crate::navstate::NavState>,
    /// Generation of the newest rail edit; only the newest one's delayed write is sent.
    folder_gen: std::sync::atomic::AtomicU64,
    dirty: Arc<Mutex<Vec<Id>>>,
    /// Signals the persist loop that gateway activity may have changed the store (no timer runs while idle).
    activity: tokio::sync::Notify,
    subs: Mutex<Subscriptions>,
    voice: Mutex<VoiceCtl>,
    /// Cancel switches for sends that are uploading, by nonce.
    cancels: Mutex<std::collections::HashMap<Snowflake, Arc<std::sync::atomic::AtomicBool>>>,
    /// What a send needs to be tried again or put back, until it goes through or is dismissed.
    retained: Mutex<std::collections::HashMap<Snowflake, Retained>>,
}

/// A message kept while it is being sent or after it failed.
#[derive(Clone)]
pub struct Retained {
    pub channel: Id,
    pub content: String,
    pub reply_to: Option<Id>,
    pub mention_reply: bool,
    pub files: Vec<LocalFile>,
}

/// The join in progress or the call we're in: what op 4 asked for and what the server answered.
#[derive(Default)]
struct VoiceCtl {
    guild: Option<Id>,
    channel: Option<Id>,
    session_id: Option<String>,
    server: Option<(String, String)>,
    handle: Option<VoiceHandle>,
    mute: bool,
    deaf: bool,
    /// A DM call we started: ring the other side once the server has created the call.
    ring: Option<(Id, UserIntent)>,
    /// Go Live streams by key: ours and the ones we watch.
    streams: std::collections::HashMap<String, streams::StreamCtl>,
    /// Where decoded-side video goes (set by the app).
    hooks: Option<streams::VideoHooks>,
}

impl Session {
    pub fn new(user_id: Id, rest: Arc<Rest>, cache: Arc<Cache>, emit: Emit) -> Result<Arc<Self>> {
        let identity = rest.identity.clone();
        let (power_tx, _) = watch::channel(PowerState::Active);
        let nav_cache = cache.clone();
        Ok(Arc::new(Self {
            user_id,
            rest,
            store: Arc::new(RwLock::new(Store::default())),
            cache,
            identity,
            gateway: Mutex::new(None),
            emit,
            power_tx,
            selected: Mutex::new(None),
            last_open: Mutex::new(Instant::now() - OPEN_BURST),
            reads: Default::default(),
            profiles: Default::default(),
            folder_gen: std::sync::atomic::AtomicU64::new(0),
            nav: Mutex::new(nav_cache.get("nav").ok().flatten().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()),
            dirty: Arc::new(Mutex::new(Vec::new())),
            activity: tokio::sync::Notify::new(),
            subs: Mutex::new(Subscriptions::default()),
            voice: Mutex::new(VoiceCtl::default()),
            cancels: Mutex::new(std::collections::HashMap::new()),
            retained: Mutex::new(std::collections::HashMap::new()),
        }))
    }

    /// Record a visit to a channel (persisted so the next launch reopens it).
    pub fn nav_visit(&self, guild: Option<Id>, channel: Id, from_history: bool) {
        let json = {
            let mut n = self.nav.lock();
            n.visit(crate::navstate::Stop { guild, channel }, from_history);
            serde_json::to_vec(&*n).ok()
        };
        if let Some(j) = json {
            let _ = self.cache.put("nav", &j);
        }
    }

    /// Paint from cache immediately, then connect.
    pub fn start(self: &Arc<Self>, token: String) {
        self.start_after(token, Duration::ZERO);
    }

    /// `start`, connecting to the gateway only after `delay`. The cache paints at once.
    pub fn start_after(self: &Arc<Self>, token: String, delay: Duration) {
        self.rest.set_token(Some(token.clone()));
        if let Ok(Some(cached)) = self.cache.load_store() {
            *self.store.write() = cached;
            (self.emit)(SessionEvent::Deltas(vec![Delta::Ready, Delta::GuildsChanged, Delta::PrivateChannelsChanged]));
        }
        self.sync_locale();
        let derived_qos = self.cache.get_string("qos_token").ok().flatten();
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = gateway::spawn(
            GatewayConfig { transport: self.rest.transport.clone(), identity: self.identity.clone(), token, derived_qos, start_delay: delay },
            tx,
        );
        *self.gateway.lock() = Some(handle);
        let me = self.clone();
        tokio::spawn(async move { me.pump(rx).await });
        let me = self.clone();
        tokio::spawn(async move { me.persist_loop().await });
    }

    /// The channel the person has open (set when it is opened).
    pub fn selected_channel(&self) -> Option<Id> {
        self.selected.lock().map(|(_, c)| c)
    }

    /// `X-Discord-Locale` is the account's own language setting (the host's language only until the
    /// settings are known), as in the official client.
    fn sync_locale(&self) {
        if let Some(l) = self.store.read().prefs.locale.clone().filter(|l| !l.is_empty()) {
            *self.identity.locale.lock() = l;
        }
    }

    /// Paint from the cache only: no token, no gateway, no writes back. For layout and motion
    /// checks that must not touch Discord (docs/ARCHITECTURE.md §5b).
    pub fn start_offline(self: &Arc<Self>) {
        if let Ok(Some(cached)) = self.cache.load_store() {
            *self.store.write() = cached;
            (self.emit)(SessionEvent::Deltas(vec![Delta::Ready, Delta::GuildsChanged, Delta::PrivateChannelsChanged]));
        }
    }

    pub fn shutdown(&self) {
        self.end_voice_locally();
        if let Some(g) = self.gateway.lock().take() {
            g.shutdown();
        }
        let _ = self.flush_cache();
    }

    pub fn set_power(&self, p: PowerState) {
        let _ = self.power_tx.send(p);
        self.identity.set_app_state(match p {
            PowerState::Active => AppState::Active,
            PowerState::Visible | PowerState::LowPower => AppState::Inactive,
            PowerState::Background => AppState::Background,
        });
    }

    /// Apply gateway events and deliver coalesced deltas at the cadence the power state allows.
    async fn pump(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<GatewayEvent>) {
        let mut power = self.power_tx.subscribe();
        let mut batch: Batcher<Delta> = Batcher::default();
        loop {
            // No armed flush means no timer: sleep an hour at a time (never wakes in practice).
            let deadline = batch.deadline().map(tokio::time::Instant::from_std).unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
            tokio::select! {
                ev = rx.recv() => {
                    let Some(ev) = ev else { break };
                    match ev {
                        GatewayEvent::Connecting => (self.emit)(SessionEvent::Connection(ConnectionState::Connecting)),
                        GatewayEvent::Closed { code, fatal, .. } => {
                            if fatal && code == 4004 {
                                (self.emit)(SessionEvent::LoggedOut);
                                return;
                            }
                            (self.emit)(SessionEvent::Connection(ConnectionState::Offline));
                        }
                        GatewayEvent::Dispatch { name, data } => {
                            tracing::debug!(%name, bytes = data.len(), "dispatch");
                            if name == "READY" {
                                (self.emit)(SessionEvent::Connection(ConnectionState::Connected));
                                self.capture_qos(&data);
                            } else if name == "RESUMED" {
                                (self.emit)(SessionEvent::Connection(ConnectionState::Connected));
                            }
                            if name == "VOICE_STATE_UPDATE" || name == "VOICE_SERVER_UPDATE" || name == "CALL_CREATE" {
                                self.on_voice_dispatch(&name, &data);
                            }
                            if name.starts_with("STREAM_") {
                                self.on_stream_dispatch(&name, &data);
                            }
                            let deltas = match self.store.write().apply(&name, &data) {
                                Ok(d) => d,
                                Err(e) => {
                                    tracing::warn!("{name}: {e}");
                                    continue;
                                }
                            };
                            if name == "READY" || name == "USER_SETTINGS_PROTO_UPDATE" {
                                self.sync_locale();
                            }
                            self.note_dirty(&deltas);
                            self.activity.notify_one();
                            if name == "READY" {
                                // A fresh session has no subscriptions: re-request the open view.
                                self.subs.lock().reset();
                                // ...and no voice state: the server dropped any call we were in.
                                self.end_voice_locally();
                                if let Some((g, c)) = *self.selected.lock() {
                                    self.subscribe(g, c);
                                }
                                let _ = self.flush_cache();
                            }
                            let p = *power.borrow();
                            batch.push(Instant::now(), p, deltas, |d| matches!(d, Delta::Typing { .. } | Delta::MemberList { .. }));
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline), if batch.deadline().is_some() => {
                    if let Some(out) = batch.take_due(Instant::now()) {
                        (self.emit)(SessionEvent::Deltas(out));
                    }
                }
                changed = power.changed() => {
                    if changed.is_err() { break }
                    // Hidden cancels the timer; coming back on screen delivers everything held.
                    let p = *power.borrow();
                    if let Some(out) = batch.on_power(p) {
                        (self.emit)(SessionEvent::Deltas(out));
                    }
                }
            }
        }
    }

    fn capture_qos(&self, data: &[u8]) {
        #[derive(serde::Deserialize)]
        struct R {
            qos_token: Option<String>,
        }
        if let Ok(R { qos_token: Some(q) }) = unison_etf::from_term(unison_etf::RawTerm(data)) {
            let _ = self.cache.put("qos_token", q.as_bytes());
        }
    }

    fn note_dirty(&self, deltas: &[Delta]) {
        let mut d = self.dirty.lock();
        for x in deltas {
            if let Delta::MessageInserted { channel, .. } | Delta::MessageUpdated { channel, .. } = x {
                if !d.contains(channel) {
                    d.push(*channel);
                }
            }
        }
    }

    async fn persist_loop(self: Arc<Self>) {
        let mut power = self.power_tx.subscribe();
        let mut unflushed = false;
        loop {
            if !unflushed {
                // Idle: no timer at all until the gateway delivers something.
                tokio::select! {
                    _ = self.activity.notified() => unflushed = true,
                    r = power.changed() => if r.is_err() { return },
                }
                continue;
            }
            let every = power.borrow().cache_flush();
            tokio::select! {
                _ = tokio::time::sleep(every) => {}
                r = power.changed() => if r.is_err() { return },
            }
            if self.gateway.lock().is_none() {
                return;
            }
            let _ = self.flush_cache();
            unflushed = false;
        }
    }

    fn flush_cache(&self) -> Result<()> {
        let channels: Vec<Id> = std::mem::take(&mut *self.dirty.lock());
        for c in channels {
            let msgs: Vec<Message> = self.store.read().history.get(&c).map(|h| h.messages.iter().cloned().collect()).unwrap_or_default();
            self.cache.save_messages(c, &msgs)?;
        }
        self.cache.save_store(&self.store.read())?;
        self.cache.put_if_changed("cookies", self.rest.transport.saved_cookies().as_bytes())?;
        Ok(())
    }

    /// Op 37 for what is on screen: the guild's features, plus the channel's member-list range
    /// for channels that have a member list (forums don't).
    fn subscribe(&self, guild: Option<Id>, channel: Id) {
        let Some(g) = guild else { return };
        let with_members = self.store.read().channels.get(&channel).is_some_and(|c| !c.is_forum_like());
        let payloads = {
            let mut subs = self.subs.lock();
            subs.guild(g);
            if with_members {
                subs.channel(g, channel, DEFAULT_RANGES);
            }
            subs.flush()
        };
        if let Some(gw) = self.gateway.lock().as_ref() {
            for d in payloads {
                gw.send(gateway::op::GUILD_SUBSCRIPTIONS_BULK, d);
            }
        }
    }

    // ----- Commands from the UI -----

    /// Open a channel: cached messages at once, then the latest page, then subscribe and ack.
    /// `intent` is `None` when the app restores a view by itself (launch); then nothing is acked.
    /// Fill an empty channel from the on-disk cache, synchronously, so the first frame after a
    /// switch already has rows instead of waiting for `open_channel`'s task to run.
    pub fn prime_history(&self, channel: Id) {
        if self.store.read().history.contains_key(&channel) {
            return;
        }
        if let Ok(cached) = self.cache.load_messages(channel, 50) {
            if !cached.is_empty() {
                self.store.write().set_history_latest(channel, cached, true);
            }
        }
    }

    pub async fn open_channel(self: &Arc<Self>, guild: Option<Id>, channel: Id, intent: Option<UserIntent>) -> Result<()> {
        if let Some((_, prev)) = self.selected.lock().replace((guild, channel)) {
            if prev != channel {
                self.store.write().trim_history(prev, 50);
            }
        }
        self.rest.set_location(guild, Some(channel));
        self.identity.touch();
        let has_history = self.store.read().history.get(&channel).is_some_and(|h| h.at_latest);
        if !has_history {
            if let Ok(cached) = self.cache.load_messages(channel, 50) {
                if !cached.is_empty() {
                    self.store.write().set_history_latest(channel, cached, true);
                    (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }]));
                }
            }
        }
        self.subscribe(guild, channel);
        // A person can't read channels faster than a few per second; a script or a held arrow key can.
        // The first open after a pause fetches at once; an open that follows another within
        // `OPEN_BURST` waits it out and fetches only if it is still the channel on screen. The
        // cached messages above are already showing, so nothing waits visibly.
        let burst = {
            let mut last = self.last_open.lock();
            let burst = last.elapsed() < OPEN_BURST;
            *last = Instant::now();
            burst
        };
        if burst {
            tokio::time::sleep(OPEN_BURST).await;
            if *self.selected.lock() != Some((guild, channel)) {
                return Ok(());
            }
        }
        // Already showing the latest page, fed by the gateway, and nothing unread to acknowledge:
        // opening it again within `REFETCH_MIN` asks for nothing new.
        let key = format!("msgs:{channel}");
        let unread_now = {
            let s = self.store.read();
            s.channels.get(&channel).is_some_and(|c| s.is_unread(c)) || s.mention_count(channel) > 0
        };
        let ttl = if has_history && !unread_now { REFETCH_MIN } else { Duration::ZERO };
        if !self.reads.begin(&key, ttl) {
            return Ok(());
        }
        let fresh = match self.rest.messages(channel, None, 50).await {
            Ok(f) => f,
            Err(e) => {
                self.reads.finish(&key, false);
                return Err(e);
            }
        };
        self.reads.finish(&key, true);
        let full = fresh.len() == 50;
        let newest = fresh.first().map(|m| m.id);
        self.store.write().set_history_latest(channel, fresh, full);
        self.dirty.lock().push(channel);
        (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }]));

        let unread = {
            let s = self.store.read();
            s.channels.get(&channel).is_some_and(|c| s.is_unread(c)) || s.mention_count(channel) > 0
        };
        if let (true, Some(last), Some(intent)) = (unread, newest, intent) {
            self.rest.ack(intent, channel, last).await?;
        }
        Ok(())
    }

    // ----- Forums -----

    /// Open a forum or media channel: subscribe the guild (active posts arrive as
    /// THREAD_LIST_SYNC), then load the first archived page for the chosen view.
    pub async fn open_forum(self: &Arc<Self>, guild: Id, forum: Id, query: ForumQuery) -> Result<()> {
        if let Some((_, prev)) = self.selected.lock().replace((Some(guild), forum)) {
            if prev != forum {
                self.store.write().trim_history(prev, 50);
            }
        }
        self.rest.set_location(Some(guild), Some(forum));
        self.identity.touch();
        self.subscribe(Some(guild), forum);
        (self.emit)(SessionEvent::Deltas(vec![Delta::ForumChanged { channel: forum }]));
        self.load_forum_page(forum, query, false).await
    }

    /// The next archived page (`more`), or the first one if this view hasn't loaded yet.
    pub async fn load_forum_page(&self, forum: Id, query: ForumQuery, more: bool) -> Result<()> {
        let offset = {
            let mut s = self.store.write();
            let Some(c) = s.channels.get(&forum) else { return Ok(()) };
            if s.permissions(c) & perm::READ_MESSAGE_HISTORY == 0 {
                return Ok(());
            }
            let list = s.forum_list_mut(forum, &query);
            if list.loading || (more && !list.has_more) || (!more && list.loaded) {
                None
            } else {
                list.loading = true;
                Some(if more { list.next_offset } else { 0 })
            }
        };
        let Some(offset) = offset else {
            if !more {
                self.request_first_previews(forum, &query).await;
            }
            return Ok(());
        };
        match self.rest.thread_search(forum, &query, offset).await {
            Ok(page) => {
                let deltas = self.store.write().absorb_thread_search(forum, &query, offset, page);
                (self.emit)(SessionEvent::Deltas(deltas));
                if offset == 0 {
                    self.request_first_previews(forum, &query).await;
                }
                Ok(())
            }
            Err(e) => {
                self.store.write().forum_list_mut(forum, &query).loading = false;
                (self.emit)(SessionEvent::Deltas(vec![Delta::ForumChanged { channel: forum }]));
                Err(e)
            }
        }
    }

    /// Like the official forum list on open: previews for the first ten posts.
    async fn request_first_previews(&self, forum: Id, query: &ForumQuery) {
        let top: Vec<Id> = self.store.read().forum_posts(forum, query).into_iter().take(forum::PREVIEW_BATCH).collect();
        let _ = self.load_previews(forum, top).await;
    }

    /// First messages for posts that scrolled into view, ten per `post-data` request.
    pub async fn load_previews(&self, forum: Id, posts: Vec<Id>) -> Result<()> {
        let missing = {
            let mut s = self.store.write();
            let missing = s.previews.missing(&posts);
            s.previews.requested.extend(missing.iter().copied());
            missing
        };
        for batch in missing.chunks(forum::PREVIEW_BATCH) {
            match self.rest.post_data(forum, batch).await {
                Ok(data) => {
                    let deltas = self.store.write().absorb_post_data(forum, batch, data);
                    (self.emit)(SessionEvent::Deltas(deltas));
                }
                Err(e) => {
                    let mut s = self.store.write();
                    for id in batch {
                        s.previews.requested.remove(id);
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Create a forum post; returns the new post's id so the shell can open it.
    pub async fn create_forum_post(
        &self,
        intent: UserIntent,
        forum: Id,
        title: String,
        content: String,
        tags: Vec<Id>,
    ) -> Result<Id> {
        let auto_archive_duration =
            self.store.read().channels.get(&forum).and_then(|c| c.default_auto_archive_duration).unwrap_or(4320);
        let post = NewForumPost { title, content, tags, auto_archive_duration };
        let v = match self.rest.create_forum_post(intent, forum, &post).await {
            Ok(v) => v,
            Err(e) => {
                if let Error::Captcha(c) = &e {
                    (self.emit)(SessionEvent::Captcha { sitekey: c.captcha_sitekey.clone(), rqdata: c.captcha_rqdata.clone() });
                }
                return Err(e);
            }
        };
        let first: Option<Message> = v.get("message").cloned().and_then(|m| serde_json::from_value(m).ok());
        let mut thread: Channel = serde_json::from_value(v)?;
        thread.guild_id = thread.guild_id.or_else(|| self.store.read().channels.get(&forum).and_then(|c| c.guild_id));
        let id = thread.id;
        let deltas = self.store.write().insert_created_post(thread, first);
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(id)
    }

    // ----- Voice -----

    /// Join a guild voice channel (op 4). The connection starts once the server answers with our
    /// voice session and a voice server.
    pub fn join_voice(self: &Arc<Self>, intent: UserIntent, guild: Id, channel: Id) -> Result<()> {
        self.join_call(intent, Some(guild), channel, false)
    }

    /// Start a call in a 1:1 or group DM: join its voice channel (op 4 with no guild), then ring
    /// the other side once the server answers with `CALL_CREATE`, as the official client does.
    pub fn start_dm_call(self: &Arc<Self>, intent: UserIntent, channel: Id) -> Result<()> {
        if !self.store.read().channels.get(&channel).is_some_and(|c| c.is_private()) {
            return Err(Error::Other("not a direct message".into()));
        }
        self.join_call(intent, None, channel, true)
    }

    /// Join a call already going in a DM (the Join button, or Accept on a ring). Never rings.
    pub fn join_dm_call(self: &Arc<Self>, intent: UserIntent, channel: Id) -> Result<()> {
        if !self.store.read().channels.get(&channel).is_some_and(|c| c.is_private()) {
            return Err(Error::Other("not a direct message".into()));
        }
        self.join_call(intent, None, channel, false)
    }

    /// Decline an incoming ring: `POST /channels/{id}/call/stop-ringing` for ourselves. Does not
    /// join, and does not end the call for anyone else.
    pub async fn decline_call(&self, intent: UserIntent, channel: Id) -> Result<()> {
        self.rest.stop_ringing(intent, channel, vec![self.user_id]).await?;
        let deltas = self.store.write().stop_ringing_local(channel, self.user_id);
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(())
    }

    fn join_call(self: &Arc<Self>, intent: UserIntent, guild: Option<Id>, channel: Id, ring: bool) -> Result<()> {
        let (intent, ring_intent) = intent.split();
        intent.redeem()?;
        let (mute, deaf) = {
            let mut v = self.voice.lock();
            if let Some(h) = v.handle.take() {
                h.shutdown();
            }
            v.guild = guild;
            v.channel = Some(channel);
            v.session_id = None;
            v.server = None;
            v.ring = ring.then_some((channel, ring_intent));
            (v.mute, v.deaf)
        };
        if let Some(gw) = self.gateway.lock().as_ref() {
            gw.voice_state(guild, Some(channel), mute, deaf);
        }
        let mut s = self.store.write();
        s.voice = crate::store::VoiceCall {
            guild,
            channel: Some(channel),
            status: Some(VoiceStatus::Connecting),
            self_mute: mute,
            self_deaf: deaf,
            ..Default::default()
        };
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
        Ok(())
    }

    pub fn leave_voice(&self, intent: UserIntent) -> Result<()> {
        intent.redeem()?;
        let guild = {
            let mut v = self.voice.lock();
            if let Some(h) = v.handle.take() {
                h.shutdown();
            }
            let g = v.guild.take();
            v.streams.clear();
            v.ring = None;
            v.channel = None;
            v.session_id = None;
            v.server = None;
            g
        };
        if let Some(gw) = self.gateway.lock().as_ref() {
            gw.voice_state(guild, None, false, false);
        }
        let mut s = self.store.write();
        let (m, d) = (s.voice.self_mute, s.voice.self_deaf);
        s.voice = crate::store::VoiceCall { self_mute: m, self_deaf: d, ..Default::default() };
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
        Ok(())
    }

    /// Mute or deafen. In a call this updates op 4 like the official client; otherwise it's
    /// remembered for the next join.
    pub fn set_voice_flags(&self, intent: UserIntent, mute: bool, deaf: bool) -> Result<()> {
        intent.redeem()?;
        let (guild, channel) = {
            let mut v = self.voice.lock();
            v.mute = mute;
            v.deaf = deaf;
            if let Some(h) = &v.handle {
                h.set_muted(mute || deaf);
                h.set_deaf(deaf);
            }
            (v.guild, v.channel)
        };
        if channel.is_some() {
            if let Some(gw) = self.gateway.lock().as_ref() {
                gw.voice_state(guild, channel, mute, deaf);
            }
        }
        let mut s = self.store.write();
        s.voice.self_mute = mute;
        s.voice.self_deaf = deaf;
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
        Ok(())
    }

    /// Drop the call without telling the server (it already knows, or we're going away).
    fn end_voice_locally(&self) {
        let had = {
            let mut v = self.voice.lock();
            let had = v.handle.take().map(|h| h.shutdown()).is_some() || v.channel.is_some();
            v.guild = None;
            v.streams.clear();
            v.ring = None;
            v.channel = None;
            v.session_id = None;
            v.server = None;
            had
        };
        if had {
            let mut s = self.store.write();
            let (m, d) = (s.voice.self_mute, s.voice.self_deaf);
            s.voice = crate::store::VoiceCall { self_mute: m, self_deaf: d, ..Default::default() };
            drop(s);
            (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
        }
    }

    fn on_voice_dispatch(self: &Arc<Self>, name: &str, data: &[u8]) {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Server {
            token: String,
            guild_id: Option<Id>,
            endpoint: Option<String>,
        }
        let me = self.user_id;
        let mut v = self.voice.lock();
        match name {
            "CALL_CREATE" => {
                #[derive(serde::Deserialize, Default)]
                #[serde(default)]
                struct Call {
                    channel_id: Option<Id>,
                }
                let Ok(c) = unison_etf::from_term::<Call>(unison_etf::RawTerm(data)) else { return };
                if let (Some(ch), Some((want, _))) = (c.channel_id, v.ring.as_ref()) {
                    if ch == *want {
                        let (_, intent) = v.ring.take().expect("checked");
                        drop(v);
                        let rest = self.rest.clone();
                        let me = self.clone();
                        tokio::spawn(async move {
                            if let Err(e) = rest.ring_call(intent, ch).await {
                                tracing::warn!("ring: {e}");
                                (me.emit)(SessionEvent::Error(e.user_message()));
                            }
                        });
                    }
                }
                return;
            }
            "VOICE_STATE_UPDATE" => {
                let Ok(st) = unison_etf::from_term::<crate::model::VoiceState>(unison_etf::RawTerm(data)) else { return };
                if st.user_id != me || v.channel.is_none() {
                    return;
                }
                match st.channel_id {
                    // Moved or disconnected by someone else.
                    None => {
                        if let Some(h) = v.handle.take() {
                            h.shutdown();
                        }
                        v.channel = None;
                        drop(v);
                        let mut s = self.store.write();
                        s.voice.channel = None;
                        s.voice.status = Some(VoiceStatus::Disconnected);
                        drop(s);
                        (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
                        return;
                    }
                    Some(c) => v.channel = Some(c),
                }
                v.session_id = Some(st.session_id);
            }
            _ => {
                let Ok(sv) = unison_etf::from_term::<Server>(unison_etf::RawTerm(data)) else { return };
                if sv.guild_id != v.guild {
                    return;
                }
                // A null endpoint means the server is being reassigned; another update follows.
                let Some(endpoint) = sv.endpoint.filter(|e| !e.is_empty()) else { return };
                if let Some(h) = v.handle.take() {
                    h.shutdown();
                }
                v.server = Some((sv.token, endpoint));
            }
        }
        if v.handle.is_some() {
            return;
        }
        let (Some(channel), Some(session_id), Some((token, endpoint))) = (v.channel, v.session_id.clone(), v.server.clone()) else {
            return;
        };
        let bitrate = self.store.read().channels.get(&channel).and_then(|c| c.bitrate).unwrap_or(64_000) as i32;
        let params = voice::Params {
            guild_id: v.guild,
            channel_id: channel,
            user_id: me,
            session_id,
            token,
            endpoint,
            bitrate,
            muted: v.mute || v.deaf,
            deaf: v.deaf,
            role: crate::voice::stream::Role::Call,
            server_id: None,
            video: v.hooks.as_ref().map(|h| h.io("call")),
        };
        let store = self.store.clone();
        let emit = self.emit.clone();
        let on_event: voice::Emit = Arc::new(move |ev| {
            {
                let mut s = store.write();
                match ev {
                    VoiceEvent::Status(st) => s.voice.status = Some(st),
                    VoiceEvent::Encryption { e2ee, privacy_code } => {
                        s.voice.e2ee = e2ee;
                        s.voice.privacy_code = privacy_code;
                    }
                    VoiceEvent::Speaking { user, speaking } => {
                        if speaking {
                            s.voice.speaking.insert(user);
                        } else {
                            s.voice.speaking.remove(&user);
                        }
                    }
                    VoiceEvent::AudioError(e) => s.voice.audio_error = Some(e),
                    VoiceEvent::Ping(ms) => {
                        s.voice.ping_ms = Some(ms);
                        s.voice.pings.push(ms);
                        if s.voice.pings.len() > 10 {
                            s.voice.pings.remove(0);
                        }
                    }
                }
            }
            emit(SessionEvent::Deltas(vec![Delta::Voice]));
        });
        v.handle = Some(voice::spawn(params, self.rest.transport.clone(), self.identity.clone(), on_event));
    }

    pub async fn load_older(&self, channel: Id) -> Result<bool> {
        let oldest = self.store.read().history.get(&channel).and_then(|h| h.messages.front().map(|m| m.id));
        let Some(before) = oldest else { return Ok(false) };
        let page = self.rest.messages(channel, Some(before), 50).await?;
        let full = page.len() == 50;
        self.store.write().prepend_history(channel, page, full);
        (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }]));
        Ok(full)
    }

    pub async fn send(
        &self,
        intent: UserIntent,
        channel: Id,
        content: String,
        reply_to: Option<Id>,
        mention_reply: bool,
        files: Vec<LocalFile>,
    ) -> Result<()> {
        self.send_tracked(intent, channel, Snowflake::nonce_now(), content, reply_to, mention_reply, files).await
    }

    /// Send a message and track it as `nonce`: its upload progress is in `store.sends`, the user
    /// can cancel it, and when it fails the echo is removed and the state stays (failed) until the
    /// shell dismisses it, so the text and files can be given back. Checks the length and the file
    /// sizes first, so Discord is never sent something it is certain to refuse.
    pub async fn send_tracked(
        &self,
        intent: UserIntent,
        channel: Id,
        nonce: Snowflake,
        content: String,
        reply_to: Option<Id>,
        mention_reply: bool,
        files: Vec<LocalFile>,
    ) -> Result<()> {
        self.begin_send(channel, nonce, &content, reply_to, mention_reply, &files);
        self.run_send(intent, channel, nonce, content, reply_to, mention_reply, files).await
    }

    /// Register a send before anything runs, so the shell sees it the moment it has the nonce and
    /// "no longer listed" can only mean "done". Call [`run_send`](Self::run_send) next.
    pub fn begin_send(&self, channel: Id, nonce: Snowflake, content: &str, reply_to: Option<Id>, mention_reply: bool, files: &[LocalFile]) {
        let named: Vec<(String, u64)> = files.iter().map(|f| (f.filename.clone(), f.bytes.len() as u64)).collect();
        let mut state = crate::send_state::SendState::new(nonce, channel, named, std::time::Instant::now());
        state.text = content.to_string();
        self.store.write().sends.push(state);
        self.retained.lock().insert(nonce, Retained { channel, content: content.to_string(), reply_to, mention_reply, files: files.to_vec() });
        self.cancels.lock().insert(nonce, Arc::new(std::sync::atomic::AtomicBool::new(false)));
        (self.emit)(SessionEvent::Deltas(vec![Delta::Sends { channel }]));
    }

    pub async fn run_send(
        &self,
        intent: UserIntent,
        channel: Id,
        nonce: Snowflake,
        content: String,
        reply_to: Option<Id>,
        mention_reply: bool,
        files: Vec<LocalFile>,
    ) -> Result<()> {
        use std::sync::atomic::Ordering;
        let (guild, premium, tier) = {
            let st = self.store.read();
            let guild = st.channels.get(&channel).and_then(|c| c.guild_id);
            let tier = guild.and_then(|g| st.guilds.get(&g)).map(|g| g.inline.premium_tier);
            (guild, st.me.as_ref().and_then(|u| u.premium_type), tier)
        };
        let named: Vec<(String, u64)> = files.iter().map(|f| (f.filename.clone(), f.bytes.len() as u64)).collect();
        let cancel = self.cancels.lock().get(&nonce).cloned().unwrap_or_default();

        let result = async {
            if crate::send_rules::char_len(content.trim()) > crate::send_rules::char_limit(premium) {
                return Err(Error::Refused(format!(
                    "That message is too long: the limit is {} characters.",
                    crate::send_rules::char_limit(premium)
                )));
            }
            crate::send_rules::check_final(&named, premium, tier).map_err(Error::Refused)?;
            self.store.write().push_pending(channel, content.clone(), nonce);
            (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }]));
            let (upload_intent, intent) = intent.split();
            let attachments = if files.is_empty() {
                vec![]
            } else {
                let (store, emit) = (self.store.clone(), self.emit.clone());
                let last_emit = Arc::new(Mutex::new(std::time::Instant::now()));
                let progress: Arc<dyn Fn(usize, u64) + Send + Sync> = Arc::new(move |i, n| {
                    let now = std::time::Instant::now();
                    if let Some(s) = store.write().sends.iter_mut().find(|s| s.nonce == nonce) {
                        s.add(i, n, now);
                    }
                    // About ten updates a second is plenty for a bar.
                    let mut last = last_emit.lock();
                    if now.duration_since(*last) >= std::time::Duration::from_millis(100) {
                        *last = now;
                        emit(SessionEvent::Deltas(vec![Delta::Sends { channel }]));
                    }
                });
                let sent = self.rest.upload_attachments(upload_intent, channel, files, progress, cancel.clone()).await?;
                if let Some(s) = self.store.write().sends.iter_mut().find(|s| s.nonce == nonce) {
                    s.finish_upload();
                }
                (self.emit)(SessionEvent::Deltas(vec![Delta::Sends { channel }]));
                sent
            };
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let draft = OutgoingMessage { content, nonce, guild_id: guild, reply_to, mention_reply, silent: false, attachments };
            self.rest.send_message(intent, channel, &draft).await.map(|_| ())
        }
        .await;

        self.cancels.lock().remove(&nonce);
        match result {
            Ok(()) => {
                self.retained.lock().remove(&nonce);
                self.store.write().sends.retain(|s| s.nonce != nonce);
                (self.emit)(SessionEvent::Deltas(vec![Delta::Sends { channel }]));
                Ok(())
            }
            Err(e) => {
                let note = (!matches!(e, Error::Cancelled)).then(|| e.user_message());
                // A refusal needs an edit, so trying the same thing again would only refuse again.
                let can_retry = !matches!(e, Error::Cancelled | Error::Refused(_));
                {
                    let mut st = self.store.write();
                    st.drop_pending(channel, nonce);
                    if let Some(s) = st.sends.iter_mut().find(|s| s.nonce == nonce) {
                        s.fail(note, can_retry);
                    }
                }
                (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }, Delta::Sends { channel }]));
                if let Error::Captcha(c) = &e {
                    (self.emit)(SessionEvent::Captcha { sitekey: c.captcha_sitekey.clone(), rqdata: c.captcha_rqdata.clone() });
                }
                Err(e)
            }
        }
    }

    /// Stop a send that is still uploading. A send already handed to Discord can't be taken back.
    pub fn cancel_send(&self, nonce: Snowflake) {
        if let Some(flag) = self.cancels.lock().get(&nonce) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Send a failed message again, the same text, files and reply, under the same id.
    pub async fn retry_send(&self, intent: UserIntent, nonce: Snowflake) -> Result<()> {
        let Some(r) = self.retained.lock().get(&nonce).cloned() else { return Ok(()) };
        self.store.write().sends.retain(|s| s.nonce != nonce);
        self.begin_send(r.channel, nonce, &r.content, r.reply_to, r.mention_reply, &r.files);
        self.run_send(intent, r.channel, nonce, r.content, r.reply_to, r.mention_reply, r.files).await
    }

    /// Take a failed or cancelled message back to edit it: it stops being tracked and its text,
    /// files and reply are handed over.
    pub fn take_back_send(&self, nonce: Snowflake) -> Option<Retained> {
        let r = self.retained.lock().remove(&nonce)?;
        self.store.write().sends.retain(|s| s.nonce != nonce);
        (self.emit)(SessionEvent::Deltas(vec![Delta::Sends { channel: r.channel }]));
        Some(r)
    }

    /// Forget a failed send.
    pub fn dismiss_send(&self, nonce: Snowflake) {
        self.retained.lock().remove(&nonce);
        let mut st = self.store.write();
        let channel = st.sends.iter().find(|s| s.nonce == nonce).map(|s| s.channel);
        st.sends.retain(|s| s.nonce != nonce);
        drop(st);
        if let Some(channel) = channel {
            (self.emit)(SessionEvent::Deltas(vec![Delta::Sends { channel }]));
        }
    }

    /// Forward `message` to `dest`; a comment goes out as a plain message right after it (one
    /// gesture, two requests). The caller invokes this once per destination.
    /// Forward one message to several destinations, in turn and no more than
    /// `MAX_FORWARD_TARGETS`, with `FORWARD_GAP` between sends. Stops at the first failure.
    pub async fn forward_all(&self, targets: Vec<(Id, UserIntent)>, from: Id, message: Id, comment: String) -> Result<()> {
        use crate::send_rules::{FORWARD_GAP, MAX_FORWARD_TARGETS};
        for (n, (dest, intent)) in targets.into_iter().take(MAX_FORWARD_TARGETS).enumerate() {
            if n > 0 {
                tokio::time::sleep(FORWARD_GAP).await;
            }
            self.forward(intent, from, message, dest, comment.clone()).await?;
        }
        Ok(())
    }

    pub async fn forward(&self, intent: UserIntent, from: Id, message: Id, dest: Id, comment: String) -> Result<()> {
        let guild = self.store.read().channels.get(&from).and_then(|c| c.guild_id);
        let source = crate::rest::ForwardSource { guild_id: guild, channel_id: from, message_id: message };
        let (first, second) = intent.split();
        self.rest.forward_message(first, dest, source, Snowflake::nonce_now()).await?;
        if !comment.trim().is_empty() {
            self.send(second, dest, comment, None, false, vec![]).await?;
        }
        Ok(())
    }

    /// Mark the open channel read up to its newest message, if it has anything unread.
    pub async fn ack_latest(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let target = {
            let s = self.store.read();
            let unread = s.channels.get(&channel).is_some_and(|c| s.is_unread(c)) || s.mention_count(channel) > 0;
            let newest = s.history.get(&channel).and_then(|h| h.messages.iter().rev().find(|m| m.id.0 != 0).map(|m| m.id));
            newest.filter(|_| unread)
        };
        let Some(message) = target else { return Ok(()) };
        self.rest.ack(intent, channel, message).await?;
        let mut s = self.store.write();
        let rs = s.read_states.entry(channel).or_insert_with(|| crate::model::ReadStateEntry { id: channel, ..Default::default() });
        rs.last_message_id = Some(message);
        rs.mention_count = 0;
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::ReadState { channel }]));
        Ok(())
    }

    pub async fn edit(&self, intent: UserIntent, channel: Id, message: Id, content: String) -> Result<()> {
        let limit = crate::send_rules::char_limit(self.store.read().me.as_ref().and_then(|u| u.premium_type));
        if crate::send_rules::char_len(content.trim()) > limit {
            return Err(Error::Refused(format!("That message is too long: the limit is {limit} characters.")));
        }
        self.rest.edit_message(intent, channel, message, &content).await.map(|_| ())
    }

    pub async fn delete(&self, intent: UserIntent, channel: Id, message: Id) -> Result<()> {
        self.rest.delete_message(intent, channel, message).await?;
        let _ = self.cache.delete_message(channel, message);
        Ok(())
    }

    /// Favourite and frecent emoji, read once per session like the official client's
    /// `loadIfUncached(FRECENCY_AND_FAVORITES_SETTINGS)`; later changes arrive over the gateway.
    pub async fn load_emoji_prefs(&self) -> Result<()> {
        if self.store.read().emoji_prefs.is_some() {
            return Ok(());
        }
        let blob = self.rest.settings_proto(2).await?;
        self.store.write().absorb_frecency_proto(&blob);
        Ok(())
    }

    /// Add (`Some`) or remove a favourite GIF: a partial FrecencyUserSettings PATCH, then the
    /// merged settings the server returns replace the local copy.
    pub async fn set_favorite_gif(&self, intent: UserIntent, url: String, gif: Option<crate::model::settings_proto::FavoriteGif>) -> Result<()> {
        use crate::model::settings_proto as sp;
        let patch = match &gif {
            Some(g) => sp::encode_favorite_gif(g),
            None => sp::encode_remove_favorite_gif(&url),
        };
        if let Some(full) = self.rest.patch_settings_proto(intent, 2, &patch).await? {
            self.store.write().absorb_frecency_proto(&full);
        }
        (self.emit)(SessionEvent::Deltas(vec![Delta::EmojiPrefs]));
        Ok(())
    }

    pub async fn react(&self, intent: UserIntent, channel: Id, message: Id, emoji: String, add: bool) -> Result<()> {
        self.rest.react(intent, channel, message, &emoji, add).await
    }

    /// The official client sends typing at most once per ~8 s per channel; every keystroke would
    /// trip the endpoint's rate limit.
    pub async fn typing(&self, intent: UserIntent, channel: Id) -> Result<()> {
        static LAST: std::sync::LazyLock<Mutex<std::collections::HashMap<(Id, Id), Instant>>> = std::sync::LazyLock::new(Default::default);
        {
            let mut last = LAST.lock();
            let now = Instant::now();
            if last.get(&(self.user_id, channel)).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(8)) {
                return Ok(());
            }
            last.insert((self.user_id, channel), now);
        }
        self.rest.typing(intent, channel).await
    }

    /// Press a button or change a select on a message's components.
    pub async fn component_interact(
        &self,
        intent: UserIntent,
        channel: Id,
        message: Id,
        component_type: u8,
        custom_id: String,
        values: Option<Vec<String>>,
    ) -> Result<()> {
        use crate::interactions::{Target, component_interaction};
        let (guild, flags, app, session) = {
            let s = self.store.read();
            let m = s.find_message(channel, message).ok_or_else(|| Error::Decode("message not loaded".into()))?;
            let app = m.application_id.unwrap_or(m.author.id);
            (
                s.channels.get(&channel).and_then(|c| c.guild_id),
                m.flags,
                app,
                s.gateway_session.clone().unwrap_or_default(),
            )
        };
        let select = values.map(|v| (component_type, v));
        let body = component_interaction(
            Target { guild, channel, application: app },
            &session,
            Snowflake::nonce_now(),
            message,
            flags,
            component_type,
            &custom_id,
            select,
        );
        self.rest.interact(intent, body).await
    }

    /// Answer the open modal (`fields` is `[(custom_id, value)]`), or dismiss it when `None`.
    pub async fn answer_modal(&self, intent: UserIntent, fields: Option<Vec<(String, String)>>) -> Result<()> {
        use crate::interactions::{Target, modal_interaction};
        let (modal, session) = {
            let mut s = self.store.write();
            (s.modal.take(), s.gateway_session.clone().unwrap_or_default())
        };
        let (Some(m), Some(fields)) = (modal, fields) else { return Ok(()) };
        let body = modal_interaction(
            Target { guild: m.guild, channel: m.channel, application: m.application },
            &session,
            Snowflake::nonce_now(),
            &m.id,
            &m.custom_id,
            &fields,
        );
        self.rest.interact(intent, body).await
    }

    /// Vote on (or clear a vote from) a poll; `answer` is the answer clicked.
    pub async fn poll_vote(&self, intent: UserIntent, channel: Id, message: Id, answer: u32) -> Result<()> {
        let ids = {
            let s = self.store.read();
            let m = s.find_message(channel, message).ok_or_else(|| Error::Decode("message not loaded".into()))?;
            let poll = m.poll.as_ref().and_then(crate::interactions::parse_poll).ok_or_else(|| Error::Decode("no poll".into()))?;
            crate::interactions::next_answer_ids(&poll, answer)
        };
        self.rest.poll_vote(intent, channel, message, &ids).await
    }

    pub async fn thread_membership(&self, intent: UserIntent, thread: Id, join: bool) -> Result<()> {
        self.rest.thread_membership(intent, thread, join).await?;
        // The gateway confirms with THREAD_MEMBERS_UPDATE; reflect it now so the button flips.
        let deltas = {
            let mut s = self.store.write();
            let me = s.my_id();
            match (s.channels.get_mut(&thread), me) {
                (Some(c), Some(me)) => {
                    c.member = join.then(|| serde_json::json!({"id": thread.to_string(), "user_id": me.to_string(), "flags": 0}));
                    vec![Delta::ChannelsChanged { guild: c.guild_id }]
                }
                _ => vec![],
            }
        };
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(())
    }

    pub async fn create_thread(&self, intent: UserIntent, channel: Id, message: Option<Id>, name: String) -> Result<Id> {
        use crate::model::channel_type;
        let (kind, archive) = {
            let s = self.store.read();
            let c = s.channels.get(&channel);
            let kind = if c.is_some_and(|c| c.kind == channel_type::GUILD_ANNOUNCEMENT) {
                channel_type::ANNOUNCEMENT_THREAD
            } else {
                channel_type::PUBLIC_THREAD
            };
            (kind, c.and_then(|c| c.default_auto_archive_duration).unwrap_or(1440))
        };
        let t = self.rest.create_thread(intent, channel, message, &name, kind, archive).await?;
        let id = t.id;
        let data = unison_etf::to_vec(&t).map_err(|e| Error::Decode(e.to_string()))?;
        let deltas = self.store.write().apply("THREAD_CREATE", &data[1..])?;
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(id)
    }

    pub async fn pin(&self, intent: UserIntent, channel: Id, message: Id, pinned: bool) -> Result<()> {
        self.rest.pin(intent, channel, message, pinned).await
    }

    pub async fn open_dm(&self, intent: UserIntent, user: Id) -> Result<Id> {
        let c = self.rest.open_dm(intent, user).await?;
        let id = c.id;
        let data = unison_etf::to_vec(&c).map_err(|e| Error::Decode(e.to_string()))?;
        let deltas = self.store.write().apply("CHANNEL_CREATE", &data[1..])?;
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(id)
    }

    /// Pinned messages, newest first (`GET /channels/{id}/messages/pins`).
    pub async fn pins(&self, channel: Id) -> Result<Vec<Message>> {
        #[derive(serde::Deserialize)]
        struct Page {
            #[serde(default)]
            items: Vec<Item>,
        }
        #[derive(serde::Deserialize)]
        struct Item {
            message: Message,
        }
        let v = self.rest.pins(channel).await?;
        // The pins endpoint returns `{items: [{message, pinned_at}]}`; older builds returned a bare array.
        if let Ok(p) = serde_json::from_value::<Page>(v.clone()) {
            if !p.items.is_empty() {
                return Ok(p.items.into_iter().map(|i| i.message).collect());
            }
        }
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    /// Replace the window with the page around `message` (jump-to).
    pub async fn load_around(&self, channel: Id, message: Id) -> Result<()> {
        let page = self.rest.messages_around(channel, message).await?;
        self.store.write().set_history_window(channel, page);
        (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }]));
        Ok(())
    }

    pub fn search_local(&self, query: &str) -> Vec<SearchHit> {
        self.cache.search(query, 50).unwrap_or_default()
    }

    /// Local hits limited to a scope: one channel, every channel of a server, or everything.
    pub fn search_local_scoped(&self, scope: crate::search_syntax::Scope, guild: Option<Id>, channel: Id, query: &str) -> Vec<SearchHit> {
        use crate::search_syntax::Scope;
        if scope == Scope::Everywhere {
            return self.search_local(query);
        }
        // Narrow scopes filter a wider page, so a busy elsewhere doesn't crowd them out.
        let hits = self.cache.search(query, 500).unwrap_or_default();
        let st = self.store.read();
        hits.into_iter()
            .filter(|h| match scope {
                Scope::Channel => h.channel == channel,
                _ => st.channels.get(&h.channel).is_some_and(|c| c.guild_id == guild),
            })
            .take(50)
            .collect()
    }

    pub async fn search_remote(&self, guild: Option<Id>, channel: Id, query: &str) -> Result<Value> {
        match guild {
            Some(g) => self.rest.search_guild(g, query).await,
            None => self.rest.search_channel(channel, query).await,
        }
    }

    /// Set the user's status the way the official client does: presence over the gateway (op 3),
    /// and the saved account setting through the settings-proto PATCH so it survives restarts and
    /// reaches other devices.
    pub async fn set_status(&self, intent: UserIntent, status: &str) -> Result<()> {
        if !crate::store::is_valid_status(status) {
            return Err(Error::Other(format!("unknown status {status:?}")));
        }
        if self.store.read().my_status == status {
            return Ok(());
        }
        if let Some(g) = self.gateway.lock().as_ref() {
            g.update_presence(status, false);
        }
        // Show it at once; SESSIONS_REPLACE / USER_SETTINGS_PROTO_UPDATE confirm it shortly after.
        self.store.write().set_my_status_local(status);
        (self.emit)(SessionEvent::Deltas(vec![Delta::OwnStatus]));
        use base64::Engine as _;
        let blob = crate::model::settings_proto::encode_status(status);
        self.rest.update_settings_proto(intent, 1, &base64::engine::general_purpose::STANDARD.encode(blob)).await
    }

    /// Edit the server folder layout: applied at once, then written to the account's settings so it
    /// follows the user to other devices. `capture`: the PATCH body (see `encode_folders`).
    /// Like the official client the write is debounced: a burst of edits (dragging servers around)
    /// becomes one PATCH of the final layout, which keeps us under Discord's settings rate limit.
    pub async fn edit_folders(self: &Arc<Self>, intent: UserIntent, edit: crate::folders::Edit) -> Result<()> {
        use std::sync::atomic::Ordering;
        let next = {
            let st = self.store.read();
            let new_id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as i64) & 0x7fff_ffff).max(1);
            let current = st.folder_layout();
            let next = crate::folders::apply(&current, &edit, new_id).ok_or_else(|| Error::Other("that server or folder isn't there".into()))?;
            // A drop that changes nothing (a server dropped where it already is) sends nothing.
            if next == current {
                return Ok(());
            }
            next
        };
        {
            let mut st = self.store.write();
            st.folders = next;
            st.folders_unsaved = true;
        }
        (self.emit)(SessionEvent::Deltas(vec![Delta::GuildsChanged]));
        let edit_no = self.folder_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(FOLDER_WRITE_DELAY).await;
            if me.folder_gen.load(Ordering::SeqCst) != edit_no {
                return; // a newer edit owns the write
            }
            let bytes = crate::model::settings_proto::encode_folders(&me.store.read().folders);
            let result = me.rest.patch_settings_proto(intent, 1, &bytes).await;
            if me.folder_gen.load(Ordering::SeqCst) != edit_no {
                return;
            }
            me.store.write().folders_unsaved = false;
            match result {
                Ok(Some(full)) => {
                    me.store.write().absorb_settings_proto(&full);
                }
                Ok(None) => {}
                Err(e) => (me.emit)(SessionEvent::Error(format!("Couldn't save the server order. {}", e.user_message()))),
            }
            (me.emit)(SessionEvent::Deltas(vec![Delta::GuildsChanged]));
        });
        Ok(())
    }

    /// Mark a DM read up to its newest message (context-menu "Mark as Read").
    pub async fn mark_read(&self, intent: UserIntent, channel: Id) -> Result<()> {
        let target = {
            let s = self.store.read();
            let unread = s.channels.get(&channel).is_some_and(|c| s.is_unread(c)) || s.mention_count(channel) > 0;
            s.channels.get(&channel).and_then(|c| c.last_message_id).filter(|_| unread)
        };
        let Some(message) = target else { return Ok(()) };
        self.rest.ack_manual(intent, channel, message).await?;
        let mut s = self.store.write();
        let rs = s.read_states.entry(channel).or_insert_with(|| crate::model::ReadStateEntry { id: channel, ..Default::default() });
        rs.last_message_id = Some(message);
        rs.mention_count = 0;
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::ReadState { channel }]));
        Ok(())
    }

    /// DM context menu "Mute" / "Unmute".
    pub async fn set_dm_muted(&self, intent: UserIntent, channel: Id, muted: bool) -> Result<()> {
        if !self.store.read().channels.get(&channel).is_some_and(|c| c.is_private()) {
            return Err(Error::Other("not a direct message".into()));
        }
        self.rest.set_dm_muted(intent, channel, muted).await?;
        let deltas = self.store.write().set_dm_muted(channel, muted);
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(())
    }

    /// Close a 1:1 DM or leave a group DM, then drop it from the list. Refuses anything that
    /// isn't a private channel.
    pub async fn close_dm(&self, intent: UserIntent, channel: Id) -> Result<()> {
        if !self.store.read().channels.get(&channel).is_some_and(|c| c.is_private()) {
            return Err(Error::Other("not a direct message".into()));
        }
        self.rest.close_private_channel(intent, channel).await?;
        let deltas = self.store.write().remove_private_channel(channel);
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(())
    }

    fn require_guild_perm(&self, guild: Id, flag: u64) -> Result<()> {
        if self.store.read().guild_permissions(guild) & flag == 0 {
            return Err(Error::Other("missing permission".into()));
        }
        Ok(())
    }

    /// Kick, ban or time out a member, checked against the role hierarchy first.
    pub async fn moderate_member(&self, intent: UserIntent, guild: Id, user: Id, action: crate::moderation::ModAction, reason: &str) -> Result<()> {
        use crate::moderation::ModAction as A;
        let rights = self.store.read().mod_rights(guild, user);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        match action {
            A::Kick if rights.kick => self.rest.kick_member(intent, guild, user, reason).await,
            A::Ban { delete_seconds } if rights.ban => self.rest.ban_member(intent, guild, user, delete_seconds, reason).await,
            A::Timeout { secs } if rights.timeout => {
                self.rest.timeout_member(intent, guild, user, crate::moderation::timeout_until(now, secs), reason).await
            }
            _ => Err(Error::Other("you can't do that to this member".into())),
        }
    }

    pub async fn set_nickname(&self, intent: UserIntent, guild: Id, user: Id, nick: &str) -> Result<()> {
        let me = self.store.read().my_id();
        if !self.store.read().mod_rights(guild, user).nickname {
            return Err(Error::Other("you can't change this nickname".into()));
        }
        self.rest.set_nickname(intent, guild, (Some(user) != me).then_some(user), nick).await
    }

    /// Show a message's long code blocks in full (until the app restarts).
    pub fn expand_code(&self, channel: Id, message: Id) {
        self.store.write().expanded_code.insert(message);
        (self.emit)(SessionEvent::Deltas(vec![Delta::MessagesReset { channel }]));
    }

    /// Give a friend a nickname only you see (empty clears it). Shown at once; put back if Discord refuses.
    pub async fn set_friend_nickname(&self, intent: UserIntent, user: Id, nickname: &str) -> Result<()> {
        let nick: Option<String> = Some(nickname.trim().chars().take(32).collect::<String>()).filter(|n| !n.is_empty());
        let previous = self.store.read().friend_nicks.get(&user).cloned();
        if !self.store.read().relationships.iter().any(|r| r.kind == 1 && r.user_id.or(r.user.as_ref().map(|u| u.id)).unwrap_or(r.id) == user) {
            return Err(Error::Other("only friends can have a nickname".into()));
        }
        let deltas = self.store.write().set_friend_nick_local(user, nick.clone());
        (self.emit)(SessionEvent::Deltas(deltas));
        let r = self.rest.set_friend_nickname(intent, user, nick.as_deref()).await;
        if r.is_err() {
            let deltas = self.store.write().set_friend_nick_local(user, previous);
            (self.emit)(SessionEvent::Deltas(deltas));
        }
        r
    }

    /// Open someone's profile data: the profile and, alongside it, your note on them.
    pub async fn load_note(&self, user: Id) {
        let key = format!("note:{user}");
        if !self.reads.begin(&key, Duration::from_secs(300)) {
            return;
        }
        let note = self.rest.user_note(user).await;
        self.store.write().notes.insert(user, note);
        self.reads.finish(&key, true);
    }

    pub async fn set_note(&self, intent: UserIntent, user: Id, note: &str) -> Result<()> {
        let note: String = note.chars().take(256).collect();
        self.rest.set_user_note(intent, user, &note).await?;
        self.store.write().notes.insert(user, note);
        Ok(())
    }

    // ----- Creating and joining servers (rules in `server_join`) -----

    pub async fn preview_invite(&self, input: &str) -> Result<crate::server_join::InvitePreview> {
        let code = crate::server_join::parse_invite_code(input).ok_or_else(|| Error::Other("that doesn't look like an invite link".into()))?;
        let v = self.rest.invite_preview(&code).await?;
        crate::server_join::parse_preview(&code, &v).ok_or_else(|| Error::Other("that invite isn't for a server".into()))
    }

    /// Wait (briefly) for the gateway to deliver a guild we just created or joined.
    async fn wait_for_guild(&self, guild: Id) {
        for _ in 0..25 {
            if self.store.read().guilds.contains_key(&guild) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Join a server by invite; returns its id once it appears in the rail.
    pub async fn join_server(&self, intent: UserIntent, input: &str) -> Result<Id> {
        let code = crate::server_join::parse_invite_code(input).ok_or_else(|| Error::Other("that doesn't look like an invite link".into()))?;
        let session = self.store.read().gateway_session.clone().unwrap_or_default();
        let v = self.rest.accept_invite(intent, &code, &session).await?;
        let guild = v["guild"]["id"].as_str().and_then(|n| n.parse::<u64>().ok()).map(Snowflake).ok_or_else(|| Error::Other("joined, but Discord didn't say which server".into()))?;
        self.wait_for_guild(guild).await;
        Ok(guild)
    }

    /// Create a server with a name and optional icon; returns its id once it appears in the rail.
    pub async fn create_server(&self, intent: UserIntent, name: &str, icon: Option<(String, Vec<u8>)>) -> Result<Id> {
        let icon = match icon {
            Some((ct, bytes)) => Some(crate::guild_admin::data_uri(&ct, &bytes).ok_or_else(|| Error::Other("use a PNG, JPEG, GIF or WebP under 10 MB".into()))?),
            None => None,
        };
        let body = crate::server_join::create_body(name, icon).ok_or_else(|| Error::Other("a server name needs at least 2 characters".into()))?;
        let v = self.rest.create_guild(intent, body).await?;
        let guild = v["id"].as_str().and_then(|n| n.parse::<u64>().ok()).map(Snowflake).ok_or_else(|| Error::Other("Discord didn't return the new server".into()))?;
        self.wait_for_guild(guild).await;
        Ok(guild)
    }

    /// The users who reacted with `emoji` (a unicode emoji or `name:id`), named as this server shows them.
    pub async fn reaction_users(&self, channel: Id, message: Id, emoji: &str) -> Result<Vec<(Id, String)>> {
        // Discord pages by user id, 100 at a time; a reaction can have thousands, so stop at 1000.
        let mut users: Vec<crate::model::User> = Vec::new();
        for _ in 0..10 {
            let after = users.last().map(|u| u.id);
            let page: Vec<crate::model::User> = serde_json::from_value(self.rest.reaction_users(channel, message, emoji, 100, after).await?).unwrap_or_default();
            let full = page.len() >= 100;
            users.extend(page);
            if !full {
                break;
            }
        }
        let mut st = self.store.write();
        let guild = st.channels.get(&channel).and_then(|c| c.guild_id);
        let mut out = Vec::new();
        for u in users {
            let id = u.id;
            st.users.entry(id).or_insert(u);
            out.push((id, st.display_name(guild, id)));
        }
        Ok(out)
    }

    // ----- Server setup: screening and onboarding (rules in `server_setup`) -----

    /// What the server still asks of me, if anything: screening first, then onboarding.
    pub async fn server_setup(&self, guild: Id, force: bool) -> Result<Option<SetupStep>> {
        use crate::server_setup as ss;
        let (mine, features) = {
            let st = self.store.read();
            (st.my_members.get(&guild).cloned(), st.guilds.get(&guild).map(|g| g.inline.features.clone()).unwrap_or_default())
        };
        let Some(me) = mine else { return Ok(None) };
        if ss::screening_due(&me) {
            let v = self.rest.member_verification(guild).await?;
            return Ok(ss::parse_screening(&v).map(SetupStep::Screening));
        }
        if force || ss::onboarding_due(&features, &me) {
            let v = self.rest.onboarding(guild).await?;
            return Ok(ss::parse_onboarding(&v).map(SetupStep::Onboarding));
        }
        Ok(None)
    }

    pub async fn accept_screening(&self, intent: UserIntent, guild: Id, answers: &[String]) -> Result<()> {
        let form = crate::server_setup::parse_screening(&self.rest.member_verification(guild).await?).ok_or_else(|| Error::Other("this server has no rules to accept".into()))?;
        let body = crate::server_setup::screening_body(&form, answers).ok_or_else(|| Error::Other("answer the required questions first".into()))?;
        self.rest.submit_member_verification(intent, guild, body).await?;
        if let Some(m) = self.store.write().my_members.get_mut(&guild) {
            m.pending = false;
        }
        Ok(())
    }

    pub async fn submit_onboarding(&self, intent: UserIntent, guild: Id, chosen: &[Id]) -> Result<()> {
        let form = crate::server_setup::parse_onboarding(&self.rest.onboarding(guild).await?).ok_or_else(|| Error::Other("this server has no onboarding".into()))?;
        let form = &form;
        if !crate::server_setup::onboarding_complete(form, chosen) {
            return Err(Error::Other("pick an option for each required question".into()));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        self.rest.submit_onboarding(intent, guild, crate::server_setup::onboarding_body(form, chosen, now)).await?;
        if let Some(m) = self.store.write().my_members.get_mut(&guild) {
            m.flags |= crate::server_setup::FLAG_COMPLETED_ONBOARDING;
        }
        Ok(())
    }

    // ----- Server administration (rules in `guild_admin`) -----

    /// Create a role (`None`) or edit one. Permission bits I can't grant stay as they were.
    pub async fn save_role(&self, intent: UserIntent, guild: Id, role: Option<Id>, draft: crate::guild_admin::RoleDraft) -> Result<()> {
        use crate::guild_admin as ga;
        let (grantable, old) = {
            let st = self.store.read();
            if st.guild_permissions(guild) & crate::store::perm::MANAGE_ROLES == 0 {
                return Err(Error::Other("missing permission".into()));
            }
            if let Some(r) = role {
                if !st.can_edit_role(guild, r) {
                    return Err(Error::Other("that role is above yours".into()));
                }
            }
            let old = role.and_then(|r| st.roles.get(&guild)?.get(&r)).and_then(|r| r.permissions.parse::<u64>().ok()).unwrap_or(0);
            (ga::grantable(st.guild_permissions(guild)), old)
        };
        let mut draft = draft;
        draft.permissions = ga::permissions_for_save(old, draft.permissions, grantable);
        match role {
            Some(r) => self.rest.edit_role(intent, guild, r, draft.body()).await.map(|_| ()),
            None => self.rest.create_role(intent, guild, draft.body()).await.map(|_| ()),
        }
    }

    pub async fn delete_role(&self, intent: UserIntent, guild: Id, role: Id) -> Result<()> {
        if role == guild || !self.store.read().can_edit_role(guild, role) {
            return Err(Error::Other("you can't delete that role".into()));
        }
        self.rest.delete_role(intent, guild, role).await
    }

    fn require_channel_manage(&self, channel: Id) -> Result<bool> {
        let st = self.store.read();
        let c = st.channels.get(&channel).ok_or_else(|| Error::Other("no such channel".into()))?;
        if c.guild_id.is_none() || st.permissions(c) & crate::store::perm::MANAGE_CHANNELS == 0 {
            return Err(Error::Other("missing permission".into()));
        }
        Ok(matches!(c.kind, crate::model::channel_type::GUILD_TEXT | crate::model::channel_type::GUILD_ANNOUNCEMENT | 15 | 16))
    }

    pub async fn edit_channel(&self, intent: UserIntent, channel: Id, draft: crate::guild_admin::ChannelDraft) -> Result<()> {
        let text_like = self.require_channel_manage(channel)?;
        self.rest.edit_channel(intent, channel, draft.body(text_like)).await.map(|_| ())
    }

    pub async fn delete_channel(&self, intent: UserIntent, channel: Id) -> Result<()> {
        self.require_channel_manage(channel)?;
        self.rest.delete_channel(intent, channel).await
    }

    /// Set (`Some`) or remove (`None`) the server icon or banner. Banners need a boosted server;
    /// Discord says so if not.
    pub async fn set_guild_image(&self, intent: UserIntent, guild: Id, banner: bool, image: Option<(String, Vec<u8>)>) -> Result<()> {
        self.require_guild_perm(guild, crate::store::perm::MANAGE_GUILD)?;
        let value = match image {
            Some((ct, bytes)) => serde_json::Value::from(crate::guild_admin::data_uri(&ct, &bytes).ok_or_else(|| Error::Other("use a PNG, JPEG, GIF or WebP under 10 MB".into()))?),
            None => serde_json::Value::Null,
        };
        let key = if banner { "banner" } else { "icon" };
        self.rest.edit_guild(intent, guild, serde_json::json!({ key: value })).await.map(|_| ())
    }

    fn require_expressions(&self, guild: Id) -> Result<()> {
        if !self.store.read().can_manage_expressions(guild) {
            return Err(Error::Other("missing permission".into()));
        }
        Ok(())
    }

    pub async fn create_emoji(&self, intent: UserIntent, guild: Id, name: &str, content_type: &str, bytes: Vec<u8>) -> Result<()> {
        self.require_expressions(guild)?;
        if bytes.len() > crate::guild_admin::MAX_EMOJI_BYTES {
            return Err(Error::Other("emoji must be under 256 KB".into()));
        }
        let image = crate::guild_admin::data_uri(content_type, &bytes).ok_or_else(|| Error::Other("use a PNG, JPEG, GIF or WebP".into()))?;
        self.rest.create_emoji(intent, guild, &crate::guild_admin::emoji_name(name), &image).await.map(|_| ())
    }

    pub async fn rename_emoji(&self, intent: UserIntent, guild: Id, emoji: Id, name: &str) -> Result<()> {
        self.require_expressions(guild)?;
        self.rest.rename_emoji(intent, guild, emoji, &crate::guild_admin::emoji_name(name)).await
    }

    pub async fn delete_emoji(&self, intent: UserIntent, guild: Id, emoji: Id) -> Result<()> {
        self.require_expressions(guild)?;
        self.rest.delete_emoji(intent, guild, emoji).await
    }

    pub async fn delete_sticker(&self, intent: UserIntent, guild: Id, sticker: Id) -> Result<()> {
        self.require_expressions(guild)?;
        self.rest.delete_sticker(intent, guild, sticker).await
    }

    /// Give or take a role, if it is one I may toggle for this member.
    pub async fn set_member_role(&self, intent: UserIntent, guild: Id, user: Id, role: Id, add: bool) -> Result<()> {
        let ok = self.store.read().assignable_roles(guild, user).iter().any(|r| r.id == role && r.toggle);
        if !ok {
            return Err(Error::Other("you can't change that role".into()));
        }
        self.rest.set_member_role(intent, guild, user, role, add).await
    }

    pub async fn unban_member(&self, intent: UserIntent, guild: Id, user: Id) -> Result<()> {
        self.require_guild_perm(guild, crate::store::perm::BAN_MEMBERS)?;
        self.rest.unban_member(intent, guild, user).await
    }

    pub async fn guild_bans(&self, guild: Id) -> Result<Vec<crate::moderation::BanEntry>> {
        self.require_guild_perm(guild, crate::store::perm::BAN_MEMBERS)?;
        Ok(crate::moderation::parse_bans(&self.rest.guild_bans(guild).await?))
    }

    pub async fn guild_audit_log(&self, guild: Id) -> Result<Vec<crate::moderation::AuditEntry>> {
        self.require_guild_perm(guild, crate::store::perm::VIEW_AUDIT_LOG)?;
        Ok(crate::moderation::parse_audit(&self.rest.guild_audit_log(guild).await?))
    }

    pub async fn guild_invites(&self, guild: Id) -> Result<Vec<crate::moderation::InviteEntry>> {
        self.require_guild_perm(guild, crate::store::perm::MANAGE_GUILD)?;
        Ok(crate::moderation::parse_invites(&self.rest.guild_invites(guild).await?))
    }

    pub async fn revoke_invite(&self, intent: UserIntent, guild: Id, code: &str) -> Result<()> {
        self.require_guild_perm(guild, crate::store::perm::MANAGE_GUILD)?;
        self.rest.revoke_invite(intent, code).await
    }

    /// Rename a server or change its description; the gateway's `GUILD_UPDATE` refreshes the store.
    pub async fn edit_guild(&self, intent: UserIntent, guild: Id, name: Option<String>, description: Option<String>) -> Result<()> {
        self.require_guild_perm(guild, crate::store::perm::MANAGE_GUILD)?;
        let mut body = serde_json::Map::new();
        if let Some(n) = name.map(|n| n.trim().to_string()) {
            if n.is_empty() || n.chars().count() > 100 {
                return Err(Error::Other("a server name is 1 to 100 characters".into()));
            }
            body.insert("name".into(), Value::String(n));
        }
        if let Some(d) = description {
            body.insert("description".into(), if d.trim().is_empty() { Value::Null } else { Value::String(d) });
        }
        if body.is_empty() {
            return Ok(());
        }
        self.rest.edit_guild(intent, guild, Value::Object(body)).await?;
        Ok(())
    }

    /// Create (`None`) or edit an event. `image`: `None` leaves the cover alone, `Some(None)` removes
    /// it, `Some(Some(..))` sets it.
    pub async fn save_event(&self, intent: UserIntent, guild: Id, event: Option<Id>, draft: crate::guild_events::EventDraft, image: Option<Option<(String, Vec<u8>)>>) -> Result<()> {
        let start_changed = {
            let st = self.store.read();
            match event {
                None => {
                    if !st.can_create_events(guild) {
                        return Err(Error::Other("missing permission".into()));
                    }
                    true
                }
                Some(e) => {
                    let cur = st.scheduled_events.get(&guild).and_then(|l| l.iter().find(|x| x.id == e)).ok_or_else(|| Error::Other("that event is gone".into()))?;
                    if !st.can_manage_event(cur) {
                        return Err(Error::Other("missing permission".into()));
                    }
                    cur.start_ms() != draft.start_ms
                }
            }
        };
        if let Some(why) = draft.check(crate::timefmt::now_ms(), start_changed) {
            return Err(Error::Refused(why));
        }
        let image = match image {
            None => None,
            Some(None) => Some(serde_json::Value::Null),
            Some(Some((ct, bytes))) => Some(serde_json::Value::from(crate::guild_admin::data_uri(&ct, &bytes).ok_or_else(|| Error::Refused("Use a PNG, JPEG, GIF or WebP under 10 MB.".into()))?)),
        };
        match event {
            None => self.rest.create_event(intent, guild, draft.body(image)).await.map(|_| ()),
            Some(e) => self.rest.edit_event(intent, guild, e, draft.body(image)).await.map(|_| ()),
        }
    }

    /// Start, end or cancel an event (the gateway confirms).
    pub async fn set_event_status(&self, intent: UserIntent, guild: Id, event: Id, wanted: u8) -> Result<()> {
        {
            let st = self.store.read();
            let cur = st.scheduled_events.get(&guild).and_then(|l| l.iter().find(|x| x.id == event)).ok_or_else(|| Error::Other("that event is gone".into()))?;
            if !st.can_manage_event(cur) {
                return Err(Error::Other("missing permission".into()));
            }
            if !crate::guild_events::next_status(cur.status, wanted) {
                return Err(Error::Refused("That event can't be changed that way.".into()));
            }
        }
        self.rest.edit_event(intent, guild, event, serde_json::json!({ "status": wanted })).await.map(|_| ())
    }

    pub async fn delete_event(&self, intent: UserIntent, guild: Id, event: Id) -> Result<()> {
        {
            let st = self.store.read();
            let cur = st.scheduled_events.get(&guild).and_then(|l| l.iter().find(|x| x.id == event)).ok_or_else(|| Error::Other("that event is gone".into()))?;
            if !st.can_manage_event(cur) {
                return Err(Error::Other("missing permission".into()));
            }
        }
        self.rest.delete_event(intent, guild, event).await
    }

    pub async fn create_channel(&self, intent: UserIntent, guild: Id, draft: crate::guild_admin::NewChannel) -> Result<()> {
        self.require_guild_perm(guild, crate::store::perm::MANAGE_CHANNELS)?;
        if let Some(why) = draft.check() {
            return Err(Error::Refused(why));
        }
        self.rest.create_channel(intent, guild, draft.body()).await.map(|_| ())
    }

    /// Move a channel one place up or down among the channels around it.
    pub async fn move_channel(&self, intent: UserIntent, channel: Id, later: bool) -> Result<()> {
        self.require_channel_manage(channel)?;
        let (guild, before) = {
            let st = self.store.read();
            (st.channels.get(&channel).and_then(|c| c.guild_id).ok_or_else(|| Error::Other("no such channel".into()))?, st.channel_siblings(channel))
        };
        let Some(after) = crate::guild_admin::step(&before, channel, later) else { return Ok(()) };
        self.rest.reorder_channels(intent, guild, crate::guild_admin::position_changes(&before, &after)).await
    }

    /// Drop a channel (or category) onto another: the PATCH is planned by the core, see `channel_drop_plan`.
    pub async fn drop_channel(&self, intent: UserIntent, dragged: Id, target: Id, zone: crate::guild_admin::DropZone) -> Result<()> {
        let plan = self.store.read().channel_drop_plan(dragged, target, zone);
        let Some((guild, changes)) = plan else { return Ok(()) };
        self.rest.reorder_channels(intent, guild, changes).await
    }

    /// Move a role one place up or down. It can't go above my own top role, and `@everyone` stays at the bottom.
    pub async fn move_role(&self, intent: UserIntent, guild: Id, role: Id, later: bool) -> Result<()> {
        let (before, after) = {
            let st = self.store.read();
            if !st.can_move_role(guild, role, later) {
                return Err(Error::Refused("You can't move that role there.".into()));
            }
            let before = st.roles_ascending(guild);
            let after = crate::guild_admin::step(&before, role, later).unwrap_or_else(|| before.clone());
            (before, after)
        };
        self.rest.reorder_roles(intent, guild, crate::guild_admin::position_changes(&before, &after)).await
    }

    fn require_channel_roles(&self, channel: Id) -> Result<(Id, u64)> {
        let st = self.store.read();
        let c = st.channels.get(&channel).ok_or_else(|| Error::Other("no such channel".into()))?;
        let g = c.guild_id.ok_or_else(|| Error::Other("not a server channel".into()))?;
        let mine = st.permissions(c);
        if mine & (crate::store::perm::MANAGE_ROLES | crate::store::perm::ADMINISTRATOR) == 0 {
            return Err(Error::Other("missing permission".into()));
        }
        Ok((g, crate::guild_admin::grantable(mine)))
    }

    /// Set what a role or member may do in one channel. Bits I don't hold stay as they were.
    pub async fn save_overwrite(&self, intent: UserIntent, channel: Id, target: Id, is_role: bool, allow: u64, deny: u64) -> Result<()> {
        let (_, grantable) = self.require_channel_roles(channel)?;
        let old = {
            let st = self.store.read();
            st.channels.get(&channel).and_then(|c| c.permission_overwrites.iter().find(|o| o.id == target)).map_or((0, 0), |o| (o.allow.parse().unwrap_or(0), o.deny.parse().unwrap_or(0)))
        };
        let (a, d) = crate::guild_admin::overwrite_for_save(old, (allow, deny), grantable);
        self.rest.put_overwrite(intent, channel, target, crate::guild_admin::overwrite_body(target, is_role, a, d)).await
    }

    pub async fn delete_overwrite(&self, intent: UserIntent, channel: Id, target: Id) -> Result<()> {
        self.require_channel_roles(channel)?;
        self.rest.delete_overwrite(intent, channel, target).await
    }

    /// Interested / not interested in a scheduled event; applied locally, confirmed by the gateway.
    pub async fn set_event_interest(&self, intent: UserIntent, guild: Id, event: Id, interested: bool) -> Result<()> {
        self.rest.set_event_interest(intent, guild, event, interested).await?;
        let mut s = self.store.write();
        let changed = if interested { s.event_interest.insert(event) } else { s.event_interest.remove(&event) };
        if changed {
            if let Some(e) = s.scheduled_events.get_mut(&guild).and_then(|l| l.iter_mut().find(|e| e.id == event)) {
                let n = e.user_count.unwrap_or(0);
                e.user_count = Some(if interested { n + 1 } else { n.saturating_sub(1) });
            }
        }
        drop(s);
        (self.emit)(SessionEvent::Deltas(vec![Delta::GuildEvents { guild }]));
        Ok(())
    }

    /// Raise or lower your hand in the stage you are in.
    pub async fn stage_hand(&self, intent: UserIntent, guild: Id, channel: Id, raise: bool) -> Result<()> {
        self.rest.stage_request_to_speak(intent, guild, channel, raise).await
    }

    /// Invite a raised hand to speak, or move a speaker to the audience (needs Mute Members).
    pub async fn stage_set_speaker(&self, intent: UserIntent, guild: Id, channel: Id, user: Id, speaker: bool) -> Result<()> {
        self.rest.stage_set_suppress(intent, guild, channel, Some(user), !speaker).await
    }

    /// Accept a message request: the DM moves to the normal list.
    pub async fn accept_message_request(&self, intent: UserIntent, channel: Id) -> Result<()> {
        if !self.store.read().channels.get(&channel).is_some_and(|c| c.is_message_request) {
            return Err(Error::Other("not a message request".into()));
        }
        self.rest.accept_message_request(intent, channel).await?;
        let deltas = self.store.write().accept_message_request(channel);
        (self.emit)(SessionEvent::Deltas(deltas));
        Ok(())
    }

    /// Write a partial preloaded-settings message. Applied locally at once (like the web client's
    /// optimistic store), then confirmed by the PATCH response.
    pub async fn update_prefs(&self, intent: UserIntent, patch: crate::prefs::Prefs) -> Result<()> {
        let bytes = patch.encode_patch();
        if bytes.is_empty() {
            return Ok(());
        }
        self.store.write().prefs.merge(&patch);
        (self.emit)(SessionEvent::Deltas(vec![Delta::SettingsChanged]));
        if let Some(full) = self.rest.patch_settings_proto(intent, 1, &bytes).await? {
            self.store.write().absorb_settings_proto(&full);
            (self.emit)(SessionEvent::Deltas(vec![Delta::SettingsChanged, Delta::GuildsChanged]));
        }
        Ok(())
    }

    /// `PATCH /users/@me` then fold the returned user into the store.
    pub async fn update_me(&self, intent: UserIntent, body: Value) -> Result<()> {
        let v = self.rest.patch_me(intent, body).await?;
        if let Ok(u) = serde_json::from_value::<crate::model::User>(v) {
            let mut st = self.store.write();
            if let Some(me) = st.me.as_mut() {
                me.username = u.username.clone();
                me.global_name = u.global_name.clone();
                me.avatar = u.avatar.clone();
            }
            st.users.insert(u.id, u.clone());
            drop(st);
            (self.emit)(SessionEvent::Deltas(vec![Delta::SettingsChanged, Delta::UserUpdated { user: u.id }]));
        }
        Ok(())
    }

    pub async fn update_profile(&self, intent: UserIntent, body: Value) -> Result<()> {
        self.rest.patch_profile(intent, body).await?;
        Ok(())
    }

    pub async fn update_guild_settings(&self, intent: UserIntent, guild: Id, body: Value) -> Result<()> {
        let v = self.rest.patch_guild_settings(intent, guild, body).await?;
        if let Ok(s) = serde_json::from_value::<crate::model::UserGuildSettings>(v) {
            self.store.write().guild_settings.insert(guild, s);
            (self.emit)(SessionEvent::Deltas(vec![Delta::ChannelsChanged { guild: Some(guild) }, Delta::SettingsChanged]));
        }
        Ok(())
    }

    /// A profile card's full data. Answered from memory for a minute after a fetch, as the official
    /// client does when a popout is reopened.
    pub async fn profile(&self, user: Id, guild: Option<Id>) -> Result<serde_json::Value> {
        const FRESH: Duration = Duration::from_secs(60);
        if let Some((at, v)) = self.profiles.lock().get(&(user, guild)) {
            if at.elapsed() < FRESH {
                return Ok(v.clone());
            }
        }
        let v = self.rest.user_profile(user, guild).await?;
        let mut cache = self.profiles.lock();
        if cache.len() > 64 {
            cache.retain(|_, (at, _)| at.elapsed() < FRESH);
        }
        cache.insert((user, guild), (Instant::now(), v.clone()));
        Ok(v)
    }

    pub async fn logout(&self, intent: UserIntent) -> Result<()> {
        let _: Value = self
            .rest
            .write(intent, Method::POST, "/auth/logout", Some(serde_json::json!({ "provider": null, "voip_provider": null })))
            .await
            .unwrap_or(Value::Null);
        self.shutdown();
        self.cache.wipe()
    }
}

/// The next thing a server wants from a new member.
#[derive(Debug, Clone)]
pub enum SetupStep {
    Screening(crate::server_setup::Screening),
    Onboarding(crate::server_setup::Onboarding),
}

#[cfg(test)]
mod send_tests {
    use super::*;
    use crate::fidelity::{ClientIdentity, HostInfo};
    use crate::intent::{Gesture, UserIntent};
    use crate::privacy::Policy;
    use crate::profile::IdentityProfile;
    use crate::transport::Transport;

    fn session() -> (Arc<Session>, Arc<Mutex<Vec<Delta>>>) {
        let identity = Arc::new(ClientIdentity::new(IdentityProfile::bundled(), HostInfo::detect()));
        let rest = Arc::new(Rest::new(Arc::new(Transport::new(None).unwrap()), identity, Arc::new(RwLock::new(Policy::default()))));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let emit: Emit = Arc::new(move |e| {
            if let SessionEvent::Deltas(d) = e {
                log.lock().extend(d);
            }
        });
        let s = Session::new(Snowflake(1), rest, Arc::new(Cache::in_memory().unwrap()), emit).unwrap();
        {
            let mut st = s.store.write();
            st.me = Some(crate::model::User { id: Snowflake(1), username: "me".into(), ..Default::default() });
            st.channels.insert(Snowflake(11), crate::model::Channel { id: Snowflake(11), ..Default::default() });
        }
        (s, seen)
    }

    fn intent() -> UserIntent {
        UserIntent::from_gesture(Gesture::Key)
    }

    fn file(name: &str, bytes: usize) -> LocalFile {
        LocalFile { filename: name.into(), content_type: "application/zip".into(), bytes: bytes::Bytes::from(vec![0; bytes]), description: None, spoiler: false }
    }

    #[tokio::test]
    async fn an_overlong_message_is_refused_before_anything_is_sent() {
        let (s, seen) = session();
        let nonce = Snowflake(77);
        let e = s.send_tracked(intent(), Snowflake(11), nonce, "a".repeat(2001), None, false, vec![]).await.unwrap_err();
        assert!(matches!(e, Error::Refused(_)));
        assert!(e.user_message().contains("limit is 2000"), "{}", e.user_message());
        let st = s.store.read();
        let state = st.sends.iter().find(|x| x.nonce == nonce).expect("kept so the shell can give the text back");
        assert_eq!(state.phase, crate::send_state::Phase::Failed);
        assert!(state.error.as_deref().unwrap().contains("2000"));
        assert!(st.history.get(&Snowflake(11)).is_none_or(|h| h.messages.iter().all(|m| m.id.0 != 0)), "no stuck echo");
        assert!(seen.lock().iter().any(|d| matches!(d, Delta::Sends { channel } if *channel == Snowflake(11))));
    }

    #[tokio::test]
    async fn a_file_over_the_limit_never_reaches_the_network() {
        let (s, _) = session();
        let nonce = Snowflake(78);
        // 11 MiB against the 10 MiB free limit; the zip can't be shrunk. If this tried the
        // network it would fail with a connection error, not this sentence.
        let e = s.send_tracked(intent(), Snowflake(11), nonce, "here".into(), None, false, vec![file("big.zip", 11 * 1024 * 1024)]).await.unwrap_err();
        let m = e.user_message();
        assert!(m.contains("big.zip is 11 MB") && m.contains("nothing was sent"), "{m}");
        let st = s.store.read();
        assert!(st.history.get(&Snowflake(11)).is_none_or(|h| h.messages.is_empty()), "the echo is gone");
        assert_eq!(st.sends[0].phase, crate::send_state::Phase::Failed);
    }

    #[tokio::test]
    async fn nitro_allows_the_longer_message_past_the_local_check() {
        let (s, _) = session();
        s.store.write().me.as_mut().unwrap().premium_type = Some(2);
        // 3000 characters pass the local check; the request then fails offline, which is a network
        // error rather than a refusal.
        let e = s.send_tracked(intent(), Snowflake(11), Snowflake(79), "a".repeat(3000), None, false, vec![]).await.unwrap_err();
        assert!(!matches!(e, Error::Refused(_)), "{e:?}");
    }

    #[tokio::test]
    async fn a_failed_send_keeps_everything_for_retry_or_take_back() {
        let (s, _) = session();
        s.store.write().me.as_mut().unwrap().premium_type = Some(2);
        let nonce = Snowflake(80);
        let f = file("a.zip", 1024);
        // No token: passes the local checks, then Discord (here, the missing login) refuses.
        let e = s.send_tracked(intent(), Snowflake(11), nonce, "hello".into(), Some(Snowflake(5)), true, vec![f]).await.unwrap_err();
        assert!(matches!(e, Error::Unauthorized), "{e:?}");
        {
            let st = s.store.read();
            let state = &st.sends[0];
            assert_eq!((state.phase, state.can_retry, state.text.as_str()), (crate::send_state::Phase::Failed, true, "hello"));
            assert!(state.error.as_deref().unwrap().contains("session expired"));
        }
        // Retrying runs it again under the same id (and fails the same way).
        let e = s.retry_send(intent(), nonce).await.unwrap_err();
        assert!(matches!(e, Error::Unauthorized));
        assert_eq!(s.store.read().sends.len(), 1, "one entry, not two");
        // Taking it back hands over everything and stops tracking it.
        let back = s.take_back_send(nonce).unwrap();
        assert_eq!((back.content.as_str(), back.reply_to, back.mention_reply, back.files.len()), ("hello", Some(Snowflake(5)), true, 1));
        assert_eq!(back.files[0].bytes.len(), 1024);
        assert!(s.store.read().sends.is_empty());
        assert!(s.take_back_send(nonce).is_none(), "only once");
        assert!(s.retry_send(intent(), nonce).await.is_ok(), "nothing to retry is not an error");
    }

    #[tokio::test]
    async fn a_refused_send_cannot_be_retried_only_edited() {
        let (s, _) = session();
        let nonce = Snowflake(81);
        let _ = s.send_tracked(intent(), Snowflake(11), nonce, "a".repeat(2500), None, false, vec![]).await;
        assert!(!s.store.read().sends[0].can_retry);
        assert_eq!(s.take_back_send(nonce).unwrap().content.len(), 2500, "the text is not lost");
    }

    #[tokio::test]
    async fn dismissing_forgets_the_message() {
        let (s, _) = session();
        let nonce = Snowflake(82);
        let _ = s.send_tracked(intent(), Snowflake(11), nonce, "x".repeat(3000), None, false, vec![]).await;
        s.dismiss_send(nonce);
        assert!(s.store.read().sends.is_empty());
        assert!(s.take_back_send(nonce).is_none());
    }

    #[test]
    fn cancel_and_dismiss_are_harmless_for_unknown_sends() {
        let (s, _) = session();
        s.cancel_send(Snowflake(5));
        s.dismiss_send(Snowflake(5));
        assert!(s.store.read().sends.is_empty());
    }
}
