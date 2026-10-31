//! Slash commands for the composer: ranked index lookups and invocation.

use unison_core::commands;

use super::*;

#[derive(uniffi::Record, Clone, Debug)]
pub struct SlashChoice {
    pub name: String,
    pub value: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SlashOptionItem {
    pub name: String,
    pub description: String,
    /// Discord option type (3 string, 4 integer, 5 boolean, 6 user, 7 channel, 8 role, 9 mentionable, 10 number, 11 attachment).
    pub kind: u8,
    pub required: bool,
    pub choices: Vec<SlashChoice>,
    pub autocomplete: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SlashCommandItem {
    /// Pass back to `run_slash_command`.
    pub key: String,
    /// `ban`, `role add`: no leading slash.
    pub name: String,
    pub description: String,
    pub app_name: String,
    pub app_icon_url: Option<String>,
    pub options: Vec<SlashOptionItem>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SlashValue {
    pub name: String,
    pub value: String,
}

fn item(c: &commands::SlashCommand) -> SlashCommandItem {
    SlashCommandItem {
        key: c.key.clone(),
        name: c.name(),
        description: c.description.clone(),
        app_name: c.app_name.clone(),
        app_icon_url: c.app_icon.clone(),
        options: c
            .options
            .iter()
            .map(|o| SlashOptionItem {
                name: o.name.clone(),
                description: o.description.clone(),
                kind: o.kind,
                required: o.required,
                choices: o.choices.iter().map(|(n, v)| SlashChoice { name: n.clone(), value: v.clone() }).collect(),
                autocomplete: o.autocomplete,
            })
            .collect(),
    }
}

#[uniffi::export]
impl UnisonEngine {
    /// Commands matching `query` (what follows the `/`). The first call for a server or DM starts
    /// the index fetch; a `MembersFound` delta arrives when it lands and the caller asks again.
    pub fn slash_commands(&self, account_id: String, channel_id: String, query: String, limit: u32) -> Vec<SlashCommandItem> {
        let Some(s) = self.inner.session(id(&account_id)) else { return vec![] };
        let channel = id(&channel_id);
        match s.slash_index(channel) {
            Some(all) => commands::rank(&all, &query, limit as usize).into_iter().map(item).collect(),
            None => {
                let s = s.clone();
                self.inner.rt.spawn(async move { s.fetch_commands(channel).await });
                vec![]
            }
        }
    }

    /// Invoke a command with its option values (`POST /interactions`).
    pub fn run_slash_command(&self, account_id: String, channel_id: String, key: String, values: Vec<SlashValue>, files: Vec<FileUpload>, gesture: UserGesture) {
        let files: Vec<LocalFile> = files.into_iter().map(FileUpload::into_local).collect();
        self.spawn_write(account_id, gesture, move |s, intent| async move {
            s.run_command(intent, id(&channel_id), &key, values.into_iter().map(|v| (v.name, v.value)).collect(), files).await
        });
    }

    /// Ask the command's application for choices for the option being typed. Returns the request
    /// id; `slash_autocomplete_result` has the answer once `UiDelta::Autocomplete` carries it.
    pub fn slash_autocomplete(&self, account_id: String, channel_id: String, key: String, values: Vec<SlashValue>, option: String, gesture: UserGesture) -> String {
        let nonce = unison_core::snowflake::Snowflake::nonce_now();
        self.spawn_write(account_id, gesture, move |s, intent| async move {
            s.request_autocomplete(intent, id(&channel_id), &key, values.into_iter().map(|v| (v.name, v.value)).collect(), &option, nonce).await
        });
        nonce.to_string()
    }

    pub fn slash_autocomplete_result(&self, account_id: String, request_id: String) -> Option<Vec<SlashChoice>> {
        let s = self.inner.session(id(&account_id))?;
        s.autocomplete_choices(id(&request_id)).map(|v| v.into_iter().map(|(name, value)| SlashChoice { name, value }).collect())
    }
}
