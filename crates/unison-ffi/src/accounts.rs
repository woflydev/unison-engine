use super::*;

#[derive(uniffi::Enum, Clone, Copy, Debug)]
pub enum ConnState {
    Connecting,
    Connected,
    Offline,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum LoginEvent {
    Qr {
        url: String,
    },
    Scanned {
        username: String,
        avatar_url: Option<String>,
    },
    LoggedIn,
    /// Password sign-in needs a second factor.
    Mfa {
        ticket: String,
        totp: bool,
        sms: bool,
        backup: bool,
        webauthn: bool,
        login_instance_id: Option<String>,
    },
    /// Show hCaptcha with these options, then call `login_password_retry` / `login_mfa` with the answer.
    Captcha {
        service: Option<String>,
        sitekey: Option<String>,
        rqdata: Option<String>,
        rqtoken: Option<String>,
        session_id: Option<String>,
    },
    VerifyEmail {
        message: String,
    },
    Cancelled,
    Failed {
        message: String,
    },
}

#[derive(uniffi::Enum, Clone, Copy, Debug)]
pub enum UserGesture {
    Click,
    Key,
    Drop,
    MenuItem,
    NotificationReply,
}

impl UserGesture {
    pub(crate) fn intent(self) -> UserIntent {
        UserIntent::from_gesture(match self {
            UserGesture::Click => CoreGesture::Click,
            UserGesture::Key => CoreGesture::Key,
            UserGesture::Drop => CoreGesture::Drop,
            UserGesture::MenuItem => CoreGesture::MenuItem,
            UserGesture::NotificationReply => CoreGesture::NotificationReply,
        })
    }
}

#[derive(uniffi::Enum, Clone, Copy, Debug)]
pub enum PowerMode {
    Active,
    Visible,
    Background,
    LowPower,
}

// ===== Records =====

#[derive(uniffi::Record, Clone, Debug)]
pub struct AccountInfo {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
}

#[uniffi::export]
impl UnisonEngine {
    pub fn start_login(&self) -> Result<(), FfiError> {
        Ok(self.inner.start_login()?)
    }

    /// Password sign-in (email or phone + password). Results arrive as `LoginEvent`s.
    pub fn login_password(&self, login: String, password: String) -> Result<(), FfiError> {
        Ok(self.inner.login_password(login, password, None)?)
    }

    /// Retry the password step with the captcha the user solved.
    pub fn login_password_retry(
        &self,
        login: String,
        password: String,
        captcha_key: String,
        rqtoken: Option<String>,
        session_id: Option<String>,
    ) -> Result<(), FfiError> {
        let c = unison_core::error::CaptchaSolution { key: captcha_key, rqtoken, session_id };
        Ok(self.inner.login_password_retry(login, password, c)?)
    }

    /// The captcha shown while signing in by QR code was solved.
    pub fn login_qr_captcha(&self, captcha_key: String, rqtoken: Option<String>, session_id: Option<String>) -> Result<(), FfiError> {
        let c = unison_core::error::CaptchaSolution { key: captcha_key, rqtoken, session_id };
        Ok(self.inner.login_qr_captcha(c)?)
    }

    /// Second factor: `kind` is `totp`, `sms`, `backup` or `webauthn`.
    pub fn login_mfa(
        &self,
        kind: String,
        code: String,
        ticket: String,
        login_instance_id: Option<String>,
    ) -> Result<(), FfiError> {
        Ok(self.inner.login_mfa(kind, code, ticket, login_instance_id, None)?)
    }

    pub fn login_sms_send(&self, ticket: String) -> Result<(), FfiError> {
        Ok(self.inner.login_sms_send(ticket)?)
    }

    pub fn cancel_login(&self) {
        self.inner.cancel_login();
    }

    pub fn restore_account(&self, account_id: String, token: String) -> Result<(), FfiError> {
        Ok(self.inner.restore_account(id(&account_id), token)?)
    }

    /// Dev checks: open an account from its cache with no token and no network (REST is refused).
    pub fn open_cached_account(&self, account_id: String) -> Result<(), FfiError> {
        unison_core::rest::set_automation(true);
        Ok(self.inner.open_cached_account(id(&account_id))?)
    }

    pub fn remove_account(&self, account_id: String) {
        self.inner.remove_account(id(&account_id));
    }

    pub fn log_out(&self, account_id: String, gesture: UserGesture) {
        let Some(s) = self.inner.session(id(&account_id)) else {
            return;
        };
        let intent = gesture.intent();
        let engine = self.inner.clone();
        self.inner.rt.spawn(async move {
            let _ = s.logout(intent).await;
            engine.remove_account(s.user_id);
        });
    }

    pub fn accounts(&self) -> Vec<AccountInfo> {
        self.inner
            .accounts()
            .into_iter()
            .filter_map(|a| self.inner.session(a))
            .map(|s| {
                let st = s.store.read();
                match &st.me {
                    Some(u) => account_info(u),
                    None => AccountInfo {
                        id: s.user_id.to_string(),
                        username: String::new(),
                        display_name: String::new(),
                        avatar_url: model::User {
                            id: s.user_id,
                            ..Default::default()
                        }
                        .avatar_url(128),
                    },
                }
            })
            .collect()
    }

    pub fn active_account(&self) -> Option<String> {
        self.inner.active_id().map(|a| a.to_string())
    }

    pub fn set_active_account(&self, account_id: String) {
        self.inner.set_active(id(&account_id));
    }

    /// Total unread mentions for an account (rail badge on the account switcher).
    pub fn account_mentions(&self, account_id: String) -> u32 {
        self.with_store(&account_id, |s| {
            s.read_states.values().map(|r| r.mention_count).sum()
        })
        .unwrap_or(0)
    }

    pub fn set_power(&self, mode: PowerMode) {
        self.inner.set_power(match mode {
            PowerMode::Active => PowerState::Active,
            PowerMode::Visible => PowerState::Visible,
            PowerMode::Background => PowerState::Background,
            PowerMode::LowPower => PowerState::LowPower,
        });
    }

    /// Set the user's status (`online`, `idle`, `dnd`, `invisible`): gateway presence plus the saved
    /// account setting.
    pub fn set_status(&self, account_id: String, status: String, gesture: UserGesture) {
        self.spawn_write(account_id, gesture, move |s, i| async move {
            s.set_status(i, &status).await
        });
    }
}
