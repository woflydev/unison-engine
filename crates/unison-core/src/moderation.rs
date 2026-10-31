//! Who may act on whom in a server: Discord's role hierarchy plus the permission each action needs.
//! Shells draw the answer; none re-derives it.

use crate::model::Id;
use crate::store::{Store, perm};

/// Preset timeout lengths of the official client (seconds).
pub const TIMEOUT_CHOICES: [(&str, u64); 6] = [
    ("60 secs", 60),
    ("5 mins", 300),
    ("10 mins", 600),
    ("1 hour", 3600),
    ("1 day", 86_400),
    ("1 week", 604_800),
];

/// Longest timeout Discord accepts: 28 days.
pub const MAX_TIMEOUT_SECS: u64 = 28 * 86_400;

/// A member action; `Timeout { secs: None }` removes the timeout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModAction {
    Kick,
    Ban { delete_seconds: u64 },
    Timeout { secs: Option<u64> },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModRights {
    pub kick: bool,
    pub ban: bool,
    pub timeout: bool,
    pub nickname: bool,
    pub roles: bool,
}

impl ModRights {
    pub fn any(&self) -> bool {
        self.kick || self.ban || self.timeout || self.nickname || self.roles
    }
}

impl Store {
    /// Position of the highest role a member holds (`@everyone` is 0).
    pub fn highest_role(&self, guild: Id, user: Id) -> i32 {
        let roles = self.roles.get(&guild);
        let ids: &[Id] = if Some(user) == self.my_id() {
            self.my_members.get(&guild).map(|m| m.roles.as_slice()).unwrap_or_default()
        } else {
            self.members.get(&(guild, user)).map(|m| m.roles.as_slice()).unwrap_or_default()
        };
        ids.iter().filter_map(|r| roles.and_then(|m| m.get(r))).map(|r| r.position).max().unwrap_or(0)
    }

    fn is_owner(&self, guild: Id, user: Id) -> bool {
        self.guilds.get(&guild).is_some_and(|g| g.props().owner_id == Some(user))
    }

    /// What I may do to `target` in `guild`. Nothing against myself (except nickname), the owner, or
    /// anyone whose top role is not below mine (the owner outranks everyone).
    pub fn mod_rights(&self, guild: Id, target: Id) -> ModRights {
        let Some(me) = self.my_id() else { return ModRights::default() };
        let p = self.guild_permissions(guild);
        let has = |f: u64| p & f != 0;
        if target == me {
            return ModRights { nickname: has(perm::CHANGE_NICKNAME) || has(perm::MANAGE_NICKNAMES), ..Default::default() };
        }
        if self.is_owner(guild, target) {
            return ModRights::default();
        }
        let outranks = self.is_owner(guild, me) || self.highest_role(guild, me) > self.highest_role(guild, target);
        if !outranks {
            return ModRights::default();
        }
        ModRights {
            kick: has(perm::KICK_MEMBERS),
            ban: has(perm::BAN_MEMBERS),
            // Administrators cannot be timed out.
            timeout: has(perm::MODERATE_MEMBERS) && self.guild_permissions_of(guild, target) & perm::ADMINISTRATOR == 0,
            nickname: has(perm::MANAGE_NICKNAMES),
            roles: has(perm::MANAGE_ROLES),
        }
    }

    /// Another member's server-wide permissions (roles only), for the administrator check.
    fn guild_permissions_of(&self, guild: Id, user: Id) -> u64 {
        let roles = self.roles.get(&guild);
        let parse = |s: &str| s.parse::<u64>().unwrap_or(0);
        let mut p = roles.and_then(|r| r.get(&guild)).map_or(0, |r| parse(&r.permissions));
        for rid in self.members.get(&(guild, user)).map(|m| m.roles.as_slice()).unwrap_or_default() {
            if let Some(r) = roles.and_then(|m| m.get(rid)) {
                p |= parse(&r.permissions);
            }
        }
        p
    }

    /// Whether I may change `target`'s roles: Manage Roles, and either it is me or my top role is
    /// above theirs (nobody changes the owner's).
    pub fn can_assign_roles(&self, guild: Id, target: Id) -> bool {
        let Some(me) = self.my_id() else { return false };
        if self.guild_permissions(guild) & perm::MANAGE_ROLES == 0 {
            return false;
        }
        target == me || (!self.is_owner(guild, target) && (self.is_owner(guild, me) || self.highest_role(guild, me) > self.highest_role(guild, target)))
    }

    /// The server's roles with whether `target` has each and whether I may toggle it: below my top
    /// role (the owner: any), and not an integration's. Highest first; `@everyone` left out.
    pub fn assignable_roles(&self, guild: Id, target: Id) -> Vec<RoleAssign> {
        let Some(me) = self.my_id() else { return vec![] };
        let may = self.can_assign_roles(guild, target);
        let (owner, mine) = (self.is_owner(guild, me), self.highest_role(guild, me));
        let has: &[Id] = if Some(target) == Some(me) {
            self.my_members.get(&guild).map(|m| m.roles.as_slice()).unwrap_or_default()
        } else {
            self.members.get(&(guild, target)).map(|m| m.roles.as_slice()).unwrap_or_default()
        };
        let mut out: Vec<RoleAssign> = self
            .roles
            .get(&guild)
            .map(|m| m.values().filter(|r| r.id != guild).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .map(|r| RoleAssign {
                id: r.id,
                name: r.name.clone(),
                color: r.color,
                position: r.position,
                has: has.contains(&r.id),
                toggle: may && !r.managed && (owner || r.position < mine),
            })
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.position));
        out
    }

    /// Whether I can open the server settings at all.
    pub fn can_manage_guild_any(&self, guild: Id) -> bool {
        let p = self.guild_permissions(guild);
        [perm::MANAGE_GUILD, perm::BAN_MEMBERS, perm::VIEW_AUDIT_LOG, perm::MANAGE_ROLES, perm::KICK_MEMBERS]
            .iter()
            .any(|f| p & f != 0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoleAssign {
    pub id: Id,
    pub name: String,
    pub color: u32,
    pub position: i32,
    pub has: bool,
    pub toggle: bool,
}

/// ISO timestamp for a timeout of `secs` from `now_ms`, or `None` to remove it. Clamped to 28 days.
pub fn timeout_until(now_ms: u64, secs: Option<u64>) -> Option<String> {
    secs.map(|s| crate::timefmt::ms_to_iso(now_ms + s.min(MAX_TIMEOUT_SECS) * 1000))
}

#[derive(Debug, Clone, PartialEq)]
pub struct BanEntry {
    pub user_id: Id,
    pub name: String,
    pub avatar_url: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: Id,
    pub actor: String,
    pub target: String,
    pub label: &'static str,
    pub reason: String,
    pub time_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InviteEntry {
    pub code: String,
    pub inviter: String,
    pub channel: String,
    pub uses: u32,
    pub max_uses: u32,
    /// 0 = never.
    pub expires_ms: u64,
}

fn id_of(v: &serde_json::Value) -> Option<Id> {
    v.as_str().and_then(|s| s.parse().ok()).map(crate::snowflake::Snowflake)
}

fn user_name(v: &serde_json::Value) -> String {
    v["global_name"].as_str().filter(|s| !s.is_empty()).or(v["username"].as_str()).unwrap_or("Unknown").to_string()
}

pub fn parse_bans(v: &serde_json::Value) -> Vec<BanEntry> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    let u = &b["user"];
                    let user_id = id_of(&u["id"])?;
                    let avatar_url = match u["avatar"].as_str() {
                        Some(h) => format!("https://cdn.discordapp.com/avatars/{user_id}/{h}.png?size=64"),
                        None => "https://cdn.discordapp.com/embed/avatars/0.png".into(),
                    };
                    Some(BanEntry { user_id, name: user_name(u), avatar_url, reason: b["reason"].as_str().unwrap_or("").into() })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What an audit-log action type means to a person.
pub fn audit_label(action: u64) -> &'static str {
    match action {
        1 => "updated the server",
        10 => "created a channel",
        11 => "updated a channel",
        12 => "deleted a channel",
        13..=15 => "changed channel permissions",
        20 => "kicked a member",
        21 => "pruned members",
        22 => "banned a member",
        23 => "unbanned a member",
        24 => "updated a member",
        25 => "changed member roles",
        26 => "moved a member",
        27 => "disconnected a member",
        28 => "added a bot",
        30 => "created a role",
        31 => "updated a role",
        32 => "deleted a role",
        40 => "created an invite",
        41 => "updated an invite",
        42 => "deleted an invite",
        50..=52 => "changed a webhook",
        60..=62 => "changed an emoji",
        72 => "deleted a message",
        73 => "bulk deleted messages",
        74 => "pinned a message",
        75 => "unpinned a message",
        80..=82 => "changed an integration",
        83..=85 => "changed a stage",
        90..=92 => "changed a sticker",
        100..=102 => "changed a scheduled event",
        110..=112 => "changed a thread",
        121 => "ran an application command",
        _ => "did something",
    }
}

pub fn parse_audit(v: &serde_json::Value) -> Vec<AuditEntry> {
    let names: std::collections::HashMap<&str, String> = v["users"]
        .as_array()
        .map(|a| a.iter().filter_map(|u| Some((u["id"].as_str()?, user_name(u)))).collect())
        .unwrap_or_default();
    let name = |id: &serde_json::Value| id.as_str().map(|i| names.get(i).cloned().unwrap_or_else(|| format!("<@{i}>"))).unwrap_or_default();
    v["audit_log_entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let id = id_of(&e["id"])?;
                    Some(AuditEntry {
                        id,
                        actor: name(&e["user_id"]),
                        target: name(&e["target_id"]),
                        label: audit_label(e["action_type"].as_u64().unwrap_or(0)),
                        reason: e["reason"].as_str().unwrap_or("").into(),
                        time_ms: id.timestamp_ms(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_invites(v: &serde_json::Value) -> Vec<InviteEntry> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    let max_age = i["max_age"].as_u64().unwrap_or(0);
                    let created = i["created_at"].as_str().and_then(crate::timefmt::iso_to_ms).unwrap_or(0);
                    Some(InviteEntry {
                        code: i["code"].as_str()?.to_string(),
                        inviter: i["inviter"].as_object().map(|_| user_name(&i["inviter"])).unwrap_or_default(),
                        channel: i["channel"]["name"].as_str().unwrap_or("").into(),
                        uses: i["uses"].as_u64().unwrap_or(0) as u32,
                        max_uses: i["max_uses"].as_u64().unwrap_or(0) as u32,
                        expires_ms: if max_age == 0 || created == 0 { 0 } else { created + max_age * 1000 },
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Guild, GuildProperties, Member, Role, User};
    use crate::snowflake::Snowflake as S;

    fn role(id: u64, pos: i32, perms: u64) -> Role {
        Role { id: S(id), position: pos, permissions: perms.to_string(), ..Default::default() }
    }

    fn setup() -> Store {
        let mut s = Store::default();
        s.me = Some(User { id: S(1), ..Default::default() });
        let g = S(7);
        s.guilds.insert(g, Guild { id: g, properties: Some(GuildProperties { owner_id: Some(S(99)), ..Default::default() }), ..Default::default() });
        let roles = s.roles.entry(g).or_default();
        roles.insert(g, role(7, 0, 0));
        roles.insert(S(20), role(20, 5, perm::KICK_MEMBERS | perm::BAN_MEMBERS | perm::MODERATE_MEMBERS));
        roles.insert(S(21), role(21, 3, 0));
        roles.insert(S(22), role(22, 8, perm::ADMINISTRATOR));
        s.my_members.insert(g, Member { roles: vec![S(20)], ..Default::default() });
        s.members.insert((g, S(2)), Member { roles: vec![S(21)], ..Default::default() });
        s.members.insert((g, S(3)), Member { roles: vec![S(22)], ..Default::default() });
        s.members.insert((g, S(4)), Member { roles: vec![S(20)], ..Default::default() });
        s
    }

    #[test]
    fn hierarchy_decides() {
        let s = setup();
        let g = S(7);
        let r = s.mod_rights(g, S(2));
        assert!(r.kick && r.ban && r.timeout && !r.roles && !r.nickname);
        assert!(!s.mod_rights(g, S(3)).any(), "higher role");
        assert!(!s.mod_rights(g, S(4)).any(), "equal role");
        assert!(!s.mod_rights(g, S(99)).any(), "owner");
        assert!(!s.mod_rights(g, S(1)).kick, "not myself");
        assert!(s.can_manage_guild_any(g));
    }

    #[test]
    fn roles_are_toggled_below_my_top_role_only() {
        let mut s = setup();
        let g = S(7);
        assert!(!s.can_assign_roles(g, S(2)), "needs Manage Roles");
        s.roles.get_mut(&g).unwrap().get_mut(&S(20)).unwrap().permissions = (perm::MANAGE_ROLES).to_string();
        s.roles.get_mut(&g).unwrap().insert(S(23), Role { id: S(23), position: 2, managed: true, ..Default::default() });
        assert!(s.can_assign_roles(g, S(2)));
        assert!(!s.can_assign_roles(g, S(3)), "member with a higher role");
        assert!(!s.can_assign_roles(g, S(99)), "owner");
        let list = s.assignable_roles(g, S(2));
        let by = |id: u64| list.iter().find(|r| r.id == S(id)).unwrap();
        assert!(by(21).has && by(21).toggle, "below mine, theirs");
        assert!(!by(20).toggle && !by(22).toggle, "at or above my top role");
        assert!(!by(23).toggle, "integration roles are not hand-assigned");
        assert!(list.iter().all(|r| r.id != g), "@everyone is not listed");
        assert!(list.windows(2).all(|w| w[0].position >= w[1].position));
    }

    #[test]
    fn admins_cannot_be_timed_out_and_owner_outranks() {
        let mut s = setup();
        let g = S(7);
        s.roles.get_mut(&g).unwrap().get_mut(&S(21)).unwrap().permissions = perm::ADMINISTRATOR.to_string();
        let r = s.mod_rights(g, S(2));
        assert!(r.kick && !r.timeout);
        s.guilds.get_mut(&g).unwrap().properties.as_mut().unwrap().owner_id = Some(S(1));
        assert!(s.mod_rights(g, S(3)).ban, "the owner can ban anyone");
    }

    #[test]
    fn parses_admin_lists() {
        use serde_json::json;
        let bans = parse_bans(&json!([{"user": {"id": "5", "username": "bad", "global_name": "Bad One", "avatar": "abc"}, "reason": "spam"}]));
        assert_eq!((bans[0].name.as_str(), bans[0].reason.as_str()), ("Bad One", "spam"));
        assert!(bans[0].avatar_url.contains("/avatars/5/abc.png"));
        let log = parse_audit(&json!({"users": [{"id": "1", "username": "mod"}], "audit_log_entries": [
            {"id": "175928847299117063", "user_id": "1", "target_id": "5", "action_type": 22, "reason": "spam"}]}));
        assert_eq!((log[0].actor.as_str(), log[0].target.as_str(), log[0].label), ("mod", "<@5>", "banned a member"));
        let inv = parse_invites(&json!([{"code": "abc", "uses": 3, "max_uses": 10, "max_age": 3600, "created_at": "1970-01-02T00:00:00Z", "inviter": {"username": "u"}, "channel": {"name": "general"}}]));
        assert_eq!((inv[0].expires_ms, inv[0].uses, inv[0].channel.as_str()), (90_000_000, 3, "general"));
    }

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(timeout_until(0, None), None);
        assert_eq!(timeout_until(0, Some(60)).unwrap(), "1970-01-01T00:01:00.000Z");
        assert_eq!(timeout_until(0, Some(u64::MAX / 4)).unwrap(), "1970-01-29T00:00:00.000Z");
    }
}
