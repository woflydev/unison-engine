//! Go Live and video: each stream (ours or one we watch) is its own voice connection, opened when
//! the main gateway answers `STREAM_CREATE` + `STREAM_SERVER_UPDATE`; the camera rides the call connection.

use std::sync::Arc;

use super::*;
use crate::store::StreamView;
use crate::voice::stream::{RemoteFrame, Role, VideoControl, VideoIo, VideoWant};
use crate::voice::video::{self, StreamScope};

/// App sinks for video: remote frames (stream key, frame) and encoder control (source, control).
#[derive(Clone)]
pub struct VideoHooks {
    pub frames: Arc<dyn Fn(&str, RemoteFrame) + Send + Sync>,
    pub control: Arc<dyn Fn(&str, VideoControl) + Send + Sync>,
}

impl VideoHooks {
    /// `source` is `"call"` for the call connection (camera) or the stream key.
    pub fn io(&self, source: &str) -> VideoIo {
        let (f, c) = (self.frames.clone(), self.control.clone());
        let (k1, k2) = (source.to_string(), source.to_string());
        VideoIo { frames: Arc::new(move |fr| f(&k1, fr)), control: Arc::new(move |ctl| c(&k2, ctl)) }
    }
}

pub struct StreamCtl {
    role: Role,
    owner: Id,
    rtc_server: Option<Id>,
    rtc_channel: Option<Id>,
    server: Option<(String, String)>,
    handle: Option<VoiceHandle>,
    viewers: Vec<Id>,
    paused: bool,
}

impl StreamCtl {
    fn new(role: Role, owner: Id) -> Self {
        Self { role, owner, rtc_server: None, rtc_channel: None, server: None, handle: None, viewers: vec![], paused: false }
    }
}

impl Session {
    pub fn set_video_hooks(&self, hooks: VideoHooks) {
        self.voice.lock().hooks = Some(hooks);
    }

    fn publish_streams(&self) {
        let views: Vec<StreamView> = self
            .voice
            .lock()
            .streams
            .iter()
            .map(|(k, s)| StreamView { key: k.clone(), owner: s.owner, own: s.role == Role::StreamPublisher, connected: s.handle.is_some(), viewers: s.viewers.len() as u32, paused: s.paused })
            .collect();
        self.store.write().voice.streams = views;
        (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
    }

    /// Start Go Live in the channel we are in: op 18.
    pub fn start_stream(&self, intent: UserIntent) -> Result<()> {
        intent.redeem()?;
        let (guild, channel) = {
            let v = self.voice.lock();
            (v.guild, v.channel)
        };
        let Some(channel) = channel else { return Err(Error::Other("not in a call".into())) };
        let scope = match guild {
            Some(g) => StreamScope::Guild(g, channel),
            None => StreamScope::Call(channel),
        };
        let key = video::stream_key(scope, self.user_id);
        self.voice.lock().streams.insert(key, StreamCtl::new(Role::StreamPublisher, self.user_id));
        if let Some(gw) = self.gateway.lock().as_ref() {
            gw.send(video::gw_op::STREAM_CREATE, video::stream_create(scope, None)["d"].clone());
        }
        Ok(())
    }

    /// Watch someone's stream: op 20.
    pub fn watch_stream(&self, intent: UserIntent, owner: Id) -> Result<()> {
        intent.redeem()?;
        let (guild, channel) = {
            let v = self.voice.lock();
            (v.guild, v.channel)
        };
        let Some(channel) = channel else { return Err(Error::Other("not in a call".into())) };
        let scope = match guild {
            Some(g) => StreamScope::Guild(g, channel),
            None => StreamScope::Call(channel),
        };
        let key = video::stream_key(scope, owner);
        self.voice.lock().streams.entry(key.clone()).or_insert_with(|| StreamCtl::new(Role::StreamViewer, owner));
        if let Some(gw) = self.gateway.lock().as_ref() {
            gw.send(video::gw_op::STREAM_WATCH, video::stream_watch(&key)["d"].clone());
        }
        Ok(())
    }

    /// Stop our stream, or stop watching one: op 19, then close the connection.
    pub fn stop_stream(&self, intent: UserIntent, key: &str) -> Result<()> {
        intent.redeem()?;
        let had = self.voice.lock().streams.remove(key).is_some();
        if had {
            if let Some(gw) = self.gateway.lock().as_ref() {
                gw.send(video::gw_op::STREAM_DELETE, video::stream_delete(key)["d"].clone());
            }
        }
        self.publish_streams();
        Ok(())
    }

    /// Op 22 for a stream we watch.
    pub fn set_stream_paused(&self, intent: UserIntent, key: &str, paused: bool) -> Result<()> {
        intent.redeem()?;
        if let Some(gw) = self.gateway.lock().as_ref() {
            gw.send(video::gw_op::STREAM_SET_PAUSED, video::stream_set_paused(key, paused)["d"].clone());
        }
        Ok(())
    }

    /// The stream connection for `key`, to feed it frames or change what it wants.
    pub fn with_stream<R>(&self, key: &str, f: impl FnOnce(&VoiceHandle) -> R) -> Option<R> {
        self.voice.lock().streams.get(key).and_then(|s| s.handle.as_ref()).map(f)
    }

    /// The first stream connection we publish on.
    pub fn with_own_stream<R>(&self, f: impl FnOnce(&VoiceHandle) -> R) -> Option<R> {
        self.voice.lock().streams.values().find(|s| s.role == Role::StreamPublisher).and_then(|s| s.handle.as_ref()).map(f)
    }

    /// The call connection (camera video).
    pub fn with_call<R>(&self, f: impl FnOnce(&VoiceHandle) -> R) -> Option<R> {
        self.voice.lock().handle.as_ref().map(f)
    }

    /// Camera on or off: op 4 `self_video` and op 12 on the call connection.
    pub fn set_camera(&self, intent: UserIntent, on: bool, want: VideoWant) -> Result<()> {
        intent.redeem()?;
        let (guild, channel, mute, deaf) = {
            let v = self.voice.lock();
            if let Some(h) = &v.handle {
                if on { h.start_video(want) } else { h.stop_video() }
            }
            (v.guild, v.channel, v.mute, v.deaf)
        };
        if channel.is_some() {
            if let Some(gw) = self.gateway.lock().as_ref() {
                let mut p = gateway::voice_state_payload(guild, channel, mute, deaf);
                p["self_video"] = on.into();
                gw.send(gateway::op::VOICE_STATE_UPDATE, p);
            }
        }
        self.store.write().voice.camera = on;
        (self.emit)(SessionEvent::Deltas(vec![Delta::Voice]));
        Ok(())
    }

    pub(super) fn on_stream_dispatch(self: &Arc<Self>, name: &str, data: &[u8]) {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Ev {
            stream_key: String,
            rtc_server_id: Option<Id>,
            rtc_channel_id: Option<Id>,
            token: Option<String>,
            endpoint: Option<String>,
            viewer_ids: Vec<Id>,
            paused: bool,
        }
        let Ok(ev) = unison_etf::from_term::<Ev>(unison_etf::RawTerm(data)) else { return };
        if ev.stream_key.is_empty() {
            return;
        }
        let parsed = video::parse_stream_key(&ev.stream_key);
        let mut v = self.voice.lock();
        // Our own key as the server spells it wins over the one we derived.
        if !v.streams.contains_key(&ev.stream_key) && parsed.as_ref().is_some_and(|p| p.owner_id == self.user_id) {
            let old = v.streams.iter().find(|(_, s)| s.role == Role::StreamPublisher).map(|(k, _)| k.clone());
            if let Some(ctl) = old.and_then(|k| v.streams.remove(&k)) {
                v.streams.insert(ev.stream_key.clone(), ctl);
            }
        }
        match name {
            "STREAM_CREATE" | "STREAM_SERVER_UPDATE" | "STREAM_UPDATE" => {
                // A stream we didn't ask for is just announced (someone else's Go Live): the UI learns of it from voice states.
                let Some(ctl) = v.streams.get_mut(&ev.stream_key) else { return };
                if let Some(s) = ev.rtc_server_id { ctl.rtc_server = Some(s) }
                if let Some(c) = ev.rtc_channel_id { ctl.rtc_channel = Some(c) }
                if let (Some(t), Some(e)) = (ev.token.clone(), ev.endpoint.clone().filter(|e| !e.is_empty())) {
                    ctl.server = Some((t, e));
                    if let Some(h) = ctl.handle.take() {
                        h.shutdown();
                    }
                }
                if name != "STREAM_SERVER_UPDATE" {
                    ctl.viewers = ev.viewer_ids;
                    ctl.paused = ev.paused;
                }
                let (Some(rs), Some(rc), Some((token, endpoint)), None) = (ctl.rtc_server, ctl.rtc_channel, ctl.server.clone(), ctl.handle.as_ref()) else {
                    drop(v);
                    self.publish_streams();
                    return;
                };
                let Some(session_id) = v.session_id.clone() else { return };
                let role = v.streams[&ev.stream_key].role;
                let io = v.hooks.as_ref().map(|h| h.io(&ev.stream_key));
                let params = voice::Params {
                    guild_id: parsed.as_ref().and_then(|p| p.guild_id),
                    channel_id: rc,
                    user_id: self.user_id,
                    session_id,
                    token,
                    endpoint,
                    bitrate: 0,
                    muted: true,
                    deaf: true,
                    role,
                    server_id: Some(rs),
                    video: io,
                };
                let emit = self.emit.clone();
                let on_event: voice::Emit = Arc::new(move |_| emit(SessionEvent::Deltas(vec![Delta::Voice])));
                let h = voice::spawn(params, self.rest.transport.clone(), self.identity.clone(), on_event);
                if role == Role::StreamPublisher {
                    // The app's capture starts feeding frames once this connection can send.
                    if let Some(want) = v.hooks.as_ref().map(|_| VideoWant { kind: "screen", width: 1920, height: 1080, fps: 30 }) {
                        h.start_video(want);
                    }
                }
                if let Some(ctl) = v.streams.get_mut(&ev.stream_key) {
                    ctl.handle = Some(h);
                }
                drop(v);
                self.publish_streams();
            }
            "STREAM_DELETE" => {
                if let Some(ctl) = v.streams.remove(&ev.stream_key) {
                    if let Some(h) = ctl.handle {
                        h.shutdown();
                    }
                }
                drop(v);
                self.publish_streams();
            }
            _ => {}
        }
    }
}
