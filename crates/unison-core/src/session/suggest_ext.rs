//! Session commands for composer autocomplete, the emoji picker and the GIF/sticker pickers.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use parking_lot::Mutex;
use serde_json::json;

use super::*;
use crate::suggest;

/// (account, guild) → queries already sent over op 8 this run, so typing never repeats a search.
static SEARCHED: LazyLock<Mutex<HashMap<(Id, Id), HashSet<String>>>> = LazyLock::new(Default::default);

impl Session {
    /// Ask the gateway for members matching `query` when the store can't be complete for the
    /// guild (op 8 as `requestMembers(guild, query, 10, presences)`). The reply arrives as
    /// `GUILD_MEMBERS_CHUNK` and surfaces as `Delta::MembersFound`.
    pub fn search_members(&self, channel: Id, query: &str) {
        let q = query.to_lowercase();
        let Some(guild) = suggest::needs_member_search(&self.store.read(), channel, &q) else { return };
        if !SEARCHED.lock().entry((self.user_id, guild)).or_default().insert(q.clone()) {
            return;
        }
        if let Some(gw) = self.gateway.lock().as_ref() {
            gw.send(
                gateway::op::REQUEST_GUILD_MEMBERS,
                json!({ "guild_id": [guild.to_string()], "query": q, "limit": 10, "presences": true }),
            );
        }
    }

    pub async fn send_poll(&self, intent: UserIntent, channel: Id, draft: &crate::poll_create::PollDraft) -> Result<()> {
        if let Some(why) = crate::poll_create::refusal(draft) {
            return Err(crate::Error::Other(why));
        }
        self.rest.send_poll(intent, channel, Snowflake::nonce_now(), draft).await
    }

    pub async fn send_sticker(&self, intent: UserIntent, channel: Id, sticker: Id) -> Result<()> {
        self.rest.send_sticker(intent, channel, Snowflake::nonce_now(), sticker).await
    }
}
