//! Server administration: role editor, channel editor, expressions, icon and banner (rules in
//! `unison_core::guild_admin`).

use unison_core::guild_admin as ga;
use unison_core::model::channel_type;
use unison_core::store::perm;

use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct PermissionDef {
    pub bit: u64,
    pub label: String,
    pub group: String,
}

#[uniffi::export]
pub fn permission_catalog() -> Vec<PermissionDef> {
    ga::PERMISSIONS.iter().map(|(bit, label, group)| PermissionDef { bit: *bit, label: (*label).into(), group: (*group).into() }).collect()
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SlowmodeChoice {
    pub label: String,
    pub secs: u32,
}

#[uniffi::export]
pub fn slowmode_choices() -> Vec<SlowmodeChoice> {
    ga::SLOWMODE_CHOICES.iter().map(|(l, s)| SlowmodeChoice { label: (*l).into(), secs: *s }).collect()
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct RoleDetail {
    pub id: String,
    pub name: String,
    pub color: u32,
    pub hoisted: bool,
    pub mentionable: bool,
    pub permissions: u64,
    pub is_everyone: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    /// Permission bits I may switch on or off.
    pub grantable: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct RoleDraftItem {
    pub name: String,
    pub color: u32,
    pub hoisted: bool,
    pub mentionable: bool,
    pub permissions: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AdminChannel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    pub category: Option<String>,
    pub topic: String,
    pub nsfw: bool,
    pub slowmode_secs: u32,
    /// Text-like channels have a topic, age restriction and slowmode; voice channels only a name.
    pub text_like: bool,
    pub can_edit: bool,
    /// I may change who can see and use it (Manage Permissions in this channel).
    pub can_permissions: bool,
    pub can_move_up: bool,
    pub can_move_down: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChannelDraftItem {
    pub name: String,
    pub topic: String,
    pub nsfw: bool,
    pub slowmode_secs: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ExpressionItem {
    pub id: String,
    pub name: String,
    pub url: String,
    pub animated: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Expressions {
    pub can_manage: bool,
    pub emojis: Vec<ExpressionItem>,
    pub stickers: Vec<ExpressionItem>,
}

#[uniffi::export]
impl UnisonEngine {
    /// A role's editable settings; `role_id` of `None` is the blank form for a new role.
    pub fn role_detail(&self, account_id: String, guild_id: String, role_id: Option<String>) -> Option<RoleDetail> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let mine = s.guild_permissions(g);
            let grantable = ga::grantable(mine);
            let can_roles = mine & perm::MANAGE_ROLES != 0;
            match role_id {
                None => Some(RoleDetail { id: String::new(), name: "new role".into(), color: 0, hoisted: false, mentionable: false, permissions: 0, is_everyone: false, can_edit: can_roles, can_delete: false, grantable }),
                Some(r) => {
                    let role = s.roles.get(&g)?.get(&id(&r))?;
                    let can_edit = s.can_edit_role(g, role.id);
                    Some(RoleDetail {
                        id: role.id.to_string(),
                        name: if role.id == g { "@everyone".into() } else { role.name.clone() },
                        color: role.color,
                        hoisted: role.hoist,
                        mentionable: role.mentionable,
                        permissions: role.permissions.parse().unwrap_or(0),
                        is_everyone: role.id == g,
                        can_edit,
                        can_delete: can_edit && role.id != g && !role.managed,
                        grantable,
                    })
                }
            }
        })
        .flatten()
    }

    pub fn save_role(&self, account_id: String, guild_id: String, role_id: Option<String>, draft: RoleDraftItem, gesture: UserGesture) {
        let d = ga::RoleDraft { name: draft.name, color: draft.color, hoist: draft.hoisted, mentionable: draft.mentionable, permissions: draft.permissions };
        self.spawn_write(account_id, gesture, move |s, i| async move { s.save_role(i, id(&guild_id), role_id.as_deref().map(id), d).await });
    }

    pub fn delete_role(&self, account_id: String, guild_id: String, role_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.delete_role(i, id(&guild_id), id(&role_id)).await });
    }

    /// The server's channels (not threads) as the channel editor lists them.
    pub fn admin_channels(&self, account_id: String, guild_id: String) -> Vec<AdminChannel> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let mut list: Vec<&Channel> = s.guild_channels.get(&g).into_iter().flatten().filter_map(|c| s.channels.get(c)).filter(|c| !c.is_thread()).collect();
            list.sort_by_key(|c| (c.kind != channel_type::GUILD_CATEGORY, c.position));
            let can_channels = s.guild_permissions(g) & perm::MANAGE_CHANNELS != 0;
            list.into_iter()
                .map(|c| {
                    let sib = s.channel_siblings(c.id);
                    let at = sib.iter().position(|x| *x == c.id);
                    AdminChannel {
                    id: c.id.to_string(),
                    name: c.name.clone().unwrap_or_default(),
                    kind: channels::kind(c),
                    category: c.parent_id.and_then(|p| s.channels.get(&p)).and_then(|p| p.name.clone()),
                    topic: c.topic.clone().unwrap_or_default(),
                    nsfw: c.nsfw,
                    slowmode_secs: c.rate_limit_per_user,
                    text_like: matches!(c.kind, channel_type::GUILD_TEXT | channel_type::GUILD_ANNOUNCEMENT | 15 | 16),
                    can_edit: s.permissions(c) & perm::MANAGE_CHANNELS != 0,
                    can_permissions: s.permissions(c) & (perm::MANAGE_ROLES | perm::ADMINISTRATOR) != 0,
                    can_move_up: can_channels && at.is_some_and(|i| i > 0),
                    can_move_down: can_channels && at.is_some_and(|i| i + 1 < sib.len()),
                }})
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn edit_channel(&self, account_id: String, channel_id: String, draft: ChannelDraftItem, gesture: UserGesture) {
        let d = ga::ChannelDraft { name: draft.name, topic: draft.topic, nsfw: draft.nsfw, slowmode_secs: draft.slowmode_secs };
        self.spawn_write(account_id, gesture, move |s, i| async move { s.edit_channel(i, id(&channel_id), d).await });
    }

    pub fn delete_channel(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.delete_channel(i, id(&channel_id)).await });
    }

    /// Set the server icon or banner; `bytes` of `None` removes it.
    pub fn set_guild_image(&self, account_id: String, guild_id: String, banner: bool, content_type: String, bytes: Option<Vec<u8>>, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.set_guild_image(i, id(&guild_id), banner, bytes.map(|b| (content_type, b))).await });
    }

    pub fn guild_expressions(&self, account_id: String, guild_id: String) -> Expressions {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let Some(guild) = s.guilds.get(&g) else { return Expressions { can_manage: false, emojis: vec![], stickers: vec![] } };
            Expressions {
                can_manage: s.can_manage_expressions(g),
                emojis: guild
                    .emojis
                    .iter()
                    .filter_map(|e| {
                        let eid = e.id?;
                        let ext = if e.animated { "gif" } else { "png" };
                        Some(ExpressionItem { id: eid.to_string(), name: e.name.clone().unwrap_or_default(), url: format!("https://cdn.discordapp.com/emojis/{eid}.{ext}?size=48"), animated: e.animated })
                    })
                    .collect(),
                stickers: guild
                    .stickers
                    .iter()
                    .map(|st| ExpressionItem { id: st.id.to_string(), name: st.name.clone(), url: format!("https://media.discordapp.net/stickers/{}.png?size=96", st.id), animated: st.format_type == 4 })
                    .collect(),
            }
        })
        .unwrap_or(Expressions { can_manage: false, emojis: vec![], stickers: vec![] })
    }

    pub fn create_emoji(&self, account_id: String, guild_id: String, name: String, content_type: String, bytes: Vec<u8>, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.create_emoji(i, id(&guild_id), &name, &content_type, bytes).await });
    }

    pub fn rename_emoji(&self, account_id: String, guild_id: String, emoji_id: String, name: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.rename_emoji(i, id(&guild_id), id(&emoji_id), &name).await });
    }

    pub fn delete_emoji(&self, account_id: String, guild_id: String, emoji_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.delete_emoji(i, id(&guild_id), id(&emoji_id)).await });
    }

    pub fn delete_sticker(&self, account_id: String, guild_id: String, sticker_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.delete_sticker(i, id(&guild_id), id(&sticker_id)).await });
    }

    pub fn create_channel(&self, account_id: String, guild_id: String, name: String, kind: u8, parent_id: Option<String>, topic: String, gesture: UserGesture) {
        let d = ga::NewChannel { name, kind, parent: parent_id.as_deref().map(id), topic };
        self.spawn_write(account_id, gesture, move |s, i| async move { s.create_channel(i, id(&guild_id), d).await });
    }

    /// Move a channel one place; `later` = down the list.
    /// Which zones of `target` a dragged channel may land in (empty: not allowed, or no permission).
    pub fn channel_drop_zones(&self, account_id: String, dragged: String, target: String) -> Vec<channels::RailDropZone> {
        use unison_core::guild_admin::DropZone as Z;
        self.with_store(&account_id, |s| s.channel_drop_zones(id(&dragged), id(&target)))
            .unwrap_or_default()
            .into_iter()
            .map(|z| match z {
                Z::Before => channels::RailDropZone::Before,
                Z::Into => channels::RailDropZone::Onto,
                Z::After => channels::RailDropZone::After,
            })
            .collect()
    }

    pub fn drop_channel(&self, account_id: String, dragged: String, target: String, zone: channels::RailDropZone, gesture: UserGesture) {
        use unison_core::guild_admin::DropZone as Z;
        let zone = match zone {
            channels::RailDropZone::Before => Z::Before,
            channels::RailDropZone::Onto => Z::Into,
            channels::RailDropZone::After => Z::After,
        };
        self.spawn_write(account_id, gesture, move |s, i| async move { s.drop_channel(i, id(&dragged), id(&target), zone).await });
    }

    pub fn move_channel(&self, account_id: String, channel_id: String, later: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.move_channel(i, id(&channel_id), later).await });
    }

    pub fn move_role(&self, account_id: String, guild_id: String, role_id: String, later: bool, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.move_role(i, id(&guild_id), id(&role_id), later).await });
    }

    /// Categories of a server, for the "Create Channel" form.
    pub fn category_choices(&self, account_id: String, guild_id: String) -> Vec<CategoryChoice> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let mut v: Vec<&Channel> = s.guild_channels.get(&g).into_iter().flatten().filter_map(|c| s.channels.get(c)).filter(|c| c.kind == channel_type::GUILD_CATEGORY).collect();
            v.sort_by_key(|c| (c.position, c.id));
            v.into_iter().map(|c| CategoryChoice { id: c.id.to_string(), name: c.name.clone().unwrap_or_default() }).collect()
        })
        .unwrap_or_default()
    }

    /// Who is set apart in a channel and what the editor may offer for it.
    pub fn channel_access(&self, account_id: String, channel_id: String) -> Option<ChannelAccess> {
        self.with_store(&account_id, |s| {
            let c = s.channels.get(&id(&channel_id))?;
            let g = c.guild_id?;
            let mine = s.permissions(c);
            let can_edit = mine & (perm::MANAGE_ROLES | perm::ADMINISTRATOR) != 0;
            let text_like = matches!(c.kind, channel_type::GUILD_TEXT | channel_type::GUILD_ANNOUNCEMENT | channel_type::GUILD_FORUM | channel_type::GUILD_MEDIA);
            let voice_like = matches!(c.kind, channel_type::GUILD_VOICE | channel_type::GUILD_STAGE_VOICE);
            let roles = s.roles.get(&g);
            let role_name = |r: Id| roles.and_then(|m| m.get(&r)).map(|r| if r.id == g { "@everyone".to_string() } else { r.name.clone() });
            let mut entries: Vec<OverwriteItem> = c
                .permission_overwrites
                .iter()
                .map(|o| {
                    let is_role = o.kind == 0;
                    OverwriteItem {
                        target_id: o.id.to_string(),
                        name: if is_role { role_name(o.id).unwrap_or_else(|| "deleted role".into()) } else { s.display_name(Some(g), o.id) },
                        is_role,
                        color: if is_role { roles.and_then(|m| m.get(&o.id)).map_or(0, |r| r.color) } else { 0 },
                        allow: o.allow.parse().unwrap_or(0),
                        deny: o.deny.parse().unwrap_or(0),
                    }
                })
                .collect();
            entries.sort_by_key(|e| (!e.is_role, e.name.to_lowercase()));
            let mut addable: Vec<OverwriteTarget> = roles
                .into_iter()
                .flat_map(|m| m.values())
                .filter(|r| !c.permission_overwrites.iter().any(|o| o.id == r.id))
                .map(|r| OverwriteTarget { id: r.id.to_string(), name: if r.id == g { "@everyone".into() } else { r.name.clone() } })
                .collect();
            let everyone = g.to_string();
            addable.sort_by(|a, b| (a.id != everyone).cmp(&(b.id != everyone)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            Some(ChannelAccess {
                name: c.name.clone().unwrap_or_default(),
                can_edit,
                grantable: if can_edit { ga::grantable(mine) } else { 0 },
                permissions: ga::channel_permissions(text_like, voice_like).into_iter().map(|(bit, label, group)| PermissionDef { bit, label: label.into(), group: group.into() }).collect(),
                entries,
                addable,
            })
        })
        .flatten()
    }

    pub fn save_overwrite(&self, account_id: String, channel_id: String, target_id: String, is_role: bool, allow: u64, deny: u64, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.save_overwrite(i, id(&channel_id), id(&target_id), is_role, allow, deny).await });
    }

    pub fn delete_overwrite(&self, account_id: String, channel_id: String, target_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move { s.delete_overwrite(i, id(&channel_id), id(&target_id)).await });
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct CategoryChoice {
    pub id: String,
    pub name: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct OverwriteItem {
    pub target_id: String,
    pub name: String,
    pub is_role: bool,
    pub color: u32,
    pub allow: u64,
    pub deny: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct OverwriteTarget {
    pub id: String,
    pub name: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChannelAccess {
    pub name: String,
    pub can_edit: bool,
    /// Permission bits I may switch (0 when I can't edit).
    pub grantable: u64,
    /// The bits that make sense in this kind of channel, in display order.
    pub permissions: Vec<PermissionDef>,
    pub entries: Vec<OverwriteItem>,
    /// Roles with no overwrite yet, `@everyone` first.
    pub addable: Vec<OverwriteTarget>,
}

#[uniffi::export]
pub fn new_channel_kinds() -> Vec<NewChannelKind> {
    ga::NEW_CHANNEL_KINDS.iter().map(|(l, k)| NewChannelKind { label: (*l).into(), kind: *k }).collect()
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct NewChannelKind {
    pub label: String,
    pub kind: u8,
}
