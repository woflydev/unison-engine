use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct GuildItem {
    pub id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub unread: bool,
    pub mentions: u32,
    pub muted: bool,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum RailItem {
    Guild {
        guild: GuildItem,
    },
    Folder {
        id: Option<i64>,
        name: Option<String>,
        color: Option<u32>,
        guilds: Vec<GuildItem>,
    },
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum ChannelKind {
    Text,
    Voice,
    Category,
    Announcement,
    Stage,
    Forum,
    Media,
    Thread,
    Dm,
    GroupDm,
    Other,
}

pub(crate) fn kind(c: &Channel) -> ChannelKind {
    match c.kind {
        channel_type::GUILD_TEXT => ChannelKind::Text,
        channel_type::GUILD_VOICE => ChannelKind::Voice,
        channel_type::GUILD_CATEGORY => ChannelKind::Category,
        channel_type::GUILD_ANNOUNCEMENT => ChannelKind::Announcement,
        channel_type::GUILD_STAGE_VOICE => ChannelKind::Stage,
        channel_type::GUILD_FORUM => ChannelKind::Forum,
        channel_type::GUILD_MEDIA => ChannelKind::Media,
        channel_type::DM => ChannelKind::Dm,
        channel_type::GROUP_DM => ChannelKind::GroupDm,
        _ if c.is_thread() => ChannelKind::Thread,
        _ => ChannelKind::Other,
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ChannelItem {
    pub id: String,
    pub guild_id: Option<String>,
    pub parent_id: Option<String>,
    pub kind: ChannelKind,
    pub name: String,
    pub topic: Option<String>,
    pub unread: bool,
    pub mentions: u32,
    pub muted: bool,
    pub nsfw: bool,
    pub can_send: bool,
    /// The user may create invites to this channel.
    pub can_invite: bool,
    /// The user may drag this channel to another place in the list (Manage Channels, a server channel).
    pub can_move: bool,
}

pub(crate) fn guild_item(s: &Store, g: Id) -> GuildItem {
    let guild = s.guilds.get(&g);
    GuildItem {
        id: g.to_string(),
        name: guild.map(|x| x.name().to_string()).unwrap_or_default(),
        icon_url: guild.and_then(|x| x.icon_url(96)),
        unread: s.guild_has_unread(g),
        mentions: s.guild_mentions(g),
        muted: s.is_guild_muted(g),
    }
}

pub(crate) fn channel_item(s: &Store, c: &Channel) -> ChannelItem {
    let perms = s.permissions(c);
    ChannelItem {
        id: c.id.to_string(),
        guild_id: c.guild_id.map(|g| g.to_string()),
        parent_id: c.parent_id.map(|p| p.to_string()),
        kind: kind(c),
        // DMs have no channel name: title them by their recipients, like the DM list.
        name: match c.name.clone().filter(|n| !n.is_empty()) {
            Some(n) => n,
            None if c.guild_id.is_none() => dm_item(s, c).name,
            None => String::new(),
        },
        // Topics use the same markdown as messages; show them as resolved plain text.
        topic: c
            .topic
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| rich(s, c.guild_id, t).text),
        unread: c.is_text_like() && s.is_unread(c) && !s.is_channel_muted(c),
        mentions: s.mention_count(c.id),
        muted: s.is_channel_muted(c),
        nsfw: c.nsfw,
        can_send: perms & unison_core::store::perm::SEND_MESSAGES != 0,
        can_invite: perms & unison_core::store::perm::CREATE_INSTANT_INVITE != 0,
        can_move: c.guild_id.is_some() && !c.is_thread() && perms & unison_core::store::perm::MANAGE_CHANNELS != 0,
    }
}

/// Something at the top of the rail to put another entry before.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum FolderAnchor {
    Guild { guild_id: String },
    Folder { folder_id: i64 },
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum FolderPlace {
    /// Top level, before an entry (the end when `None`).
    Top { before: Option<FolderAnchor> },
    /// Inside a folder, before one of its servers (the end when `None`).
    InFolder { folder_id: i64, before: Option<String> },
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum FolderEdit {
    MoveGuild { guild_id: String, to: FolderPlace },
    /// Drop `guild_id` onto `onto_id`: a new folder holds both, or `guild_id` joins `onto_id`'s folder.
    MakeFolder { guild_id: String, onto_id: String },
    MoveFolder { folder_id: i64, before: Option<FolderAnchor> },
    Rename { folder_id: i64, name: String },
    /// `0xRRGGBB`, or `None` for the default colour.
    Recolor { folder_id: i64, color: Option<u32> },
}

impl FolderEdit {
    fn into_core(self) -> unison_core::folders::Edit {
        use unison_core::folders::{Anchor, Edit, Place};
        let anchor = |a: FolderAnchor| match a {
            FolderAnchor::Guild { guild_id } => Anchor::Guild(id(&guild_id)),
            FolderAnchor::Folder { folder_id } => Anchor::Folder(folder_id),
        };
        match self {
            FolderEdit::MoveGuild { guild_id, to } => Edit::MoveGuild {
                guild: id(&guild_id),
                to: match to {
                    FolderPlace::Top { before } => Place::Top { before: before.map(anchor) },
                    FolderPlace::InFolder { folder_id, before } => Place::InFolder { id: folder_id, before: before.map(|b| id(&b)) },
                },
            },
            FolderEdit::MakeFolder { guild_id, onto_id } => Edit::MakeFolder { guild: id(&guild_id), onto: id(&onto_id) },
            FolderEdit::MoveFolder { folder_id, before } => Edit::MoveFolder { id: folder_id, before: before.map(anchor) },
            FolderEdit::Rename { folder_id, name } => Edit::Rename { id: folder_id, name },
            FolderEdit::Recolor { folder_id, color } => Edit::Recolor { id: folder_id, color: color.map(u64::from) },
        }
    }
}

/// What the user is dragging in the rail.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum RailDragged {
    Guild { guild_id: String },
    Folder { folder_id: i64 },
}

/// What it was dropped on.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum RailDropTarget {
    Guild { guild_id: String },
    Folder { folder_id: i64 },
    GuildInFolder { guild_id: String, folder_id: i64 },
    /// The empty space below the last entry.
    End,
}

/// Where in the target the pointer was when the drag ended.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailDropZone {
    /// The upper part: put the dragged entry before the target.
    Before,
    /// The middle: onto the target (make or fill a folder).
    Onto,
    /// The lower part: put the dragged entry after the target.
    After,
}

impl RailDropTarget {
    /// The edit a drop means. `layout` is the current top-level order, needed to turn "after X"
    /// into "before whatever follows X".
    fn edit(dragged: RailDragged, target: RailDropTarget, zone: RailDropZone, layout: &[unison_core::model::GuildFolder]) -> Option<FolderEdit> {
        use FolderAnchor as A;
        let top = |id: &str| layout.iter().position(|f| f.id.is_none() && f.guild_ids.first().map(|g| g.to_string()).as_deref() == Some(id));
        let anchor_at = |i: usize| -> Option<A> {
            layout.get(i).and_then(|f| match f.id {
                Some(folder_id) => Some(A::Folder { folder_id }),
                None => f.guild_ids.first().map(|g| A::Guild { guild_id: g.to_string() }),
            })
        };
        // The entry before which a drop "before/after the target" lands (None: the end).
        let slot = |target_index: Option<usize>| -> Option<Option<A>> {
            let i = target_index?;
            Some(match zone {
                RailDropZone::After => anchor_at(i + 1),
                _ => anchor_at(i),
            })
        };
        let folder_index = |folder_id: i64| layout.iter().position(|f| f.id == Some(folder_id));
        Some(match (dragged, target) {
            (RailDragged::Guild { guild_id }, RailDropTarget::Guild { guild_id: t }) if zone == RailDropZone::Onto => FolderEdit::MakeFolder { guild_id, onto_id: t },
            (RailDragged::Guild { guild_id }, RailDropTarget::Guild { guild_id: t }) => FolderEdit::MoveGuild { guild_id, to: FolderPlace::Top { before: slot(top(&t))? } },
            (RailDragged::Guild { guild_id }, RailDropTarget::Folder { folder_id }) if zone == RailDropZone::Onto => {
                FolderEdit::MoveGuild { guild_id, to: FolderPlace::InFolder { folder_id, before: None } }
            }
            (RailDragged::Guild { guild_id }, RailDropTarget::Folder { folder_id }) => FolderEdit::MoveGuild { guild_id, to: FolderPlace::Top { before: slot(folder_index(folder_id))? } },
            (RailDragged::Guild { guild_id }, RailDropTarget::GuildInFolder { guild_id: t, folder_id }) => {
                let members = &layout.iter().find(|f| f.id == Some(folder_id))?.guild_ids;
                let at = members.iter().position(|g| g.to_string() == t)?;
                let before = if zone == RailDropZone::After { members.get(at + 1).map(|g| g.to_string()) } else { Some(t) };
                FolderEdit::MoveGuild { guild_id, to: FolderPlace::InFolder { folder_id, before } }
            }
            (RailDragged::Guild { guild_id }, RailDropTarget::End) => FolderEdit::MoveGuild { guild_id, to: FolderPlace::Top { before: None } },
            (RailDragged::Folder { folder_id }, RailDropTarget::Guild { guild_id }) => FolderEdit::MoveFolder { folder_id, before: slot(top(&guild_id))? },
            (RailDragged::Folder { folder_id }, RailDropTarget::Folder { folder_id: t }) => FolderEdit::MoveFolder { folder_id, before: slot(folder_index(t))? },
            (RailDragged::Folder { folder_id }, RailDropTarget::End) => FolderEdit::MoveFolder { folder_id, before: None },
            (RailDragged::Folder { .. }, RailDropTarget::GuildInFolder { .. }) => return None,
        })
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FolderColor {
    pub name: String,
    pub rgb: u32,
}

/// The colours offered for a folder.
#[uniffi::export]
pub fn folder_palette() -> Vec<FolderColor> {
    unison_core::folders::PALETTE.iter().map(|(n, c)| FolderColor { name: n.to_string(), rgb: *c }).collect()
}

#[uniffi::export]
impl UnisonEngine {
    pub fn rail(&self, account_id: String) -> Vec<RailItem> {
        self.with_store(&account_id, |s| {
            s.rail()
                .into_iter()
                .map(|e| match e {
                    RailEntry::Guild(g) => RailItem::Guild {
                        guild: guild_item(s, g),
                    },
                    RailEntry::Folder(f) => RailItem::Folder {
                        id: f.id,
                        name: f.name.clone(),
                        color: f.color.map(|c| c as u32),
                        guilds: f.guild_ids.iter().map(|g| guild_item(s, *g)).collect(),
                    },
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// A drag in the rail ended on `target`; does what that means (see `RailDropTarget::edit`).
    pub fn drop_on_rail(&self, account_id: String, dragged: RailDragged, target: RailDropTarget, zone: RailDropZone, gesture: UserGesture) {
        let Some(layout) = self.with_store(&account_id, |s| s.folder_layout()) else { return };
        if let Some(edit) = RailDropTarget::edit(dragged, target, zone, &layout) {
            self.edit_folders(account_id, edit, gesture);
        }
    }

    /// Which zones a drag may land in over this target, so the shell only offers real outcomes:
    /// folders and entries inside folders only reorder (before/after); a server onto a server or a
    /// folder may also go "onto".
    pub fn rail_drop_zones(&self, dragged: RailDragged, target: RailDropTarget) -> Vec<RailDropZone> {
        use RailDropZone::*;
        match (dragged, target) {
            (RailDragged::Guild { .. }, RailDropTarget::Guild { .. } | RailDropTarget::Folder { .. }) => vec![Before, Onto, After],
            (_, RailDropTarget::End) => vec![Onto],
            (RailDragged::Folder { .. }, RailDropTarget::GuildInFolder { .. }) => vec![],
            _ => vec![Before, After],
        }
    }

    pub fn folder_open(&self, folder_id: i64) -> bool {
        self.inner.local.open_folders().contains(&folder_id)
    }

    pub fn set_folder_open(&self, folder_id: i64, open: bool) {
        self.inner.local.set_folder_open(folder_id, open);
    }

    /// Rearrange the server rail: move a server, make or fill a folder, rename, recolor, reorder.
    pub fn edit_folders(&self, account_id: String, edit: FolderEdit, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, intent| async move { s.edit_folders(intent, edit.into_core()).await });
    }

    pub fn guild(&self, account_id: String, guild_id: String) -> Option<GuildItem> {
        self.with_store(&account_id, |s| {
            s.guilds
                .contains_key(&id(&guild_id))
                .then(|| guild_item(s, id(&guild_id)))
        })
        .flatten()
    }

    /// Sidebar rows: uncategorised channels first, then each category followed by its visible children.
    pub fn guild_channels(&self, account_id: String, guild_id: String) -> Vec<ChannelItem> {
        self.with_store(&account_id, |s| {
            let g = id(&guild_id);
            let Some(ids) = s.guild_channels.get(&g) else {
                return vec![];
            };
            let visible: Vec<&Channel> = ids
                .iter()
                .filter_map(|c| s.channels.get(c))
                .filter(|c| !c.is_thread() && s.can_view(c))
                .collect();
            // Active, unarchived threads appear under their parent, like the official sidebar.
            // Only joined threads that have something for you or are recent (`thread_in_sidebar`).
            let now = now_ms();
            let open = self.inner.session(id(&account_id)).and_then(|sess| sess.selected_channel());
            let threads: Vec<&Channel> = ids
                .iter()
                .filter_map(|c| s.channels.get(c))
                .filter(|c| s.thread_in_sidebar(c, now, open))
                .collect();
            let mut out = Vec::with_capacity(visible.len() + threads.len());
            let push = |out: &mut Vec<ChannelItem>, c: &Channel| {
                out.push(channel_item(s, c));
                for t in threads.iter().filter(|t| t.parent_id == Some(c.id)) {
                    out.push(channel_item(s, t));
                }
            };
            for c in visible
                .iter()
                .filter(|c| c.parent_id.is_none() && c.kind != channel_type::GUILD_CATEGORY)
            {
                push(&mut out, c);
            }
            for cat in visible
                .iter()
                .filter(|c| c.kind == channel_type::GUILD_CATEGORY)
            {
                let children: Vec<_> = visible
                    .iter()
                    .filter(|c| c.parent_id == Some(cat.id))
                    .collect();
                if children.is_empty() {
                    continue;
                }
                out.push(channel_item(s, cat));
                for c in children {
                    push(&mut out, c);
                }
            }
            out
        })
        .unwrap_or_default()
    }

    pub fn channel(&self, account_id: String, channel_id: String) -> Option<ChannelItem> {
        self.with_store(&account_id, |s| {
            s.channels.get(&id(&channel_id)).map(|c| channel_item(s, c))
        })
        .flatten()
    }

    /// `gesture` is `None` when the shell restores a view on its own (launch); nothing is acked then.
    pub fn open_channel(
        &self,
        account_id: String,
        guild_id: Option<String>,
        channel_id: String,
        gesture: Option<UserGesture>,
    ) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.map(UserGesture::intent);
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = s
                .open_channel(guild_id.as_deref().map(id), id(&channel_id), intent)
                .await
            {
                listener.on_error(account_id, e.user_message());
            }
        });
    }
}

/// What an invite leads to, for the join step.
#[derive(uniffi::Record, Clone, Debug)]
pub struct InvitePreviewItem {
    pub code: String,
    pub guild_id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub description: String,
    pub members: Option<u32>,
    pub online: Option<u32>,
    pub channel: String,
}

fn account_session(e: &UnisonEngine, account_id: &str) -> Result<Arc<unison_core::session::Session>, FfiError> {
    e.inner.session(id(account_id)).ok_or_else(|| FfiError::Failed { message: "no such account".into() })
}

#[uniffi::export]
impl UnisonEngine {
    /// Read an invite link (or bare code) and say what server it leads to.
    pub async fn preview_invite(&self, account_id: String, input: String) -> Result<InvitePreviewItem, FfiError> {
        let s = account_session(self, &account_id)?;
        let jh = self.inner.rt.spawn(async move { s.preview_invite(&input).await });
        let p = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(InvitePreviewItem { code: p.code, guild_id: p.guild_id.to_string(), name: p.name, icon_url: p.icon_url, description: p.description, members: p.members, online: p.online, channel: p.channel })
    }

    /// Join a server by invite. Returns the server's id once it is in the rail.
    pub async fn join_server(&self, account_id: String, input: String, gesture: UserGesture) -> Result<String, FfiError> {
        let s = account_session(self, &account_id)?;
        let intent = gesture.intent();
        let jh = self.inner.rt.spawn(async move { s.join_server(intent, &input).await });
        Ok(jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??.to_string())
    }

    /// Create a server. Returns its id once it is in the rail.
    pub async fn create_server(&self, account_id: String, name: String, content_type: String, icon: Option<Vec<u8>>, gesture: UserGesture) -> Result<String, FfiError> {
        let s = account_session(self, &account_id)?;
        let intent = gesture.intent();
        let jh = self.inner.rt.spawn(async move { s.create_server(intent, &name, icon.map(|b| (content_type, b))).await });
        Ok(jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??.to_string())
    }
}

#[cfg(test)]
mod rail_drop_tests {
    use super::*;
    use unison_core::model::GuildFolder;
    use unison_core::snowflake::Snowflake as S;

    fn single(n: u64) -> GuildFolder {
        GuildFolder { id: None, name: None, color: None, guild_ids: vec![S(n)] }
    }

    #[test]
    fn after_means_before_the_next_entry() {
        let layout = [single(1), single(2), single(3)];
        let d = || RailDragged::Guild { guild_id: "1".into() };
        let t = || RailDropTarget::Guild { guild_id: "2".into() };
        match RailDropTarget::edit(d(), t(), RailDropZone::After, &layout) {
            Some(FolderEdit::MoveGuild { to: FolderPlace::Top { before: Some(FolderAnchor::Guild { guild_id }) }, .. }) => assert_eq!(guild_id, "3"),
            other => panic!("{other:?}"),
        }
        // After the last entry is the end of the rail.
        let last = RailDropTarget::Guild { guild_id: "3".into() };
        assert!(matches!(RailDropTarget::edit(d(), last, RailDropZone::After, &layout), Some(FolderEdit::MoveGuild { to: FolderPlace::Top { before: None }, .. })));
        assert!(matches!(RailDropTarget::edit(d(), t(), RailDropZone::Onto, &layout), Some(FolderEdit::MakeFolder { .. })));
    }
}
