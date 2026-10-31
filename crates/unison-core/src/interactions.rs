//! Message components, polls and application-command interactions.
//!
//! Request shapes follow the web client's `POST /interactions` callers (component click, command,
//! autocomplete); shapes the statically fetchable bundle chunks don't contain (modal submit,
//! poll vote body) are recorded as "capture" in docs/FIDELITY.md.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::model::Id;

/// `InteractionType` as the web client sends it.
pub mod interaction_type {
    pub const APPLICATION_COMMAND: u8 = 2;
    pub const MESSAGE_COMPONENT: u8 = 3;
    pub const AUTOCOMPLETE: u8 = 4;
    pub const MODAL_SUBMIT: u8 = 5;
}

fn put(m: &mut Map<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(k.into(), v);
    }
}

/// Where an interaction is sent from.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub guild: Option<Id>,
    pub channel: Id,
    pub application: Id,
}

/// Button press or select change: `MESSAGE_COMPONENT`. `select` is `(component_type, values)`.
#[allow(clippy::too_many_arguments)]
pub fn component_interaction(
    t: Target,
    session_id: &str,
    nonce: Id,
    message: Id,
    message_flags: u64,
    component_type: u8,
    custom_id: &str,
    select: Option<(u8, Vec<String>)>,
) -> Value {
    let mut data = Map::new();
    data.insert("component_type".into(), component_type.into());
    data.insert("custom_id".into(), custom_id.into());
    if let Some((ty, values)) = select {
        data.insert("type".into(), ty.into());
        data.insert("values".into(), values.into());
    }
    let mut m = Map::new();
    m.insert("type".into(), interaction_type::MESSAGE_COMPONENT.into());
    m.insert("nonce".into(), nonce.to_string().into());
    put(&mut m, "guild_id", t.guild.map(|g| g.to_string().into()));
    m.insert("channel_id".into(), t.channel.to_string().into());
    m.insert("message_flags".into(), message_flags.into());
    m.insert("message_id".into(), message.to_string().into());
    m.insert("application_id".into(), t.application.to_string().into());
    m.insert("session_id".into(), session_id.into());
    m.insert("data".into(), data.into());
    m.into()
}

fn command_like(ty: u8, t: Target, session_id: &str, nonce: Id, data: Value) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), ty.into());
    m.insert("application_id".into(), t.application.to_string().into());
    put(&mut m, "guild_id", t.guild.map(|g| g.to_string().into()));
    m.insert("channel_id".into(), t.channel.to_string().into());
    m.insert("session_id".into(), session_id.into());
    m.insert("data".into(), data);
    m.insert("nonce".into(), nonce.to_string().into());
    m.into()
}

/// Slash command invocation (`handleCommand`). capture: `analytics_location`, `section_name`
/// and `source` are sent by the web client; their values are not in the static chunks.
pub fn command_interaction(t: Target, session_id: &str, nonce: Id, data: Value) -> Value {
    command_like(interaction_type::APPLICATION_COMMAND, t, session_id, nonce, data)
}

/// Option-value autocomplete request (`APPLICATION_COMMAND_AUTOCOMPLETE`); `data` is the command
/// data with the focused option marked `focused: true`.
pub fn autocomplete_interaction(t: Target, session_id: &str, nonce: Id, data: Value) -> Value {
    command_like(interaction_type::AUTOCOMPLETE, t, session_id, nonce, data)
}

/// The `data` object of a slash command: id, name, version, `type: 1`, options, and the command
/// descriptor itself. `options` is `[{type, name, value}]`.
pub fn command_data(id: Id, name: &str, version: &str, options: Vec<Value>, descriptor: Value) -> Value {
    json!({
        "version": version,
        "id": id.to_string(),
        "name": name,
        "type": 1,
        "options": options,
        "application_command": descriptor,
        "attachments": [],
    })
}

/// Modal submit. `fields` is `[(custom_id, value)]`, one per action row as Discord expects.
pub fn modal_interaction(t: Target, session_id: &str, nonce: Id, modal_id: &str, custom_id: &str, fields: &[(String, String)]) -> Value {
    let rows: Vec<Value> = fields
        .iter()
        .map(|(id, v)| json!({"type": 1, "components": [{"type": 4, "custom_id": id, "value": v}]}))
        .collect();
    let data = json!({"id": modal_id, "custom_id": custom_id, "components": rows});
    command_like(interaction_type::MODAL_SUBMIT, t, session_id, nonce, data)
}

/// Body of `PUT /channels/{c}/polls/{m}/answers/@me`: the full set of chosen answers (empty
/// removes the vote). capture: exact body not in the statically fetchable bundle chunks.
pub fn poll_vote_body(answer_ids: &[u32]) -> Value {
    json!({ "answer_ids": answer_ids.iter().map(|a| a.to_string()).collect::<Vec<_>>() })
}

// ---------- gateway events ----------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PollVoteEvent {
    pub user_id: Id,
    pub channel_id: Id,
    pub message_id: Id,
    pub answer_id: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThreadMembersUpdate {
    pub id: Id,
    pub member_count: Option<u32>,
    pub added_members: Vec<Value>,
    pub removed_member_ids: Vec<Id>,
}

/// Fold one vote into `poll.results.answer_counts` (what `MESSAGE_POLL_VOTE_ADD/REMOVE` mean).
pub fn apply_poll_vote(poll: &mut Option<Value>, answer: u32, add: bool, mine: bool) {
    let Some(p) = poll.as_mut().and_then(Value::as_object_mut) else { return };
    let results = p.entry("results").or_insert_with(|| json!({"is_finalized": false, "answer_counts": []}));
    let Some(r) = results.as_object_mut() else { return };
    let Some(arr) = r.entry("answer_counts").or_insert_with(|| json!([])).as_array_mut() else { return };
    let i = arr.iter().position(|c| c["id"].as_u64() == Some(answer as u64)).unwrap_or_else(|| {
        arr.push(json!({"id": answer, "count": 0, "me_voted": false}));
        arr.len() - 1
    });
    let c = &mut arr[i];
    let n = c["count"].as_i64().unwrap_or(0);
    c["count"] = (if add { n + 1 } else { (n - 1).max(0) }).into();
    if mine {
        c["me_voted"] = add.into();
    }
}

// ---------- read models ----------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PollAnswer {
    pub id: u32,
    pub text: String,
    pub emoji: Option<String>,
    pub count: u32,
    pub me: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PollView {
    pub question: String,
    pub answers: Vec<PollAnswer>,
    pub multiselect: bool,
    pub finalized: bool,
    pub expiry: Option<String>,
    pub total: u32,
}

pub fn parse_poll(v: &Value) -> Option<PollView> {
    let question = v["question"]["text"].as_str()?.to_string();
    let counts = v["results"]["answer_counts"].as_array().cloned().unwrap_or_default();
    let mut answers: Vec<PollAnswer> = v["answers"]
        .as_array()?
        .iter()
        .map(|a| {
            let id = a["answer_id"].as_u64().unwrap_or(0) as u32;
            let c = counts.iter().find(|c| c["id"].as_u64() == Some(id as u64));
            let media = &a["poll_media"];
            PollAnswer {
                id,
                text: media["text"].as_str().unwrap_or("").to_string(),
                emoji: media["emoji"]["name"].as_str().map(str::to_string),
                count: c.and_then(|c| c["count"].as_u64()).unwrap_or(0) as u32,
                me: c.and_then(|c| c["me_voted"].as_bool()).unwrap_or(false),
            }
        })
        .collect();
    answers.truncate(10);
    let total = answers.iter().map(|a| a.count).sum();
    Some(PollView {
        question,
        answers,
        multiselect: v["allow_multiselect"].as_bool().unwrap_or(false),
        finalized: v["results"]["is_finalized"].as_bool().unwrap_or(false),
        expiry: v["expiry"].as_str().map(str::to_string),
        total,
    })
}

/// New answer set after clicking `answer`: toggles in multiselect polls, replaces otherwise.
pub fn next_answer_ids(poll: &PollView, answer: u32) -> Vec<u32> {
    let mine: Vec<u32> = poll.answers.iter().filter(|a| a.me).map(|a| a.id).collect();
    let on = mine.contains(&answer);
    if poll.multiselect {
        if on { mine.into_iter().filter(|a| *a != answer).collect() } else { [mine, vec![answer]].concat() }
    } else if on {
        vec![]
    } else {
        vec![answer]
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ButtonView {
    pub style: u8,
    pub label: String,
    pub emoji: Option<String>,
    pub custom_id: Option<String>,
    pub url: Option<String>,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectView {
    pub component_type: u8,
    pub custom_id: String,
    pub placeholder: Option<String>,
    pub options: Vec<SelectOption>,
    pub min_values: u32,
    pub max_values: u32,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaView {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: Option<String>,
}

fn parse_media(m: &Value) -> Option<MediaView> {
    let media = &m["media"];
    let url = media["proxy_url"].as_str().or_else(|| media["url"].as_str())?;
    // `attachment://name` points at a file of the same message, which we have no URL for here.
    if url.starts_with("attachment://") {
        return None;
    }
    Some(MediaView {
        url: url.to_string(),
        width: media["width"].as_u64().map(|n| n as u32),
        height: media["height"].as_u64().map(|n| n as u32),
        alt: m["description"].as_str().map(str::to_string),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComponentBlock {
    Buttons(Vec<ButtonView>),
    Select(SelectView),
    Text(String),
    Separator,
    /// Media gallery items and section thumbnails (components v2).
    Media(Vec<MediaView>),
    /// A component type we don't draw: shown as a web-link card.
    Unsupported,
}

/// Flatten action rows and containers into drawable blocks.
pub fn parse_components(rows: &[Value]) -> Vec<ComponentBlock> {
    let mut out = Vec::new();
    for r in rows {
        flatten(r, &mut out);
    }
    out
}

fn flatten(c: &Value, out: &mut Vec<ComponentBlock>) {
    match c["type"].as_u64().unwrap_or(0) {
        1 => {
            let mut buttons = Vec::new();
            for k in c["components"].as_array().into_iter().flatten() {
                match k["type"].as_u64().unwrap_or(0) {
                    2 => buttons.push(parse_button(k)),
                    3 | 5..=8 => {
                        if !buttons.is_empty() {
                            out.push(ComponentBlock::Buttons(std::mem::take(&mut buttons)));
                        }
                        out.push(ComponentBlock::Select(parse_select(k)));
                    }
                    _ => out.push(ComponentBlock::Unsupported),
                }
            }
            if !buttons.is_empty() {
                out.push(ComponentBlock::Buttons(buttons));
            }
        }
        10 => out.push(ComponentBlock::Text(c["content"].as_str().unwrap_or("").to_string())),
        // Section: a few text lines with a button or thumbnail beside them.
        9 => {
            for k in c["components"].as_array().into_iter().flatten() {
                flatten(k, out);
            }
            let a = &c["accessory"];
            match a["type"].as_u64().unwrap_or(0) {
                2 => out.push(ComponentBlock::Buttons(vec![parse_button(a)])),
                11 => out.extend(parse_media(a).map(|m| ComponentBlock::Media(vec![m]))),
                _ => {}
            }
        }
        11 => out.extend(parse_media(c).map(|m| ComponentBlock::Media(vec![m]))),
        12 => {
            let items: Vec<MediaView> = c["items"].as_array().into_iter().flatten().filter_map(parse_media).collect();
            if !items.is_empty() {
                out.push(ComponentBlock::Media(items));
            }
        }
        // File: only its name is known (the bytes are an attachment of the same message).
        13 => {
            let name = c["file"]["url"].as_str().map(|u| u.trim_start_matches("attachment://")).unwrap_or("file");
            out.push(ComponentBlock::Text(format!("\u{1F4CE} {name}")));
        }
        14 => out.push(ComponentBlock::Separator),
        17 => {
            for k in c["components"].as_array().into_iter().flatten() {
                flatten(k, out);
            }
        }
        2 => out.push(ComponentBlock::Buttons(vec![parse_button(c)])),
        _ => out.push(ComponentBlock::Unsupported),
    }
}

fn parse_button(k: &Value) -> ButtonView {
    ButtonView {
        style: k["style"].as_u64().unwrap_or(2) as u8,
        label: k["label"].as_str().unwrap_or("").to_string(),
        emoji: k["emoji"]["name"].as_str().filter(|_| k["emoji"]["id"].is_null()).map(str::to_string),
        custom_id: k["custom_id"].as_str().map(str::to_string),
        url: k["url"].as_str().map(str::to_string),
        disabled: k["disabled"].as_bool().unwrap_or(false),
    }
}

fn parse_select(k: &Value) -> SelectView {
    SelectView {
        component_type: k["type"].as_u64().unwrap_or(3) as u8,
        custom_id: k["custom_id"].as_str().unwrap_or("").to_string(),
        placeholder: k["placeholder"].as_str().map(str::to_string),
        options: k["options"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| SelectOption {
                label: o["label"].as_str().unwrap_or("").to_string(),
                value: o["value"].as_str().unwrap_or("").to_string(),
                description: o["description"].as_str().map(str::to_string),
                default: o["default"].as_bool().unwrap_or(false),
            })
            .collect(),
        min_values: k["min_values"].as_u64().unwrap_or(1) as u32,
        max_values: k["max_values"].as_u64().unwrap_or(1) as u32,
        disabled: k["disabled"].as_bool().unwrap_or(false),
    }
}

/// A modal the application asked us to show (`INTERACTION_MODAL_CREATE`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModalView {
    pub id: String,
    pub custom_id: String,
    pub title: String,
    pub channel: Id,
    pub guild: Option<Id>,
    pub application: Id,
    pub fields: Vec<ModalField>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModalField {
    pub custom_id: String,
    pub label: String,
    pub placeholder: Option<String>,
    pub value: Option<String>,
    pub paragraph: bool,
    pub required: bool,
}

/// capture: the application id's location in the payload (`application.id` / `application_id`).
pub fn parse_modal(v: &Value, fallback_channel: Id) -> Option<ModalView> {
    fn id_of(v: &Value) -> Option<Id> {
        v.as_str().and_then(|s| s.parse().ok())
    }
    fn walk(c: &Value, out: &mut Vec<ModalField>, label: &str) {
        match c["type"].as_u64().unwrap_or(0) {
            1 => c["components"].as_array().into_iter().flatten().for_each(|k| walk(k, out, label)),
            // Label components wrap the input and carry its label.
            18 => walk(&c["component"], out, c["label"].as_str().unwrap_or("")),
            4 => out.push(ModalField {
                custom_id: c["custom_id"].as_str().unwrap_or("").to_string(),
                label: c["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(label).to_string(),
                placeholder: c["placeholder"].as_str().map(str::to_string),
                value: c["value"].as_str().map(str::to_string),
                paragraph: c["style"].as_u64() == Some(2),
                required: c["required"].as_bool().unwrap_or(true),
            }),
            _ => {}
        }
    }
    let mut fields = Vec::new();
    for c in v["components"].as_array()? {
        walk(c, &mut fields, "");
    }
    Some(ModalView {
        id: v["id"].as_str()?.to_string(),
        custom_id: v["custom_id"].as_str().unwrap_or("").to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        channel: id_of(&v["channel_id"]).unwrap_or(fallback_channel),
        guild: id_of(&v["guild_id"]),
        application: id_of(&v["application"]["id"]).or_else(|| id_of(&v["application_id"]))?,
        fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    fn t() -> Target {
        Target { guild: Some(Snowflake(1)), channel: Snowflake(2), application: Snowflake(3) }
    }

    #[test]
    fn button_payload_matches_web_shape() {
        let v = component_interaction(t(), "sess", Snowflake(9), Snowflake(5), 64, 2, "ok", None);
        assert_eq!(
            v,
            json!({"type": 3, "nonce": "9", "guild_id": "1", "channel_id": "2", "message_flags": 64,
                   "message_id": "5", "application_id": "3", "session_id": "sess",
                   "data": {"component_type": 2, "custom_id": "ok"}})
        );
    }

    #[test]
    fn select_payload_and_dm_omits_guild() {
        let mut tg = t();
        tg.guild = None;
        let v = component_interaction(tg, "s", Snowflake(9), Snowflake(5), 0, 3, "menu", Some((3, vec!["a".into()])));
        assert!(v.get("guild_id").is_none());
        assert_eq!(v["data"], json!({"component_type": 3, "custom_id": "menu", "type": 3, "values": ["a"]}));
    }

    #[test]
    fn command_and_modal_shapes() {
        let d = command_data(Snowflake(7), "ping", "8", vec![json!({"type": 3, "name": "q", "value": "x"})], json!({"id": "7"}));
        let v = command_interaction(t(), "s", Snowflake(9), d);
        assert_eq!(v["type"], 2);
        assert_eq!(v["data"]["type"], 1);
        assert_eq!(v["data"]["options"][0]["value"], "x");
        let m = modal_interaction(t(), "s", Snowflake(9), "11", "form", &[("a".into(), "b".into())]);
        assert_eq!(m["type"], 5);
        assert_eq!(m["data"]["components"][0]["components"][0], json!({"type": 4, "custom_id": "a", "value": "b"}));
    }

    #[test]
    fn poll_parse_vote_and_toggle() {
        let mut poll = Some(json!({"question": {"text": "Q"}, "allow_multiselect": false,
            "answers": [{"answer_id": 1, "poll_media": {"text": "A"}}, {"answer_id": 2, "poll_media": {"text": "B", "emoji": {"name": "x"}}}]}));
        apply_poll_vote(&mut poll, 2, true, true);
        apply_poll_vote(&mut poll, 2, true, false);
        apply_poll_vote(&mut poll, 1, true, false);
        let v = parse_poll(poll.as_ref().unwrap()).unwrap();
        assert_eq!((v.total, v.answers[1].count, v.answers[1].me, v.answers[0].me), (3, 2, true, false));
        assert_eq!(next_answer_ids(&v, 2), Vec::<u32>::new());
        assert_eq!(next_answer_ids(&v, 1), vec![1]);
        apply_poll_vote(&mut poll, 2, false, true);
        let v = parse_poll(poll.as_ref().unwrap()).unwrap();
        assert!(!v.answers[1].me && v.answers[1].count == 1);
        assert_eq!(poll_vote_body(&[1, 2]), json!({"answer_ids": ["1", "2"]}));
    }

    #[test]
    fn modal_parse() {
        let v = json!({"id": "9", "custom_id": "f", "title": "T", "channel_id": "2", "application": {"id": "3"},
            "components": [{"type": 1, "components": [{"type": 4, "custom_id": "a", "label": "A", "style": 2}]}]});
        let m = parse_modal(&v, Snowflake(0)).unwrap();
        assert_eq!((m.application, m.fields.len(), m.fields[0].paragraph), (Snowflake(3), 1, true));
    }

    #[test]
    fn components_flatten() {
        let rows = vec![
            json!({"type": 1, "components": [{"type": 2, "style": 1, "label": "Yes", "custom_id": "y"}, {"type": 2, "style": 5, "label": "Docs", "url": "https://x"}]}),
            json!({"type": 1, "components": [{"type": 3, "custom_id": "m", "options": [{"label": "A", "value": "a"}]}]}),
            json!({"type": 17, "components": [{"type": 10, "content": "hi"}, {"type": 14}, {"type": 99}]}),
        ];
        let b = parse_components(&rows);
        assert_eq!(b.len(), 5);
        assert!(matches!(&b[0], ComponentBlock::Buttons(v) if v.len() == 2 && v[1].url.is_some()));
        assert!(matches!(&b[1], ComponentBlock::Select(s) if s.options.len() == 1));
        assert_eq!(b[4], ComponentBlock::Unsupported);
    }

    #[test]
    fn v2_sections_and_galleries() {
        let rows = [json!({"type": 17, "components": [
            {"type": 9, "components": [{"type": 10, "content": "Pick a role"}], "accessory": {"type": 2, "style": 1, "label": "Get", "custom_id": "r"}},
            {"type": 12, "items": [{"media": {"url": "https://x/a.png", "width": 100, "height": 50}}, {"media": {"url": "attachment://b.png"}}]},
            {"type": 9, "components": [{"type": 10, "content": "t"}], "accessory": {"type": 11, "media": {"url": "https://x/t.png"}}},
        ]})];
        let b = parse_components(&rows);
        assert_eq!(b.len(), 5);
        assert!(matches!(&b[0], ComponentBlock::Text(t) if t == "Pick a role"));
        assert!(matches!(&b[1], ComponentBlock::Buttons(v) if v[0].label == "Get"));
        assert!(matches!(&b[2], ComponentBlock::Media(m) if m.len() == 1 && m[0].width == Some(100)));
        assert!(matches!(&b[4], ComponentBlock::Media(_)));
    }
}
