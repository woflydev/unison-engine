//! Creating a server and joining one by invite: reading what the user typed, previewing an invite,
//! and the request bodies. Pure, so every shell accepts the same links.

use serde_json::{Value, json};

use crate::model::Id;
use crate::snowflake::Snowflake;

/// The invite code in whatever the user pasted: `https://discord.gg/abc`, `discord.com/invite/abc`,
/// `discordapp.com/invite/abc?event=1`, or a bare code. `None` if nothing code-shaped is there.
pub fn parse_invite_code(input: &str) -> Option<String> {
    let t = input.trim().trim_end_matches('/');
    let t = t.split(['?', '#']).next().unwrap_or(t);
    let code = match t.rsplit_once('/') {
        Some((host, code)) if host.contains("discord") => code,
        Some(_) => return None,
        None => t,
    };
    (code.len() >= 2 && code.len() <= 32 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| code.to_string())
}

/// What an invite leads to, for the "join this server?" step.
#[derive(Debug, Clone, PartialEq)]
pub struct InvitePreview {
    pub code: String,
    pub guild_id: Id,
    pub name: String,
    pub icon_url: Option<String>,
    pub description: String,
    pub members: Option<u32>,
    pub online: Option<u32>,
    pub channel: String,
}

pub fn parse_preview(code: &str, v: &Value) -> Option<InvitePreview> {
    let g = &v["guild"];
    let guild_id = g["id"].as_str()?.parse::<u64>().ok().map(Snowflake)?;
    let icon_url = g["icon"].as_str().map(|h| {
        let ext = if h.starts_with("a_") { "gif" } else { "webp" };
        format!("https://cdn.discordapp.com/icons/{guild_id}/{h}.{ext}?size=128")
    });
    Some(InvitePreview {
        code: code.to_string(),
        guild_id,
        name: g["name"].as_str().unwrap_or_default().to_string(),
        icon_url,
        description: g["description"].as_str().unwrap_or_default().to_string(),
        members: v["approximate_member_count"].as_u64().map(|n| n as u32),
        online: v["approximate_presence_count"].as_u64().map(|n| n as u32),
        channel: v["channel"]["name"].as_str().unwrap_or_default().to_string(),
    })
}

/// `POST /guilds` body for a new server with Discord's default channels. Names are 2 to 100
/// characters.
pub fn create_body(name: &str, icon_data_uri: Option<String>) -> Option<Value> {
    let n: String = name.trim().chars().take(100).collect();
    if n.chars().count() < 2 {
        return None;
    }
    Some(json!({ "name": n, "icon": icon_data_uri, "channels": [], "system_channel_id": null }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_invite_links() {
        for (input, code) in [
            ("https://discord.gg/abc123", "abc123"),
            ("discord.gg/abc123/", "abc123"),
            ("https://discord.com/invite/AbC-d?event=5", "AbC-d"),
            ("http://discordapp.com/invite/xyz", "xyz"),
            ("  abc123  ", "abc123"),
        ] {
            assert_eq!(parse_invite_code(input).as_deref(), Some(code), "{input}");
        }
        for bad in ["", "a", "https://example.com/abc123", "has space", "https://discord.gg/"] {
            assert_eq!(parse_invite_code(bad), None, "{bad}");
        }
    }

    #[test]
    fn previews_and_create_bodies() {
        let v = json!({"guild": {"id": "7", "name": "Club", "icon": "abc", "description": "hi"}, "approximate_member_count": 120, "approximate_presence_count": 30, "channel": {"name": "welcome"}});
        let p = parse_preview("x", &v).unwrap();
        assert_eq!((p.name.as_str(), p.members, p.online, p.channel.as_str()), ("Club", Some(120), Some(30), "welcome"));
        assert!(p.icon_url.unwrap().contains("/icons/7/abc.webp"));
        assert!(parse_preview("x", &json!({"guild": {}})).is_none());
        assert!(create_body(" a ", None).is_none());
        assert_eq!(create_body("  My Club ", None).unwrap()["name"], "My Club");
    }
}
