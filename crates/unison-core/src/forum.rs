//! Forum and media channels: which posts a forum shows, in what order, and what the
//! official client fetches to fill them in.
//!
//! Active posts arrive over the gateway (`THREAD_LIST_SYNC` after the op 37 `threads`
//! subscription, then `THREAD_CREATE`/`UPDATE`). Archived posts are paged in with
//! `GET /channels/{id}/threads/search`, one list per (sort, tags, tag setting), exactly as
//! `loadArchivedThreads` keys them. First-message previews for posts the search didn't
//! cover come from `POST /channels/{id}/post-data`, ten threads at a time.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::model::{Channel, Id, Message, channel_flags};

/// `loadArchivedThreads` page size.
pub const PAGE: u32 = 25;
/// `post-data` batch size (`getNextBatch(e, 10)`).
pub const PREVIEW_BATCH: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortOrder {
    #[default]
    LatestActivity,
    CreationDate,
}

impl SortOrder {
    pub fn from_default(v: Option<u8>) -> Self {
        if v == Some(1) { SortOrder::CreationDate } else { SortOrder::LatestActivity }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TagMatch {
    #[default]
    Some,
    All,
}

impl TagMatch {
    pub fn from_default(v: Option<&str>) -> Self {
        if v == Some("match_all") { TagMatch::All } else { TagMatch::Some }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TagMatch::Some => "match_some",
            TagMatch::All => "match_all",
        }
    }
}

/// One view of a forum: the sort and filter the user picked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ForumQuery {
    pub sort: SortOrder,
    /// In the order the user picked them; the official filter is a `Set`, joined in insertion order.
    pub tags: Vec<Id>,
    pub tag_match: TagMatch,
}

impl ForumQuery {
    pub fn new(sort: SortOrder, tags: Vec<Id>, tag_match: TagMatch) -> Self {
        let mut seen = HashSet::new();
        let tags = tags.into_iter().filter(|t| seen.insert(*t)).collect();
        Self { sort, tags, tag_match }
    }

    /// Query string for `threads/search`, in `loadArchivedThreads` order. The sort is
    /// always last-message-time descending on the wire; the chosen order is applied locally.
    pub fn search_query(&self, offset: u32) -> String {
        let mut q = format!("archived=true&sort_by=last_message_time&sort_order=desc&limit={PAGE}");
        if !self.tags.is_empty() {
            let tags: Vec<String> = self.tags.iter().map(Id::to_string).collect();
            q.push_str("&tag=");
            q.push_str(&tags.join("%2C"));
        }
        q.push_str("&tag_setting=");
        q.push_str(self.tag_match.as_str());
        q.push_str(&format!("&offset={offset}"));
        q
    }

    fn admits(&self, post: &Channel) -> bool {
        if self.tags.is_empty() {
            return true;
        }
        match self.tag_match {
            TagMatch::Some => self.tags.iter().any(|t| post.applied_tags.contains(t)),
            TagMatch::All => self.tags.iter().all(|t| post.applied_tags.contains(t)),
        }
    }
}

/// Archived posts loaded for one query.
#[derive(Debug, Default)]
pub struct ArchivedList {
    pub ids: Vec<Id>,
    pub has_more: bool,
    pub next_offset: u32,
    pub loading: bool,
    pub loaded: bool,
}

#[derive(Debug, Default)]
pub struct ForumState {
    pub archived: HashMap<ForumQuery, ArchivedList>,
}

/// Previews shared by every forum: a post's first message, once known (`None` means
/// Discord said there isn't one, e.g. it was deleted), and which posts were already asked for.
#[derive(Debug, Default)]
pub struct Previews {
    pub first: HashMap<Id, Option<Message>>,
    pub requested: HashSet<Id>,
}

impl Previews {
    /// Posts from `ids` whose first message is neither known nor already requested.
    pub fn missing(&self, ids: &[Id]) -> Vec<Id> {
        ids.iter().copied().filter(|t| !self.first.contains_key(t) && !self.requested.contains(t)).collect()
    }
}

/// `threads/search` response.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ThreadSearch {
    pub threads: Vec<Channel>,
    pub members: Vec<Value>,
    pub has_more: bool,
    pub first_messages: Vec<Message>,
    pub most_recent_messages: Vec<Message>,
}

/// `post-data` response: `{threads: {id: {first_message, most_recent_message}}}`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PostData {
    pub threads: HashMap<String, PostDataEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PostDataEntry {
    pub first_message: Option<Message>,
    pub most_recent_message: Option<Message>,
}

/// Newest activity in a post, as the official sort compares it: its last message id, or
/// the post's own id when nothing has been said yet.
pub fn last_activity(post: &Channel) -> Id {
    post.last_message_id.unwrap_or(post.id).max(post.id)
}

/// The posts a forum lists for `query`: pinned first, then active posts in the chosen
/// order, then archived posts in the order the server paged them.
pub fn ordered_posts<'a>(
    forum: Id,
    channels: impl Iterator<Item = &'a Channel>,
    archived: Option<&ArchivedList>,
    query: &ForumQuery,
    lookup: impl Fn(Id) -> Option<&'a Channel>,
) -> Vec<Id> {
    let mut active: Vec<&Channel> =
        channels.filter(|c| c.is_thread() && c.parent_id == Some(forum) && !c.is_archived() && query.admits(c)).collect();
    active.sort_by_key(|c| {
        let pinned = c.flags & channel_flags::PINNED != 0;
        let key = match query.sort {
            SortOrder::LatestActivity => last_activity(c),
            SortOrder::CreationDate => c.id,
        };
        (std::cmp::Reverse(pinned), std::cmp::Reverse(key))
    });
    let mut out: Vec<Id> = active.iter().map(|c| c.id).collect();
    let seen: HashSet<Id> = out.iter().copied().collect();
    if let Some(list) = archived {
        // A post can be unarchived after it was paged in; it then sorts with the active ones.
        // The search doesn't reliably honour `tag`, so the filter is applied here too.
        out.extend(list.ids.iter().copied().filter(|id| {
            !seen.contains(id) && lookup(*id).is_some_and(|c| c.is_archived() && query.admits(c))
        }));
    }
    out
}

/// The image a post card shows: the first image attachment of the first message, else an
/// embed image or thumbnail. Returns (url, width, height).
pub fn thumbnail(m: &Message) -> Option<(String, Option<u32>, Option<u32>)> {
    if let Some(a) = m.attachments.iter().find(|a| (a.is_image() || a.is_video()) && !a.is_spoiler()) {
        let url = if a.proxy_url.is_empty() { a.url.clone() } else { a.proxy_url.clone() };
        return Some((url, a.width, a.height));
    }
    m.embeds.iter().find_map(|e| {
        let media = e.image.as_ref().or(e.thumbnail.as_ref())?;
        let url = media.proxy_url.clone().or_else(|| media.url.clone())?;
        Some((url, media.width, media.height))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel_type;
    use crate::snowflake::Snowflake;
    use serde_json::json;

    fn post(id: u64, last: Option<u64>, tags: &[u64], archived: bool, flags: u64) -> Channel {
        Channel {
            id: Snowflake(id),
            kind: channel_type::PUBLIC_THREAD,
            parent_id: Some(Snowflake(9)),
            last_message_id: last.map(Snowflake),
            applied_tags: tags.iter().copied().map(Snowflake).collect(),
            thread_metadata: Some(json!({ "archived": archived })),
            flags,
            ..Default::default()
        }
    }

    #[test]
    fn search_query_matches_load_archived_threads() {
        let q = ForumQuery::new(SortOrder::CreationDate, vec![Snowflake(5), Snowflake(3)], TagMatch::All);
        assert_eq!(
            q.search_query(25),
            "archived=true&sort_by=last_message_time&sort_order=desc&limit=25&tag=5%2C3&tag_setting=match_all&offset=25"
        );
        let plain = ForumQuery::default();
        assert_eq!(
            plain.search_query(0),
            "archived=true&sort_by=last_message_time&sort_order=desc&limit=25&tag_setting=match_some&offset=0"
        );
    }

    #[test]
    fn orders_pinned_then_activity_then_archived() {
        let posts = [
            post(10, Some(50), &[1], false, 0),
            post(11, Some(80), &[2], false, 0),
            post(12, None, &[1, 2], false, channel_flags::PINNED),
            post(13, Some(90), &[1], true, 0),
            post(14, Some(95), &[], true, 0),
        ];
        let lookup = |id: Id| posts.iter().find(|p| p.id == id);
        let archived = ArchivedList { ids: vec![Snowflake(14), Snowflake(13)], ..Default::default() };
        let all = ordered_posts(Snowflake(9), posts.iter(), Some(&archived), &ForumQuery::default(), lookup);
        assert_eq!(all, [12, 11, 10, 14, 13].map(Snowflake));

        let by_creation = ForumQuery::new(SortOrder::CreationDate, vec![], TagMatch::Some);
        let ids = ordered_posts(Snowflake(9), posts.iter(), None, &by_creation, lookup);
        assert_eq!(ids, [12, 11, 10].map(Snowflake));
    }

    #[test]
    fn tag_filters() {
        let posts = [post(10, None, &[1], false, 0), post(11, None, &[2], false, 0), post(12, None, &[1, 2], false, 0)];
        let lookup = |id: Id| posts.iter().find(|p| p.id == id);
        let some = ForumQuery::new(SortOrder::CreationDate, vec![Snowflake(1)], TagMatch::Some);
        assert_eq!(ordered_posts(Snowflake(9), posts.iter(), None, &some, lookup), [12, 10].map(Snowflake));
        let all = ForumQuery::new(SortOrder::CreationDate, vec![Snowflake(1), Snowflake(2)], TagMatch::All);
        assert_eq!(ordered_posts(Snowflake(9), posts.iter(), None, &all, lookup), [12].map(Snowflake));
    }

    #[test]
    fn archived_pages_are_filtered_locally() {
        // threads/search returns posts outside the requested tags; they must not show.
        let posts = [post(20, None, &[1], true, 0), post(21, None, &[2], true, 0)];
        let lookup = |id: Id| posts.iter().find(|p| p.id == id);
        let archived = ArchivedList { ids: vec![Snowflake(20), Snowflake(21)], ..Default::default() };
        let q = ForumQuery::new(SortOrder::LatestActivity, vec![Snowflake(1)], TagMatch::Some);
        assert_eq!(ordered_posts(Snowflake(9), posts.iter(), Some(&archived), &q, lookup), vec![Snowflake(20)]);
    }

    #[test]
    fn unarchived_post_leaves_archived_tail() {
        let posts = [post(10, Some(20), &[], false, 0)];
        let lookup = |id: Id| posts.iter().find(|p| p.id == id);
        let archived = ArchivedList { ids: vec![Snowflake(10)], ..Default::default() };
        let ids = ordered_posts(Snowflake(9), posts.iter(), Some(&archived), &ForumQuery::default(), lookup);
        assert_eq!(ids, vec![Snowflake(10)]);
    }

    #[test]
    fn thumbnail_prefers_attachments() {
        let m: Message = serde_json::from_value(json!({
            "id": "1", "channel_id": "1",
            "attachments": [{"id": "2", "filename": "a.png", "content_type": "image/png", "url": "u", "proxy_url": "p", "width": 4, "height": 3}],
            "embeds": [{"image": {"url": "e"}}]
        }))
        .unwrap();
        assert_eq!(thumbnail(&m), Some(("p".into(), Some(4), Some(3))));
    }
}
