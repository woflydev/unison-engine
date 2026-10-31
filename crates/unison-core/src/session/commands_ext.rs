//! Slash command index per scope (guild, or the DM channel) and invocation.

use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::Mutex;

use super::*;
use crate::commands::{self, SlashCommand};

type Cache = Mutex<HashMap<(Id, Id), Arc<Vec<SlashCommand>>>>;
/// (account, scope) → index. The scope is the guild, or the channel for DMs.
static INDEX: LazyLock<Cache> = LazyLock::new(Default::default);
static FETCHING: LazyLock<Mutex<std::collections::HashSet<(Id, Id)>>> = LazyLock::new(Default::default);

impl Session {
    fn command_scope(&self, channel: Id) -> (Option<Id>, Id) {
        let guild = self.store.read().channels.get(&channel).and_then(|c| c.guild_id);
        (guild, guild.unwrap_or(channel))
    }

    /// The cached index, or `None` while it is still unknown (the caller starts `fetch_commands`).
    pub fn slash_index(&self, channel: Id) -> Option<Arc<Vec<SlashCommand>>> {
        let (_, scope) = self.command_scope(channel);
        INDEX.lock().get(&(self.user_id, scope)).cloned()
    }

    /// Fetch the index once per scope per run; emits `MembersFound` (the composer's "suggestions may have changed" tick) when it lands.
    pub async fn fetch_commands(&self, channel: Id) {
        let (guild, scope) = self.command_scope(channel);
        let key = (self.user_id, scope);
        if INDEX.lock().contains_key(&key) || !FETCHING.lock().insert(key) {
            return;
        }
        let res = self.rest.command_index(guild, channel).await;
        FETCHING.lock().remove(&key);
        let list = match res {
            Ok(v) => commands::parse_index(&v),
            Err(e) => {
                tracing::debug!("command index: {e}");
                Vec::new()
            }
        };
        INDEX.lock().insert(key, Arc::new(list));
        (self.emit)(SessionEvent::Deltas(vec![Delta::MembersFound { guild: scope }]));
    }

    /// Run a command: `POST /interactions` with the nested options.
    pub async fn run_command(&self, intent: UserIntent, channel: Id, key: &str, values: Vec<(String, String)>, files: Vec<crate::rest::LocalFile>) -> Result<()> {
        use crate::interactions::{Target, command_data, command_interaction};
        let cmd = self
            .slash_index(channel)
            .and_then(|i| i.iter().find(|c| c.key == key).cloned())
            .ok_or_else(|| Error::Decode("command not loaded".into()))?;
        let (guild, _) = self.command_scope(channel);
        let session = self.store.read().gateway_session.clone().unwrap_or_default();
        let mut data = command_data(cmd.id, &cmd.path[0], &cmd.version, commands::build_options(&cmd, &values), cmd.descriptor.clone());
        let (upload_intent, intent) = intent.split();
        if !files.is_empty() {
            // Same two-step upload as a message; the command refers to each file by index.
            let sent = self.rest.upload_attachments(upload_intent, channel, files, Arc::new(|_, _| {}), Arc::new(std::sync::atomic::AtomicBool::new(false))).await?;
            data["attachments"] = sent
                .iter()
                .enumerate()
                .map(|(i, a)| crate::rest::attachment_json(i, a))
                .collect();
        }
        let body = command_interaction(Target { guild, channel, application: cmd.application_id }, &session, Snowflake::nonce_now(), data);
        self.rest.interact(intent, body).await
    }

    /// Ask the application for choices for `focused` as typed so far. The answer arrives as
    /// `Delta::Autocomplete { nonce }`; `autocomplete_choices` reads it.
    pub async fn request_autocomplete(&self, intent: UserIntent, channel: Id, key: &str, values: Vec<(String, String)>, focused: &str, nonce: Id) -> Result<()> {
        use crate::interactions::{Target, autocomplete_interaction, command_data};
        let cmd = self
            .slash_index(channel)
            .and_then(|i| i.iter().find(|c| c.key == key).cloned())
            .ok_or_else(|| Error::Decode("command not loaded".into()))?;
        let (guild, _) = self.command_scope(channel);
        let session = self.store.read().gateway_session.clone().unwrap_or_default();
        let opts = commands::build_options_focused(&cmd, &values, Some(focused));
        let data = command_data(cmd.id, &cmd.path[0], &cmd.version, opts, cmd.descriptor.clone());
        let body = autocomplete_interaction(Target { guild, channel, application: cmd.application_id }, &session, nonce, data);
        self.rest.interact(intent, body).await
    }

    pub fn autocomplete_choices(&self, nonce: Id) -> Option<Vec<(String, String)>> {
        self.store.read().autocomplete.get(&nonce).cloned()
    }
}
