use super::*;
use unison_core::moderation::{self, ModAction};
use unison_core::store::perm;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum ModKind {
    Kick,
    Ban,
    Timeout,
    RemoveTimeout,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct TimeoutChoice {
    pub label: String,
    pub secs: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct BanItem {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
    pub reason: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AuditItem {
    pub id: String,
    pub actor: String,
    pub target: String,
    /// "banned a member", "deleted a role", ...
    pub label: String,
    pub reason: String,
    pub time_ms: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct InviteAdminItem {
    pub code: String,
    pub inviter: String,
    pub channel: String,
    pub uses: u32,
    pub max_uses: u32,
    pub expires_ms: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MemberRoleItem {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub has: bool,
    /// I may give or take this role.
    pub toggle: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct RoleItem {
    pub id: String,
    pub name: String,
    /// 0 when the role has no colour.
    pub color: u32,
    pub position: i32,
    pub hoisted: bool,
    /// Moving it one place is allowed (rules in the core).
    pub can_move_up: bool,
    pub can_move_down: bool,
}

/// What the server settings window offers me, and its current values.
#[derive(uniffi::Record, Clone, Debug)]
pub struct GuildSettingsInfo {
    pub name: String,
    pub description: String,
    pub icon_url: Option<String>,
    pub is_owner: bool,
    pub can_edit: bool,
    pub can_bans: bool,
    pub can_audit: bool,
    pub can_invites: bool,
    pub can_roles: bool,
    pub can_channels: bool,
    pub can_expressions: bool,
    pub roles: Vec<RoleItem>,
    pub member_count: Option<u32>,
}

/// A server icon or banner edit made in Settings, applied on Save Changes.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum ImageChange {
    Keep,
    Remove,
    Set { content_type: String, bytes: Vec<u8> },
}

impl From<ImageChange> for unison_core::guild_admin::ImageEdit {
    fn from(c: ImageChange) -> Self {
        match c {
            ImageChange::Keep => Self::Keep,
            ImageChange::Remove => Self::Remove,
            ImageChange::Set { content_type, bytes } => Self::Set(content_type, bytes),
        }
    }
}

#[uniffi::export]
pub fn timeout_choices() -> Vec<TimeoutChoice> {
    moderation::TIMEOUT_CHOICES.iter().map(|(l, s)| TimeoutChoice { label: (*l).into(), secs: *s }).collect()
}

fn session(e: &UnisonEngine, account: &str) -> Result<Arc<unison_core::session::Session>, FfiError> {
    e.inner.session(id(account)).ok_or_else(|| FfiError::Failed { message: "no such account".into() })
}

#[uniffi::export]
impl UnisonEngine {
    /// A member's menu in a server: the person actions plus the moderation ones I'm allowed.
    pub fn member_actions(&self, account_id: String, guild_id: String, user_id: String, channel_id: Option<String>) -> Vec<actions::Action> {
        let mut out = self.user_actions(account_id.clone(), user_id.clone(), channel_id);
        let can_roles = self.with_store(&account_id, |s| s.can_assign_roles(id(&guild_id), id(&user_id))).unwrap_or(false);
        let (rights, timed_out) = self
            .with_store(&account_id, |s| {
                let (g, u) = (id(&guild_id), id(&user_id));
                let until = s.members.get(&(g, u)).and_then(|m| m.communication_disabled_until.as_deref()).and_then(unison_core::timefmt::iso_to_ms);
                (s.mod_rights(g, u), until.is_some_and(|t| t > now_ms()))
            })
            .unwrap_or_default();
        let at = out.iter().position(|a| a.id == actions::ActionId::CopyUserId).unwrap_or(out.len());
        let mut extra = Vec::new();
        let mut mk = |id, title: &str| extra.push(actions::Action { id, title: title.into(), enabled: true, destructive: false, group: 2 });
        if rights.nickname {
            mk(actions::ActionId::ChangeNickname, "Change Nickname…");
        }
        if can_roles {
            mk(actions::ActionId::EditRoles, "Roles…");
        }
        if rights.timeout {
            mk(if timed_out { actions::ActionId::RemoveTimeout } else { actions::ActionId::Timeout }, if timed_out { "Remove Timeout" } else { "Timeout…" });
        }
        let mut danger = |id, title: &str| extra.push(actions::Action { id, title: title.into(), enabled: true, destructive: true, group: 2 });
        if rights.kick {
            danger(actions::ActionId::Kick, "Kick…");
        }
        if rights.ban {
            danger(actions::ActionId::Ban, "Ban…");
        }
        out.splice(at..at, extra);
        out
    }

    pub fn moderate_member(&self, account_id: String, guild_id: String, user_id: String, kind: ModKind, delete_seconds: u64, timeout_secs: u64, reason: String, gesture: UserGesture) {
        let action = match kind {
            ModKind::Kick => ModAction::Kick,
            ModKind::Ban => ModAction::Ban { delete_seconds },
            ModKind::Timeout => ModAction::Timeout { secs: Some(timeout_secs) },
            ModKind::RemoveTimeout => ModAction::Timeout { secs: None },
        };
        self.spawn_write(account_id, gesture, move |s, i| async move { s.moderate_member(i, id(&guild_id), id(&user_id), action, &reason).await });
    }

    pub fn set_nickname(&self, account_id: String, guild_id: String, user_id: String, nick: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.set_nickname(i, id(&guild_id), id(&user_id), &nick).await });
    }

    /// A member's roles for the role sheet, highest first.
    pub fn member_roles(&self, account_id: String, guild_id: String, user_id: String) -> Vec<MemberRoleItem> {
        self.with_store(&account_id, |s| {
            s.assignable_roles(id(&guild_id), id(&user_id))
                .into_iter()
                .map(|r| MemberRoleItem { id: r.id.to_string(), name: r.name, color: r.color, has: r.has, toggle: r.toggle })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn set_member_role(&self, account_id: String, guild_id: String, user_id: String, role_id: String, add: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.set_member_role(i, id(&guild_id), id(&user_id), id(&role_id), add).await });
    }

    pub fn unban_member(&self, account_id: String, guild_id: String, user_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.unban_member(i, id(&guild_id), id(&user_id)).await });
    }

    pub fn revoke_invite(&self, account_id: String, guild_id: String, code: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.revoke_invite(i, id(&guild_id), &code).await });
    }

    /// Save Changes on the Overview page: one `PATCH /guilds/{id}` with only what changed.
    pub fn save_guild_overview(&self, account_id: String, guild_id: String, name: Option<String>, description: Option<String>, icon: ImageChange, banner: ImageChange, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.save_guild_overview(i, id(&guild_id), name, description, icon.into(), banner.into()).await
        });
    }

    pub fn edit_guild(&self, account_id: String, guild_id: String, name: Option<String>, description: Option<String>, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.edit_guild(i, id(&guild_id), name, description).await });
    }

    /// Whether the server settings window has anything for me in this server.
    pub fn can_open_guild_settings(&self, account_id: String, guild_id: String) -> bool {
        self.with_store(&account_id, |s| s.can_manage_guild_any(id(&guild_id))).unwrap_or(false)
    }

    pub fn guild_settings(&self, account_id: String, guild_id: String) -> Option<GuildSettingsInfo> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let guild = s.guilds.get(&g)?;
            let p = s.guild_permissions(g);
            let mut roles: Vec<RoleItem> = s
                .roles
                .get(&g)
                .map(|m| {
                    m.values()
                        .filter(|r| r.id != g)
                        .map(|r| RoleItem { id: r.id.to_string(), name: r.name.clone(), color: r.color, position: r.position, hoisted: r.hoist, can_move_up: s.can_move_role(g, r.id, true), can_move_down: s.can_move_role(g, r.id, false) })
                        .collect()
                })
                .unwrap_or_default();
            roles.sort_by_key(|r| std::cmp::Reverse(r.position));
            Some(GuildSettingsInfo {
                name: guild.name().to_string(),
                description: guild.props().description.clone().unwrap_or_default(),
                icon_url: guild.icon_url(128),
                is_owner: guild.props().owner_id == s.my_id(),
                can_edit: p & perm::MANAGE_GUILD != 0,
                can_bans: p & perm::BAN_MEMBERS != 0,
                can_audit: p & perm::VIEW_AUDIT_LOG != 0,
                can_invites: p & perm::MANAGE_GUILD != 0,
                can_roles: p & perm::MANAGE_ROLES != 0,
                can_channels: p & perm::MANAGE_CHANNELS != 0,
                can_expressions: s.can_manage_expressions(g),
                roles,
                member_count: guild.member_count,
            })
        })
        .flatten()
    }

    pub async fn guild_bans(&self, account_id: String, guild_id: String) -> Result<Vec<BanItem>, FfiError> {
        let s = session(self, &account_id)?;
        let g = id(&guild_id);
        let jh = self.inner.rt.spawn(async move { s.guild_bans(g).await });
        let list = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(list.into_iter().map(|b| BanItem { user_id: b.user_id.to_string(), name: b.name, avatar_url: b.avatar_url, reason: b.reason }).collect())
    }

    pub async fn guild_audit_log(&self, account_id: String, guild_id: String) -> Result<Vec<AuditItem>, FfiError> {
        let s = session(self, &account_id)?;
        let g = id(&guild_id);
        let jh = self.inner.rt.spawn(async move { s.guild_audit_log(g).await });
        let list = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(list
            .into_iter()
            .map(|a| AuditItem { id: a.id.to_string(), actor: a.actor, target: a.target, label: a.label.into(), reason: a.reason, time_ms: a.time_ms })
            .collect())
    }

    pub async fn guild_invites(&self, account_id: String, guild_id: String) -> Result<Vec<InviteAdminItem>, FfiError> {
        let s = session(self, &account_id)?;
        let g = id(&guild_id);
        let jh = self.inner.rt.spawn(async move { s.guild_invites(g).await });
        let list = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(list
            .into_iter()
            .map(|i| InviteAdminItem { code: i.code, inviter: i.inviter, channel: i.channel, uses: i.uses, max_uses: i.max_uses, expires_ms: i.expires_ms })
            .collect())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct RoleCardMember {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
    pub color: Option<u32>,
}

/// What clicking a role mention shows: the role and the members of it the client knows about.
#[derive(uniffi::Record, Clone, Debug)]
pub struct RoleCard {
    pub name: String,
    pub color: Option<u32>,
    pub mentionable: bool,
    pub members: Vec<RoleCardMember>,
    /// More members than the client has loaded may hold the role (large servers send members lazily).
    pub partial: bool,
}

#[uniffi::export]
impl UnisonEngine {
    pub fn role_card(&self, account_id: String, guild_id: String, role_id: String) -> Option<RoleCard> {
        self.with_store(&account_id, |s| {
            let (g, r) = (id(&guild_id), id(&role_id));
            let role = s.roles.get(&g)?.get(&r)?;
            let mut members: Vec<RoleCardMember> = s
                .members
                .iter()
                .filter(|((mg, _), m)| *mg == g && m.roles.contains(&r))
                .map(|((_, uid), _)| RoleCardMember {
                    user_id: uid.to_string(),
                    name: s.display_name(Some(g), *uid),
                    avatar_url: s.users.get(uid).map(|u| u.avatar_url(64)).unwrap_or_default(),
                    color: s.name_color(Some(g), *uid),
                })
                .collect();
            members.sort_by_key(|m| m.name.to_lowercase());
            let total = s.guilds.get(&g).and_then(|x| x.member_count).unwrap_or(0) as usize;
            let partial = total > s.members.keys().filter(|(mg, _)| *mg == g).count();
            members.truncate(100);
            Some(RoleCard { name: role.name.clone(), color: (role.color != 0).then_some(role.color), mentionable: role.mentionable, members, partial })
        })
        .flatten()
    }
}

/// What clicking a custom emoji shows: its name and the server it belongs to (when we are in it).
#[derive(uniffi::Record, Clone, Debug)]
pub struct EmojiCard {
    pub name: String,
    pub url: String,
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    /// The emoji comes from a server the user has joined, so it is usable with Nitro-free rules there.
    pub joined: bool,
}

#[uniffi::export]
impl UnisonEngine {
    pub fn emoji_card(&self, account_id: String, emoji_id: String, animated: bool) -> Option<EmojiCard> {
        self.with_store(&account_id, |s| {
            let e = id(&emoji_id);
            let url = format!("https://cdn.discordapp.com/emojis/{e}.{}?size=96", if animated { "gif" } else { "webp" });
            for (gid, g) in s.guilds.iter() {
                if let Some(em) = g.emojis.iter().find(|x| x.id == Some(e)) {
                    return Some(EmojiCard { name: em.name.clone().unwrap_or_default(), url, guild_id: Some(gid.to_string()), guild_name: Some(g.name().to_string()), joined: true });
                }
            }
            None
        })
        .flatten()
    }

    /// Whether a built-in server link (`<id:customize>` ...) can open anything here; the shell
    /// shows the onboarding questions again for "customize".
    pub fn server_setup_again(&self, account_id: String, guild_id: String) -> bool {
        self.with_store(&account_id, |s| s.guilds.get(&id(&guild_id)).is_some_and(|g| g.inline.features.iter().any(|f| f == "COMMUNITY"))).unwrap_or(false)
    }
}
