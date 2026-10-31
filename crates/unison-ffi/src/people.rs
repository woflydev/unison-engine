use super::*;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum RelationshipKind {
    Friend,
    Blocked,
    Incoming,
    Outgoing,
    Other,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct FriendItem {
    pub user_id: String,
    pub name: String,
    pub username: String,
    pub avatar_url: String,
    pub kind: RelationshipKind,
}

#[uniffi::export]
impl UnisonEngine {
    pub fn friends(&self, account_id: String) -> Vec<FriendItem> {
        self.with_store(&account_id, |s| {
            let mut v: Vec<FriendItem> = s
                .relationships
                .iter()
                .filter_map(|r| {
                    let uid = r.user_id.or(r.user.as_ref().map(|u| u.id)).unwrap_or(r.id);
                    let u = s.users.get(&uid).cloned().or_else(|| r.user.clone())?;
                    Some(FriendItem {
                        user_id: uid.to_string(),
                        name: r
                            .nickname
                            .clone()
                            .unwrap_or_else(|| u.display_name().to_string()),
                        username: u.username.clone(),
                        avatar_url: u.avatar_url(64),
                        kind: match r.kind {
                            1 => RelationshipKind::Friend,
                            2 => RelationshipKind::Blocked,
                            3 => RelationshipKind::Incoming,
                            4 => RelationshipKind::Outgoing,
                            _ => RelationshipKind::Other,
                        },
                    })
                })
                .collect();
            v.sort_by_key(|f| f.name.to_lowercase());
            v
        })
        .unwrap_or_default()
    }

    /// How the user relates to us: friend, blocked, incoming or outgoing request; `Other` for none.
    pub fn relationship(&self, account_id: String, user_id: String) -> RelationshipKind {
        let u = id(&user_id);
        self.with_store(&account_id, |s| {
            s.relationships
                .iter()
                .find(|r| r.user_id.or(r.user.as_ref().map(|x| x.id)).unwrap_or(r.id) == u)
                .map_or(RelationshipKind::Other, |r| match r.kind {
                    1 => RelationshipKind::Friend,
                    2 => RelationshipKind::Blocked,
                    3 => RelationshipKind::Incoming,
                    4 => RelationshipKind::Outgoing,
                    _ => RelationshipKind::Other,
                })
        })
        .unwrap_or(RelationshipKind::Other)
    }

    /// Send a friend request, or accept an incoming one.
    pub fn add_friend(&self, account_id: String, user_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.rest.put_relationship(i, id(&user_id), None).await
        });
    }

    pub fn block_user(&self, account_id: String, user_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.rest.put_relationship(i, id(&user_id), Some(2)).await
        });
    }

    /// Unfriend, cancel a request or unblock.
    pub fn remove_relationship(&self, account_id: String, user_id: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.rest.delete_relationship(i, id(&user_id)).await
        });
    }

    /// Call a user: open the 1:1 DM if there isn't one, join its call and ring them.
    pub fn call_user(&self, account_id: String, user_id: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let listener = self.listener.clone();
        let _guard = self.inner.rt.enter();
        let existing = {
            let st = s.store.read();
            st.private_channels
                .iter()
                .filter_map(|c| st.channels.get(c))
                .find(|c| c.kind == channel_type::DM && c.recipient_ids == [id(&user_id)])
                .map(|c| c.id)
        };
        if let Some(c) = existing {
            if let Err(e) = s.start_dm_call(intent, c) {
                listener.on_error(account_id, e.user_message());
            }
            return;
        }
        self.inner.rt.spawn(async move {
            let (open, call) = intent.split();
            match s.open_dm(open, id(&user_id)).await {
                Ok(c) => {
                    if let Err(e) = s.start_dm_call(call, c) {
                        listener.on_error(account_id, e.user_message());
                    }
                }
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }

    /// The current user's status: online, idle, dnd or invisible.
    pub fn own_status(&self, account_id: String) -> String {
        self.with_store(&account_id, |s| s.my_status.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "online".into())
    }
}
