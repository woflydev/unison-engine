//! What each object offers, and when: the action lists behind message bars and menus, person menus
//! and DM menus. Every shell draws the list it is given (icons and handlers are the shell's);
//! none re-derives who may pin, delete, invite or call.
use super::*;

use unison_core::store::perm;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionId {
    // Messages
    Reply,
    Edit,
    Forward,
    AddReaction,
    OpenThread,
    CreateThread,
    CopyText,
    CopyLink,
    Pin,
    Unpin,
    MarkUnread,
    CopyMessageId,
    DeleteMessage,
    // People
    Profile,
    Mention,
    Message,
    Call,
    RemoveFriend,
    AcceptFriend,
    IgnoreFriend,
    CancelFriend,
    AddFriend,
    Unblock,
    Block,
    CopyUserId,
    /// A nickname only you see, for a friend.
    FriendNickname,
    // Moderation
    ChangeNickname,
    Timeout,
    EditRoles,
    SaveForLater,
    RemoveSaved,
    RemindMe,
    RemoveTimeout,
    Kick,
    Ban,
    // DMs
    MarkRead,
    Mute,
    Unmute,
    AddToDm,
    LeaveGroup,
    CloseDm,
    CopyChannelId,
    /// Save the loaded messages as a text file.
    ExportChat,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Action {
    pub id: ActionId,
    /// English default; shells may localise by `id`.
    pub title: String,
    pub enabled: bool,
    pub destructive: bool,
    /// Consecutive actions in different groups are separated by a divider.
    pub group: u8,
}

fn a(id: ActionId, title: &str, group: u8) -> Action {
    Action { id, title: title.into(), enabled: true, destructive: false, group }
}

fn danger(id: ActionId, title: &str, group: u8) -> Action {
    Action { destructive: true, ..a(id, title, group) }
}

/// Where a message's actions are shown.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionSurface {
    /// The hover bar's quick buttons (the shell adds its own "More").
    Bar,
    /// The hover bar while Shift is held.
    ExtendedBar,
    /// The full context menu.
    Menu,
}

#[uniffi::export]
impl UnisonEngine {
    /// Whether the user may pin and unpin in this channel (DMs: always; servers: Manage Messages).
    pub fn can_manage_pins(&self, account_id: String, channel_id: String) -> bool {
        self.with_store(&account_id, |s| {
            s.channels.get(&id(&channel_id)).is_some_and(|c| c.guild_id.is_none() || s.permissions(c) & perm::MANAGE_MESSAGES != 0)
        })
        .unwrap_or(false)
    }

    pub fn message_actions(&self, account_id: String, channel_id: String, message_id: String, surface: ActionSurface) -> Vec<Action> {
        self.with_store(&account_id, |s| {
            let (c, mid) = (id(&channel_id), id(&message_id));
            let Some(m) = s.history.get(&c).and_then(|h| h.messages.iter().find(|m| m.id == mid)) else { return vec![] };
            let ch = s.channels.get(&c);
            let me = s.my_id();
            let mine = Some(m.author.id) == me;
            let system = crate::messages::system_text(s, ch.and_then(|c| c.guild_id), m).is_some();
            let supported = model::message_type::is_supported(m.kind);
            let can_manage = ch.is_some_and(|c| c.guild_id.is_none() || s.permissions(c) & perm::MANAGE_MESSAGES != 0);
            let can_pin = can_manage;
            let can_delete = mine || ch.is_some_and(|c| c.guild_id.is_some() && s.permissions(c) & perm::MANAGE_MESSAGES != 0);
            let forwardable = !system && supported;
            let thread = m.thread.is_some();
            let in_text_guild = ch.is_some_and(|c| c.guild_id.is_some() && matches!(c.kind, channel_type::GUILD_TEXT | channel_type::GUILD_ANNOUNCEMENT));
            let pin = if m.pinned { a(ActionId::Unpin, "Unpin", 1) } else { a(ActionId::Pin, "Pin", 1) };

            let mut out = Vec::new();
            match surface {
                ActionSurface::Bar | ActionSurface::ExtendedBar => {
                    out.push(a(ActionId::AddReaction, "Add Reaction", 0));
                    if surface == ActionSurface::ExtendedBar && mine && !system {
                        out.push(a(ActionId::Edit, "Edit", 0));
                    }
                    out.push(a(ActionId::Reply, "Reply", 0));
                    if forwardable {
                        out.push(a(ActionId::Forward, "Forward", 0));
                    }
                    if surface == ActionSurface::Bar && mine && !system {
                        out.push(a(ActionId::Edit, "Edit", 0));
                    }
                    if surface == ActionSurface::ExtendedBar {
                        if can_pin {
                            out.push(pin);
                        }
                        out.push(a(ActionId::MarkUnread, "Mark Unread", 0));
                        out.push(a(ActionId::CopyText, "Copy Text", 0));
                        out.push(a(ActionId::CopyLink, "Copy Message Link", 0));
                        out.push(a(ActionId::CopyMessageId, "Copy Message ID", 0));
                        if can_delete {
                            out.push(danger(ActionId::DeleteMessage, "Delete Message", 0));
                        }
                    }
                }
                ActionSurface::Menu => {
                    out.push(a(ActionId::Reply, "Reply", 0));
                    if mine && !system {
                        out.push(a(ActionId::Edit, "Edit", 0));
                    }
                    if forwardable {
                        out.push(a(ActionId::Forward, "Forward…", 0));
                    }
                    out.push(a(ActionId::AddReaction, "Add Reaction…", 0));
                    if thread {
                        out.push(a(ActionId::OpenThread, "Open Thread", 1));
                    } else if forwardable && in_text_guild {
                        out.push(a(ActionId::CreateThread, "Create Thread", 1));
                    }
                    out.push(a(ActionId::CopyText, "Copy Text", 1));
                    out.push(a(ActionId::CopyLink, "Copy Message Link", 1));
                    if can_pin {
                        out.push(pin);
                    }
                    out.push(a(ActionId::MarkUnread, "Mark Unread", 1));
                    if !system {
                        let saved = self.inner.local.saved().iter().any(|s| s.account == id(&account_id) && s.message == mid);
                        out.push(if saved { a(ActionId::RemoveSaved, "Remove from Saved", 1) } else { a(ActionId::SaveForLater, "Save for Later", 1) });
                        out.push(a(ActionId::RemindMe, "Remind Me…", 1));
                    }
                    if can_delete {
                        out.push(danger(ActionId::DeleteMessage, "Delete Message", 2));
                    }
                }
            }
            out
        })
        .unwrap_or_default()
    }

    /// A person's menu. `channel_id` is the open conversation (for "Mention", which needs a place to type).
    pub fn user_actions(&self, account_id: String, user_id: String, channel_id: Option<String>) -> Vec<Action> {
        let me = self.with_store(&account_id, |s| s.my_id().map(|m| m.to_string())).flatten();
        let is_me = me.as_deref() == Some(user_id.as_str());
        let can_type = channel_id.as_ref().is_some_and(|c| {
            self.with_store(&account_id, |s| s.channels.get(&id(c)).is_some_and(|ch| s.permissions(ch) & perm::SEND_MESSAGES != 0)).unwrap_or(false)
        });
        let rel = self.relationship(account_id, user_id);
        let mut out = vec![a(ActionId::Profile, "Profile", 0)];
        if can_type {
            out.push(a(ActionId::Mention, "Mention", 0));
        }
        if !is_me {
            out.push(a(ActionId::Message, "Message", 0));
            out.push(a(ActionId::Call, "Call", 0));
            match rel {
                RelationshipKind::Friend => {
                    out.push(a(ActionId::FriendNickname, "Edit Friend Nickname…", 1));
                    out.push(danger(ActionId::RemoveFriend, "Remove Friend", 1));
                }
                RelationshipKind::Incoming => {
                    out.push(a(ActionId::AcceptFriend, "Accept Friend Request", 1));
                    out.push(a(ActionId::IgnoreFriend, "Ignore Friend Request", 1));
                }
                RelationshipKind::Outgoing => out.push(a(ActionId::CancelFriend, "Cancel Friend Request", 1)),
                RelationshipKind::Blocked => {}
                RelationshipKind::Other => out.push(a(ActionId::AddFriend, "Add Friend", 1)),
            }
            if rel == RelationshipKind::Blocked {
                out.push(a(ActionId::Unblock, "Unblock", 1));
            } else {
                out.push(danger(ActionId::Block, "Block", 1));
            }
        }
        out.push(a(ActionId::CopyUserId, "Copy User ID", 2));
        out
    }

    /// A DM's menu. For a 1:1 DM it also carries the other person's friend/block actions.
    pub fn dm_actions(&self, account_id: String, channel_id: String) -> Vec<Action> {
        let Some(dm) = self.private_channels(account_id.clone()).into_iter().find(|d| d.channel_id == channel_id) else { return vec![] };
        let mut out = Vec::new();
        let user = (!dm.is_group).then(|| dm.recipient_ids.first().cloned()).flatten();
        if user.is_some() {
            out.push(a(ActionId::Profile, "Profile", 0));
        }
        out.push(a(ActionId::Call, "Call", 0));
        out.push(a(ActionId::AddToDm, if dm.is_group { "Add Friends to Group…" } else { "Add Friends to DM…" }, 0));
        if let Some(u) = &user {
            let people = self.user_actions(account_id, u.clone(), None);
            out.extend(people.into_iter().filter(|x| !matches!(x.id, ActionId::Profile | ActionId::Message | ActionId::Call | ActionId::CopyUserId)).map(|mut x| {
                x.group = 1;
                x
            }));
        }
        out.push(Action { enabled: dm.unread || dm.mentions > 0, ..a(ActionId::MarkRead, "Mark as Read", 2) });
        out.push(if dm.muted { a(ActionId::Unmute, "Unmute", 2) } else { a(ActionId::Mute, "Mute", 2) });
        out.push(if dm.is_group { danger(ActionId::LeaveGroup, "Leave Group…", 3) } else { a(ActionId::CloseDm, "Close DM", 3) });
        if user.is_some() {
            out.push(a(ActionId::CopyUserId, "Copy User ID", 4));
        }
        out.push(a(ActionId::ExportChat, "Export Loaded Messages…", 4));
        out.push(a(ActionId::CopyChannelId, "Copy Channel ID", 4));
        out
    }
}
