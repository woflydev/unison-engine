//! Which categories of traffic and UI Unison allows. Everything here is off by
//! default; the user turns a category on explicitly (Settings → Privacy & Compatibility).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// `/science` events, `/metrics`, client heartbeats.
    Analytics,
    /// Sentry crash/error envelopes.
    CrashReports,
    /// Experiment exposure/trigger reporting. Assignments are still fetched.
    ExperimentExposure,
    /// Running-game detection and "Now playing".
    ActivityDetection,
    /// Quests, quest bars, quest progress heartbeats.
    Quests,
    /// Ads and sponsored placements.
    Ads,
    /// Nitro/boost upsells, shop promos, coachmarks, "NEW" badges.
    Upsells,
    /// Changelog modals, surveys, tours, prompts.
    Nags,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Analytics,
        Category::CrashReports,
        Category::ExperimentExposure,
        Category::ActivityDetection,
        Category::Quests,
        Category::Ads,
        Category::Upsells,
        Category::Nags,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Analytics => "analytics",
            Category::CrashReports => "crash_reports",
            Category::ExperimentExposure => "experiment_exposure",
            Category::ActivityDetection => "activity_detection",
            Category::Quests => "quests",
            Category::Ads => "ads",
            Category::Upsells => "upsells",
            Category::Nags => "nags",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    /// Categories the user explicitly enabled. Empty = everything blocked.
    #[serde(default)]
    pub enabled: Vec<Category>,
}

impl Policy {
    pub fn allows(&self, c: Category) -> bool {
        self.enabled.contains(&c)
    }

    pub fn set(&mut self, c: Category, on: bool) {
        self.enabled.retain(|x| *x != c);
        if on {
            self.enabled.push(c);
        }
    }

    /// "Behave exactly like the official client" mode.
    pub fn compatibility_all() -> Self {
        Self { enabled: Category::ALL.to_vec() }
    }

    /// Decide whether a REST path (relative to `/api/vN`) may be sent.
    pub fn check_route(&self, path: &str) -> Result<(), Category> {
        match classify_route(path) {
            Some(c) if !self.allows(c) => Err(c),
            _ => Ok(()),
        }
    }
}

/// Map an API path to its category, if it belongs to one.
pub fn classify_route(path: &str) -> Option<Category> {
    let p = path.split('?').next().unwrap_or(path);
    let starts = |pre: &str| p == pre || p.starts_with(&format!("{pre}/"));
    if starts("/science") || starts("/metrics") || p.starts_with("/metrics/v") || starts("/track") {
        return Some(Category::Analytics);
    }
    if p.contains("/client-heartbeat") || p == "/reporting/client-heartbeat" {
        return Some(Category::Analytics);
    }
    if starts("/reporting/unauthenticated/experiment") || p.ends_with("/experiments/exposures") {
        return Some(Category::ExperimentExposure);
    }
    if starts("/quests") || starts("/users/@me/quests") {
        return Some(Category::Quests);
    }
    if starts("/ads") {
        return Some(Category::Ads);
    }
    if starts("/outbound-promotions") || starts("/promotions") || starts("/users/@me/outbound-promotions") {
        return Some(Category::Upsells);
    }
    if starts("/users/@me/survey") || starts("/users/@me/surveys") || starts("/changelogs") {
        return Some(Category::Nags);
    }
    None
}

/// Hosts that are blocked outright when their category is off (used by the web view content rules too).
pub fn classify_host(host: &str) -> Option<Category> {
    if host.ends_with("sentry.io") {
        return Some(Category::CrashReports);
    }
    if host == "ads.discord.com" {
        return Some(Category::Ads);
    }
    None
}

/// Strip tracking parameters from an outbound link before opening it.
pub fn clean_outbound_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else { return url.to_string() };
    let (query, frag) = match query.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query, None),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            let k = kv.split('=').next().unwrap_or("").to_ascii_lowercase();
            !(k.starts_with("utm_") || matches!(k.as_str(), "fbclid" | "gclid" | "dclid" | "mc_eid" | "igshid" | "si" | "ref_src"))
        })
        .collect();
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_block_everything() {
        let p = Policy::default();
        assert_eq!(p.check_route("/science"), Err(Category::Analytics));
        assert_eq!(p.check_route("/metrics/v2"), Err(Category::Analytics));
        assert_eq!(p.check_route("/quests/decision"), Err(Category::Quests));
        assert_eq!(p.check_route("/ads/ios-attributions/sign-payload"), Err(Category::Ads));
        assert_eq!(p.check_route("/outbound-promotions?locale=en"), Err(Category::Upsells));
        assert!(p.check_route("/channels/1/messages").is_ok());
        assert!(p.check_route("/scienceish").is_ok());
    }

    #[test]
    fn compat_toggle() {
        let mut p = Policy::default();
        p.set(Category::Analytics, true);
        assert!(p.check_route("/science").is_ok());
        assert!(Policy::compatibility_all().check_route("/quests").is_ok());
    }

    #[test]
    fn strips_tracking() {
        assert_eq!(clean_outbound_url("https://x.io/a?utm_source=d&id=3&fbclid=9#top"), "https://x.io/a?id=3#top");
        assert_eq!(clean_outbound_url("https://x.io/a?utm_source=d"), "https://x.io/a");
    }
}
