//! Profile cosmetics a user wears: avatar decoration, server tag, nameplate, display name style.
//!
//! Parsed once from the user object (`User::{avatar_decoration_data, primary_guild, collectibles,
//! display_name_styles}`); shells only draw what is returned. Every URL is a static image so a
//! shell never animates cosmetics in a scrolling list. Shapes are in docs/FIDELITY.md (`capture`).

use serde_json::Value;

use crate::model::User;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildTag {
    pub text: String,
    /// Badge image, if the server picked one.
    pub badge_url: Option<String>,
    pub guild_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NameStyle {
    /// RGB colours, first to last. One is a solid colour, two or more a gradient.
    pub colors: Vec<u32>,
    /// Discord's effect id (1 solid, 2 gradient, 3 neon, 4 toon, 5 pop, 6 glow); only colours are drawn.
    pub effect: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cosmetics {
    pub decoration_url: Option<String>,
    pub tag: Option<GuildTag>,
    pub nameplate_url: Option<String>,
    pub name_style: Option<NameStyle>,
}

impl Cosmetics {
    pub fn is_empty(&self) -> bool {
        *self == Cosmetics::default()
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Only plain id/asset characters go into a URL.
fn clean(s: String) -> Option<String> {
    s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.')).then_some(s)
}

pub fn of_user(u: &User) -> Cosmetics {
    let decoration_url = u
        .avatar_decoration_data
        .as_ref()
        .and_then(|d| text(d, "asset"))
        .and_then(clean)
        .map(|a| format!("https://cdn.discordapp.com/avatar-decoration-presets/{a}.png?size=96&passthrough=false"));

    let tag = u.primary_guild.as_ref().and_then(|g| {
        if g.get("identity_enabled").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        let text_ = text(g, "tag")?;
        let guild_id = text(g, "identity_guild_id").and_then(clean)?;
        let badge_url = text(g, "badge")
            .and_then(clean)
            .map(|b| format!("https://cdn.discordapp.com/guild-tag-badges/{guild_id}/{b}.png?size=64"));
        Some(GuildTag { text: text_, badge_url, guild_id })
    });

    let nameplate_url = u
        .collectibles
        .as_ref()
        .and_then(|c| c.get("nameplate"))
        .and_then(|n| text(n, "asset"))
        .and_then(clean)
        .map(|a| format!("https://cdn.discordapp.com/assets/collectibles/{a}static.png"));

    let name_style = u.display_name_styles.as_ref().and_then(|d| {
        let colors: Vec<u32> = d
            .get("colors")?
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .map(|c| (c & 0xff_ffff) as u32)
            .take(4)
            .collect();
        (!colors.is_empty()).then(|| NameStyle { colors, effect: d.get("effect_id").and_then(Value::as_u64).unwrap_or(1) as u8 })
    });

    Cosmetics { decoration_url, tag, nameplate_url, name_style }
}

/// The icon of the member's highest role that has one (`role-icons` CDN image), as the official client shows beside the name.
pub fn role_icon_url(s: &crate::store::Store, guild: Option<crate::model::Id>, user: crate::model::Id) -> Option<String> {
    let g = guild?;
    let member = s.members.get(&(g, user))?;
    let roles = s.roles.get(&g)?;
    let r = member.roles.iter().filter_map(|r| roles.get(r)).filter(|r| r.icon.is_some()).max_by_key(|r| r.position)?;
    let hash = r.icon.as_deref().and_then(|h| clean(h.to_string()))?;
    Some(format!("https://cdn.discordapp.com/role-icons/{}/{hash}.webp?size=40&quality=lossless", r.id))
}

/// The colour at `t` (0 to 1) along a style's colours.
pub fn color_at(colors: &[u32], t: f32) -> u32 {
    match colors {
        [] => 0,
        [c] => *c,
        _ => {
            let span = (colors.len() - 1) as f32;
            let x = t.clamp(0.0, 1.0) * span;
            let i = (x.floor() as usize).min(colors.len() - 2);
            let f = x - i as f32;
            let ch = |shift: u32| {
                let (a, b) = (((colors[i] >> shift) & 0xff) as f32, ((colors[i + 1] >> shift) & 0xff) as f32);
                (a + (b - a) * f).round() as u32
            };
            (ch(16) << 16) | (ch(8) << 8) | ch(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(v: Value) -> User {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn plain_user_has_no_cosmetics() {
        assert!(of_user(&user(json!({"id": "1", "username": "a"}))).is_empty());
    }

    #[test]
    fn parses_everything() {
        let u = user(json!({
            "id": "1", "username": "a",
            "avatar_decoration_data": {"asset": "a_abc", "sku_id": "2"},
            "primary_guild": {"identity_guild_id": "99", "identity_enabled": true, "tag": "ABCD", "badge": "h1"},
            "collectibles": {"nameplate": {"asset": "nameplates/nameplates/twilight/", "palette": "cobalt"}},
            "display_name_styles": {"font_id": 3, "effect_id": 2, "colors": [16711680, 255]}
        }));
        let c = of_user(&u);
        assert_eq!(c.decoration_url.as_deref(), Some("https://cdn.discordapp.com/avatar-decoration-presets/a_abc.png?size=96&passthrough=false"));
        let t = c.tag.unwrap();
        assert_eq!((t.text.as_str(), t.guild_id.as_str()), ("ABCD", "99"));
        assert_eq!(t.badge_url.as_deref(), Some("https://cdn.discordapp.com/guild-tag-badges/99/h1.png?size=64"));
        assert_eq!(c.nameplate_url.as_deref(), Some("https://cdn.discordapp.com/assets/collectibles/nameplates/nameplates/twilight/static.png"));
        assert_eq!(c.name_style, Some(NameStyle { colors: vec![0xff0000, 0xff], effect: 2 }));
    }

    #[test]
    fn disabled_tag_and_odd_assets_are_dropped() {
        let u = user(json!({
            "id": "1", "username": "a",
            "avatar_decoration_data": {"asset": "x/../y?z"},
            "primary_guild": {"identity_guild_id": "99", "identity_enabled": false, "tag": "ABCD"}
        }));
        assert!(of_user(&u).is_empty());
    }

    #[test]
    fn gradient_interpolates() {
        assert_eq!(color_at(&[0x000000, 0xffffff], 0.5), 0x808080);
        assert_eq!(color_at(&[0xff0000], 0.9), 0xff0000);
        assert_eq!(color_at(&[0xff0000, 0x00ff00, 0x0000ff], 1.0), 0x0000ff);
    }
}
