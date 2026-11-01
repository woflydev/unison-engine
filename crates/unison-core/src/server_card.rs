//! The card a server tag opens (a server preview beside the name, like a profile popout). Built
//! from what the store knows: a server the account is in has its name, icon, banner, description
//! and member count; one it is not in shows the tag only, until the official preview request is
//! confirmed (FIDELITY "Server tag preview", capture).

use crate::model::Id;
use crate::store::Store;

#[derive(Debug, Clone, PartialEq)]
pub struct ServerCard {
    pub guild_id: Id,
    /// The account is in this server: the card can open it.
    pub joined: bool,
    pub name: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
    pub description: Option<String>,
    pub member_count: Option<u32>,
}

pub fn of(store: &Store, guild: Id) -> ServerCard {
    let Some(g) = store.guilds.get(&guild) else {
        return ServerCard { guild_id: guild, joined: false, name: None, icon_url: None, banner_url: None, description: None, member_count: None };
    };
    let p = g.props();
    ServerCard {
        guild_id: guild,
        joined: true,
        name: Some(g.name().to_string()).filter(|n| !n.is_empty()),
        icon_url: g.icon_url(128),
        banner_url: p.banner.as_ref().map(|h| {
            let ext = if h.starts_with("a_") { "gif" } else { "webp" };
            format!("https://cdn.discordapp.com/banners/{}/{}.{}?size=600", guild, h, ext)
        }),
        description: p.description.clone().filter(|d| !d.trim().is_empty()),
        member_count: g.member_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_you_are_not_in_has_only_its_id() {
        let s = Store::default();
        let c = of(&s, crate::snowflake::Snowflake(7));
        assert!(!c.joined);
        assert!(c.name.is_none());
    }
}
