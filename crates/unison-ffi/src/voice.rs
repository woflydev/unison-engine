use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct VoiceMember {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
    pub muted: bool,
    pub deafened: bool,
    /// Muted or deafened by a moderator rather than themselves.
    pub server_muted: bool,
    pub video: bool,
    pub streaming: bool,
    pub speaking: bool,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum VoiceConn {
    Idle,
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct VoiceCallItem {
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub guild_name: Option<String>,
    pub status: VoiceConn,
    /// End-to-end encrypted (DAVE), with the code people compare to verify it.
    pub e2ee: bool,
    pub privacy_code: Option<String>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub audio_error: Option<String>,
    pub ping_ms: Option<u32>,
    pub avg_ping_ms: Option<u32>,
    /// Our camera is sending.
    pub camera: bool,
    pub streams: Vec<StreamItem>,
}

/// A Go Live stream we publish or watch.
#[derive(uniffi::Record, Clone, Debug)]
pub struct StreamItem {
    pub key: String,
    pub owner_id: String,
    pub own: bool,
    pub connected: bool,
    pub viewers: u32,
    pub paused: bool,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum VideoSource {
    Camera,
    Screen,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum VideoControlKind {
    Keyframe,
    Bitrate,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct RingingUser {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
}

/// A call going on in a DM or group DM.
#[derive(uniffi::Record, Clone, Debug)]
pub struct DmCallItem {
    pub channel_id: String,
    pub participants: Vec<String>,
    pub participant_names: Vec<String>,
    /// Others still being rung.
    pub ringing: Vec<RingingUser>,
    /// The call is ringing you and you have not joined.
    pub ringing_me: bool,
    /// You are in this call.
    pub joined: bool,
    /// Someone in the call other than you (who is calling you, for an incoming ring).
    pub caller_name: Option<String>,
}

#[uniffi::export]
impl UnisonEngine {
    /// Who is in a voice channel, for the sidebar.
    pub fn voice_members(
        &self,
        account_id: String,
        guild_id: String,
        channel_id: String,
    ) -> Vec<VoiceMember> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let c = id(&channel_id);
            let in_call = s.voice.channel == Some(c);
            s.voice_members(g, c)
                .into_iter()
                .map(|v| VoiceMember {
                    user_id: v.user_id.to_string(),
                    name: s.display_name(Some(g), v.user_id),
                    avatar_url: s
                        .users
                        .get(&v.user_id)
                        .map(|u| u.avatar_url(64))
                        .unwrap_or_default(),
                    muted: v.self_mute || v.mute,
                    deafened: v.self_deaf || v.deaf,
                    server_muted: v.mute || v.deaf,
                    video: v.self_video,
                    streaming: v.self_stream.unwrap_or(false),
                    speaking: in_call && s.voice.speaking.contains(&v.user_id),
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Everyone in the call you're in (a server voice channel, DM or group DM), you included.
    pub fn call_members(&self, account_id: String) -> Vec<VoiceMember> {
        self.with_store(&account_id, |s| {
            let Some(c) = s.voice.channel else {
                return vec![];
            };
            let states: Vec<&unison_core::model::VoiceState> = match s.voice.guild {
                Some(g) => s.voice_members(g, c),
                None => s
                    .dm_voice
                    .get(&c)
                    .map(|m| m.values().collect())
                    .unwrap_or_default(),
            };
            let mut out: Vec<VoiceMember> = states
                .into_iter()
                .map(|v| VoiceMember {
                    user_id: v.user_id.to_string(),
                    name: s.display_name(s.voice.guild, v.user_id),
                    avatar_url: s
                        .users
                        .get(&v.user_id)
                        .map(|u| u.avatar_url(128))
                        .unwrap_or_default(),
                    muted: v.self_mute || v.mute,
                    deafened: v.self_deaf || v.deaf,
                    server_muted: v.mute || v.deaf,
                    video: v.self_video,
                    streaming: v.self_stream.unwrap_or(false),
                    speaking: s.voice.speaking.contains(&v.user_id),
                })
                .collect();
            out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            out
        })
        .unwrap_or_default()
    }

    pub fn voice_call(&self, account_id: String) -> Option<VoiceCallItem> {
        use unison_core::voice::connection::Status;
        self.with_store(&account_id, |s| {
            let v = &s.voice;
            let channel = v.channel.and_then(|c| s.channels.get(&c));
            VoiceCallItem {
                guild_id: v.guild.map(|g| g.to_string()),
                channel_id: v.channel.map(|c| c.to_string()),
                channel_name: channel.and_then(|c| {
                    if c.is_private() {
                        Some(dm_item(s, c).name)
                    } else {
                        c.name.clone()
                    }
                }),
                guild_name: v
                    .guild
                    .and_then(|g| s.guilds.get(&g))
                    .map(|g| g.name().to_string()),
                status: match v.status {
                    None => VoiceConn::Idle,
                    Some(Status::Connecting) => VoiceConn::Connecting,
                    Some(Status::Connected) => VoiceConn::Connected,
                    Some(Status::Reconnecting) => VoiceConn::Reconnecting,
                    Some(Status::Disconnected) => VoiceConn::Disconnected,
                },
                e2ee: v.e2ee,
                privacy_code: v.privacy_code.clone(),
                self_mute: v.self_mute,
                self_deaf: v.self_deaf,
                audio_error: v.audio_error.clone(),
                ping_ms: v.ping_ms,
                avg_ping_ms: (!v.pings.is_empty())
                    .then(|| v.pings.iter().sum::<u32>() / v.pings.len() as u32),
                camera: v.camera,
                streams: v
                    .streams
                    .iter()
                    .map(|t| StreamItem { key: t.key.clone(), owner_id: t.owner.to_string(), own: t.own, connected: t.connected, viewers: t.viewers, paused: t.paused })
                    .collect(),
            }
        })
    }

    pub fn join_voice(
        &self,
        account_id: String,
        guild_id: String,
        channel_id: String,
        gesture: UserGesture,
    ) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let _guard = self.inner.rt.enter();
        if let Err(e) = s.join_voice(intent, id(&guild_id), id(&channel_id)) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    /// Start a call in a DM or group DM and ring the other side.
    pub fn start_dm_call(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let _guard = self.inner.rt.enter();
        if let Err(e) = s.start_dm_call(intent, id(&channel_id)) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    /// Join the call going on in a DM (or accept a ring). Never rings anyone.
    pub fn join_dm_call(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let _guard = self.inner.rt.enter();
        if let Err(e) = s.join_dm_call(intent, id(&channel_id)) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    /// Decline an incoming call ring.
    pub fn decline_call(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.decline_call(i, id(&channel_id)).await
        });
    }

    /// DM calls going on right now, whether or not you are in them.
    pub fn dm_calls(&self, account_id: String) -> Vec<DmCallItem> {
        self.with_store(&account_id, |s| {
            let mut chans: Vec<Id> = s.dm_voice.keys().copied().collect();
            chans.sort();
            chans
                .into_iter()
                .filter(|c| s.dm_call_ongoing(*c))
                .map(|c| {
                    let me = s.my_id();
                    let participants = s.dm_call_participants(c);
                    let ringing = s.dm_call_ringing(c);
                    let name = |u: &Id| s.display_name(None, *u);
                    DmCallItem {
                        channel_id: c.to_string(),
                        participants: participants.iter().map(|u| u.to_string()).collect(),
                        participant_names: participants.iter().map(name).collect(),
                        ringing: ringing
                            .iter()
                            .filter(|u| Some(**u) != me)
                            .map(|u| RingingUser {
                                user_id: u.to_string(),
                                name: name(u),
                                avatar_url: s
                                    .users
                                    .get(u)
                                    .map(|x| x.avatar_url(128))
                                    .unwrap_or_default(),
                            })
                            .collect(),
                        ringing_me: me.is_some_and(|m| ringing.contains(&m)),
                        joined: s.voice.channel == Some(c),
                        caller_name: participants.iter().find(|u| Some(**u) != me).map(name),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn leave_voice(&self, account_id: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let _guard = self.inner.rt.enter();
        if let Err(e) = s.leave_voice(gesture.intent()) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    pub fn set_voice_flags(
        &self,
        account_id: String,
        mute: bool,
        deaf: bool,
        gesture: UserGesture,
    ) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        if let Err(e) = s.set_voice_flags(gesture.intent(), mute, deaf) {
            self.listener.on_error(account_id, e.user_message());
        }
    }
}

#[uniffi::export]
impl UnisonEngine {
    /// Route decoded-side video to the app: remote frames and encoder control. Idempotent.
    fn ensure_video_hooks(&self, account_id: &str) {
        use unison_core::session::streams::VideoHooks;
        use unison_core::voice::stream::VideoControl;
        let Some(s) = self.inner.session(id(account_id)) else { return };
        let (l1, l2) = (self.listener.clone(), self.listener.clone());
        let (a1, a2) = (account_id.to_string(), account_id.to_string());
        s.set_video_hooks(VideoHooks {
            frames: Arc::new(move |stream, f| l1.on_remote_video(a1.clone(), stream.to_string(), f.user.to_string(), f.annexb, f.keyframe, f.pts)),
            control: Arc::new(move |source, c| {
                let (kind, value) = match c {
                    VideoControl::Keyframe => (VideoControlKind::Keyframe, 0),
                    VideoControl::Bitrate(b) => (VideoControlKind::Bitrate, b),
                };
                l2.on_video_control(a2.clone(), source.to_string(), kind, value)
            }),
        });
    }

    /// Start Go Live in the call. The capture picker and encoder start on the Swift side; frames
    /// come back through `push_video_frame` once the stream connection is up.
    pub fn start_stream(&self, account_id: String, gesture: UserGesture) {
        self.ensure_video_hooks(&account_id);
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        if let Err(e) = s.start_stream(gesture.intent()) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    pub fn watch_stream(&self, account_id: String, owner_id: String, gesture: UserGesture) {
        self.ensure_video_hooks(&account_id);
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        if let Err(e) = s.watch_stream(gesture.intent(), id(&owner_id)) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    /// Stop our stream, or stop watching one.
    pub fn stop_stream(&self, account_id: String, key: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        if let Err(e) = s.stop_stream(gesture.intent(), &key) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    pub fn set_stream_paused(&self, account_id: String, key: String, paused: bool, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        let _ = s.set_stream_paused(gesture.intent(), &key, paused);
    }

    pub fn set_camera(&self, account_id: String, on: bool, width: u32, height: u32, fps: u32, gesture: UserGesture) {
        use unison_core::voice::stream::VideoWant;
        self.ensure_video_hooks(&account_id);
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        if let Err(e) = s.set_camera(gesture.intent(), on, VideoWant { kind: "video", width, height, fps }) {
            self.listener.on_error(account_id, e.user_message());
        }
    }

    /// One encoded H.264 access unit (Annex B) from the hardware encoder.
    pub fn push_video_frame(&self, account_id: String, source: VideoSource, data: Vec<u8>, pts_90k: u32) {
        let Some(s) = self.inner.session(id(&account_id)) else { return };
        match source {
            VideoSource::Camera => s.with_call(|h| h.push_video(&data, pts_90k)),
            VideoSource::Screen => s.with_own_stream(|h| h.push_video(&data, pts_90k)),
        };
    }

    /// Whether frames from `source` have somewhere to go right now (so the encoder can idle).
    pub fn video_sink_ready(&self, account_id: String, source: VideoSource) -> bool {
        let Some(s) = self.inner.session(id(&account_id)) else { return false };
        match source {
            VideoSource::Camera => s.with_call(|h| h.sending_video()),
            VideoSource::Screen => s.with_own_stream(|h| h.sending_video()),
        }
        .unwrap_or(false)
    }
}
