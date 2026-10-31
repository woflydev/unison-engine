use super::*;

#[derive(uniffi::Enum, Clone, Debug)]
pub enum UiDelta {
    /// A message in this channel started, progressed, finished or failed sending (`sends`).
    Sends { channel_id: String },
    Ready,
    Guilds,
    Channels {
        guild_id: Option<String>,
    },
    PrivateChannels,
    /// A server's scheduled events changed.
    GuildEvents {
        guild_id: String,
    },
    EmojiPrefs,
    Messages {
        channel_id: String,
    },
    MessageInserted {
        channel_id: String,
        message_id: String,
    },
    ReadState {
        channel_id: String,
    },
    Typing {
        channel_id: String,
    },
    MemberList {
        guild_id: String,
    },
    /// Members, emoji or stickers arrived: re-run composer suggestions.
    MembersFound {
        guild_id: String,
    },
    Relationships,
    Mentioned {
        channel_id: String,
        message_id: String,
    },
    OwnStatus,
    /// Someone's status or activity changed.
    Presence {
        user_id: String,
    },
    /// A forum's posts or previews changed.
    Forum {
        channel_id: String,
    },
    /// Voice channel occupancy changed in a guild.
    VoiceStates {
        guild_id: String,
    },
    /// Our call changed (status, encryption, mute, speaking).
    Voice,
    /// A DM call is ringing you.
    IncomingCall {
        channel_id: String,
    },
    /// User settings (preloaded proto, account info, per-server notification levels) changed.
    Settings,
    /// An application opened a modal (`pending_modal`).
    Modal,
    /// Autocomplete choices arrived for the request with this nonce (`slash_autocomplete_result`).
    Autocomplete { nonce: String },
}

pub(crate) fn ui_delta(d: &Delta) -> Option<UiDelta> {
    Some(match d {
        Delta::Ready => UiDelta::Ready,
        Delta::Sends { channel } => UiDelta::Sends { channel_id: channel.to_string() },
        Delta::GuildsChanged => UiDelta::Guilds,
        Delta::ChannelsChanged { guild } => UiDelta::Channels {
            guild_id: guild.map(|g| g.to_string()),
        },
        Delta::PrivateChannelsChanged => UiDelta::PrivateChannels,
        Delta::GuildEvents { guild } => UiDelta::GuildEvents { guild_id: guild.to_string() },
        Delta::EmojiPrefs => UiDelta::EmojiPrefs,
        Delta::MessagesReset { channel }
        | Delta::MessageUpdated { channel, .. }
        | Delta::MessageDeleted { channel, .. } => UiDelta::Messages {
            channel_id: channel.to_string(),
        },
        Delta::MessageInserted { channel, message } => UiDelta::MessageInserted {
            channel_id: channel.to_string(),
            message_id: message.to_string(),
        },
        Delta::ReadState { channel } => UiDelta::ReadState {
            channel_id: channel.to_string(),
        },
        Delta::Typing { channel } => UiDelta::Typing {
            channel_id: channel.to_string(),
        },
        Delta::MemberList { guild } => UiDelta::MemberList {
            guild_id: guild.to_string(),
        },
        Delta::MembersFound { guild } => UiDelta::MembersFound {
            guild_id: guild.to_string(),
        },
        Delta::UserUpdated { user } => UiDelta::Presence { user_id: user.to_string() },
        Delta::Relationships => UiDelta::Relationships,
        Delta::OwnStatus => UiDelta::OwnStatus,
        Delta::SettingsChanged => UiDelta::Settings,
        Delta::Mentioned { channel, message } => UiDelta::Mentioned {
            channel_id: channel.to_string(),
            message_id: message.to_string(),
        },
        Delta::ForumChanged { channel } => UiDelta::Forum {
            channel_id: channel.to_string(),
        },
        Delta::VoiceStates { guild } => UiDelta::VoiceStates {
            guild_id: guild.to_string(),
        },
        Delta::Voice => UiDelta::Voice,
        Delta::Modal => UiDelta::Modal,
        Delta::Autocomplete { nonce } => UiDelta::Autocomplete { nonce: nonce.to_string() },
        Delta::IncomingCall { channel } => UiDelta::IncomingCall {
            channel_id: channel.to_string(),
        },
    })
}
