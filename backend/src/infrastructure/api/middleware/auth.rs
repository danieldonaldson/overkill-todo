use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::DecodingKey;

use crate::{
    application::ports::outgoing::{SessionAuthorizationState, UserRepository},
    domain::value_objects::{UserId, UserRole},
    infrastructure::api::{cookies::extract_cookie, error::AppError, verify_jwt},
    shared::crypto::Crypto,
};

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: UserId,
    pub email_verified: bool,
    pub role: UserRole,
    /// Session issue time in whole Unix seconds.
    pub issued_at: i64,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(
            parts
                .extensions
                .get::<Self>()
                .cloned()
                .ok_or_else(AppError::token_missing),
        )
    }
}

/// Optional auth extractor that attempts to authenticate from cookies/headers.
/// Returns None if no valid token is present (instead of erroring).
#[derive(Clone, Debug)]
pub struct OptionalAuthUser(pub Option<AuthUser>);

impl<S> FromRequestParts<S> for OptionalAuthUser
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // Nothing to await: resolve synchronously and hand back a ready future.
        let result = (|| -> Result<Self, Self::Rejection> {
            // First check if middleware already set the user
            if let Some(user) = parts.extensions.get::<AuthUser>().cloned() {
                return Ok(Self(Some(user)));
            }

            // Otherwise try to resolve a principal from headers/cookies, using the
            // crypto handle injected as a request extension by the router.
            let Some(crypto) = parts.extensions.get::<Arc<Crypto>>().cloned() else {
                return Ok(Self(None));
            };
            Ok(Self(
                resolve_principal(crypto.decoding_key(), &parts.headers).ok(),
            ))
        })();
        std::future::ready(result)
    }
}

/// Extract the session token from the `Authorization: Bearer` header, falling
/// back to the `auth_token` cookie.
fn extract_token(headers: &HeaderMap) -> Option<String> {
    // First, try Authorization header (Bearer token)
    if let Some(auth_header) = headers.get(AUTHORIZATION).and_then(|h| h.to_str().ok())
        && let Some(token) = auth_header.strip_prefix("Bearer ")
    {
        return Some(token.to_string());
    }

    // Fallback: try auth_token cookie
    extract_cookie(headers, "auth_token")
}

/// Verify a request's session token (Bearer or `auth_token` cookie) and build
/// the [`AuthUser`], preserving the specific failure: `token_missing` when no
/// token is present, or the `verify_jwt` error (invalid/expired) otherwise.
///
/// The single place that turns a token into a principal — used by the
/// middleware, the `Option`-returning [`authenticate`], and the
/// [`OptionalAuthUser`] extractor.
fn resolve_principal(
    decoding_key: &DecodingKey,
    headers: &HeaderMap,
) -> Result<AuthUser, AppError> {
    let token = extract_token(headers).ok_or_else(AppError::token_missing)?;
    let claims = verify_jwt(&token, decoding_key)?;
    let user_id = claims.user_id()?;
    let role = UserRole::from_db_str(&claims.role).unwrap_or_default();
    Ok(AuthUser {
        user_id: UserId::from_uuid(user_id),
        email_verified: claims.verified,
        role,
        issued_at: claims.iat,
    })
}

/// Verify the request's session token and build the [`AuthUser`]. Returns `None`
/// when no valid session token is present.
fn authenticate(decoding_key: &DecodingKey, req: &Request) -> Option<AuthUser> {
    resolve_principal(decoding_key, req.headers()).ok()
}

/// Ensure the request's [`AuthUser`] extension is populated, verifying the
/// session token if it isn't already, and return the resolved principal.
/// Returns `None` when no valid session token is present.
///
/// Used by the CSRF middleware, which runs *before* `auth_middleware`.
/// Stashing the verified principal here lets `auth_middleware` reuse it instead
/// of verifying the (RS256) JWT a second time on the same request.
pub(crate) fn ensure_authenticated(
    decoding_key: &DecodingKey,
    req: &mut Request,
) -> Option<AuthUser> {
    if let Some(user) = req.extensions().get::<AuthUser>().cloned() {
        return Some(user);
    }
    let user = authenticate(decoding_key, req)?;
    req.extensions_mut().insert(user.clone());
    Some(user)
}

/// # Errors
///
/// Returns an error if the authentication token is missing, invalid, or expired.
pub async fn auth_middleware(mut req: Request, next: Next) -> Result<Response, AppError> {
    // Reuse the principal if an earlier layer (the CSRF middleware) already
    // verified the session token, so the JWT is verified at most once per
    // request. Falls through to verification for GET/excluded routes the CSRF
    // layer skips.
    if req.extensions().get::<AuthUser>().is_none() {
        // The crypto handle is injected as a request extension by the router's
        // `Extension(Arc<Crypto>)` layer; it is always present in a built router.
        let crypto = req
            .extensions()
            .get::<Arc<Crypto>>()
            .cloned()
            .ok_or_else(|| AppError::Internal("crypto handle not injected".into()))?;
        let user = resolve_principal(crypto.decoding_key(), req.headers())?;
        // Record user_id on the current span for trace correlation.
        tracing::Span::current().record("user_id", user.user_id.to_string());
        req.extensions_mut().insert(user);
    }

    Ok(next.run(req).await)
}

/// Reject a session token whose role no longer matches persistence.
///
/// Applied to sensitive (admin) routes only, so ordinary authenticated traffic
/// pays no extra DB read. Runs after [`auth_middleware`] (which populates
/// [`AuthUser`]). A role change makes the persisted role differ from every
/// previously issued token, so stale privileges are rejected on the next admin
/// request instead of lingering until access-token expiry.
///
/// # Errors
/// Returns `token_missing` if no principal was resolved upstream, `token_invalid`
/// when the token's role is stale, or a persistence error from the repository.
pub async fn require_fresh_session(
    State(users): State<Arc<dyn UserRepository>>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let auth = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or_else(AppError::token_missing)?;

    if !session_authorization_is_current(
        auth.role,
        auth.issued_at,
        users.session_authorization_state(&auth.user_id).await?,
    ) {
        return Err(AppError::token_invalid(
            "session superseded by a privilege change; please re-authenticate",
        ));
    }

    Ok(next.run(req).await)
}

fn session_authorization_is_current(
    token_role: UserRole,
    issued_at: i64,
    current: Option<SessionAuthorizationState>,
) -> bool {
    matches!(current, Some(state) if state.role == token_role && state.tokens_valid_after.is_none_or(|cutoff| issued_at > cutoff.timestamp()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::COOKIE;
    use chrono::{TimeZone, Utc};

    #[test]
    fn changed_role_invalidates_session() {
        assert!(!session_authorization_is_current(
            UserRole::Admin,
            100,
            Some(SessionAuthorizationState {
                role: UserRole::User,
                tokens_valid_after: None,
            })
        ));
    }

    #[test]
    fn unchanged_role_keeps_session_current() {
        assert!(session_authorization_is_current(
            UserRole::Admin,
            100,
            Some(SessionAuthorizationState {
                role: UserRole::Admin,
                tokens_valid_after: None,
            })
        ));
    }

    #[test]
    fn token_from_cutoff_second_remains_revoked_after_role_cycle() {
        let cutoff = Utc.timestamp_opt(100, 500_000_000).unwrap();
        assert!(!session_authorization_is_current(
            UserRole::Admin,
            100,
            Some(SessionAuthorizationState {
                role: UserRole::Admin,
                tokens_valid_after: Some(cutoff),
            })
        ));
    }

    #[test]
    fn test_extract_token_from_bearer_header() {
        let req = Request::builder()
            .header(AUTHORIZATION, "Bearer test-jwt-token-123")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert_eq!(token, Some("test-jwt-token-123".to_string()));
    }

    #[test]
    fn test_extract_token_from_cookie() {
        let req = Request::builder()
            .header(COOKIE, "auth_token=cookie-jwt-token-456")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert_eq!(token, Some("cookie-jwt-token-456".to_string()));
    }

    #[test]
    fn test_extract_token_prefers_bearer_over_cookie() {
        let req = Request::builder()
            .header(AUTHORIZATION, "Bearer bearer-token")
            .header(COOKIE, "auth_token=cookie-token")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        // Bearer header should take precedence
        assert_eq!(token, Some("bearer-token".to_string()));
    }

    #[test]
    fn test_extract_token_from_cookie_with_multiple_cookies() {
        let req = Request::builder()
            .header(COOKIE, "other=value; auth_token=my-token; another=foo")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert_eq!(token, Some("my-token".to_string()));
    }

    #[test]
    fn test_extract_token_none_when_no_auth() {
        let req = Request::builder().body(()).unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert!(token.is_none());
    }

    #[test]
    fn test_extract_token_none_with_wrong_auth_scheme() {
        let req = Request::builder()
            .header(AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert!(token.is_none());
    }

    #[test]
    fn test_extract_token_none_with_wrong_cookie_name() {
        let req = Request::builder()
            .header(COOKIE, "wrong_cookie=token-value")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert!(token.is_none());
    }

    #[test]
    fn test_auth_user_clone() {
        let user = AuthUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::User,
            issued_at: 0,
        };

        let cloned = user.clone();
        assert_eq!(cloned.user_id, user.user_id);
        assert_eq!(cloned.email_verified, user.email_verified);
        assert_eq!(cloned.role, user.role);
    }

    #[test]
    fn test_auth_user_debug() {
        let user = AuthUser {
            user_id: UserId::new(),
            email_verified: false,
            role: UserRole::Admin,
            issued_at: 0,
        };

        let debug_str = format!("{user:?}");
        assert!(debug_str.contains("AuthUser"));
        assert!(debug_str.contains("email_verified"));
        assert!(debug_str.contains("role"));
    }

    #[test]
    fn test_optional_auth_user_clone() {
        let optional = OptionalAuthUser(Some(AuthUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::User,
            issued_at: 0,
        }));

        let cloned = optional;
        assert!(cloned.0.is_some());
    }

    #[test]
    fn test_optional_auth_user_none() {
        let optional = OptionalAuthUser(None);
        assert!(optional.0.is_none());
    }
}
