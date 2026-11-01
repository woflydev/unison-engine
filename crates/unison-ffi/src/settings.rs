use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct PrivacyCategory {
    pub key: String,
    pub title: String,
    pub detail: String,
    pub enabled: bool,
}

pub(crate) fn describe(c: Category) -> (&'static str, &'static str) {
    match c {
        Category::Analytics => (
            "Usage analytics",
            "Discord's /science events, metrics and client heartbeats.",
        ),
        Category::CrashReports => ("Crash reports", "Error reports sent to Sentry."),
        Category::ExperimentExposure => (
            "Experiment reporting",
            "Reports which A/B experiments you saw. Experiments still load.",
        ),
        Category::ActivityDetection => (
            "Game activity",
            "Detect running games and show them as your status.",
        ),
        Category::Quests => (
            "Quests",
            "Quest banners, the Quests page and quest progress tracking.",
        ),
        Category::Ads => ("Sponsored content", "Ads and sponsored placements."),
        Category::Upsells => (
            "Nitro & shop promotions",
            "Upsell popups, shop promos, coachmarks and NEW badges.",
        ),
        Category::Nags => (
            "Announcements & surveys",
            "What's-new modals, surveys and feature tours.",
        ),
    }
}

/// Preloaded user settings the settings window reads and writes. For writes only the `Some` fields are sent.
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct UserPrefs {
    pub theme: Option<u32>,
    pub timestamp_hour_cycle: Option<u32>,
    pub inline_attachment_media: Option<bool>,
    pub inline_embed_media: Option<bool>,
    pub gif_auto_play: Option<bool>,
    pub render_embeds: Option<bool>,
    pub render_reactions: Option<bool>,
    pub animate_emoji: Option<bool>,
    pub animate_stickers: Option<u32>,
    pub message_display_compact: Option<bool>,
    pub explicit_content_filter: Option<u32>,
    pub convert_emoticons: Option<bool>,
    pub dm_spam_filter_v2: Option<u32>,
    pub show_in_app_notifications: Option<bool>,
    pub enable_burst_reaction_notifications: Option<bool>,
    pub reaction_notifications: Option<u32>,
    pub default_guilds_restricted: Option<bool>,
    pub friend_source_flags: Option<u32>,
    pub default_message_request_restricted: Option<bool>,
    pub status: Option<String>,
    pub show_current_game: Option<bool>,
    pub locale: Option<String>,
    pub view_nsfw_guilds: Option<bool>,
    pub view_nsfw_commands: Option<bool>,
    pub afk_timeout: Option<u32>,
    pub stream_notifications_enabled: Option<bool>,
    pub allow_activity_party_friends: Option<bool>,
    pub allow_activity_party_voice: Option<bool>,
    pub inappropriate_conversation_warnings: Option<bool>,
    pub recent_games_enabled: Option<bool>,
    pub show_local_time: Option<bool>,
    /// Custom status (read: set when one is active; write: set text to change it).
    pub custom_status_text: Option<String>,
    pub custom_status_emoji: Option<String>,
    pub custom_status_expires_ms: Option<u64>,
    /// Write only: remove the custom status.
    pub clear_custom_status: bool,
}

impl UserPrefs {
    fn from_core(p: &unison_core::prefs::Prefs) -> Self {
        let cs = p
            .custom_status
            .as_ref()
            .filter(|c| unison_core::prefs::custom_status_active(c, now_ms()));
        UserPrefs {
            theme: p.theme,
            timestamp_hour_cycle: p.timestamp_hour_cycle,
            inline_attachment_media: p.inline_attachment_media,
            inline_embed_media: p.inline_embed_media,
            gif_auto_play: p.gif_auto_play,
            render_embeds: p.render_embeds,
            render_reactions: p.render_reactions,
            animate_emoji: p.animate_emoji,
            animate_stickers: p.animate_stickers,
            message_display_compact: p.message_display_compact,
            explicit_content_filter: p.explicit_content_filter,
            convert_emoticons: p.convert_emoticons,
            dm_spam_filter_v2: p.dm_spam_filter_v2,
            show_in_app_notifications: p.show_in_app_notifications,
            enable_burst_reaction_notifications: p.enable_burst_reaction_notifications,
            reaction_notifications: p.reaction_notifications,
            default_guilds_restricted: p.default_guilds_restricted,
            friend_source_flags: p.friend_source_flags,
            default_message_request_restricted: p.default_message_request_restricted,
            status: p.status.clone(),
            show_current_game: p.show_current_game,
            locale: p.locale.clone(),
            view_nsfw_guilds: p.view_nsfw_guilds,
            view_nsfw_commands: p.view_nsfw_commands,
            afk_timeout: p.afk_timeout,
            stream_notifications_enabled: p.stream_notifications_enabled,
            allow_activity_party_friends: p.allow_activity_party_friends,
            allow_activity_party_voice: p.allow_activity_party_voice,
            inappropriate_conversation_warnings: p.inappropriate_conversation_warnings,
            recent_games_enabled: p.recent_games_enabled,
            show_local_time: p.show_local_time,
            custom_status_text: cs.map(|c| c.text.clone()),
            custom_status_emoji: cs.map(|c| c.emoji_name.clone()).filter(|e| !e.is_empty()),
            custom_status_expires_ms: cs.map(|c| c.expires_at_ms).filter(|e| *e != 0),
            clear_custom_status: false,
        }
    }

    fn to_core(&self) -> unison_core::prefs::Prefs {
        use unison_core::prefs::{CustomStatus, Prefs};
        let custom_status = if self.clear_custom_status {
            Some(CustomStatus::default())
        } else {
            self.custom_status_text.as_ref().map(|t| CustomStatus {
                text: t.clone(),
                emoji_name: self.custom_status_emoji.clone().unwrap_or_default(),
                expires_at_ms: self.custom_status_expires_ms.unwrap_or(0),
                ..Default::default()
            })
        };
        // The web client stamps status_created_at_ms whenever it writes the status.
        let status_created_at_ms = self.status.as_ref().map(|_| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
        Prefs {
            theme: self.theme,
            timestamp_hour_cycle: self.timestamp_hour_cycle,
            inline_attachment_media: self.inline_attachment_media,
            inline_embed_media: self.inline_embed_media,
            gif_auto_play: self.gif_auto_play,
            render_embeds: self.render_embeds,
            render_reactions: self.render_reactions,
            animate_emoji: self.animate_emoji,
            animate_stickers: self.animate_stickers,
            message_display_compact: self.message_display_compact,
            explicit_content_filter: self.explicit_content_filter,
            convert_emoticons: self.convert_emoticons,
            dm_spam_filter_v2: self.dm_spam_filter_v2,
            show_in_app_notifications: self.show_in_app_notifications,
            enable_burst_reaction_notifications: self.enable_burst_reaction_notifications,
            reaction_notifications: self.reaction_notifications,
            default_guilds_restricted: self.default_guilds_restricted,
            friend_source_flags: self.friend_source_flags,
            default_message_request_restricted: self.default_message_request_restricted,
            status: self.status.clone(),
            custom_status,
            show_current_game: self.show_current_game,
            status_created_at_ms,
            locale: self.locale.clone(),
            view_nsfw_guilds: self.view_nsfw_guilds,
            view_nsfw_commands: self.view_nsfw_commands,
            afk_timeout: self.afk_timeout,
            stream_notifications_enabled: self.stream_notifications_enabled,
            allow_activity_party_friends: self.allow_activity_party_friends,
            allow_activity_party_voice: self.allow_activity_party_voice,
            inappropriate_conversation_warnings: self.inappropriate_conversation_warnings,
            recent_games_enabled: self.recent_games_enabled,
            show_local_time: self.show_local_time,
        }
    }
}

/// Account facts from READY, shown read-only (changing them needs a password or 2FA on Discord).
#[derive(uniffi::Record, Clone, Debug)]
pub struct UserAccountInfo {
    pub user_id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub verified: bool,
    pub mfa_enabled: bool,
    pub avatar_url: String,
}

/// Editable raw profile fields from `GET /users/{id}/profile`.
#[derive(uniffi::Record, Clone, Debug)]
pub struct OwnProfile {
    pub bio: String,
    pub pronouns: String,
    pub accent_color: Option<u32>,
    pub banner_url: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct GuildNotifItem {
    pub guild_id: String,
    pub name: String,
    /// 0 all messages, 1 only @mentions, 2 nothing, 3 server default.
    pub level: u8,
    pub muted: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChannelOverrideItem {
    pub channel_id: String,
    pub name: String,
    pub muted: bool,
    /// 0 all messages, 1 only mentions, 2 nothing, 3 use the category or server default.
    pub level: u8,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct KeybindItem {
    pub id: String,
    pub group: String,
    pub title: String,
    /// Platform-neutral tokens joined with `+` (`mod+shift+i`).
    pub keys: String,
}

/// Every shortcut the app ships, in display order.
#[uniffi::export]
pub fn keybind_table() -> Vec<KeybindItem> {
    unison_core::keybinds::TABLE
        .iter()
        .map(|b| KeybindItem { id: b.id.into(), group: b.group.into(), title: b.title.into(), keys: b.keys.into() })
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Epoch ms at which a custom status set now clears (`never`, `30m`, `1h`, `4h`, `today`); 0 = never.
#[uniffi::export]
pub fn custom_status_expiry(choice: String, utc_offset_secs: i32) -> u64 {
    unison_core::prefs::StatusExpiry::parse(&choice).map(|e| e.at_ms(now_ms(), utc_offset_secs)).unwrap_or(0)
}

/// An all-`None` patch to fill in and pass to `update_prefs`.
#[uniffi::export]
pub fn blank_user_prefs() -> UserPrefs {
    UserPrefs::default()
}

#[uniffi::export]
impl UnisonEngine {
    pub fn privacy_categories(&self) -> Vec<PrivacyCategory> {
        let p = self.inner.policy.read();
        Category::ALL
            .iter()
            .map(|c| {
                let (title, detail) = describe(*c);
                PrivacyCategory {
                    key: c.label().into(),
                    title: title.into(),
                    detail: detail.into(),
                    enabled: p.allows(*c),
                }
            })
            .collect()
    }

    pub fn set_privacy_category(&self, key: String, enabled: bool) {
        let Some(c) = Category::ALL.iter().find(|c| c.label() == key) else {
            return;
        };
        let mut p = self.inner.policy.write();
        p.set(*c, enabled);
        let _ = std::fs::write(
            &self.policy_path,
            serde_json::to_vec(&*p).unwrap_or_default(),
        );
    }

    pub fn is_ui_allowed(&self, key: String) -> bool {
        Category::ALL
            .iter()
            .find(|c| c.label() == key)
            .is_some_and(|c| self.inner.policy.read().allows(*c))
    }

    pub fn clean_url(&self, url: String) -> String {
        privacy::clean_outbound_url(&url)
    }

    /// Diagnostics: gateway events this build doesn't model yet (watcher input).
    pub fn unknown_events(&self, account_id: String) -> Vec<String> {
        self.with_store(&account_id, |s| {
            let mut v: Vec<_> = s
                .unknown_events
                .iter()
                .map(|(k, n)| format!("{k} ×{n}"))
                .collect();
            v.sort();
            v
        })
        .unwrap_or_default()
    }

    pub fn profile_summary(&self) -> String {
        let p = &self.inner.profile;
        format!(
            "Discord {} · build {} · Electron {} · profile #{}",
            p.client_version, p.client_build_number, p.electron_version, p.serial
        )
    }

    pub fn user_prefs(&self, account_id: String) -> UserPrefs {
        self.with_store(&account_id, |s| UserPrefs::from_core(&s.prefs))
            .unwrap_or_default()
    }

    /// Writes the `Some` fields via `PATCH /users/@me/settings-proto/1`.
    pub fn update_prefs(&self, account_id: String, patch: UserPrefs, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.update_prefs(i, patch.to_core()).await
        });
    }

    pub fn user_account_info(&self, account_id: String) -> Option<UserAccountInfo> {
        self.with_store(&account_id, |s| {
            s.me.as_ref().map(|u| UserAccountInfo {
                user_id: u.id.to_string(),
                username: u.username.clone(),
                global_name: u.global_name.clone(),
                email: u.email.clone(),
                phone: u.phone.clone(),
                verified: u.verified,
                mfa_enabled: u.mfa_enabled,
                avatar_url: u.avatar_url(128),
            })
        })
        .flatten()
    }

    pub fn fetch_own_profile(&self, account_id: String) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let Some(me) = s.store.read().my_id() else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            if let Ok(v) = s.profile(me, None).await {
                let str_of = |x: &serde_json::Value| x.as_str().unwrap_or_default().to_string();
                let up = &v["user_profile"];
                let banner = v["user"]["banner"]
                    .as_str()
                    .map(|b| format!("https://cdn.discordapp.com/banners/{me}/{b}.png?size=600"));
                listener.on_own_profile(
                    account_id,
                    OwnProfile {
                        bio: str_of(&up["bio"]).trim().to_string(),
                        pronouns: str_of(&up["pronouns"]),
                        accent_color: v["user"]["accent_color"].as_u64().map(|c| c as u32),
                        banner_url: banner,
                    },
                );
            }
        });
    }

    /// `PATCH /users/@me { global_name }` (empty clears it).
    pub fn update_display_name(&self, account_id: String, name: String, gesture: UserGesture) {
        let value = if name.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(name)
        };
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.update_me(i, serde_json::json!({ "global_name": value }))
                .await
        });
    }

    /// `PATCH /users/@me/profile { bio, pronouns }`; only the `Some` fields are sent.
    pub fn update_profile_text(
        &self,
        account_id: String,
        bio: Option<String>,
        pronouns: Option<String>,
        gesture: UserGesture,
    ) {
        let mut body = serde_json::Map::new();
        if let Some(b) = bio {
            body.insert("bio".into(), b.into());
        }
        if let Some(p) = pronouns {
            body.insert("pronouns".into(), p.into());
        }
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.update_profile(i, serde_json::Value::Object(body)).await
        });
    }

    /// Channels in a server that have their own mute or level, for the Notifications pane.
    pub fn channel_notification_overrides(&self, account_id: String, guild_id: String) -> Vec<ChannelOverrideItem> {
        let g = id(&guild_id);
        self.with_store(&account_id, |s| {
            let mut v: Vec<ChannelOverrideItem> = s
                .guild_settings
                .get(&g)
                .into_iter()
                .flat_map(|x| x.channel_overrides.iter())
                .filter(|o| o.muted || o.message_notifications != 3)
                .filter_map(|o| {
                    let c = s.channels.get(&o.channel_id)?;
                    Some(ChannelOverrideItem {
                        channel_id: o.channel_id.to_string(),
                        name: c.name.clone().unwrap_or_default(),
                        muted: o.muted,
                        level: o.message_notifications.min(3),
                    })
                })
                .collect();
            v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            v
        })
        .unwrap_or_default()
    }

    pub fn guild_notification_levels(&self, account_id: String) -> Vec<GuildNotifItem> {
        self.with_store(&account_id, |s| {
            let mut v: Vec<GuildNotifItem> = s
                .guilds
                .values()
                .map(|g| {
                    let gs = s.guild_settings.get(&g.id);
                    GuildNotifItem {
                        guild_id: g.id.to_string(),
                        name: g.name().to_string(),
                        level: gs
                            .map(|x| x.message_notifications)
                            .filter(|l| *l <= 3)
                            .unwrap_or(3),
                        muted: gs.map(|x| x.muted).unwrap_or(false),
                    }
                })
                .collect();
            v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            v
        })
        .unwrap_or_default()
    }

    /// `PATCH /users/@me/guilds/{id}/settings { message_notifications }`.
    pub fn set_guild_notification_level(
        &self,
        account_id: String,
        guild_id: String,
        level: u8,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.update_guild_settings(
                i,
                id(&guild_id),
                serde_json::json!({ "message_notifications": level }),
            )
            .await
        });
    }
}
