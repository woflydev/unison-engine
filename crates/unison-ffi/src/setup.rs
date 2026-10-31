//! Server setup for new members: membership screening and onboarding (rules in
//! `unison_core::server_setup`).
use super::*;
use unison_core::session::SetupStep;

#[derive(uniffi::Record, Clone, Debug)]
pub struct ScreeningFieldItem {
    /// `TERMS`, `TEXT_INPUT`, `PARAGRAPH` or `MULTIPLE_CHOICE`.
    pub kind: String,
    pub label: String,
    pub values: Vec<String>,
    pub required: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct OnboardingOptionItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub emoji: Option<String>,
    pub emoji_url: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct OnboardingPromptItem {
    pub id: String,
    pub title: String,
    pub single_select: bool,
    pub required: bool,
    pub options: Vec<OnboardingOptionItem>,
}

/// The next thing a server wants from me before it shows its channels and members.
#[derive(uniffi::Enum, Clone, Debug)]
pub enum SetupItem {
    Screening { description: String, fields: Vec<ScreeningFieldItem> },
    Onboarding { prompts: Vec<OnboardingPromptItem> },
}

#[uniffi::export]
impl UnisonEngine {
    /// What the server still asks of me (rules first, then onboarding), or nothing.
    pub async fn server_setup(&self, account_id: String, guild_id: String, force: bool) -> Result<Option<SetupItem>, FfiError> {
        let s = self.inner.session(id(&account_id)).ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let g = id(&guild_id);
        let jh = self.inner.rt.spawn(async move { s.server_setup(g, force).await });
        let step = jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(step.map(|st| match st {
            SetupStep::Screening(f) => SetupItem::Screening {
                description: f.description,
                fields: f.fields.into_iter().map(|x| ScreeningFieldItem { kind: x.kind, label: x.label, values: x.values, required: x.required }).collect(),
            },
            SetupStep::Onboarding(o) => SetupItem::Onboarding {
                prompts: o
                    .prompts
                    .into_iter()
                    .map(|p| OnboardingPromptItem {
                        id: p.id.to_string(),
                        title: p.title,
                        single_select: p.single_select,
                        required: p.required,
                        options: p
                            .options
                            .into_iter()
                            .map(|x| OnboardingOptionItem {
                                id: x.id.to_string(),
                                title: x.title,
                                description: x.description,
                                emoji: x.emoji,
                                emoji_url: x.emoji_id.map(|e| format!("https://cdn.discordapp.com/emojis/{e}.webp?size=48")),
                            })
                            .collect(),
                    })
                    .collect(),
            },
        }))
    }

    /// Whether a field list's answers are enough to submit (rules accepted, required fields filled).
    pub fn screening_ready(&self, fields: Vec<ScreeningFieldItem>, answers: Vec<String>) -> bool {
        fields.iter().enumerate().all(|(i, f)| {
            let a = answers.get(i).map(|a| a.trim()).unwrap_or_default();
            !f.required || (!a.is_empty() && (f.kind != "TERMS" || a == "true"))
        })
    }

    /// Whether the chosen options satisfy every required prompt.
    pub fn onboarding_ready(&self, prompts: Vec<OnboardingPromptItem>, chosen: Vec<String>) -> bool {
        prompts.iter().all(|p| !p.required || p.options.iter().any(|o| chosen.contains(&o.id)))
    }

    /// Accept the rules / answer the screening questions (one answer per field, in order).
    pub async fn accept_screening(&self, account_id: String, guild_id: String, answers: Vec<String>, gesture: UserGesture) -> Result<(), FfiError> {
        let s = self.inner.session(id(&account_id)).ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let (g, intent) = (id(&guild_id), gesture.intent());
        let jh = self.inner.rt.spawn(async move { s.accept_screening(intent, g, &answers).await });
        jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(())
    }

    /// Submit the onboarding choices (option ids; empty skips optional prompts).
    pub async fn submit_onboarding(&self, account_id: String, guild_id: String, chosen: Vec<String>, gesture: UserGesture) -> Result<(), FfiError> {
        let s = self.inner.session(id(&account_id)).ok_or_else(|| FfiError::Failed { message: "no such account".into() })?;
        let (g, intent) = (id(&guild_id), gesture.intent());
        let chosen: Vec<_> = chosen.iter().map(|c| id(c)).collect();
        let jh = self.inner.rt.spawn(async move { s.submit_onboarding(intent, g, &chosen).await });
        jh.await.map_err(|e| FfiError::Failed { message: e.to_string() })??;
        Ok(())
    }
}
