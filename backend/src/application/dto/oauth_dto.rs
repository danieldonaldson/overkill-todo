//! OAuth-related DTOs for requests and responses.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// OAuth flow mode.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OAuthMode {
    /// Login or register via OAuth.
    Login,
    /// Link OAuth to existing account.
    Link,
}

/// Query params for OAuth start endpoint.
#[derive(Debug, Deserialize, ToSchema)]
pub struct OAuthStartQuery {
    pub mode: OAuthMode,
}

/// Request body for storing the OAuth device binding cookie before redirecting.
#[derive(Debug, Deserialize, ToSchema)]
pub struct OAuthDeviceFingerprintRequest {
    pub device_fingerprint: String,
}

/// Query params for OAuth callback (from provider).
#[derive(Debug, Deserialize, ToSchema)]
pub struct OAuthCallbackQuery {
    pub code: String,
    pub state: String,
}

/// Claims stored in the OAuth state JWT (CSRF/session binding for the redirect
/// dance). Produced and verified by `infrastructure::api::oauth_state`.
#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthStateClaims {
    pub nonce: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub provider: String,
    pub exp: i64,
}

/// Test bypass query params (only works with `TEST_OAUTH_BYPASS=true`).
#[cfg(debug_assertions)]
#[derive(Debug, Deserialize)]
pub struct OAuthTestBypassQuery {
    pub code: String,
    pub sub: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub state: Option<String>,
}

#[cfg(debug_assertions)]
impl OAuthTestBypassQuery {
    #[must_use]
    pub fn is_test_bypass(&self) -> bool {
        self.code == "test"
    }
}

/// Provider user info extracted from ID token.
#[derive(Debug, Clone)]
pub struct OAuthUserInfo {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
}
