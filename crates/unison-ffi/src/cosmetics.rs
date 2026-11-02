//! Cosmetics as the shells draw them (decoration, server tag, nameplate, name colours). Parsed in
//! `unison_core::cosmetics`; this only reshapes.

use crate::UnisonEngine;
use unison_core::cosmetics::of_user;
use unison_core::model::User;

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct CosmeticsItem {
    /// Static ring image drawn at 1.2 times the avatar.
    pub decoration_url: Option<String>,
    pub tag_text: Option<String>,
    pub tag_badge_url: Option<String>,
    /// The server the tag is from (clicking the tag opens it when you are in it).
    pub tag_guild_id: Option<String>,
    /// Static nameplate strip for member rows.
    pub nameplate_url: Option<String>,
    /// One colour is solid, more a gradient across the name.
    pub name_colors: Vec<u32>,
}

/// `None` for the common person without cosmetics, so rows carry nothing extra.
pub(crate) fn item_of(u: &User) -> Option<CosmeticsItem> {
    let c = of_user(u);
    if c.is_empty() {
        return None;
    }
    Some(CosmeticsItem {
        decoration_url: c.decoration_url,
        tag_text: c.tag.as_ref().map(|t| t.text.clone()),
        tag_guild_id: c.tag.as_ref().map(|t| t.guild_id.to_string()),
        tag_badge_url: c.tag.and_then(|t| t.badge_url),
        nameplate_url: c.nameplate_url,
        name_colors: c.name_style.map(|n| n.colors).unwrap_or_default(),
    })
}

/// The preview a server tag opens (`unison_core::server_card`).
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ServerCardItem {
    pub guild_id: String,
    pub joined: bool,
    pub name: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
    pub description: Option<String>,
    pub member_count: Option<u32>,
}

#[uniffi::export]
impl UnisonEngine {
    /// The server a tag is from, as far as this account knows it.
    pub fn server_card(&self, account_id: String, guild_id: String) -> Option<ServerCardItem> {
        self.with_store(&account_id, |s| {
            let c = unison_core::server_card::of(s, crate::id(&guild_id));
            ServerCardItem {
                guild_id: c.guild_id.to_string(),
                joined: c.joined,
                name: c.name,
                icon_url: c.icon_url,
                banner_url: c.banner_url,
                description: c.description,
                member_count: c.member_count,
            }
        })
    }
}

/// One colour per character for a gradient name (`count` characters), or the single colour repeated.
#[uniffi::export]
pub fn name_gradient(colors: Vec<u32>, count: u32) -> Vec<u32> {
    let n = count.max(1) as usize;
    (0..n)
        .map(|i| unison_core::cosmetics::color_at(&colors, if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 }))
        .collect()
}

/// Whether an animated sticker plays now: the setting, Reduce Motion, whether it is on screen and hovered.
#[uniffi::export]
pub fn sticker_should_play(setting: u32, reduce_motion: bool, visible: bool, hovered: bool) -> bool {
    unison_core::sticker_motion::should_play(setting, reduce_motion, visible, hovered)
}

/// Whether an animated custom emoji plays now (setting, Reduce Motion, on screen, app in front).
#[uniffi::export]
pub fn emoji_should_play(setting: bool, reduce_motion: bool, visible: bool, app_active: bool) -> bool {
    unison_core::sticker_motion::emoji_should_play(setting, reduce_motion, visible, app_active)
}
