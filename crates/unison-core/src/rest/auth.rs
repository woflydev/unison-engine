//! Password sign-in: `POST /auth/login`, the MFA step, SMS send. Shapes read from the web client
//! (build 652418, `LoginActionCreators.login` / `loginMFAv2`); see docs/FIDELITY.md.
//!
//! These are the only unauthenticated POSTs besides remote-auth. They are reachable solely from the
//! sign-in form (an explicit gesture), so they carry no `UserIntent`. The password never touches
//! disk or the logs, and the token is returned to the engine only to be stored in the Keychain.

use super::*;

/// What the second-factor step offers, from the `/auth/login` response.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MfaChallenge {
    pub ticket: String,
    pub totp: bool,
    pub sms: bool,
    pub backup: bool,
    /// A WebAuthn challenge exists. We do not run it natively: the shell hands off to the web fallback.
    pub webauthn: bool,
    pub login_instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginOutcome {
    Token(String),
    Mfa(MfaChallenge),
    Captcha(CaptchaChallenge),
    /// "New login location detected, please check your e-mail": approve it from the email, then retry.
    VerifyEmail(String),
    /// Error 70007: needs phone verification on the account (done on the web).
    PhoneVerification,
    /// A response with `suspended_user_token` (or a disabled / scheduled-for-deletion account).
    Blocked(String),
    Failed(String),
}

impl PartialEq for CaptchaChallenge {
    fn eq(&self, o: &Self) -> bool {
        self.captcha_sitekey == o.captcha_sitekey && self.captcha_rqtoken == o.captcha_rqtoken
    }
}

const PHONE_VERIFICATION_REQUIRED: u64 = 70007;
const ACCOUNT_SCHEDULED_FOR_DELETION: u64 = 20011;
const ACCOUNT_DISABLED: u64 = 20013;

/// Pure classification of a login or MFA response, so it can be unit-tested.
pub fn classify_login_response(status: u16, v: &Value) -> LoginOutcome {
    if (200..300).contains(&status) {
        if let Some(t) = v["token"].as_str().filter(|t| !t.is_empty()) {
            return LoginOutcome::Token(t.to_string());
        }
        if v["mfa"].as_bool().unwrap_or(false) {
            if let Some(ticket) = v["ticket"].as_str() {
                return LoginOutcome::Mfa(MfaChallenge {
                    ticket: ticket.to_string(),
                    totp: v["totp"].as_bool().unwrap_or(false),
                    sms: v["sms"].as_bool().unwrap_or(false),
                    backup: v["backup"].as_bool().unwrap_or(false),
                    webauthn: v["webauthn"].as_str().is_some_and(|s| !s.is_empty()),
                    login_instance_id: v["login_instance_id"].as_str().map(str::to_string),
                });
            }
        }
        return LoginOutcome::Failed("Discord's sign-in response had no token.".into());
    }
    if v.get("captcha_key").is_some() || v.get("captcha_sitekey").is_some() {
        if let Ok(c) = serde_json::from_value::<CaptchaChallenge>(v.clone()) {
            return LoginOutcome::Captcha(c);
        }
    }
    if v["suspended_user_token"].as_str().is_some() {
        return LoginOutcome::Blocked("This account is suspended. Sign in on discord.com to resolve it.".into());
    }
    match v["code"].as_u64() {
        Some(PHONE_VERIFICATION_REQUIRED) => return LoginOutcome::PhoneVerification,
        Some(ACCOUNT_SCHEDULED_FOR_DELETION) => {
            return LoginOutcome::Blocked("This account is scheduled for deletion. Restore it on discord.com.".into());
        }
        Some(ACCOUNT_DISABLED) => return LoginOutcome::Blocked("This account is disabled.".into()),
        _ => {}
    }
    // Form errors nest: {errors:{login:{_errors:[{code, message}]}}}
    if let Some((code, message)) = first_form_error(&v["errors"]) {
        if code == "ACCOUNT_LOGIN_VERIFICATION_EMAIL" {
            return LoginOutcome::VerifyEmail(message);
        }
        return LoginOutcome::Failed(message);
    }
    LoginOutcome::Failed(v["message"].as_str().unwrap_or("Sign-in failed.").to_string())
}

fn first_form_error(v: &Value) -> Option<(String, String)> {
    match v {
        Value::Object(m) => {
            if let Some(Value::Array(a)) = m.get("_errors") {
                if let Some(e) = a.first() {
                    return Some((
                        e["code"].as_str().unwrap_or("").to_string(),
                        e["message"].as_str().unwrap_or("").to_string(),
                    ));
                }
            }
            m.values().find_map(first_form_error)
        }
        _ => None,
    }
}

impl Rest {
    /// `POST /auth/login {login, password, undelete, login_source, gift_code_sku_id}`.
    /// `undelete:false` and the two nulls are the web client's defaults for a plain sign-in (**capture**).
    pub async fn login_password(
        &self,
        login: &str,
        password: &str,
        captcha: Option<CaptchaSolution>,
    ) -> Result<LoginOutcome> {
        let body = json!({
            "login": login, "password": password, "undelete": false,
            "login_source": null, "gift_code_sku_id": null,
        });
        self.auth_post("/auth/login", body, captcha).await
    }

    /// `POST /auth/mfa/{totp|sms|backup|webauthn} {code, ticket, login_source, gift_code_sku_id, login_instance_id}`.
    pub async fn login_mfa(
        &self,
        kind: &str,
        code: &str,
        ticket: &str,
        login_instance_id: Option<&str>,
        captcha: Option<CaptchaSolution>,
    ) -> Result<LoginOutcome> {
        if !matches!(kind, "totp" | "sms" | "backup" | "webauthn") {
            return Err(Error::Other(format!("unknown MFA type {kind}")));
        }
        let body = json!({
            "code": code, "ticket": ticket, "login_source": null,
            "gift_code_sku_id": null, "login_instance_id": login_instance_id,
        });
        self.auth_post(&format!("/auth/mfa/{kind}"), body, captcha).await
    }

    /// `POST /auth/mfa/sms/send {ticket}`: body **capture** (the call site is in a chunk not read).
    pub async fn login_sms_send(&self, ticket: &str) -> Result<()> {
        match self.auth_post("/auth/mfa/sms/send", json!({ "ticket": ticket }), None).await? {
            LoginOutcome::Failed(m) => Err(Error::Other(m)),
            _ => Ok(()),
        }
    }

    /// Like `request`, but keeps the body of an error response: the sign-in screen needs it.
    async fn auth_post(&self, path: &str, body: Value, captcha: Option<CaptchaSolution>) -> Result<LoginOutcome> {
        if let Err(c) = self.policy.read().check_route(path) {
            return Err(Error::BlockedByPolicy(c.label()));
        }
        let payload = serde_json::to_vec(&body)?;
        let mut extra: Vec<(&str, String)> = Vec::new();
        if let Some(c) = &captcha {
            extra.push(("x-captcha-key", c.key.clone()));
            if let Some(t) = &c.rqtoken {
                extra.push(("x-captcha-rqtoken", t.clone()));
            }
            if let Some(s) = &c.session_id {
                extra.push(("x-captcha-session-id", s.clone()));
            }
        }
        let cookie = self.transport.cookie_header();
        let fingerprint = self.fingerprint.lock().clone();
        let referer = "https://discord.com/login".to_string();
        let headers = self.identity.rest_headers(&RestHeaderInput {
            token: None,
            content_type: Some("application/json"),
            content_length: Some(payload.len()),
            sends_origin: true,
            referer: Some(&referer),
            cookie: Some(&cookie),
            fingerprint: fingerprint.as_deref(),
            extra: &extra,
        });
        let resp = self
            .transport
            .send(Method::POST, &self.url(path), headers, Some(payload))
            .await?;
        if resp.status == 429 {
            let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
            return Err(Error::RateLimited(Duration::from_secs_f64(v["retry_after"].as_f64().unwrap_or(1.0))));
        }
        let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
        Ok(classify_login_response(resp.status, &v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_and_mfa() {
        assert_eq!(classify_login_response(200, &json!({"token":"abc"})), LoginOutcome::Token("abc".into()));
        let o = classify_login_response(
            200,
            &json!({"mfa":true,"ticket":"t","sms":true,"totp":true,"backup":true,"webauthn":null,"token":null,"login_instance_id":"i"}),
        );
        assert_eq!(
            o,
            LoginOutcome::Mfa(MfaChallenge {
                ticket: "t".into(), totp: true, sms: true, backup: true, webauthn: false,
                login_instance_id: Some("i".into())
            })
        );
    }

    #[test]
    fn errors() {
        let v = json!({"code":50035,"errors":{"login":{"_errors":[{"code":"ACCOUNT_LOGIN_VERIFICATION_EMAIL","message":"check your e-mail"}]}}});
        assert_eq!(classify_login_response(400, &v), LoginOutcome::VerifyEmail("check your e-mail".into()));
        let v = json!({"code":50035,"errors":{"login":{"_errors":[{"code":"INVALID_LOGIN","message":"Login or password is invalid."}]}}});
        assert_eq!(classify_login_response(400, &v), LoginOutcome::Failed("Login or password is invalid.".into()));
        let v = json!({"captcha_key":["captcha-required"],"captcha_sitekey":"k","captcha_service":"hcaptcha","captcha_rqdata":"d","captcha_rqtoken":"q"});
        assert!(matches!(classify_login_response(400, &v), LoginOutcome::Captcha(c) if c.captcha_rqtoken.as_deref()==Some("q")));
        assert_eq!(classify_login_response(400, &json!({"code":70007})), LoginOutcome::PhoneVerification);
    }
}
