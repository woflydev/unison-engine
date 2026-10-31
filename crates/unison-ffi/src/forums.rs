use super::*;

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum ForumSort {
    LatestActivity,
    CreationDate,
}

/// What the forum header is set to: sort, tag filter and whether every tag must match.
#[derive(uniffi::Record, Clone, Debug)]
pub struct ForumView {
    pub sort: ForumSort,
    pub tag_ids: Vec<String>,
    pub match_all: bool,
}

impl ForumView {
    pub(crate) fn query(&self) -> ForumQuery {
        ForumQuery::new(
            match self.sort {
                ForumSort::LatestActivity => SortOrder::LatestActivity,
                ForumSort::CreationDate => SortOrder::CreationDate,
            },
            self.tag_ids.iter().map(|t| id(t)).collect(),
            if self.match_all {
                TagMatch::All
            } else {
                TagMatch::Some
            },
        )
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ForumTagItem {
    pub id: String,
    pub name: String,
    /// Unicode emoji, or a custom emoji's image URL.
    pub emoji: Option<String>,
    pub emoji_url: Option<String>,
    /// Only moderators can apply it.
    pub moderated: bool,
    /// The user may apply it (moderated tags need Manage Threads).
    pub usable: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ForumInfo {
    pub channel: ChannelItem,
    pub is_media: bool,
    pub tags: Vec<ForumTagItem>,
    /// The forum's own defaults, for the header's initial state.
    pub default_view: ForumView,
    /// Post guidelines (the forum's topic), as plain text.
    pub guidelines: Option<String>,
    pub can_post: bool,
    /// Manage Threads: may apply moderated tags.
    pub can_moderate: bool,
    pub require_tag: bool,
    /// Whether the archived list for the current view can page further, and is paging now.
    pub has_more: bool,
    pub loading: bool,
    pub loaded: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ForumPostItem {
    pub id: String,
    pub title: String,
    pub author_id: Option<String>,
    pub author_name: String,
    pub author_avatar_url: Option<String>,
    pub tag_ids: Vec<String>,
    /// Replies, not counting the opening message.
    pub reply_count: u32,
    pub created_ms: u64,
    pub last_activity_ms: u64,
    /// `None` until the first message is loaded; empty when it has no text.
    pub preview: Option<String>,
    pub preview_loaded: bool,
    pub thumbnail_url: Option<String>,
    pub thumbnail_width: Option<u32>,
    pub thumbnail_height: Option<u32>,
    pub pinned: bool,
    pub archived: bool,
    pub locked: bool,
    pub joined: bool,
    pub unread: bool,
}

pub(crate) fn forum_post_item(s: &Store, p: &Channel) -> ForumPostItem {
    let guild = p.guild_id;
    let first = s.previews.first.get(&p.id);
    let first_msg = first.and_then(Option::as_ref);
    let author = p.owner_id.or(first_msg.map(|m| m.author.id));
    let thumb = first_msg.and_then(forum::thumbnail);
    let preview = first.map(|m| {
        m.as_ref()
            .map(|m| preview_text(s, guild, &m.content, 280))
            .unwrap_or_default()
    });
    ForumPostItem {
        id: p.id.to_string(),
        title: p.name.clone().unwrap_or_default(),
        author_id: author.map(|a| a.to_string()),
        author_name: author.map(|a| s.display_name(guild, a)).unwrap_or_default(),
        author_avatar_url: author
            .and_then(|a| s.users.get(&a))
            .map(|u| u.avatar_url(64)),
        tag_ids: p.applied_tags.iter().map(Id::to_string).collect(),
        reply_count: p.message_count.unwrap_or(0),
        created_ms: p.id.timestamp_ms(),
        last_activity_ms: forum::last_activity(p).timestamp_ms(),
        preview_loaded: first.is_some(),
        preview,
        thumbnail_url: thumb.as_ref().map(|t| t.0.clone()),
        thumbnail_width: thumb.as_ref().and_then(|t| t.1),
        thumbnail_height: thumb.as_ref().and_then(|t| t.2),
        pinned: p.flags & model::channel_flags::PINNED != 0,
        archived: p.is_archived(),
        locked: p.is_locked(),
        joined: p.member.is_some(),
        unread: p.member.is_some() && s.is_unread(p) && !s.is_channel_muted(p),
    }
}

#[uniffi::export]
impl UnisonEngine {
    pub fn forum_info(
        &self,
        account_id: String,
        channel_id: String,
        view: ForumView,
    ) -> Option<ForumInfo> {
        self.with_store(&account_id, |s| {
            let c = s.channels.get(&id(&channel_id))?;
            if !c.is_forum_like() {
                return None;
            }
            let list = s.forum_list(c.id, &view.query());
            let perms = s.permissions(c);
            Some(ForumInfo {
                channel: channel_item(s, c),
                is_media: c.kind == channel_type::GUILD_MEDIA,
                tags: c
                    .available_tags
                    .iter()
                    .map(|t| ForumTagItem {
                        id: t.id.to_string(),
                        name: t.name.clone(),
                        emoji: t
                            .emoji_name
                            .clone()
                            .filter(|e| !e.is_empty() && t.emoji_id.is_none()),
                        emoji_url: t
                            .emoji_id
                            .filter(|e| e.0 != 0)
                            .map(|e| emoji_url(&e.to_string(), false, false)),
                        moderated: t.moderated,
                        usable: !t.moderated || perms & unison_core::store::perm::MANAGE_THREADS != 0,
                    })
                    .collect(),
                default_view: ForumView {
                    sort: match SortOrder::from_default(c.default_sort_order) {
                        SortOrder::LatestActivity => ForumSort::LatestActivity,
                        SortOrder::CreationDate => ForumSort::CreationDate,
                    },
                    tag_ids: vec![],
                    match_all: TagMatch::from_default(c.default_tag_setting.as_deref())
                        == TagMatch::All,
                },
                guidelines: c
                    .topic
                    .as_deref()
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| rich(s, c.guild_id, t).text),
                can_post: perms & unison_core::store::perm::SEND_MESSAGES != 0,
                can_moderate: perms & unison_core::store::perm::MANAGE_THREADS != 0,
                require_tag: c.flags & model::channel_flags::REQUIRE_TAG != 0,
                has_more: list.is_some_and(|l| l.has_more),
                loading: list.is_some_and(|l| l.loading),
                loaded: list.is_some_and(|l| l.loaded),
            })
        })
        .flatten()
    }

    /// The posts a forum lists for `view`, top to bottom.
    pub fn forum_posts(
        &self,
        account_id: String,
        channel_id: String,
        view: ForumView,
    ) -> Vec<ForumPostItem> {
        self.with_store(&account_id, |s| {
            let forum = id(&channel_id);
            s.forum_posts(forum, &view.query())
                .into_iter()
                .filter_map(|p| s.channels.get(&p))
                .map(|p| forum_post_item(s, p))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Show a forum. `gesture` is `None` when the shell restores it on launch.
    pub fn open_forum(
        &self,
        account_id: String,
        guild_id: String,
        channel_id: String,
        view: ForumView,
    ) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = s
                .open_forum(id(&guild_id), id(&channel_id), view.query())
                .await
            {
                listener.on_error(account_id, e.user_message());
            }
        });
    }

    /// The view changed (sort or tags), or the list scrolled to its end (`more`).
    pub fn forum_load(&self, account_id: String, channel_id: String, view: ForumView, more: bool) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = s.load_forum_page(id(&channel_id), view.query(), more).await {
                listener.on_error(account_id, e.user_message());
            }
        });
    }

    /// Posts that scrolled into view; previews are fetched for those still missing one.
    pub fn forum_previews(&self, account_id: String, channel_id: String, post_ids: Vec<String>) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        self.inner.rt.spawn(async move {
            let _ = s
                .load_previews(id(&channel_id), post_ids.iter().map(|p| id(p)).collect())
                .await;
        });
    }

    /// Publish a new post; on success the shell is sent to it via `on_navigate`.
    pub fn create_forum_post(
        &self,
        account_id: String,
        channel_id: String,
        title: String,
        content: String,
        tag_ids: Vec<String>,
        gesture: UserGesture,
    ) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let listener = self.listener.clone();
        self.inner.rt.spawn(async move {
            let tags = tag_ids.iter().map(|t| id(t)).collect();
            match s
                .create_forum_post(intent, id(&channel_id), title, content, tags)
                .await
            {
                Ok(post) => {
                    let guild = s
                        .store
                        .read()
                        .channels
                        .get(&post)
                        .and_then(|c| c.guild_id)
                        .map(|g| g.to_string());
                    listener.on_navigate(account_id, guild, post.to_string());
                }
                Err(e) => listener.on_error(account_id, e.user_message()),
            }
        });
    }
}
