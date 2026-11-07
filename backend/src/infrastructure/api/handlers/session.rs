//! Shared request-context and session-issuing helpers for the auth handlers.
//!
//! The "issue a session for a logged-in user" flow — mint the access JWT, compute
//! its expiry, derive the user-bound CSRF token, and build the `Set-Cookie`
//! headers (tokens are `HttpOnly` cookies only) — was copy-pasted across
//! `login`, `refresh`, `reset_password`, `complete_signup`, and the verify-email
//! auto-login path. [`build_session`] is the single source of truth; each handler
//! only supplies the user and the (already-created) refresh token and then wraps
//! the result in its own response DTO.

use axum::http::header::{self, HeaderMap};
use chrono::{Duration, Utc};

use crate::domain::entities::User;
use crate::infrastructure::api::cookies::{
    create_csrf_cookie, create_jwt_cookie, create_refresh_cookie, extract_cookie,
};
use crate::infrastructure::api::error::AppError;
use crate::infrastructure::api::{create_jwt, generate_csrf_token};
use crate::shared::crypto::Crypto;

/// Header carrying the client's device fingerprint, used to bind refresh tokens.
const DEVICE_FINGERPRINT_HEADER: &str = "x-device-fingerprint";

/// Extract the device fingerprint header, if present. The fingerprint binds a
/// refresh token to the originating device; callers that can issue an
/// access-only session (e.g. verify-email auto-login) treat it as optional.
pub(crate) fn extract_device_fingerprint(headers: &HeaderMap) -> Option<String> {
    headers
        .get(DEVICE_FINGERPRINT_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
}

/// Like [`extract_device_fingerprint`], but rejects a missing header. Used by the
/// flows that always mint a refresh token (login, refresh, reset, complete-signup).
pub(crate) fn require_device_fingerprint(headers: &HeaderMap) -> Result<String, AppError> {
    extract_device_fingerprint(headers).ok_or_else(|| {
        AppError::Validation(format!(
            "Missing required header '{DEVICE_FINGERPRINT_HEADER}'. Provide a unique device identifier for refresh token binding."
        ))
    })
}

/// Extract the refresh token from the `refresh_token` cookie.
pub(crate) fn extract_refresh_token_cookie(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, "refresh_token")
}

/// The non-secret pieces of a freshly issued session: the `Set-Cookie` headers
/// (access JWT, CSRF, and optionally the refresh token) plus the body metadata a
/// client needs (access-token expiry and the readable CSRF token).
pub(crate) struct SessionTokens {
    pub headers: HeaderMap,
    pub expires_at: i64,
    pub csrf_token: String,
}

/// Issue a session for `user`: mint the access JWT, derive the user-bound CSRF
/// token, and build the session cookies. Pass `refresh_token` to include a refresh
/// cookie (a full session); pass `None` for an access-only session.
///
/// The caller owns response-DTO construction (`AuthResponse` /
/// `VerifyEmailResponse`) since the body shapes differ.
///
/// # Errors
/// Returns [`AppError`] if JWT signing fails or a token contains characters that
/// cannot be encoded into a cookie value.
pub(crate) fn build_session(
    crypto: &Crypto,
    user: &User,
    refresh_token: Option<&str>,
) -> Result<SessionTokens, AppError> {
    let jwt_expiration_minutes = crypto.jwt_expiration_minutes();
    let token = create_jwt(
        *user.id().as_uuid(),
        user.email_verified(),
        user.role().to_db_str(),
        crypto.encoding_key(),
        jwt_expiration_minutes,
    )?;
    let expires_at = (Utc::now() + Duration::minutes(jwt_expiration_minutes)).timestamp();
    // CSRF token bound to this user.
    let csrf_token = generate_csrf_token(crypto.csrf_secret(), &user.id().to_string());
    let headers = build_session_cookies(crypto, &token, refresh_token, &csrf_token)?;
    Ok(SessionTokens {
        headers,
        expires_at,
        csrf_token,
    })
}

/// Build the `Set-Cookie` headers for a session: access JWT, the (JS-readable)
/// CSRF token, and — when `refresh_token` is `Some` — the refresh token. A `None`
/// refresh token yields an access-only session (e.g. email verification without a
/// device fingerprint).
///
/// # Errors
/// Returns [`AppError::Internal`] if a token contains characters that cannot be
/// encoded into a cookie value.
pub(crate) fn build_session_cookies(
    crypto: &Crypto,
    token: &str,
    refresh_token: Option<&str>,
    csrf_token: &str,
) -> Result<HeaderMap, AppError> {
    let jwt_expiration_minutes = crypto.jwt_expiration_minutes();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        create_jwt_cookie(token, jwt_expiration_minutes * 60)
            .ok_or_else(|| AppError::Internal("Invalid JWT token characters".into()))?,
    );
    if let Some(refresh_token) = refresh_token {
        headers.append(
            header::SET_COOKIE,
            create_refresh_cookie(
                refresh_token,
                crypto.refresh_token_expiration_days() * 24 * 60 * 60,
            )
            .ok_or_else(|| AppError::Internal("Invalid refresh token characters".into()))?,
        );
    }
    headers.append(
        header::SET_COOKIE,
        create_csrf_cookie(csrf_token)
            .ok_or_else(|| AppError::Internal("Invalid CSRF token characters".into()))?,
    );
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn token_strategy() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_-]{1,64}"
    }

    fn other_cookie_name_strategy() -> impl Strategy<Value = String> {
        "[a-zA-Z_][a-zA-Z0-9_]{0,20}"
            .prop_filter("must not be refresh_token", |s| s != "refresh_token")
    }

    fn cookie_pair_strategy() -> impl Strategy<Value = String> {
        (other_cookie_name_strategy(), token_strategy())
            .prop_map(|(name, value)| format!("{name}={value}"))
    }

    /// Build a `HeaderMap` carrying the given `Cookie` header value, then run the
    /// real `extract_refresh_token_cookie` against it.
    fn extract_refresh(cookie_str: &str) -> Option<String> {
        let mut headers = HeaderMap::new();
        if let Ok(value) = axum::http::HeaderValue::from_str(cookie_str) {
            headers.insert(header::COOKIE, value);
        }
        extract_refresh_token_cookie(&headers)
    }

    #[test]
    fn parse_refresh_token_returns_none_for_empty_string() {
        assert_eq!(extract_refresh(""), None);
    }

    #[test]
    fn parse_refresh_token_extracts_single_cookie() {
        assert_eq!(
            extract_refresh("refresh_token=abc123"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_refresh_token_extracts_from_multiple_cookies() {
        let cookies = "auth_token=xyz; refresh_token=abc123; session_id=999";
        assert_eq!(extract_refresh(cookies), Some("abc123".to_string()));
    }

    #[test]
    fn parse_refresh_token_handles_whitespace() {
        assert_eq!(
            extract_refresh("  refresh_token=abc123  "),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_refresh_token_returns_none_when_missing() {
        assert_eq!(extract_refresh("auth_token=xyz; session_id=999"), None);
    }

    proptest! {
        #[test]
        fn prop_refresh_token_always_extracted_when_present(token in token_strategy()) {
            let result = extract_refresh(&format!("refresh_token={token}"));
            prop_assert_eq!(result, Some(token));
        }

        #[test]
        fn prop_refresh_token_extracted_with_prefix_cookies(
            prefix in prop::collection::vec(cookie_pair_strategy(), 0..5),
            token in token_strategy()
        ) {
            let mut cookies: Vec<String> = prefix;
            cookies.push(format!("refresh_token={token}"));
            let cookie_str = cookies.join("; ");

            let result = extract_refresh(&cookie_str);
            prop_assert_eq!(result, Some(token));
        }

        #[test]
        fn prop_refresh_token_extracted_with_suffix_cookies(
            token in token_strategy(),
            suffix in prop::collection::vec(cookie_pair_strategy(), 0..5)
        ) {
            let mut cookies = vec![format!("refresh_token={token}")];
            cookies.extend(suffix);
            let cookie_str = cookies.join("; ");

            let result = extract_refresh(&cookie_str);
            prop_assert_eq!(result, Some(token));
        }

        #[test]
        fn prop_refresh_token_extracted_regardless_of_position(
            prefix in prop::collection::vec(cookie_pair_strategy(), 0..3),
            token in token_strategy(),
            suffix in prop::collection::vec(cookie_pair_strategy(), 0..3)
        ) {
            let mut cookies: Vec<String> = prefix;
            cookies.push(format!("refresh_token={token}"));
            cookies.extend(suffix);
            let cookie_str = cookies.join("; ");

            let result = extract_refresh(&cookie_str);
            prop_assert_eq!(result, Some(token));
        }

        #[test]
        fn prop_returns_none_without_refresh_token(
            cookies in prop::collection::vec(cookie_pair_strategy(), 1..10)
        ) {
            let cookie_str = cookies.join("; ");
            let result = extract_refresh(&cookie_str);
            prop_assert_eq!(result, None);
        }

        #[test]
        fn prop_handles_varying_whitespace(
            token in token_strategy(),
            spaces_before in 0usize..5,
            spaces_after in 0usize..5
        ) {
            let before = " ".repeat(spaces_before);
            let after = " ".repeat(spaces_after);
            let cookies = format!("{before}refresh_token={token}{after}");

            let result = extract_refresh(&cookies);
            prop_assert_eq!(result, Some(token));
        }
    }
}
