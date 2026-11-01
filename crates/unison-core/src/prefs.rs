//! The slice of `PreloadedUserSettings` (settings-proto type 1) that Unison reads and writes.
//!
//! Field numbers, wrapper types and enum values were read from the official web client bundle
//! (see docs/FIDELITY.md, "User settings"). Writes are partial messages: only the fields that are
//! `Some` are encoded, and the server merges them, exactly like the web client's `updateAsync`.

/// `CustomStatus` (StatusSettings.custom_status = 2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CustomStatus {
    pub text: String,
    pub emoji_id: u64,
    pub emoji_name: String,
    pub expires_at_ms: u64,
}

/// When a custom status clears itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusExpiry {
    Never,
    Minutes(u32),
    /// At the next local midnight.
    Today,
}

impl StatusExpiry {
    /// `never`, `30m`, `1h`, `4h`, `today` (the official client's choices).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "never" => Self::Never,
            "30m" => Self::Minutes(30),
            "1h" => Self::Minutes(60),
            "4h" => Self::Minutes(240),
            "today" => Self::Today,
            _ => return None,
        })
    }

    /// Absolute expiry in epoch milliseconds, `0` for never. `utc_offset_secs` is the user's local offset.
    pub fn at_ms(self, now_ms: u64, utc_offset_secs: i32) -> u64 {
        match self {
            Self::Never => 0,
            Self::Minutes(m) => now_ms + u64::from(m) * 60_000,
            Self::Today => {
                let local = now_ms as i64 + i64::from(utc_offset_secs) * 1000;
                let midnight_local = (local.div_euclid(86_400_000) + 1) * 86_400_000;
                (midnight_local - i64::from(utc_offset_secs) * 1000) as u64
            }
        }
    }
}

/// Whether a custom status is still showing at `now_ms` (an expired one is gone for everyone).
pub fn custom_status_active(c: &CustomStatus, now_ms: u64) -> bool {
    (!c.text.is_empty() || !c.emoji_name.is_empty()) && (c.expires_at_ms == 0 || c.expires_at_ms > now_ms)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Prefs {
    // appearance (13)
    pub theme: Option<u32>,
    pub timestamp_hour_cycle: Option<u32>,
    // text_and_images (6)
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
    // notifications (7)
    pub show_in_app_notifications: Option<bool>,
    pub enable_burst_reaction_notifications: Option<bool>,
    pub reaction_notifications: Option<u32>,
    // privacy (8)
    pub default_guilds_restricted: Option<bool>,
    pub friend_source_flags: Option<u32>,
    pub default_message_request_restricted: Option<bool>,
    // status (11)
    pub status: Option<String>,
    pub custom_status: Option<CustomStatus>,
    pub show_current_game: Option<bool>,
    pub status_created_at_ms: Option<u64>,
    // localization (12)
    pub locale: Option<String>,
    // text_and_images, more (6)
    pub view_nsfw_guilds: Option<bool>,
    pub view_nsfw_commands: Option<bool>,
    // voice_and_video (5)
    pub afk_timeout: Option<u32>,
    pub stream_notifications_enabled: Option<bool>,
    // privacy, more (8)
    pub allow_activity_party_friends: Option<bool>,
    pub allow_activity_party_voice: Option<bool>,
    pub inappropriate_conversation_warnings: Option<bool>,
    pub recent_games_enabled: Option<bool>,
    pub show_local_time: Option<bool>,
}

// ----- wire helpers -----

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_tag(out: &mut Vec<u8>, num: u64, wire: u64) {
    put_varint(out, (num << 3) | wire);
}

fn put_msg(out: &mut Vec<u8>, num: u64, body: &[u8]) {
    put_tag(out, num, 2);
    put_varint(out, body.len() as u64);
    out.extend_from_slice(body);
}

/// A `google.protobuf.*Value` wrapper holding a varint (Bool, UInt32, Int64, UInt64). The zero
/// value is an empty wrapper, as protobuf-ts writes it.
fn put_wrapped_varint(out: &mut Vec<u8>, num: u64, v: u64) {
    let mut w = Vec::new();
    if v != 0 {
        put_tag(&mut w, 1, 0);
        put_varint(&mut w, v);
    }
    put_msg(out, num, &w);
}

fn put_wrapped_string(out: &mut Vec<u8>, num: u64, s: &str) {
    let mut w = Vec::new();
    if !s.is_empty() {
        put_msg(&mut w, 1, s.as_bytes());
    }
    put_msg(out, num, &w);
}

fn put_enum(out: &mut Vec<u8>, num: u64, v: u32) {
    if v != 0 {
        put_tag(out, num, 0);
        put_varint(out, v as u64);
    }
}

fn put_fixed64(out: &mut Vec<u8>, num: u64, v: u64) {
    if v != 0 {
        put_tag(out, num, 1);
        out.extend_from_slice(&v.to_le_bytes());
    }
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

enum Val<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Skip,
}

impl<'a> Rd<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.p)?;
            self.p += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    /// `Some(None)` at the end, `None` if malformed.
    fn next(&mut self) -> Option<Option<(u64, Val<'a>)>> {
        if self.p >= self.b.len() {
            return Some(None);
        }
        let tag = self.varint()?;
        let v = match tag & 7 {
            0 => Val::Varint(self.varint()?),
            1 => {
                let s = self.b.get(self.p..self.p + 8)?;
                self.p += 8;
                Val::Fixed64(u64::from_le_bytes(s.try_into().ok()?))
            }
            2 => {
                let n = self.varint()? as usize;
                let s = self.b.get(self.p..self.p.checked_add(n)?)?;
                self.p += n;
                Val::Bytes(s)
            }
            5 => {
                self.p += 4;
                Val::Skip
            }
            _ => return None,
        };
        Some(Some((tag >> 3, v)))
    }
}

fn each(b: &[u8], mut f: impl FnMut(u64, Val)) -> Option<()> {
    let mut r = Rd { b, p: 0 };
    while let Some((n, v)) = r.next()? {
        f(n, v);
    }
    Some(())
}

/// Value of a wrapper message (`{ value = 1 }`); a present-but-empty wrapper is the zero value.
fn wrapped_varint(b: &[u8]) -> u64 {
    let mut out = 0;
    let _ = each(b, |n, v| {
        if let (1, Val::Varint(x)) = (n, v) {
            out = x;
        }
    });
    out
}

fn wrapped_string(b: &[u8]) -> String {
    let mut out = String::new();
    let _ = each(b, |n, v| {
        if let (1, Val::Bytes(s)) = (n, v) {
            out = String::from_utf8_lossy(s).into_owned();
        }
    });
    out
}

fn custom_status(b: &[u8]) -> CustomStatus {
    let mut c = CustomStatus::default();
    let _ = each(b, |n, v| match (n, v) {
        (1, Val::Bytes(s)) => c.text = String::from_utf8_lossy(s).into_owned(),
        (2, Val::Fixed64(x)) => c.emoji_id = x,
        (3, Val::Bytes(s)) => c.emoji_name = String::from_utf8_lossy(s).into_owned(),
        (4, Val::Fixed64(x)) => c.expires_at_ms = x,
        _ => {}
    });
    c
}

impl Prefs {
    /// Decode the whole preloaded blob (unknown fields ignored). `None` if malformed.
    pub fn decode(blob: &[u8]) -> Option<Prefs> {
        let mut p = Prefs::default();
        each(blob, |sec, v| {
            let Val::Bytes(b) = v else { return };
            let _ = match sec {
                5 => each(b, |n, v| p.voice_field(n, v)),
                6 => each(b, |n, v| p.text_field(n, v)),
                7 => each(b, |n, v| p.notif_field(n, v)),
                8 => each(b, |n, v| p.privacy_field(n, v)),
                11 => each(b, |n, v| p.status_field(n, v)),
                12 => each(b, |n, v| {
                    if let (1, Val::Bytes(s)) = (n, v) {
                        p.locale = Some(wrapped_string(s));
                    }
                }),
                13 => each(b, |n, v| match (n, v) {
                    (1, Val::Varint(x)) => p.theme = Some(x as u32),
                    (9, Val::Varint(x)) => p.timestamp_hour_cycle = Some(x as u32),
                    _ => {}
                }),
                _ => Some(()),
            };
        })?;
        Some(p)
    }

    fn text_field(&mut self, n: u64, v: Val) {
        let w = |v: Val| if let Val::Bytes(s) = v { Some(wrapped_varint(s)) } else { None };
        match n {
            9 => self.inline_attachment_media = w(v).map(|x| x != 0),
            10 => self.inline_embed_media = w(v).map(|x| x != 0),
            11 => self.gif_auto_play = w(v).map(|x| x != 0),
            12 => self.render_embeds = w(v).map(|x| x != 0),
            13 => self.render_reactions = w(v).map(|x| x != 0),
            14 => self.animate_emoji = w(v).map(|x| x != 0),
            15 => self.animate_stickers = w(v).map(|x| x as u32),
            17 => self.message_display_compact = w(v).map(|x| x != 0),
            19 => self.explicit_content_filter = w(v).map(|x| x as u32),
            20 => self.view_nsfw_guilds = w(v).map(|x| x != 0),
            23 => self.view_nsfw_commands = w(v).map(|x| x != 0),
            21 => self.convert_emoticons = w(v).map(|x| x != 0),
            27 => {
                if let Val::Varint(x) = v {
                    self.dm_spam_filter_v2 = Some(x as u32)
                }
            }
            _ => {}
        }
    }

    fn voice_field(&mut self, n: u64, v: Val) {
        match (n, v) {
            (6, Val::Bytes(s)) => self.afk_timeout = Some(wrapped_varint(s) as u32),
            (7, Val::Bytes(s)) => self.stream_notifications_enabled = Some(wrapped_varint(s) != 0),
            _ => {}
        }
    }

    fn notif_field(&mut self, n: u64, v: Val) {
        match (n, v) {
            (1, Val::Bytes(s)) => self.show_in_app_notifications = Some(wrapped_varint(s) != 0),
            (4, Val::Bytes(s)) => self.enable_burst_reaction_notifications = Some(wrapped_varint(s) != 0),
            (7, Val::Varint(x)) => self.reaction_notifications = Some(x as u32),
            _ => {}
        }
    }

    fn privacy_field(&mut self, n: u64, v: Val) {
        match (n, v) {
            (1, Val::Bytes(s)) => self.allow_activity_party_friends = Some(wrapped_varint(s) != 0),
            (2, Val::Bytes(s)) => self.allow_activity_party_voice = Some(wrapped_varint(s) != 0),
            (4, Val::Varint(x)) => self.default_guilds_restricted = Some(x != 0),
            (23, Val::Bytes(s)) => self.inappropriate_conversation_warnings = Some(wrapped_varint(s) != 0),
            (24, Val::Bytes(s)) => self.recent_games_enabled = Some(wrapped_varint(s) != 0),
            (31, Val::Bytes(s)) => self.show_local_time = Some(wrapped_varint(s) != 0),
            (11, Val::Bytes(s)) => self.friend_source_flags = Some(wrapped_varint(s) as u32),
            (17, Val::Bytes(s)) => self.default_message_request_restricted = Some(wrapped_varint(s) != 0),
            _ => {}
        }
    }

    fn status_field(&mut self, n: u64, v: Val) {
        match (n, v) {
            (1, Val::Bytes(s)) => self.status = Some(wrapped_string(s)),
            (2, Val::Bytes(s)) => self.custom_status = Some(custom_status(s)),
            (3, Val::Bytes(s)) => self.show_current_game = Some(wrapped_varint(s) != 0),
            (5, Val::Bytes(s)) => self.status_created_at_ms = Some(wrapped_varint(s)),
            _ => {}
        }
    }

    /// Fold a partial settings message (a gateway update or a PATCH response) into `self`.
    pub fn merge(&mut self, newer: &Prefs) {
        macro_rules! m {
            ($($f:ident),*) => { $( if newer.$f.is_some() { self.$f = newer.$f.clone(); } )* }
        }
        m!(
            theme, timestamp_hour_cycle, inline_attachment_media, inline_embed_media, gif_auto_play, render_embeds,
            render_reactions, animate_emoji, animate_stickers, message_display_compact, explicit_content_filter,
            convert_emoticons, dm_spam_filter_v2, show_in_app_notifications, enable_burst_reaction_notifications,
            reaction_notifications, default_guilds_restricted, friend_source_flags, default_message_request_restricted,
            status, custom_status, show_current_game, status_created_at_ms, locale, view_nsfw_guilds,
            view_nsfw_commands, afk_timeout, stream_notifications_enabled, allow_activity_party_friends,
            allow_activity_party_voice, inappropriate_conversation_warnings, recent_games_enabled, show_local_time
        );
    }

    /// Encode the `Some` fields as a partial PreloadedUserSettings: the body of
    /// `PATCH /users/@me/settings-proto/1` after base64. Empty when nothing is set.
    pub fn encode_patch(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let flag = |v: bool| v as u64;

        let mut vv = Vec::new();
        if let Some(v) = self.afk_timeout { put_wrapped_varint(&mut vv, 6, v as u64) }
        if let Some(v) = self.stream_notifications_enabled { put_wrapped_varint(&mut vv, 7, flag(v)) }
        if !vv.is_empty() { put_msg(&mut out, 5, &vv) }

        let mut t = Vec::new();
        if let Some(v) = self.inline_attachment_media { put_wrapped_varint(&mut t, 9, flag(v)) }
        if let Some(v) = self.inline_embed_media { put_wrapped_varint(&mut t, 10, flag(v)) }
        if let Some(v) = self.gif_auto_play { put_wrapped_varint(&mut t, 11, flag(v)) }
        if let Some(v) = self.render_embeds { put_wrapped_varint(&mut t, 12, flag(v)) }
        if let Some(v) = self.render_reactions { put_wrapped_varint(&mut t, 13, flag(v)) }
        if let Some(v) = self.animate_emoji { put_wrapped_varint(&mut t, 14, flag(v)) }
        if let Some(v) = self.animate_stickers { put_wrapped_varint(&mut t, 15, v as u64) }
        if let Some(v) = self.message_display_compact { put_wrapped_varint(&mut t, 17, flag(v)) }
        if let Some(v) = self.explicit_content_filter { put_wrapped_varint(&mut t, 19, v as u64) }
        if let Some(v) = self.view_nsfw_guilds { put_wrapped_varint(&mut t, 20, flag(v)) }
        if let Some(v) = self.convert_emoticons { put_wrapped_varint(&mut t, 21, flag(v)) }
        if let Some(v) = self.view_nsfw_commands { put_wrapped_varint(&mut t, 23, flag(v)) }
        if let Some(v) = self.dm_spam_filter_v2 { put_enum(&mut t, 27, v) }
        if !t.is_empty() { put_msg(&mut out, 6, &t) }

        let mut n = Vec::new();
        if let Some(v) = self.show_in_app_notifications { put_wrapped_varint(&mut n, 1, flag(v)) }
        if let Some(v) = self.enable_burst_reaction_notifications { put_wrapped_varint(&mut n, 4, flag(v)) }
        if let Some(v) = self.reaction_notifications { put_enum(&mut n, 7, v) }
        if !n.is_empty() { put_msg(&mut out, 7, &n) }

        let mut p = Vec::new();
        if let Some(v) = self.default_guilds_restricted {
            // A plain `bool` field, not a wrapper; written even when false so it can override true.
            put_tag(&mut p, 4, 0);
            put_varint(&mut p, flag(v));
        }
        if let Some(v) = self.friend_source_flags { put_wrapped_varint(&mut p, 11, v as u64) }
        if let Some(v) = self.allow_activity_party_friends { put_wrapped_varint(&mut p, 1, flag(v)) }
        if let Some(v) = self.allow_activity_party_voice { put_wrapped_varint(&mut p, 2, flag(v)) }
        if let Some(v) = self.default_message_request_restricted { put_wrapped_varint(&mut p, 17, flag(v)) }
        if let Some(v) = self.inappropriate_conversation_warnings { put_wrapped_varint(&mut p, 23, flag(v)) }
        if let Some(v) = self.recent_games_enabled { put_wrapped_varint(&mut p, 24, flag(v)) }
        if let Some(v) = self.show_local_time { put_wrapped_varint(&mut p, 31, flag(v)) }
        if !p.is_empty() { put_msg(&mut out, 8, &p) }

        let mut s = Vec::new();
        if let Some(v) = &self.status { put_wrapped_string(&mut s, 1, v) }
        if let Some(c) = &self.custom_status {
            let mut cs = Vec::new();
            if !c.text.is_empty() { put_msg(&mut cs, 1, c.text.as_bytes()) }
            put_fixed64(&mut cs, 2, c.emoji_id);
            if !c.emoji_name.is_empty() { put_msg(&mut cs, 3, c.emoji_name.as_bytes()) }
            put_fixed64(&mut cs, 4, c.expires_at_ms);
            put_msg(&mut s, 2, &cs);
        }
        if let Some(v) = self.show_current_game { put_wrapped_varint(&mut s, 3, flag(v)) }
        if let Some(v) = self.status_created_at_ms { put_wrapped_varint(&mut s, 5, v) }
        if !s.is_empty() { put_msg(&mut out, 11, &s) }

        if let Some(v) = &self.locale {
            let mut l = Vec::new();
            put_wrapped_string(&mut l, 1, v);
            put_msg(&mut out, 12, &l);
        }

        let mut a = Vec::new();
        if let Some(v) = self.theme { put_enum(&mut a, 1, v) }
        if let Some(v) = self.timestamp_hour_cycle { put_enum(&mut a, 9, v) }
        if !a.is_empty() { put_msg(&mut out, 13, &a) }
        out
    }
}

/// Enum values from the web client bundle.
pub mod values {
    pub const THEME_DARK: u32 = 1;
    pub const THEME_LIGHT: u32 = 2;
    pub const ANIMATE_ALWAYS: u32 = 0;
    pub const ANIMATE_ON_INTERACTION: u32 = 1;
    pub const ANIMATE_NEVER: u32 = 2;
    pub const DM_SPAM_DEFAULT: u32 = 0;
    pub const DM_SPAM_DISABLED: u32 = 1;
    pub const DM_SPAM_NON_FRIENDS: u32 = 2;
    pub const DM_SPAM_ALL: u32 = 3;
    pub const FRIEND_MUTUAL_FRIENDS: u32 = 2;
    pub const FRIEND_MUTUAL_GUILDS: u32 = 4;
    pub const FRIEND_NO_RELATION: u32 = 8;
    pub const FRIEND_EVERYONE: u32 = 14;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_expiry_choices() {
        let now = 1_700_000_000_000u64; // 2023-11-14 22:13:20 UTC
        assert_eq!(StatusExpiry::Never.at_ms(now, 0), 0);
        assert_eq!(StatusExpiry::parse("1h").unwrap().at_ms(now, 0), now + 3_600_000);
        assert_eq!(StatusExpiry::Today.at_ms(now, 0), 1_700_006_400_000); // next UTC midnight
        // UTC+10: local time is already the 15th, so local midnight is 14:00 UTC on the 15th.
        assert_eq!(StatusExpiry::Today.at_ms(now, 36_000), 1_700_056_800_000);
        assert!(StatusExpiry::parse("bogus").is_none());
        let c = CustomStatus { text: "x".into(), expires_at_ms: now - 1, ..Default::default() };
        assert!(!custom_status_active(&c, now));
        assert!(custom_status_active(&CustomStatus { text: "x".into(), ..Default::default() }, now));
    }

    #[test]
    fn theme_light_bytes() {
        let p = Prefs { theme: Some(values::THEME_LIGHT), ..Default::default() };
        // appearance (13) { theme (1) = 2 }
        assert_eq!(p.encode_patch(), vec![0x6a, 2, 0x08, 2]);
    }

    #[test]
    fn compact_false_still_written() {
        // text_and_images (6) { message_display_compact (17) = BoolValue{} (false is an empty wrapper) }
        let p = Prefs { message_display_compact: Some(false), ..Default::default() };
        assert_eq!(p.encode_patch(), vec![0x32, 3, 0x8a, 1, 0]);
        let p = Prefs { message_display_compact: Some(true), ..Default::default() };
        assert_eq!(p.encode_patch(), vec![0x32, 5, 0x8a, 1, 2, 0x08, 1]);
    }

    #[test]
    fn status_and_custom_roundtrip() {
        let p = Prefs {
            status: Some("dnd".into()),
            custom_status: Some(CustomStatus { text: "hi".into(), emoji_name: "x".into(), expires_at_ms: 99, ..Default::default() }),
            status_created_at_ms: Some(1234),
            ..Default::default()
        };
        assert_eq!(Prefs::decode(&p.encode_patch()).unwrap(), p);
    }

    #[test]
    fn roundtrip_many() {
        let p = Prefs {
            theme: Some(2),
            message_display_compact: Some(true),
            animate_stickers: Some(1),
            dm_spam_filter_v2: Some(3),
            friend_source_flags: Some(values::FRIEND_EVERYONE),
            default_guilds_restricted: Some(true),
            locale: Some("en-GB".into()),
            show_in_app_notifications: Some(true),
            ..Default::default()
        };
        assert_eq!(Prefs::decode(&p.encode_patch()).unwrap(), p);
        assert!(Prefs::default().encode_patch().is_empty());
    }

    #[test]
    fn newer_settings_roundtrip() {
        let p = Prefs {
            explicit_content_filter: Some(2),
            view_nsfw_guilds: Some(true),
            view_nsfw_commands: Some(false),
            afk_timeout: Some(900),
            stream_notifications_enabled: Some(true),
            allow_activity_party_friends: Some(true),
            allow_activity_party_voice: Some(false),
            inappropriate_conversation_warnings: Some(true),
            recent_games_enabled: Some(true),
            show_local_time: Some(false),
            ..Default::default()
        };
        assert_eq!(Prefs::decode(&p.encode_patch()).unwrap(), p);
    }

    #[test]
    fn clearing_custom_status_is_empty_message() {
        let p = Prefs { custom_status: Some(CustomStatus::default()), ..Default::default() };
        // status (11) { custom_status (2) = {} }
        assert_eq!(p.encode_patch(), vec![0x5a, 2, 0x12, 0]);
    }

    #[test]
    fn decode_ignores_other_sections() {
        // guild_folders (14) present alongside appearance
        let blob = vec![0x72, 0, 0x6a, 2, 0x08, 1];
        assert_eq!(Prefs::decode(&blob).unwrap().theme, Some(1));
    }

    #[test]
    fn merge_overlays() {
        let mut a = Prefs { theme: Some(1), locale: Some("en-US".into()), ..Default::default() };
        a.merge(&Prefs { theme: Some(2), ..Default::default() });
        assert_eq!((a.theme, a.locale.as_deref()), (Some(2), Some("en-US")));
    }
}
