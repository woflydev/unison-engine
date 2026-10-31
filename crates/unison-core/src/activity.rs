//! What other people are doing: the `activities` array of a presence (playing, listening, streaming,
//! watching, competing, custom status). The core parses it and decides what a row or card says; shells
//! only draw the resulting text and image.

use serde_json::Value;

pub const PLAYING: u8 = 0;
pub const STREAMING: u8 = 1;
pub const LISTENING: u8 = 2;
pub const WATCHING: u8 = 3;
pub const CUSTOM: u8 = 4;
pub const COMPETING: u8 = 5;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Activity {
    pub kind: u8,
    pub name: String,
    pub details: String,
    pub state: String,
    /// Unicode emoji, or the custom emoji's name when `emoji_id` is set.
    pub emoji_name: String,
    pub emoji_id: Option<u64>,
    pub emoji_animated: bool,
    pub application_id: Option<u64>,
    pub large_image: String,
    pub large_text: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn num(v: &Value) -> Option<u64> {
    match v {
        Value::String(x) => x.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

impl Activity {
    pub fn parse(v: &Value) -> Option<Activity> {
        let kind = v["type"].as_u64()? as u8;
        let emoji = &v["emoji"];
        Some(Activity {
            kind,
            name: s(&v["name"]),
            details: s(&v["details"]),
            state: s(&v["state"]),
            emoji_name: s(&emoji["name"]),
            emoji_id: num(&emoji["id"]),
            emoji_animated: emoji["animated"].as_bool().unwrap_or(false),
            application_id: num(&v["application_id"]),
            large_image: s(&v["assets"]["large_image"]),
            large_text: s(&v["assets"]["large_text"]),
            start_ms: num(&v["timestamps"]["start"]),
            end_ms: num(&v["timestamps"]["end"]),
        })
    }

    pub fn parse_list(v: &Value) -> Vec<Activity> {
        v.as_array().map(|a| a.iter().filter_map(Activity::parse).collect()).unwrap_or_default()
    }

    /// "Playing", "Listening to", ... followed by the name in the UI.
    pub fn verb(&self) -> &'static str {
        match self.kind {
            PLAYING => "Playing",
            STREAMING => "Streaming",
            LISTENING => "Listening to",
            WATCHING => "Watching",
            COMPETING => "Competing in",
            _ => "",
        }
    }

    /// The one-line summary a member row shows: `Playing Factorio`, or the custom status text.
    pub fn line(&self) -> String {
        if self.kind == CUSTOM {
            return self.state.clone();
        }
        format!("{} {}", self.verb(), self.name)
    }

    /// Where the artwork lives. Spotify and external assets use their own hosts; application assets
    /// live on the CDN under the application id.
    pub fn image_url(&self) -> Option<String> {
        let a = self.large_image.as_str();
        if a.is_empty() {
            return None;
        }
        if let Some(id) = a.strip_prefix("spotify:") {
            return Some(format!("https://i.scdn.co/image/{id}"));
        }
        if let Some(rest) = a.strip_prefix("mp:external/") {
            // `mp:external/<hash>/https/host/path` -> proxied by Discord's media proxy.
            return Some(format!("https://media.discordapp.net/external/{rest}"));
        }
        if let Some(rest) = a.strip_prefix("mp:") {
            return Some(format!("https://media.discordapp.net/{rest}"));
        }
        self.application_id.map(|app| format!("https://cdn.discordapp.com/app-assets/{app}/{a}.png?size=128"))
    }

    pub fn emoji_url(&self) -> Option<String> {
        self.emoji_id.map(|id| {
            let ext = if self.emoji_animated { "gif" } else { "png" };
            format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=32")
        })
    }
}

/// The custom status among `list`, if any.
pub fn custom(list: &[Activity]) -> Option<&Activity> {
    list.iter().find(|a| a.kind == CUSTOM && (!a.state.is_empty() || !a.emoji_name.is_empty()))
}

/// The first real activity (not a custom status), preferring one being streamed or played.
pub fn primary(list: &[Activity]) -> Option<&Activity> {
    list.iter().find(|a| a.kind != CUSTOM)
}

/// Text for a member or DM row: the custom status, else the primary activity.
pub fn row_text(list: &[Activity]) -> Option<String> {
    custom(list)
        .map(|c| c.state.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| primary(list).map(Activity::line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list() -> Vec<Activity> {
        Activity::parse_list(&json!([
            {"type": 2, "name": "Spotify", "details": "Song", "state": "Artist", "assets": {"large_image": "spotify:abc"}, "timestamps": {"start": 10, "end": 20}},
            {"type": 4, "name": "Custom Status", "state": "busy", "emoji": {"name": "x", "id": "5", "animated": true}},
            {"type": 0, "name": "Factorio", "application_id": "77", "assets": {"large_image": "img1"}}
        ]))
    }

    #[test]
    fn parses_and_summarises() {
        let l = list();
        assert_eq!(l.len(), 3);
        assert_eq!(primary(&l).unwrap().line(), "Listening to Spotify");
        assert_eq!(custom(&l).unwrap().state, "busy");
        assert_eq!(row_text(&l).as_deref(), Some("busy"));
        assert_eq!(l[1].emoji_url().unwrap(), "https://cdn.discordapp.com/emojis/5.gif?size=32");
        assert_eq!(l[0].image_url().unwrap(), "https://i.scdn.co/image/abc");
        assert_eq!(l[2].image_url().unwrap(), "https://cdn.discordapp.com/app-assets/77/img1.png?size=128");
        assert_eq!((l[0].start_ms, l[0].end_ms), (Some(10), Some(20)));
    }

    #[test]
    fn row_text_falls_back_to_activity() {
        let l = Activity::parse_list(&json!([{"type": 0, "name": "Factorio"}]));
        assert_eq!(row_text(&l).as_deref(), Some("Playing Factorio"));
        assert_eq!(row_text(&[]), None);
        // An emoji-only custom status has no text, so the activity wins.
        let l = Activity::parse_list(&json!([{"type": 4, "emoji": {"name": "x"}}, {"type": 3, "name": "Frieren"}]));
        assert_eq!(row_text(&l).as_deref(), Some("Watching Frieren"));
    }
}
