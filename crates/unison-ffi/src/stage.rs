use super::*;
use unison_core::guild_events;

#[derive(uniffi::Record, Clone, Debug)]
pub struct EventItem {
    pub id: String,
    pub guild_id: String,
    pub name: String,
    pub description: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub live: bool,
    /// "stage", "voice" or "external".
    pub kind: String,
    /// Channel name for stage/voice events, or the free-text location.
    pub place: String,
    pub channel_id: Option<String>,
    pub interested: u32,
    pub i_am_interested: bool,
    pub image_url: Option<String>,
    /// I may edit, start, end or delete it.
    pub can_manage: bool,
    /// 1 scheduled, 2 live, 3 done, 4 canceled.
    pub status: u8,
    /// Who made it, when known.
    pub creator: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct StageInfo {
    pub topic: String,
    pub speakers: Vec<VoiceMember>,
    pub hands: Vec<VoiceMember>,
    pub audience_count: u32,
    /// I am a listener in this stage and can raise my hand.
    pub can_request: bool,
    pub hand_raised: bool,
    /// I may invite people to speak (Mute Members).
    pub can_moderate: bool,
}

#[uniffi::export]
impl UnisonEngine {
    /// Open scheduled events of a server, live ones first.
    pub fn guild_events(&self, account_id: String, guild_id: String) -> Vec<EventItem> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            s.guild_events(g)
                .into_iter()
                .map(|e| EventItem {
                    id: e.id.to_string(),
                    guild_id: guild_id.clone(),
                    name: e.name.clone(),
                    description: e.description.clone().unwrap_or_default(),
                    start_ms: e.start_ms(),
                    end_ms: e.end_ms(),
                    live: e.is_live(),
                    kind: match e.entity_type {
                        guild_events::entity::STAGE => "stage",
                        guild_events::entity::VOICE => "voice",
                        _ => "external",
                    }
                    .into(),
                    place: e
                        .channel_id
                        .and_then(|c| s.channels.get(&c))
                        .and_then(|c| c.name.clone())
                        .or_else(|| e.entity_metadata.as_ref().and_then(|m| m.location.clone()))
                        .unwrap_or_default(),
                    channel_id: e.channel_id.map(|c| c.to_string()),
                    interested: e.user_count.unwrap_or(0),
                    i_am_interested: s.event_interest.contains(&e.id),
                    image_url: e.image_url(),
                    can_manage: s.can_manage_event(e),
                    status: e.status,
                    creator: e.creator_id.map(|u| s.display_name(Some(g), u)).unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn set_event_interest(&self, account_id: String, guild_id: String, event_id: String, interested: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.set_event_interest(i, id(&guild_id), id(&event_id), interested).await
        });
    }

    /// The live stage in a channel, if any.
    pub fn stage_info(&self, account_id: String, guild_id: String, channel_id: String) -> Option<StageInfo> {
        self.with_store(&account_id, |s| {
            let (g, c) = (id(&guild_id), id(&channel_id));
            let inst = s.stages.get(&c)?;
            let (speakers, hands, audience) = s.stage_roles(g, c);
            let me = s.my_id();
            let member = |v: &model::VoiceState| VoiceMember {
                user_id: v.user_id.to_string(),
                name: s.display_name(Some(g), v.user_id),
                avatar_url: s.users.get(&v.user_id).map(|u| u.avatar_url(64)).unwrap_or_default(),
                muted: v.self_mute || v.mute,
                deafened: v.self_deaf || v.deaf,
                server_muted: v.mute || v.deaf,
                video: v.self_video,
                streaming: v.self_stream.unwrap_or(false),
                speaking: s.voice.channel == Some(c) && s.voice.speaking.contains(&v.user_id),
            };
            let in_stage = s.voice.channel == Some(c);
            let my_hand = me.is_some_and(|m| hands.iter().any(|v| v.user_id == m));
            let i_listen = me.is_some_and(|m| audience.iter().any(|v| v.user_id == m) || my_hand);
            Some(StageInfo {
                topic: inst.topic.clone(),
                speakers: speakers.iter().map(|v| member(v)).collect(),
                hands: hands.iter().map(|v| member(v)).collect(),
                audience_count: audience.len() as u32,
                can_request: in_stage && i_listen,
                hand_raised: my_hand,
                can_moderate: s.channels.get(&c).is_some_and(|ch| s.permissions(ch) & unison_core::store::perm::MUTE_MEMBERS != 0),
            })
        })
        .flatten()
    }

    pub fn stage_hand(&self, account_id: String, guild_id: String, channel_id: String, raise: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.stage_hand(i, id(&guild_id), id(&channel_id), raise).await
        });
    }

    pub fn stage_set_speaker(&self, account_id: String, guild_id: String, channel_id: String, user_id: String, speaker: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.stage_set_speaker(i, id(&guild_id), id(&channel_id), id(&user_id), speaker).await
        });
    }

    /// May I schedule events in this server?
    pub fn can_create_events(&self, account_id: String, guild_id: String) -> bool {
        self.with_store(&account_id, |s| s.can_create_events(id(&guild_id))).unwrap_or(false)
    }

    /// Voice and stage channels an event can take place in.
    pub fn event_channels(&self, account_id: String, guild_id: String) -> Vec<EventChannel> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let mut v: Vec<&model::Channel> = s
                .guild_channels
                .get(&g)
                .into_iter()
                .flatten()
                .filter_map(|c| s.channels.get(c))
                .filter(|c| matches!(c.kind, model::channel_type::GUILD_VOICE | model::channel_type::GUILD_STAGE_VOICE))
                .collect();
            v.sort_by_key(|c| (c.kind == model::channel_type::GUILD_VOICE, c.position, c.id));
            v.into_iter().map(|c| EventChannel { id: c.id.to_string(), name: c.name.clone().unwrap_or_default(), stage: c.kind == model::channel_type::GUILD_STAGE_VOICE }).collect()
        })
        .unwrap_or_default()
    }

    /// Why the event form can't be saved yet; `None` when it can. `start_changed` is false when
    /// editing an event and leaving its start time alone.
    pub fn event_problem(&self, draft: EventDraftItem, start_changed: bool) -> Option<String> {
        draft.to_core().check(unison_core::timefmt::now_ms(), start_changed)
    }

    /// Create (`event_id` of `None`) or edit an event.
    pub fn save_event(&self, account_id: String, guild_id: String, event_id: Option<String>, draft: EventDraftItem, cover: CoverChange, gesture: UserGesture) {
        let image = match cover {
            CoverChange::Keep => None,
            CoverChange::Remove => Some(None),
            CoverChange::Set { content_type, bytes } => Some(Some((content_type, bytes))),
        };
        let d = draft.to_core();
        self.spawn_write(account_id, gesture, move |s, i| async move { s.save_event(i, id(&guild_id), event_id.as_deref().map(id), d, image).await });
    }

    /// 2 starts it now, 3 ends it, 4 cancels it.
    pub fn set_event_status(&self, account_id: String, guild_id: String, event_id: String, status: u8, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.set_event_status(i, id(&guild_id), id(&event_id), status).await });
    }

    pub fn delete_event(&self, account_id: String, guild_id: String, event_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.delete_event(i, id(&guild_id), id(&event_id)).await });
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct EventChannel {
    pub id: String,
    pub name: String,
    pub stage: bool,
}

/// What the event form holds. `kind` is 1 stage, 2 voice, 3 somewhere else.
#[derive(uniffi::Record, Clone, Debug)]
pub struct EventDraftItem {
    pub name: String,
    pub description: String,
    pub kind: u8,
    pub channel_id: Option<String>,
    pub location: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

impl EventDraftItem {
    fn to_core(&self) -> guild_events::EventDraft {
        guild_events::EventDraft {
            name: self.name.clone(),
            description: self.description.clone(),
            kind: self.kind,
            channel_id: self.channel_id.as_deref().map(id),
            location: self.location.clone(),
            start_ms: self.start_ms,
            end_ms: self.end_ms,
        }
    }
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum CoverChange {
    Keep,
    Remove,
    Set { content_type: String, bytes: Vec<u8> },
}
