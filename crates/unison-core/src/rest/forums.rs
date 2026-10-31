use super::*;

#[derive(Debug, Clone)]
pub struct NewForumPost {
    pub title: String,
    pub content: String,
    pub tags: Vec<Id>,
    pub auto_archive_duration: u32,
}

impl NewForumPost {
    fn body(&self) -> Value {
        let tags: Vec<String> = self.tags.iter().map(Id::to_string).collect();
        json!({
            "name": self.title,
            "auto_archive_duration": self.auto_archive_duration,
            "applied_tags": tags,
            "message": { "content": self.content },
        })
    }
}

impl Rest {
    pub async fn active_threads(&self, guild: Id) -> Result<Value> {
        self.get(&format!("/guilds/{guild}/threads/active")).await
    }

    /// Archived forum posts (`loadArchivedThreads`).
    pub async fn thread_search(
        &self,
        forum: Id,
        query: &ForumQuery,
        offset: u32,
    ) -> Result<ThreadSearch> {
        let path = format!(
            "/channels/{forum}/threads/search?{}",
            query.search_query(offset)
        );
        tracing::debug!(%path, "thread search");
        self.get(&path).await
    }

    /// First and most recent messages for up to ten posts (`FORUM_POSTS`).
    pub async fn post_data(&self, forum: Id, threads: &[Id]) -> Result<PostData> {
        let ids: Vec<String> = threads.iter().map(Id::to_string).collect();
        self.post_read(
            &format!("/channels/{forum}/post-data"),
            json!({ "thread_ids": ids }),
        )
        .await
    }

    /// New forum post. Body as the forum composer builds it: title, the forum's default
    /// archive duration, tags in the order picked, and the first message.
    pub async fn create_forum_post(
        &self,
        intent: UserIntent,
        forum: Id,
        post: &NewForumPost,
    ) -> Result<Value> {
        let path = format!("/channels/{forum}/threads?use_nested_fields=true");
        self.write(intent, Method::POST, &path, Some(post.body()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forum_post_body_shape() {
        let post = NewForumPost {
            title: "T".into(),
            content: "Body".into(),
            tags: vec![Snowflake(7), Snowflake(3)],
            auto_archive_duration: 4320,
        };
        assert_eq!(
            post.body().to_string(),
            r#"{"name":"T","auto_archive_duration":4320,"applied_tags":["7","3"],"message":{"content":"Body"}}"#
        );
    }
}
