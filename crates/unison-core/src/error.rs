use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Network(String),
    /// The WebSocket upgrade was answered with a refusal. Never retried: a repeat only adds traffic.
    #[error("websocket upgrade refused: HTTP {0}")]
    UpgradeRefused(u16),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, code: Option<u64>, message: String },
    #[error("rate limited, retry after {0:?}")]
    RateLimited(Duration),
    /// Discord wants a captcha. The UI shows it to the user; we never solve it.
    #[error("captcha required")]
    Captcha(Box<CaptchaChallenge>),
    #[error("multi-factor authentication required")]
    MfaRequired { ticket: String, methods: Vec<String> },
    #[error("unauthorized: token is invalid or expired")]
    Unauthorized,
    #[error("blocked by privacy policy ({0})")]
    BlockedByPolicy(&'static str),
    #[error("write without a user gesture was refused")]
    NoIntent,
    #[error("gateway closed: {code} {reason}")]
    GatewayClosed { code: u16, reason: String },
    #[error("decode: {0}")]
    Decode(String),
    #[error("cache: {0}")]
    Cache(String),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("{0}")]
    Other(String),
    /// Stopped by the user (the upload's Cancel button). Never shown as an error.
    #[error("cancelled")]
    Cancelled,
    /// Refused before anything was sent, with a sentence the user can act on.
    #[error("{0}")]
    Refused(String),
}

impl Error {
    /// What to tell the user: one plain sentence, never a status line or JSON. The raw error stays
    /// available through `Display` for logs.
    pub fn user_message(&self) -> String {
        match self {
            Error::Refused(m) => m.clone(),
            Error::Cancelled => "Cancelled.".into(),
            Error::Network(_) => "Couldn't reach Discord. Check your connection and try again.".into(),
            Error::UpgradeRefused(c) => format!("Discord refused the live connection (HTTP {c}). Restart the app to try again."),
            Error::RateLimited(d) => format!("Slow down a moment: try again in {}.", wait(*d)),
            Error::Unauthorized => "Your session expired. Sign in again.".into(),
            Error::Captcha(_) => "Discord wants you to prove you're human first.".into(),
            // Our own refusals, never Discord's: say which.
            Error::BlockedByPolicy("automation") => "Unison is in test-automation mode and isn't contacting Discord.".into(),
            Error::BlockedByPolicy(c) => format!("Blocked by your privacy settings ({}). Unison didn't send it.", c.replace('_', " ")),
            Error::NoIntent => "Unison only does that from a direct click or key press, so it didn't send it.".into(),
            Error::Http { status, code, message } => http_message(*status, *code, message),
            Error::Decode(_) | Error::Cache(_) | Error::Crypto(_) => "Something went wrong inside Unison. Try again.".into(),
            Error::MfaRequired { .. } | Error::GatewayClosed { .. } => "Connection problem. Try again.".into(),
            Error::Other(m) => m.clone(),
        }
    }
}

fn wait(d: Duration) -> String {
    let s = d.as_secs_f64().ceil() as u64;
    if s >= 120 { format!("{} minutes", s.div_ceil(60)) } else if s <= 1 { "a second".into() } else { format!("{s} seconds") }
}

/// Friendly text for Discord's error codes the client can hit while writing. `message` may carry
/// field details after " | " (see `rest::error_details`).
fn http_message(status: u16, code: Option<u64>, message: &str) -> String {
    let details = message.split_once(" | ").map_or("", |(_, d)| d);
    let lower = details.to_lowercase();
    if lower.contains("or fewer in length") {
        // "content: Must be 2000 or fewer in length."
        let n: String = lower.split("must be ").nth(1).unwrap_or("").chars().take_while(char::is_ascii_digit).collect();
        let what = if lower.starts_with("content") { "message" } else { "text" };
        return if n.is_empty() { format!("That {what} is too long.") } else { format!("That {what} is too long: the limit is {n} characters.") };
    }
    match (status, code) {
        (413, _) | (_, Some(40005)) => "That file is too large to upload here.".into(),
        (_, Some(50013)) => "You don't have permission to do that here.".into(),
        (_, Some(50001)) => "You don't have access to that.".into(),
        (_, Some(50007)) => "This person doesn't accept messages from you.".into(),
        (_, Some(50008)) => "You can't send messages in this channel.".into(),
        (_, Some(50021)) => "You can't do that to a system message.".into(),
        (_, Some(50024)) => "You can't do that in this kind of channel.".into(),
        (_, Some(50045)) => "That file is too large to upload here.".into(),
        (_, Some(50046)) => "That file type isn't allowed.".into(),
        (_, Some(40002)) => "Discord wants you to verify your account before you can do that. Finish verifying on discord.com, then try again.".into(),
        (_, Some(40058)) => "That reply is no longer possible.".into(),
        (_, Some(40060)) => "That was already done.".into(),
        (_, Some(20016)) => "Slowmode is on in this channel. Wait a little before sending again.".into(),
        (_, Some(20028)) => "Slow down: this channel is rate limited right now.".into(),
        (_, Some(10003)) => "That channel no longer exists.".into(),
        (_, Some(10008)) => "That message no longer exists.".into(),
        (_, Some(10013)) => "That person no longer exists.".into(),
        (_, Some(30003)) => "You've reached the pin limit for this channel.".into(),
        (_, Some(30010)) => "That message already has the maximum number of reactions.".into(),
        (_, Some(90001)) => "You can't add reactions to this message.".into(),
        (_, Some(50035)) => if message.is_empty() || details.is_empty() { "Discord rejected that.".into() } else { format!("Discord rejected that: {}", details.trim_end_matches('.')) },
        (403, _) => "You don't have permission to do that.".into(),
        (404, _) => "That no longer exists.".into(),
        (500..=599, _) => "Discord is having trouble right now. Try again in a moment.".into(),
        _ => {
            let m = message.split(" | ").next().unwrap_or("").trim();
            if m.is_empty() { "Discord couldn't do that.".into() } else { format!("Discord said: {}", m.trim_end_matches('.')) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16, code: Option<u64>, message: &str) -> String {
        Error::Http { status, code, message: message.into() }.user_message()
    }

    #[test]
    fn upload_and_length_errors_are_plain_sentences() {
        assert_eq!(http(413, None, ""), "That file is too large to upload here.");
        assert_eq!(http(400, Some(40005), "Request entity too large"), "That file is too large to upload here.");
        assert_eq!(
            http(400, Some(50035), "Invalid Form Body | content: Must be 2000 or fewer in length."),
            "That message is too long: the limit is 2000 characters."
        );
        assert_eq!(http(400, Some(50035), "Invalid Form Body | embeds: Something odd."), "Discord rejected that: embeds: Something odd");
        assert_eq!(http(400, Some(50035), "Invalid Form Body"), "Discord rejected that.");
    }

    #[test]
    fn permission_and_state_errors_are_plain_sentences() {
        assert_eq!(http(403, Some(50013), "Missing Permissions"), "You don't have permission to do that here.");
        assert_eq!(http(403, Some(50007), "Cannot send messages to this user"), "This person doesn't accept messages from you.");
        assert_eq!(http(400, Some(20016), "x"), "Slowmode is on in this channel. Wait a little before sending again.");
        assert_eq!(http(404, Some(10008), "Unknown Message"), "That message no longer exists.");
        assert_eq!(http(502, None, ""), "Discord is having trouble right now. Try again in a moment.");
        assert_eq!(http(400, None, "Weird thing."), "Discord said: Weird thing");
        assert_eq!(http(418, None, ""), "Discord couldn't do that.");
    }

    #[test]
    fn non_http_errors_have_user_text() {
        assert_eq!(Error::Network("dns".into()).user_message(), "Couldn't reach Discord. Check your connection and try again.");
        assert_eq!(Error::RateLimited(Duration::from_millis(2400)).user_message(), "Slow down a moment: try again in 3 seconds.");
        assert_eq!(Error::RateLimited(Duration::from_secs(300)).user_message(), "Slow down a moment: try again in 5 minutes.");
        assert_eq!(Error::Refused("Too big.".into()).user_message(), "Too big.");
        assert!(!Error::Decode("expected value at line 1".into()).user_message().contains("expected"));
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct CaptchaChallenge {
    #[serde(default)]
    pub captcha_key: Vec<String>,
    pub captcha_sitekey: Option<String>,
    pub captcha_service: Option<String>,
    pub captcha_rqdata: Option<String>,
    pub captcha_rqtoken: Option<String>,
    pub captcha_session_id: Option<String>,
}

/// The user's answer to a [`CaptchaChallenge`], sent back on the retried request.
#[derive(Debug, Clone)]
pub struct CaptchaSolution {
    pub key: String,
    pub rqtoken: Option<String>,
    pub session_id: Option<String>,
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e.to_string())
    }
}

impl From<unison_etf::Error> for Error {
    fn from(e: unison_etf::Error) -> Self {
        Error::Decode(e.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Cache(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
