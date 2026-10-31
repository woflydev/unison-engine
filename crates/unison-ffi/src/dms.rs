use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct DmItem {
    pub channel_id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub is_group: bool,
    pub unread: bool,
    pub mentions: u32,
    pub recipient_ids: Vec<String>,
    /// The other person's status (`online`, `idle`, `dnd`, `offline`) for a 1:1 DM; `None` for groups.
    pub status: Option<String>,
    pub muted: bool,
    /// The other person's custom status or activity for a 1:1 DM.
    pub activity: Option<presence::ActivityItem>,
}

pub(crate) fn dm_item(s: &Store, c: &Channel) -> DmItem {
    let names: Vec<String> = c
        .recipient_ids
        .iter()
        .map(|u| s.display_name(None, *u))
        .collect();
    let name = c.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| {
        if names.is_empty() {
            "Unnamed".into()
        } else {
            names.join(", ")
        }
    });
    let icon_url = if c.kind == channel_type::GROUP_DM {
        c.icon.as_ref().map(|i| {
            format!(
                "https://cdn.discordapp.com/channel-icons/{}/{}.webp?size=64",
                c.id, i
            )
        })
    } else {
        c.recipient_ids
            .first()
            .and_then(|u| s.users.get(u))
            .map(|u| u.avatar_url(64))
    };
    DmItem {
        channel_id: c.id.to_string(),
        name,
        icon_url,
        is_group: c.kind == channel_type::GROUP_DM,
        unread: s.is_unread(c),
        mentions: s.mention_count(c.id),
        recipient_ids: c.recipient_ids.iter().map(|u| u.to_string()).collect(),
        muted: s.is_channel_muted(c),
        activity: (c.kind == channel_type::DM)
            .then(|| c.recipient_ids.first().and_then(|u| presence::row_activity(s.user_activities(*u))))
            .flatten(),
        status: (c.kind == channel_type::DM)
            .then(|| c.recipient_ids.first().map(|u| s.presence(*u).to_string()))
            .flatten(),
    }
}

/// One person in a group DM's member pane.
#[derive(uniffi::Record, Clone, Debug)]
pub struct GroupMember {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
    pub status: String,
    pub is_owner: bool,
    pub is_me: bool,
}

/// A group DM holds at most this many people, the user included.
const GROUP_DM_MAX_MEMBERS: usize = 10;

/// People you can pick when starting a new group DM (everyone but yourself).
#[uniffi::export]
pub fn new_group_dm_capacity() -> u32 {
    GROUP_DM_MAX_MEMBERS as u32 - 1
}

#[uniffi::export]
impl UnisonEngine {
    /// How many more people can be added to this DM (1:1 DMs become groups, so they count too).
    pub fn dm_add_capacity(&self, account_id: String, channel_id: String) -> u32 {
        self.with_store(&account_id, |s| {
            let n = s.channels.get(&id(&channel_id)).map_or(2, |c| c.recipient_ids.len() + 1);
            GROUP_DM_MAX_MEMBERS.saturating_sub(n) as u32
        })
        .unwrap_or(0)
    }

    /// Members of a group DM, owner first, then by name; includes the signed-in user.
    pub fn group_members(&self, account_id: String, channel_id: String) -> Vec<GroupMember> {
        self.with_store(&account_id, |s| {
            let Some(c) = s.channels.get(&id(&channel_id)) else { return vec![] };
            let me = s.my_id();
            let mut ids: Vec<Id> = c.recipient_ids.clone();
            ids.extend(me);
            let mut out: Vec<GroupMember> = ids
                .into_iter()
                .map(|u| GroupMember {
                    user_id: u.to_string(),
                    name: s.display_name(None, u),
                    avatar_url: s.users.get(&u).map(|x| x.avatar_url(64)).unwrap_or_default(),
                    status: if Some(u) == me { if s.my_status.is_empty() { "online".into() } else { s.my_status.clone() } } else { s.presence(u).to_string() },
                    is_owner: c.owner_id == Some(u),
                    is_me: Some(u) == me,
                })
                .collect();
            out.sort_by(|a, b| b.is_owner.cmp(&a.is_owner).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            out
        })
        .unwrap_or_default()
    }

    /// Mark a DM read up to its newest message.
    pub fn mark_dm_read(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.mark_read(i, id(&channel_id)).await
        });
    }

    /// Close a 1:1 DM, or leave a group DM. Refused for anything that isn't a DM.
    pub fn set_dm_muted(
        &self,
        account_id: String,
        channel_id: String,
        muted: bool,
        gesture: UserGesture,
    ) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.set_dm_muted(i, id(&channel_id), muted).await
        });
    }

    pub fn close_dm(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.close_dm(i, id(&channel_id)).await
        });
    }

    pub fn private_channels(&self, account_id: String) -> Vec<DmItem> {
        self.with_store(&account_id, |s| {
            s.private_channels
                .iter()
                .filter_map(|c| s.channels.get(c))
                .filter(|c| !s.is_message_request(c))
                .map(|c| dm_item(s, c))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Pending message requests (`spam`: the spam-filtered ones), newest first.
    pub fn message_requests(&self, account_id: String, spam: bool) -> Vec<DmItem> {
        self.with_store(&account_id, |s| {
            s.message_requests(spam).iter().filter_map(|c| s.channels.get(c)).map(|c| dm_item(s, c)).collect()
        })
        .unwrap_or_default()
    }

    /// Number of non-spam message requests (the badge on the Message Requests row).
    pub fn message_request_count(&self, account_id: String) -> u32 {
        self.with_store(&account_id, |s| s.message_requests(false).len() as u32).unwrap_or(0)
    }

    pub fn accept_message_request(&self, account_id: String, channel_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.accept_message_request(i, id(&channel_id)).await
        });
    }

    /// Open (or create) the DM with a user; navigation arrives via `on_navigate`.
    pub fn open_dm(&self, account_id: String, user_id: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let listener = self.listener.clone();
        // Reuse an existing 1:1 DM instead of asking the API.
        let existing = {
            let st = s.store.read();
            st.private_channels
                .iter()
                .filter_map(|c| st.channels.get(c))
                .find(|c| c.kind == channel_type::DM && c.recipient_ids == [id(&user_id)])
                .map(|c| c.id)
        };
        if let Some(c) = existing {
            listener.on_navigate(account_id, None, c.to_string());
            return;
        }
        self.inner.rt.spawn(async move {
            match s.open_dm(intent, id(&user_id)).await {
                Ok(c) => listener.on_navigate(account_id, None, c.to_string()),
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }
}
