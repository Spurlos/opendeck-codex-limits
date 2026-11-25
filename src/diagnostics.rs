use serde::Serialize;
use std::fmt;

/// Sanitized failure codes used in logs and inspector status messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Failure {
    MissingExecutable,
    StartFailed,
    LoginRequired,
    UnsupportedAccount,
    Timeout,
    InvalidResponse,
    RequestFailed,
    ProcessFailed,
}

impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingExecutable => "Codex not found. Make sure codex is on OpenDeck's PATH.",
            Self::StartFailed => {
                "Codex could not start. Check executable permissions and the OpenDeck plugin log."
            }
            Self::LoginRequired => "Codex login required. Run codex login, then refresh again.",
            Self::UnsupportedAccount => {
                "Subscription quota is unavailable for this account. Sign in to Codex with a supported ChatGPT account."
            }
            Self::Timeout => {
                "Codex did not respond within 30 seconds. Check your connection and try again."
            }
            Self::InvalidResponse => {
                "Codex returned an unexpected response. Check your Codex CLI version and try again."
            }
            Self::RequestFailed => {
                "Codex could not read quota. Check your connection and Codex login, then try again."
            }
            Self::ProcessFailed => {
                "Codex exited unexpectedly. Check your Codex CLI installation and try again."
            }
        }
    }

    pub fn from_error(error: &anyhow::Error) -> Self {
        error
            .downcast_ref::<Self>()
            .copied()
            .unwrap_or(Self::RequestFailed)
    }

    /// Classifies a Codex RPC error without exposing the original message,
    /// which may contain account information.
    pub fn remote(error: &serde_json::Value) -> Self {
        let text = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if [
            "not logged in",
            "not authenticated",
            "login required",
            "authentication required",
            "unauthorized",
            "401",
        ]
        .iter()
        .any(|s| text.contains(s))
        {
            Self::LoginRequired
        } else if [
            "api key",
            "api-key",
            "chatgpt account",
            "chatgpt authentication",
            "unsupported account",
        ]
        .iter()
        .any(|s| text.contains(s))
        {
            Self::UnsupportedAccount
        } else {
            Self::RequestFailed
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for Failure {}

/// Runtime status sent to each open property inspector as `quotaStatus`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub r#type: &'static str,
    pub last_success_at: Option<i64>,
    pub refreshing: bool,
    pub error_code: Option<Failure>,
    pub error_message: Option<&'static str>,
    pub consecutive_failures: u32,
    pub next_refresh_at: Option<i64>,
    pub auto_refresh_enabled: bool,
    pub has_visible_buttons: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_remote_errors_without_exposing_private_details() {
        for (message, expected) in [
            (
                "Unauthorized: private account token",
                Failure::LoginRequired,
            ),
            (
                "Rate limits require ChatGPT authentication",
                Failure::UnsupportedAccount,
            ),
            (
                "API key accounts not supported",
                Failure::UnsupportedAccount,
            ),
            ("private diagnostic", Failure::RequestFailed),
        ] {
            let failure = Failure::remote(&json!({"message":message}));
            assert_eq!(failure, expected);
            assert!(!failure.message().contains("private"));
        }
        assert_eq!(
            Failure::from_error(&anyhow::anyhow!("private token")),
            Failure::RequestFailed
        );
    }
}
