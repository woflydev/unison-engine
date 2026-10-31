//! What a public (community) server asks of a new member before it shows them everything:
//! membership screening (rules and questions, `pending` until accepted) and onboarding (pick
//! interests; the choices decide which channels and roles you get). Pure parsing and request
//! bodies, so every shell runs the same flow. Wire shapes are `capture` in `docs/FIDELITY.md`.

use serde_json::{Value, json};

use crate::model::{Id, Member};
use crate::snowflake::Snowflake;

/// Member flag: has finished (or skipped) onboarding.
pub const FLAG_COMPLETED_ONBOARDING: u64 = 1 << 1;
/// Member flag: bypasses screening (moderators, boosters).
pub const FLAG_BYPASSES_VERIFICATION: u64 = 1 << 2;

/// Screening is outstanding: the member is `pending` and does not bypass it.
pub fn screening_due(m: &Member) -> bool {
    m.pending && m.flags & FLAG_BYPASSES_VERIFICATION == 0
}

/// Onboarding is worth asking about: a community server whose member never completed it.
pub fn onboarding_due(features: &[String], m: &Member) -> bool {
    features.iter().any(|f| f == "COMMUNITY") && m.flags & FLAG_COMPLETED_ONBOARDING == 0 && !m.pending
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScreeningField {
    /// `TERMS`, `TEXT_INPUT`, `PARAGRAPH` or `MULTIPLE_CHOICE`.
    pub kind: String,
    pub label: String,
    /// Rules for `TERMS`, the choices for `MULTIPLE_CHOICE`.
    pub values: Vec<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screening {
    pub version: String,
    pub description: String,
    pub fields: Vec<ScreeningField>,
    /// The raw form, echoed back with the answers filled in.
    pub raw: Value,
}

pub fn parse_screening(v: &Value) -> Option<Screening> {
    let fields: Vec<ScreeningField> = v["form_fields"]
        .as_array()?
        .iter()
        .map(|f| ScreeningField {
            kind: f["field_type"].as_str().unwrap_or("TERMS").to_string(),
            label: f["label"].as_str().unwrap_or_default().to_string(),
            values: f["values"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            required: f["required"].as_bool().unwrap_or(true),
        })
        .collect();
    if fields.is_empty() {
        return None;
    }
    Some(Screening {
        version: v["version"].as_str().unwrap_or_default().to_string(),
        description: v["description"].as_str().unwrap_or_default().to_string(),
        fields,
        raw: v.clone(),
    })
}

/// One answer per field, in order: `"true"` for accepted rules, text, or the chosen index.
/// `None` when a required field is unanswered.
pub fn screening_body(s: &Screening, answers: &[String]) -> Option<Value> {
    let mut fields = Vec::new();
    for (i, f) in s.fields.iter().enumerate() {
        let a = answers.get(i).map(String::as_str).unwrap_or_default().trim();
        if f.required && (a.is_empty() || (f.kind == "TERMS" && a != "true")) {
            return None;
        }
        let mut raw = s.raw["form_fields"][i].clone();
        raw["response"] = match f.kind.as_str() {
            "TERMS" => json!(true),
            "MULTIPLE_CHOICE" => a.parse::<u32>().map(|n| json!(n)).unwrap_or(Value::Null),
            _ => json!(a),
        };
        fields.push(raw);
    }
    Some(json!({ "version": s.version, "form_fields": fields }))
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnboardingOption {
    pub id: Id,
    pub title: String,
    pub description: String,
    /// A unicode emoji, or a custom emoji id the shell can draw from the CDN.
    pub emoji: Option<String>,
    pub emoji_id: Option<Id>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnboardingPrompt {
    pub id: Id,
    pub title: String,
    pub single_select: bool,
    pub required: bool,
    pub options: Vec<OnboardingOption>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Onboarding {
    pub prompts: Vec<OnboardingPrompt>,
}

fn id_of(v: &Value) -> Option<Id> {
    v.as_str().and_then(|s| s.parse::<u64>().ok()).map(Snowflake)
}

/// `None` when onboarding is off or has nothing to ask (only prompts shown during onboarding count).
pub fn parse_onboarding(v: &Value) -> Option<Onboarding> {
    if v["enabled"].as_bool() == Some(false) {
        return None;
    }
    let prompts: Vec<OnboardingPrompt> = v["prompts"]
        .as_array()?
        .iter()
        .filter(|p| p["in_onboarding"].as_bool().unwrap_or(true))
        .filter_map(|p| {
            let options: Vec<OnboardingOption> = p["options"]
                .as_array()?
                .iter()
                .filter_map(|o| {
                    Some(OnboardingOption {
                        id: id_of(&o["id"])?,
                        title: o["title"].as_str().unwrap_or_default().to_string(),
                        description: o["description"].as_str().unwrap_or_default().to_string(),
                        emoji: o["emoji"]["name"].as_str().filter(|_| o["emoji"]["id"].is_null()).map(String::from),
                        emoji_id: id_of(&o["emoji"]["id"]),
                    })
                })
                .collect();
            let id = id_of(&p["id"])?;
            (!options.is_empty()).then(|| OnboardingPrompt {
                id,
                title: p["title"].as_str().unwrap_or_default().to_string(),
                single_select: p["single_select"].as_bool().unwrap_or(false),
                required: p["required"].as_bool().unwrap_or(false),
                options,
            })
        })
        .collect();
    (!prompts.is_empty()).then_some(Onboarding { prompts })
}

/// Whether every required prompt has a choice.
pub fn onboarding_complete(o: &Onboarding, chosen: &[Id]) -> bool {
    o.prompts.iter().all(|p| !p.required || p.options.iter().any(|x| chosen.contains(&x.id)))
}

/// `POST /guilds/{id}/onboarding-responses` body. `now_ms` stamps the "seen" maps like the web client.
pub fn onboarding_body(o: &Onboarding, chosen: &[Id], now_ms: u64) -> Value {
    let mut prompts_seen = serde_json::Map::new();
    let mut options_seen = serde_json::Map::new();
    for p in &o.prompts {
        prompts_seen.insert(p.id.to_string(), json!(now_ms));
        for x in &p.options {
            options_seen.insert(x.id.to_string(), json!(now_ms));
        }
    }
    json!({
        "onboarding_responses": chosen.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "onboarding_prompts_seen": prompts_seen,
        "onboarding_responses_seen": options_seen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Value {
        json!({"version": "2024-01-01", "description": "Welcome", "form_fields": [
            {"field_type": "TERMS", "label": "Read and agree to the server rules", "values": ["Be kind", "No spam"], "required": true, "response": null},
            {"field_type": "MULTIPLE_CHOICE", "label": "Where from?", "values": ["EU", "US"], "required": false}
        ]})
    }

    #[test]
    fn screening_needs_the_rules_accepted() {
        let s = parse_screening(&form()).unwrap();
        assert_eq!(s.fields.len(), 2);
        assert!(screening_body(&s, &["".into(), "".into()]).is_none());
        assert!(screening_body(&s, &["false".into()]).is_none());
        let b = screening_body(&s, &["true".into(), "1".into()]).unwrap();
        assert_eq!(b["form_fields"][0]["response"], json!(true));
        assert_eq!(b["form_fields"][1]["response"], json!(1));
        assert_eq!(b["version"], "2024-01-01");
    }

    #[test]
    fn due_checks() {
        let pending = Member { pending: true, ..Default::default() };
        assert!(screening_due(&pending));
        assert!(!screening_due(&Member { pending: true, flags: FLAG_BYPASSES_VERIFICATION, ..Default::default() }));
        let community = vec!["COMMUNITY".to_string()];
        assert!(onboarding_due(&community, &Member::default()));
        assert!(!onboarding_due(&community, &pending));
        assert!(!onboarding_due(&community, &Member { flags: FLAG_COMPLETED_ONBOARDING, ..Default::default() }));
        assert!(!onboarding_due(&[], &Member::default()));
    }

    #[test]
    fn onboarding_prompts_and_body() {
        let v = json!({"enabled": true, "prompts": [
            {"id": "1", "title": "Interests", "single_select": false, "required": true, "in_onboarding": true,
             "options": [{"id": "10", "title": "Art", "description": "", "emoji": {"name": "🎨", "id": null}},
                         {"id": "11", "title": "Games", "emoji": {"name": "pad", "id": "55"}}]},
            {"id": "2", "title": "Hidden", "in_onboarding": false, "options": [{"id": "20", "title": "x"}]}
        ]});
        let o = parse_onboarding(&v).unwrap();
        assert_eq!(o.prompts.len(), 1);
        assert_eq!(o.prompts[0].options[0].emoji.as_deref(), Some("🎨"));
        assert_eq!(o.prompts[0].options[1].emoji_id, Some(Snowflake(55)));
        assert!(!onboarding_complete(&o, &[]));
        assert!(onboarding_complete(&o, &[Snowflake(11)]));
        let b = onboarding_body(&o, &[Snowflake(10)], 5);
        assert_eq!(b["onboarding_responses"], json!(["10"]));
        assert_eq!(b["onboarding_prompts_seen"]["1"], 5);
        assert_eq!(parse_onboarding(&json!({"enabled": false, "prompts": []})), None);
    }
}
