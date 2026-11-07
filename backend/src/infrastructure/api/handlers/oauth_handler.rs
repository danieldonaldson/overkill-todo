//! OAuth authentication handlers.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Redirect,
};
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(not(debug_assertions))]
use crate::application::dto::OAuthCallbackQuery;
#[cfg(debug_assertions)]
use crate::application::dto::OAuthTestBypassQuery;
use crate::application::dto::{
    OAuthDeviceFingerprintRequest, OAuthMode, OAuthStartQuery, OAuthStateClaims, OAuthUserInfo,
};
use crate::application::ports::OAuthProviderClient;
use crate::application::use_cases::{
    OAuthCallbackMode, OAuthCallbackResult, OAuthCallbackUseCase, OAuthUnlinkUseCase,
    RefreshTokenUseCase,
};
use crate::domain::entities::OAuthProvider;
use crate::domain::value_objects::UserId;
use crate::infrastructure::api::cookies::{
    clear_oauth_device_fingerprint_cookie, clear_oauth_state_cookie,
    create_oauth_device_fingerprint_cookie, create_oauth_state_cookie, extract_cookie,
};
use crate::infrastructure::api::error::AppError;
use crate::infrastructure::api::handlers::session::build_session;
use crate::infrastructure::api::middleware::{AuthUser, OptionalAuthUser};
use crate::infrastructure::api::{create_oauth_state, verify_oauth_state};
use crate::shared::crypto::Crypto;

/// Check if test bypass mode is enabled.
#[cfg(debug_assertions)]
fn is_test_bypass_enabled() -> bool {
    std::env::var("TEST_OAUTH_BYPASS").is_ok()
}

#[cfg(debug_assertions)]
type CallbackQuery = OAuthTestBypassQuery;
#[cfg(not(debug_assertions))]
type CallbackQuery = OAuthCallbackQuery;

/// Dependencies for OAuth handlers.
pub struct OAuthHandlerDeps {
    pub callback_use_case: Arc<OAuthCallbackUseCase>,
    pub unlink_use_case: Arc<OAuthUnlinkUseCase>,
    pub refresh_token_use_case: Arc<RefreshTokenUseCase>,
    /// Registered OAuth provider clients, keyed by provider. Adding a provider is
    /// a registration here (in `main`) — no handler dispatch changes.
    pub oauth_clients: HashMap<OAuthProvider, Arc<dyn OAuthProviderClient>>,
    pub crypto: Arc<Crypto>,
}

impl OAuthHandlerDeps {
    /// Look up the client for `provider`, or a 4xx error if it isn't configured.
    fn oauth_client(&self, provider: OAuthProvider) -> Result<&dyn OAuthProviderClient, AppError> {
        self.oauth_clients
            .get(&provider)
            .map(AsRef::as_ref)
            .ok_or_else(|| {
                AppError::Validation(format!("{} OAuth is not supported", provider.as_str()))
            })
    }
}

fn extract_oauth_device_fingerprint_cookie(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, "oauth_device_fingerprint")
}

/// Read the OAuth state-binding nonce stored at `oauth_start`.
fn extract_oauth_state_cookie(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, "oauth_state")
}

/// Append the `Set-Cookie` headers that clear the OAuth handoff cookies (the
/// state nonce and device fingerprint set during the redirect dance). Every
/// callback outcome must clear them.
fn append_clear_oauth_handoff_cookies(headers: &mut HeaderMap) {
    headers.append(header::SET_COOKIE, clear_oauth_device_fingerprint_cookie());
    headers.append(header::SET_COOKIE, clear_oauth_state_cookie());
}

/// A fresh `HeaderMap` containing only the OAuth handoff-cookie clears, for the
/// callback outcomes that issue no session cookies.
fn cleared_oauth_handoff_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    append_clear_oauth_handoff_cookies(&mut headers);
    headers
}

/// Run the real OAuth flow: verify the state JWT, confirm the provider matches,
/// and exchange the authorization code for the provider's user info. Shared by
/// the debug and release callback paths so the exchange logic lives in one place.
async fn run_real_oauth_flow(
    deps: &OAuthHandlerDeps,
    provider: &str,
    provider_enum: OAuthProvider,
    state: &str,
    code: &str,
) -> Result<(OAuthUserInfo, OAuthStateClaims), AppError> {
    if state.is_empty() {
        return Err(AppError::Validation("Missing state parameter".into()));
    }
    let state_claims = verify_oauth_state(deps.crypto.decoding_key(), state)?;

    if state_claims.provider != provider {
        return Err(AppError::Validation("Provider mismatch in state".into()));
    }

    let user_info = deps
        .oauth_client(provider_enum)?
        .exchange_code(code)
        .await?;

    Ok((user_info, state_claims))
}

/// Store the browser-generated OAuth device binding in an `HttpOnly` scoped cookie.
#[utoipa::path(
    post,
    path = "/api/v1/auth/oauth/device-fingerprint",
    tag = "OAuth",
    request_body = OAuthDeviceFingerprintRequest,
    responses(
        (status = 204, description = "OAuth device fingerprint cookie stored"),
        (status = 400, description = "Invalid fingerprint")
    )
)]
/// # Errors
///
/// Returns an error if the fingerprint contains invalid cookie characters.
pub async fn oauth_store_device_fingerprint(
    Json(request): Json<OAuthDeviceFingerprintRequest>,
) -> Result<(HeaderMap, StatusCode), AppError> {
    let fingerprint = request.device_fingerprint.trim();
    if fingerprint.is_empty() {
        return Err(AppError::Validation(
            "Device fingerprint is required".to_string(),
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        create_oauth_device_fingerprint_cookie(fingerprint).ok_or_else(|| {
            AppError::Internal("Invalid OAuth device fingerprint characters".into())
        })?,
    );

    Ok((headers, StatusCode::NO_CONTENT))
}

/// Parse provider string to enum.
fn parse_provider(provider: &str) -> Result<OAuthProvider, AppError> {
    OAuthProvider::parse(provider)
        .ok_or_else(|| AppError::Validation(format!("Unknown provider: {provider}")))
}

/// Start OAuth flow - redirect to provider.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oauth/{provider}",
    tag = "OAuth",
    params(
        ("provider" = String, Path, description = "OAuth provider (google, apple)"),
        ("mode" = OAuthMode, Query, description = "Flow mode (login or link)")
    ),
    responses(
        (status = 302, description = "Redirect to OAuth provider"),
        (status = 400, description = "Invalid provider"),
        (status = 401, description = "Auth required for link mode")
    )
)]
/// # Errors
///
/// Returns an error if the provider is invalid, authentication is required but not provided, or service errors occur.
#[axum::debug_handler]
pub async fn oauth_start(
    State(deps): State<Arc<OAuthHandlerDeps>>,
    Path(provider): Path<String>,
    Query(query): Query<OAuthStartQuery>,
    OptionalAuthUser(auth_user): OptionalAuthUser,
) -> Result<(HeaderMap, Redirect), AppError> {
    let provider_enum = parse_provider(&provider)?;

    let mode_str = match query.mode {
        OAuthMode::Login => "login",
        OAuthMode::Link => "link",
    };

    // For link mode, require authentication
    let user_id = match query.mode {
        OAuthMode::Link => {
            let user = auth_user.ok_or_else(AppError::token_missing)?;
            Some(user.user_id.to_string())
        }
        OAuthMode::Login => None,
    };

    let (state, nonce) = create_oauth_state(
        deps.crypto.encoding_key(),
        &provider,
        mode_str,
        user_id.as_deref(),
    )?;

    // Bind the flow to this browser by storing the state nonce in an
    // HttpOnly cookie; the callback rejects any state whose nonce doesn't match.
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        create_oauth_state_cookie(&nonce)
            .ok_or_else(|| AppError::Internal("Invalid OAuth state characters".into()))?,
    );

    let auth_url = deps.oauth_client(provider_enum)?.authorization_url(&state);

    tracing::info!(provider = %provider, mode = %mode_str, "Starting OAuth flow");
    Ok((headers, Redirect::temporary(&auth_url)))
}

/// OAuth callback from provider.
///
/// Handles both real OAuth flow and test bypass mode.
#[utoipa::path(
    get,
    path = "/api/v1/auth/oauth/{provider}/callback",
    tag = "OAuth",
    params(
        ("provider" = String, Path, description = "OAuth provider"),
        ("code" = String, Query, description = "Authorization code"),
        ("state" = String, Query, description = "State parameter")
    ),
    responses(
        (status = 302, description = "Redirect to app with result"),
        (status = 400, description = "OAuth error")
    )
)]
/// # Errors
///
/// Returns an error if OAuth flow fails, provider is invalid, or service errors occur.
#[allow(clippy::too_many_lines)]
pub async fn oauth_callback(
    State(deps): State<Arc<OAuthHandlerDeps>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Query(query): Query<CallbackQuery>,
) -> Result<(HeaderMap, Redirect), AppError> {
    let provider_enum = parse_provider(&provider)?;

    // Handle test bypass mode for E2E tests (debug builds only)
    #[cfg(debug_assertions)]
    let (user_info, state_claims, enforce_binding) =
        if is_test_bypass_enabled() && query.is_test_bypass() {
            tracing::warn!(provider = %provider, "Using OAuth test bypass mode");

            let user_info = OAuthUserInfo {
                sub: query.sub.unwrap_or_else(|| "test-sub-123".to_string()),
                email: query.email.clone(),
                email_verified: query.email_verified.unwrap_or(true),
                name: None,
                given_name: Some("Test".to_string()),
                family_name: Some("User".to_string()),
            };

            // For test bypass, state determines mode (optional)
            let claims = if let Some(ref state) = query.state {
                if state.is_empty() {
                    default_test_claims(&provider)
                } else {
                    verify_oauth_state(deps.crypto.decoding_key(), state)?
                }
            } else {
                default_test_claims(&provider)
            };

            // Test bypass does not exercise the browser cookie handshake.
            (user_info, claims, false)
        } else {
            // Real OAuth flow (debug build, bypass not active).
            let state = query.state.as_deref().unwrap_or("");
            let (user_info, state_claims) =
                run_real_oauth_flow(&deps, &provider, provider_enum, state, &query.code).await?;
            (user_info, state_claims, true)
        };

    // Real OAuth flow (release build — CallbackQuery is OAuthCallbackQuery with required state)
    #[cfg(not(debug_assertions))]
    let (user_info, state_claims, enforce_binding) = {
        let (user_info, state_claims) =
            run_real_oauth_flow(&deps, &provider, provider_enum, &query.state, &query.code).await?;
        (user_info, state_claims, true)
    };

    // Reject a callback whose state nonce isn't bound to this browser. An
    // attacker replaying a valid code+state from their own flow has no matching
    // `oauth_state` cookie in the victim's browser, so this fails closed.
    if enforce_binding {
        let provided_nonce = extract_oauth_state_cookie(&headers);
        if provided_nonce.as_deref() != Some(state_claims.nonce.as_str()) {
            tracing::warn!(provider = %provider, "OAuth state binding mismatch — rejecting callback");
            return Err(AppError::Validation("OAuth state binding mismatch".into()));
        }
    }

    let mode = match state_claims.mode.as_str() {
        "link" => OAuthCallbackMode::Link,
        _ => OAuthCallbackMode::Login,
    };

    // The state JWT carries the linking user id as a string; parse it back into a
    // typed `UserId` at the boundary so the use case never re-parses or compares by
    // stringified UUID.
    let linking_user_id = state_claims
        .user_id
        .as_deref()
        .map(str::parse::<UserId>)
        .transpose()
        .map_err(|_| AppError::Validation("Invalid user_id in OAuth state".into()))?;

    let result = deps
        .callback_use_case
        .execute(provider_enum, user_info, mode, linking_user_id.as_ref())
        .await?;
    let device_fingerprint = extract_oauth_device_fingerprint_cookie(&headers);

    match result {
        OAuthCallbackResult::LoggedIn(user) => {
            let refresh_token = if let Some(device_fingerprint) = device_fingerprint.as_deref() {
                Some(
                    deps.refresh_token_use_case
                        .create_for_user(user.id(), device_fingerprint)
                        .await?,
                )
            } else {
                None
            };
            // Share the single session builder with the login / refresh /
            // verification / reset paths so JWT, CSRF, cookie attributes, and
            // error mapping can't drift between them. The redirect carries no body,
            // so the returned `expires_at`/`csrf_token` are unused here.
            let mut headers = build_session(&deps.crypto, &user, refresh_token.as_deref())?.headers;
            append_clear_oauth_handoff_cookies(&mut headers);

            tracing::info!(user_id = %user.id(), provider = %provider, "OAuth login successful");
            Ok((headers, Redirect::temporary("/dashboard")))
        }
        OAuthCallbackResult::Linked => {
            tracing::info!(provider = %provider, "OAuth provider linked");
            Ok((
                cleared_oauth_handoff_headers(),
                Redirect::temporary("/dashboard/settings?oauth=linked"),
            ))
        }
        OAuthCallbackResult::EmailConflict => {
            tracing::warn!(provider = %provider, "OAuth email conflict");
            Ok((
                cleared_oauth_handoff_headers(),
                Redirect::temporary("/login?error=oauth_email_conflict"),
            ))
        }
        OAuthCallbackResult::AlreadyLinked => {
            tracing::warn!(provider = %provider, "OAuth provider already linked to different user");
            Ok((
                cleared_oauth_handoff_headers(),
                Redirect::temporary("/login?error=oauth_already_linked"),
            ))
        }
    }
}

/// Create default test claims for bypass mode.
#[cfg(debug_assertions)]
fn default_test_claims(provider: &str) -> OAuthStateClaims {
    OAuthStateClaims {
        nonce: "test".to_string(),
        mode: "login".to_string(),
        user_id: None,
        provider: provider.to_string(),
        exp: 0,
    }
}

/// Unlink OAuth provider from account.
#[utoipa::path(
    delete,
    path = "/api/v1/auth/oauth/{provider}/unlink",
    tag = "OAuth",
    params(
        ("provider" = String, Path, description = "OAuth provider to unlink (google, apple)")
    ),
    responses(
        (status = 200, description = "Provider unlinked"),
        (status = 400, description = "Cannot unlink last auth method"),
        (status = 404, description = "Provider not linked")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
///
/// Returns an error if the provider is invalid, not linked, or unlinking fails.
pub async fn oauth_unlink(
    State(deps): State<Arc<OAuthHandlerDeps>>,
    Path(provider): Path<String>,
    auth_user: AuthUser,
) -> Result<StatusCode, AppError> {
    let provider_enum = parse_provider(&provider)?;

    deps.unlink_use_case
        .execute(&auth_user.user_id, provider_enum)
        .await?;

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // parse_provider tests
    // =========================================================================

    #[test]
    fn parse_provider_google() {
        let result = parse_provider("google");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), OAuthProvider::Google);
    }

    #[test]
    fn parse_provider_apple() {
        let result = parse_provider("apple");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), OAuthProvider::Apple);
    }

    #[test]
    fn parse_provider_unknown() {
        let result = parse_provider("facebook");
        assert!(result.is_err());
        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.contains("Unknown provider"));
                assert!(msg.contains("facebook"));
            }
            _ => panic!("Expected Validation error"),
        }
    }

    #[test]
    fn parse_provider_empty() {
        let result = parse_provider("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_provider_case_sensitive() {
        // OAuthProvider::parse might be case-sensitive
        let result_upper = parse_provider("GOOGLE");
        let result_mixed = parse_provider("Google");
        // Both should fail if parse is case-sensitive, or succeed if not
        // Just ensure no panic
        let _ = result_upper;
        let _ = result_mixed;
    }

    #[tokio::test]
    async fn oauth_store_device_fingerprint_sets_scoped_cookie() {
        let (headers, status) =
            oauth_store_device_fingerprint(Json(OAuthDeviceFingerprintRequest {
                device_fingerprint: "fingerprint123".to_string(),
            }))
            .await
            .unwrap();

        assert_eq!(status, StatusCode::NO_CONTENT);
        let cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(cookie.starts_with("oauth_device_fingerprint=fingerprint123"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Path=/api/v1/auth/oauth"));
    }

    // Cookie construction is covered by tests in `infrastructure::api::cookies`.

    // =========================================================================
    // is_test_bypass_enabled tests
    // =========================================================================

    // Note: is_test_bypass_enabled tests are omitted because std::env::set_var
    // is unsafe in Rust 2024 edition. The function is tested indirectly
    // through E2E tests that set TEST_OAUTH_BYPASS in the environment.

    // =========================================================================
    // default_test_claims tests (debug only)
    // =========================================================================

    #[cfg(debug_assertions)]
    #[test]
    fn default_test_claims_google() {
        let claims = default_test_claims("google");

        assert_eq!(claims.nonce, "test");
        assert_eq!(claims.mode, "login");
        assert!(claims.user_id.is_none());
        assert_eq!(claims.provider, "google");
        assert_eq!(claims.exp, 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn default_test_claims_apple() {
        let claims = default_test_claims("apple");

        assert_eq!(claims.provider, "apple");
        assert_eq!(claims.mode, "login");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn default_test_claims_arbitrary_provider() {
        let claims = default_test_claims("custom-provider");

        assert_eq!(claims.provider, "custom-provider");
    }

    // =========================================================================
    // OAuthHandlerDeps construction tests
    // =========================================================================

    // Note: Full handler tests would require setting up the complete app context
    // with mock use cases. The helper function tests above cover the key logic
    // without needing the full async infrastructure.
}
