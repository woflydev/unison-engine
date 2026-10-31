//! Slash commands: the application-command index of a channel, ranking for the `/` popup, and
//! the `options` payload of an invocation (subcommands nest as the API wants).

use serde_json::{Value, json};

use crate::model::Id;

pub const SUB_COMMAND: u64 = 1;
pub const SUB_COMMAND_GROUP: u64 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct SlashOption {
    pub name: String,
    pub description: String,
    /// Discord's option type: 3 string, 4 integer, 5 boolean, 6 user, 7 channel, 8 role, 9 mentionable, 10 number, 11 attachment.
    pub kind: u8,
    pub required: bool,
    pub choices: Vec<(String, String)>,
    pub autocomplete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlashCommand {
    /// `command id` + the sub-command path: unique per runnable entry.
    pub key: String,
    pub id: Id,
    pub application_id: Id,
    pub app_name: String,
    pub app_icon: Option<String>,
    /// Names from the root: `["role", "add"]` shows as `/role add`.
    pub path: Vec<String>,
    pub description: String,
    pub version: String,
    /// The leaf's options (sub-commands already unwrapped).
    pub options: Vec<SlashOption>,
    /// The root command as the index sent it (goes back in `application_command`).
    pub descriptor: Value,
}

impl SlashCommand {
    pub fn name(&self) -> String {
        self.path.join(" ")
    }
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn id_of(v: &Value, k: &str) -> Option<Id> {
    v[k].as_str().and_then(|s| s.parse().ok()).map(crate::snowflake::Snowflake)
}

fn option(v: &Value) -> SlashOption {
    SlashOption {
        name: str_of(v, "name"),
        description: str_of(v, "description"),
        kind: v["type"].as_u64().unwrap_or(3) as u8,
        required: v["required"].as_bool().unwrap_or(false),
        choices: v["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| (str_of(c, "name"), c["value"].as_str().map(str::to_string).unwrap_or_else(|| c["value"].to_string())))
            .collect(),
        autocomplete: v["autocomplete"].as_bool().unwrap_or(false),
    }
}

/// `GET /…/application-command-index`: `{application_commands, applications}`. Only chat-input
/// commands (type 1) are runnable from the composer; sub-commands become their own entries.
pub fn parse_index(v: &Value) -> Vec<SlashCommand> {
    let apps: Vec<&Value> = v["applications"].as_array().into_iter().flatten().collect();
    let mut out = Vec::new();
    for c in v["application_commands"].as_array().into_iter().flatten() {
        if c["type"].as_u64().unwrap_or(1) != 1 {
            continue;
        }
        let (Some(id), Some(app)) = (id_of(c, "id"), id_of(c, "application_id")) else { continue };
        let app_v = apps.iter().find(|a| id_of(a, "id") == Some(app));
        let app_name = app_v.map(|a| str_of(a, "name")).unwrap_or_default();
        let app_icon = app_v.and_then(|a| a["icon"].as_str()).map(|h| format!("https://cdn.discordapp.com/app-icons/{app}/{h}.png?size=64"));
        let base = |path: Vec<String>, description: String, options: Vec<SlashOption>| SlashCommand {
            key: format!("{id}:{}", path.join(" ")),
            id,
            application_id: app,
            app_name: app_name.clone(),
            app_icon: app_icon.clone(),
            path,
            description,
            version: str_of(c, "version"),
            options,
            descriptor: c.clone(),
        };
        let name = str_of(c, "name");
        let opts: Vec<&Value> = c["options"].as_array().into_iter().flatten().collect();
        let has_sub = opts.iter().any(|o| matches!(o["type"].as_u64(), Some(SUB_COMMAND | SUB_COMMAND_GROUP)));
        if !has_sub {
            out.push(base(vec![name], str_of(c, "description"), opts.iter().map(|o| option(o)).collect()));
            continue;
        }
        for o in opts {
            match o["type"].as_u64() {
                Some(SUB_COMMAND) => out.push(base(
                    vec![name.clone(), str_of(o, "name")],
                    str_of(o, "description"),
                    o["options"].as_array().into_iter().flatten().map(option).collect(),
                )),
                Some(SUB_COMMAND_GROUP) => {
                    for s in o["options"].as_array().into_iter().flatten() {
                        out.push(base(
                            vec![name.clone(), str_of(o, "name"), str_of(s, "name")],
                            str_of(s, "description"),
                            s["options"].as_array().into_iter().flatten().map(option).collect(),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Commands matching what follows the `/`: prefix on the full name beats a word prefix beats a
/// substring; ties go to shorter names. Several words match the path and the app name.
pub fn rank<'a>(all: &'a [SlashCommand], query: &str, limit: usize) -> Vec<&'a SlashCommand> {
    let q = query.trim_start_matches('/').to_lowercase();
    let mut scored: Vec<(u8, usize, &SlashCommand)> = all
        .iter()
        .filter_map(|c| {
            let n = c.name().to_lowercase();
            let s = if q.is_empty() || n.starts_with(&q) {
                0
            } else if n.split(' ').any(|w| w.starts_with(&q)) {
                1
            } else if n.contains(&q) {
                2
            } else if c.app_name.to_lowercase().starts_with(&q) {
                3
            } else {
                return None;
            };
            Some((s, n.len(), c))
        })
        .collect();
    scored.sort_by(|a, b| (a.0, a.1, &a.2.app_name).cmp(&(b.0, b.1, &b.2.app_name)));
    scored.into_iter().take(limit).map(|s| s.2).collect()
}

/// Option values `(name, value)` → the nested `options` array: leaf values inside the sub-command
/// (and group) wrappers. Values are coerced to the option's type; empty optional values are dropped.
pub fn build_options(cmd: &SlashCommand, values: &[(String, String)]) -> Vec<Value> {
    build_options_focused(cmd, values, None)
}

/// `build_options` for an autocomplete request: the option being typed is sent even while empty
/// and marked `focused`.
pub fn build_options_focused(cmd: &SlashCommand, values: &[(String, String)], focused: Option<&str>) -> Vec<Value> {
    let mut leaf: Vec<Value> = Vec::new();
    for o in &cmd.options {
        let is_focused = focused == Some(o.name.as_str());
        let Some((_, raw)) = values.iter().find(|(n, _)| *n == o.name) else {
            if is_focused {
                leaf.push(json!({ "type": o.kind, "name": o.name, "value": "", "focused": true }));
            }
            continue;
        };
        if raw.is_empty() && !is_focused {
            continue;
        }
        let value = match o.kind {
            4 => raw.trim().parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(raw.clone())),
            10 => raw.trim().parse::<f64>().map(Value::from).unwrap_or_else(|_| Value::from(raw.clone())),
            // An attachment option holds the index of the file uploaded with the command.
            11 => raw.trim().parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(raw.clone())),
            5 => Value::from(matches!(raw.trim().to_lowercase().as_str(), "true" | "yes" | "1" | "on")),
            _ => Value::from(raw.clone()),
        };
        let mut opt = json!({ "type": o.kind, "name": o.name, "value": value });
        if is_focused {
            opt["focused"] = true.into();
        }
        leaf.push(opt);
    }
    // Wrap from the leaf up: path[1..] are sub-command / group names.
    let mut wrapped = leaf;
    for (depth, name) in cmd.path.iter().enumerate().skip(1).rev() {
        let kind = if depth == cmd.path.len() - 1 { SUB_COMMAND } else { SUB_COMMAND_GROUP };
        wrapped = vec![json!({ "type": kind, "name": name, "options": wrapped })];
    }
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Value {
        json!({
            "applications": [{"id": "10", "name": "Mod Bot", "icon": "abc"}],
            "application_commands": [
                {"id": "1", "application_id": "10", "type": 1, "name": "ban", "description": "Ban", "version": "5",
                 "options": [{"type": 6, "name": "user", "description": "Who", "required": true},
                             {"type": 4, "name": "days", "description": "Days"}]},
                {"id": "2", "application_id": "10", "type": 1, "name": "role", "description": "Roles", "version": "6",
                 "options": [{"type": 1, "name": "add", "description": "Add", "options": [{"type": 8, "name": "role", "required": true}]},
                             {"type": 2, "name": "bulk", "options": [{"type": 1, "name": "clear", "options": []}]}]},
                {"id": "3", "application_id": "10", "type": 2, "name": "user context"},
            ]
        })
    }

    #[test]
    fn autocomplete_marks_the_focused_option_even_when_empty() {
        let cmds = parse_index(&index());
        let ban = cmds.iter().find(|c| c.name() == "ban").unwrap();
        let o = build_options_focused(ban, &[], Some("days"));
        assert_eq!(o, vec![json!({"type": 4, "name": "days", "value": "", "focused": true})]);
        let o = build_options_focused(ban, &[("days".into(), "3".into())], Some("days"));
        assert_eq!(o[0]["focused"], true);
        assert!(build_options(ban, &[("days".into(), "3".into())])[0].get("focused").is_none());
    }

    #[test]
    fn flattens_subcommands_and_skips_context_menus() {
        let cmds = parse_index(&index());
        let names: Vec<String> = cmds.iter().map(|c| c.name()).collect();
        assert_eq!(names, ["ban", "role add", "role bulk clear"]);
        assert_eq!(cmds[0].app_name, "Mod Bot");
        assert!(cmds[0].options[0].required);
    }

    #[test]
    fn ranks_prefix_before_substring() {
        let cmds = parse_index(&index());
        assert_eq!(rank(&cmds, "ro", 5)[0].name(), "role add");
        assert_eq!(rank(&cmds, "add", 5)[0].name(), "role add");
        assert_eq!(rank(&cmds, "", 2).len(), 2);
        assert!(rank(&cmds, "zzz", 5).is_empty());
    }

    #[test]
    fn options_nest_and_coerce() {
        let cmds = parse_index(&index());
        let ban = build_options(&cmds[0], &[("user".into(), "42".into()), ("days".into(), "7".into())]);
        assert_eq!(ban, vec![json!({"type": 6, "name": "user", "value": "42"}), json!({"type": 4, "name": "days", "value": 7})]);
        let add = build_options(&cmds[1], &[("role".into(), "9".into())]);
        assert_eq!(add, vec![json!({"type": 1, "name": "add", "options": [{"type": 8, "name": "role", "value": "9"}]})]);
        let clear = build_options(&cmds[2], &[]);
        assert_eq!(clear, vec![json!({"type": 2, "name": "bulk", "options": [{"type": 1, "name": "clear", "options": []}]})]);
    }
}
