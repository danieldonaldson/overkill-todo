use std::sync::Arc;

use axum::{
    Extension,
    extract::{FromRequestParts, Request},
    http::{HeaderMap, Method, request::Parts},
    middleware::Next,
    response::Response,
};

use application::ports::outgoing::{SessionAuthorizationState, UserRepository};
use domain::value_objects::{UserId, UserRole};

use platform::crypto::{Crypto, TokenKeyring};

use crate::{
    cookies::{AUTH_COOKIE, extract_cookie},
    error::AppError,
    verify_jwt,
};

/// Header naming the account a change was made for (OT-015 design D1).
pub(crate) const ACCOUNT_ID_HEADER: &str = "x-account-id";

/// Methods the account check applies to: reads are answered for the session.
pub(crate) const fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// Whether the account named in the request, if any, is the session's. Only a
/// missing header passes unchecked: an empty, repeated or malformed one must
/// not count as "not given".
fn account_matches(headers: &HeaderMap, user_id: &UserId) -> bool {
    let mut named = headers.get_all(ACCOUNT_ID_HEADER).iter();
    match (named.next(), named.next()) {
        (None, _) => true,
        (Some(id), None) => id
            .to_str()
            .ok()
            .and_then(|id| id.parse::<uuid::Uuid>().ok())
            .is_some_and(|id| id == *user_id.as_uuid()),
        (Some(_), Some(_)) => false,
    }
}

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
                resolve_principal(crypto.tokens(), &parts.headers).ok(),
            ))
        })();
        std::future::ready(result)
    }
}

/// Extract the session token from the session cookie. An `Authorization`
/// header is ignored: the cookie is the only credential the API accepts and
/// describes (`openapi::SESSION_COOKIE_SCHEME`).
fn extract_token(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, AUTH_COOKIE)
}

/// Verify a request's session token (the `auth_token` cookie) and build
/// the [`AuthUser`], preserving the specific failure: `token_missing` when no
/// token is present, or the `verify_jwt` error (invalid/expired) otherwise.
///
/// The single place that turns a token into a principal — used by the
/// middleware, the `Option`-returning [`authenticate`], and the
/// [`OptionalAuthUser`] extractor.
fn resolve_principal(keys: &TokenKeyring, headers: &HeaderMap) -> Result<AuthUser, AppError> {
    let token = extract_token(headers).ok_or_else(AppError::token_missing)?;
    let claims = verify_jwt(&token, keys)?;
    let user_id = claims.user_id()?;
    let role = UserRole::from_db_str(&claims.role).unwrap_or_else(|| {
        tracing::warn!(
            user_id = %user_id,
            role = %claims.role,
            "unknown role claim in session token; defaulting to user"
        );
        UserRole::default()
    });
    Ok(AuthUser {
        user_id: UserId::from_uuid(user_id),
        email_verified: claims.verified,
        role,
        issued_at: claims.iat,
    })
}

/// Verify the request's session token and build the [`AuthUser`]. Returns `None`
/// when no valid session token is present.
fn authenticate(keys: &TokenKeyring, req: &Request) -> Option<AuthUser> {
    resolve_principal(keys, req.headers()).ok()
}

/// Ensure the request's [`AuthUser`] extension is populated, verifying the
/// session token if it isn't already, and return the resolved principal.
/// Returns `None` when no valid session token is present.
///
/// Used by the CSRF middleware, which runs *before* `auth_middleware`.
/// Stashing the verified principal here lets `auth_middleware` reuse it instead
/// of verifying the JWT a second time on the same request.
pub(crate) fn ensure_authenticated(keys: &TokenKeyring, req: &mut Request) -> Option<AuthUser> {
    if let Some(user) = req.extensions().get::<AuthUser>().cloned() {
        return Some(user);
    }
    let user = authenticate(keys, req)?;
    req.extensions_mut().insert(user.clone());
    Some(user)
}

/// # Errors
///
/// Returns an error if the authentication token is missing, invalid, or expired,
/// or if a state-changing request names another account ([`ACCOUNT_ID_HEADER`]).
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
        let user = resolve_principal(crypto.tokens(), req.headers())?;
        // Record user_id on the current span for trace correlation.
        tracing::Span::current().record("user_id", user.user_id.to_string());
        req.extensions_mut().insert(user);
    }

    if is_state_changing(req.method()) {
        let user = req
            .extensions()
            .get::<AuthUser>()
            .ok_or_else(AppError::token_missing)?;
        if !account_matches(req.headers(), &user.user_id) {
            return Err(AppError::AccountChanged);
        }
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
/// The user repository comes from a request extension (installed by
/// `create_router`), not router state, so the route tree can be declared without
/// any dependency values and its `OpenAPI` description built from it alone.
///
/// # Errors
/// Returns `token_missing` if no principal was resolved upstream, `token_invalid`
/// when the token's role is stale, or a persistence error from the repository.
pub async fn require_fresh_session(
    Extension(users): Extension<Arc<dyn UserRepository>>,
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
    matches!(current, Some(state) if state.role == token_role && state.tokens_valid_after.is_none_or(|cutoff| issued_at > cutoff.as_second()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::{AUTHORIZATION, COOKIE};
    use jiff::Timestamp;

    #[test]
    fn only_a_missing_header_or_the_own_id_matches() {
        let user = UserId::new();
        let own = user.as_uuid().to_string();
        let headers = |values: &[&str]| {
            let mut map = HeaderMap::new();
            for value in values {
                map.append(ACCOUNT_ID_HEADER, value.parse().unwrap());
            }
            map
        };
        assert!(account_matches(&headers(&[]), &user));
        assert!(account_matches(&headers(&[&own]), &user));
        let other = UserId::new().as_uuid().to_string();
        for refused in [
            vec![other.as_str()],
            vec![""],
            vec!["not-a-uuid"],
            vec![own.as_str(), own.as_str()],
            vec![own.as_str(), other.as_str()],
        ] {
            assert!(!account_matches(&headers(&refused), &user), "{refused:?}");
        }
    }

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
        let cutoff = Timestamp::new(100, 500_000_000).unwrap();
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
    fn test_extract_token_ignores_bearer_header() {
        let req = Request::builder()
            .header(AUTHORIZATION, "Bearer test-jwt-token-123")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert!(token.is_none());
    }

    #[test]
    fn test_extract_token_from_cookie() {
        let req = Request::builder()
            .header(COOKIE, "__Host-auth_token=cookie-jwt-token-456")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert_eq!(token, Some("cookie-jwt-token-456".to_string()));
    }

    #[test]
    fn test_extract_token_uses_cookie_even_with_bearer_header() {
        let req = Request::builder()
            .header(AUTHORIZATION, "Bearer bearer-token")
            .header(COOKIE, "__Host-auth_token=cookie-token")
            .body(())
            .unwrap();

        let token = extract_token(req.map(|()| axum::body::Body::empty()).headers());
        assert_eq!(token, Some("cookie-token".to_string()));
    }

    #[test]
    fn test_extract_token_from_cookie_with_multiple_cookies() {
        let req = Request::builder()
            .header(
                COOKIE,
                "other=value; __Host-auth_token=my-token; another=foo",
            )
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
