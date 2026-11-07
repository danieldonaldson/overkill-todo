//! Shared auth cookie construction.
//!
//! All auth-related cookies (`auth_token`, `refresh_token`, `csrf_token`) are
//! built here so their attributes — in particular the `Secure` flag — stay
//! consistent across every handler. Cookies are marked `Secure` outside of dev
//! mode so auth tokens are never transmitted over plaintext HTTP in production.

use axum::http::HeaderMap;
use axum::http::header::{COOKIE, HeaderValue};

use crate::infrastructure::config::is_dev_mode;

/// Scan the request's `Cookie` header for the first cookie named `name` and
/// return its value. Centralizes cookie parsing so every extractor
/// (`auth_token`, `refresh_token`, `csrf_token`, `oauth_state`,
/// `oauth_device_fingerprint`) shares one implementation. `name` is the bare
/// cookie name without the `=` (e.g. `"refresh_token"`).
#[must_use]
pub(crate) fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                cookie
                    .trim()
                    .strip_prefix(name)
                    .and_then(|rest| rest.strip_prefix('='))
                    .map(str::to_string)
            })
        })
}

/// CSRF token cookie lifetime in seconds (15 minutes, matches the JWT).
pub const CSRF_TOKEN_MAX_AGE: i64 = 15 * 60;
/// OAuth fingerprint handoff cookie lifetime in seconds (5 minutes, matches OAuth state).
pub const OAUTH_DEVICE_FINGERPRINT_MAX_AGE: i64 = 5 * 60;
/// OAuth state-binding cookie lifetime in seconds (5 minutes, matches the state JWT).
pub const OAUTH_STATE_MAX_AGE: i64 = 5 * 60;

/// `; Secure` outside dev mode, empty in dev (where TLS is typically absent).
fn secure() -> &'static str {
    if is_dev_mode() { "" } else { "; Secure" }
}

/// Build an httpOnly cookie for the JWT access token.
///
/// Returns `None` only if the token contains invalid header characters (extremely rare).
#[must_use]
pub fn create_jwt_cookie(token: &str, max_age_seconds: i64) -> Option<HeaderValue> {
    let s = secure();
    HeaderValue::from_str(&format!(
        "auth_token={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_seconds}{s}"
    ))
    .ok()
}

/// Build an httpOnly cookie for the refresh token.
///
/// Path is restricted to `/api/v1/auth` so it isn't sent on every request.
/// Returns `None` only if the token contains invalid header characters (extremely rare).
#[must_use]
pub fn create_refresh_cookie(token: &str, max_age_seconds: i64) -> Option<HeaderValue> {
    let s = secure();
    HeaderValue::from_str(&format!(
        "refresh_token={token}; Path=/api/v1/auth; HttpOnly; SameSite=Lax; Max-Age={max_age_seconds}{s}"
    ))
    .ok()
}

/// Build a cookie for the CSRF token.
///
/// This cookie is intentionally NOT httpOnly so JavaScript can read it and echo
/// it back in the `X-CSRF-Token` header (double-submit pattern).
/// Returns `None` only if the token contains invalid header characters (extremely rare).
#[must_use]
pub fn create_csrf_cookie(token: &str) -> Option<HeaderValue> {
    let s = secure();
    HeaderValue::from_str(&format!(
        "csrf_token={token}; Path=/; SameSite=Lax; Max-Age={CSRF_TOKEN_MAX_AGE}{s}"
    ))
    .ok()
}

/// Build an httpOnly cookie used to carry the dashboard device fingerprint
/// across an OAuth provider redirect.
#[must_use]
pub fn create_oauth_device_fingerprint_cookie(fingerprint: &str) -> Option<HeaderValue> {
    let s = secure();
    HeaderValue::from_str(&format!(
        "oauth_device_fingerprint={fingerprint}; Path=/api/v1/auth/oauth; HttpOnly; SameSite=Lax; Max-Age={OAUTH_DEVICE_FINGERPRINT_MAX_AGE}{s}"
    ))
    .ok()
}

/// Build an httpOnly cookie binding the OAuth `state` nonce to this browser.
///
/// INVARIANT: the nonce embedded in the state JWT is also stored here at
/// `oauth_start` and compared at the callback, so a valid `code`+`state` captured
/// from an attacker's own flow cannot be replayed against a victim's browser
/// (login CSRF / session fixation).
#[must_use]
pub fn create_oauth_state_cookie(nonce: &str) -> Option<HeaderValue> {
    let s = secure();
    HeaderValue::from_str(&format!(
        "oauth_state={nonce}; Path=/api/v1/auth/oauth; HttpOnly; SameSite=Lax; Max-Age={OAUTH_STATE_MAX_AGE}{s}"
    ))
    .ok()
}

/// Build a cookie that clears the OAuth state-binding cookie.
///
/// # Panics
///
/// Never in practice — the cookie string is a fixed template with no caller input.
#[must_use]
pub fn clear_oauth_state_cookie() -> HeaderValue {
    let s = secure();
    HeaderValue::from_str(&format!(
        "oauth_state=; Path=/api/v1/auth/oauth; HttpOnly; SameSite=Lax; Max-Age=0{s}"
    ))
    .expect("static cookie string is always valid")
}

/// Build a cookie that clears the JWT access token.
///
/// # Panics
///
/// Never in practice — the cookie string is a fixed template with no caller input.
#[must_use]
pub fn clear_jwt_cookie() -> HeaderValue {
    let s = secure();
    HeaderValue::from_str(&format!(
        "auth_token=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{s}"
    ))
    .expect("static cookie string is always valid")
}

/// Build a cookie that clears the refresh token.
///
/// # Panics
///
/// Never in practice — the cookie string is a fixed template with no caller input.
#[must_use]
pub fn clear_refresh_cookie() -> HeaderValue {
    let s = secure();
    HeaderValue::from_str(&format!(
        "refresh_token=; Path=/api/v1/auth; HttpOnly; SameSite=Lax; Max-Age=0{s}"
    ))
    .expect("static cookie string is always valid")
}

/// Build a cookie that clears the CSRF token.
///
/// # Panics
///
/// Never in practice — the cookie string is a fixed template with no caller input.
#[must_use]
pub fn clear_csrf_cookie() -> HeaderValue {
    let s = secure();
    HeaderValue::from_str(&format!("csrf_token=; Path=/; SameSite=Lax; Max-Age=0{s}"))
        .expect("static cookie string is always valid")
}

/// Build a cookie that clears the OAuth fingerprint handoff cookie.
///
/// # Panics
///
/// Never in practice — the cookie string is a fixed template with no caller input.
#[must_use]
pub fn clear_oauth_device_fingerprint_cookie() -> HeaderValue {
    let s = secure();
    HeaderValue::from_str(&format!(
        "oauth_device_fingerprint=; Path=/api/v1/auth/oauth; HttpOnly; SameSite=Lax; Max-Age=0{s}"
    ))
    .expect("static cookie string is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    // In unit tests DEV_MODE is unset, so `is_dev_mode()` is false and cookies
    // are marked `Secure`.

    #[test]
    fn jwt_cookie_has_expected_attributes() {
        let c = create_jwt_cookie("tok123", 3600).unwrap();
        let s = c.to_str().unwrap();
        assert!(s.contains("auth_token=tok123"));
        assert!(s.contains("Path=/"));
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("SameSite=Lax"));
        assert!(s.contains("Max-Age=3600"));
        assert!(s.contains("Secure"));
    }

    #[test]
    fn refresh_cookie_is_scoped_and_secure() {
        let c = create_refresh_cookie("rtok", 86400).unwrap();
        let s = c.to_str().unwrap();
        assert!(s.contains("refresh_token=rtok"));
        assert!(s.contains("Path=/api/v1/auth"));
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("Secure"));
    }

    #[test]
    fn csrf_cookie_is_readable_by_js_but_secure() {
        let c = create_csrf_cookie("csrf456").unwrap();
        let s = c.to_str().unwrap();
        assert!(s.contains("csrf_token=csrf456"));
        assert!(s.contains(&format!("Max-Age={CSRF_TOKEN_MAX_AGE}")));
        // Not HttpOnly: the SPA reads it to echo back in the X-CSRF-Token header.
        assert!(!s.contains("HttpOnly"));
        assert!(s.contains("Secure"));
    }

    #[test]
    fn clear_cookies_expire_immediately_and_are_secure() {
        for c in [
            clear_jwt_cookie(),
            clear_refresh_cookie(),
            clear_csrf_cookie(),
            clear_oauth_device_fingerprint_cookie(),
        ] {
            let s = c.to_str().unwrap();
            assert!(s.contains("Max-Age=0"));
            assert!(s.contains("Secure"));
        }
    }

    #[test]
    fn oauth_device_fingerprint_cookie_is_scoped_http_only_and_short_lived() {
        let c = create_oauth_device_fingerprint_cookie("fingerprint123").unwrap();
        let s = c.to_str().unwrap();
        assert!(s.contains("oauth_device_fingerprint=fingerprint123"));
        assert!(s.contains("Path=/api/v1/auth/oauth"));
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("SameSite=Lax"));
        assert!(s.contains(&format!("Max-Age={OAUTH_DEVICE_FINGERPRINT_MAX_AGE}")));
        assert!(s.contains("Secure"));
    }

    #[test]
    fn csrf_max_age_matches_jwt_lifetime() {
        assert_eq!(CSRF_TOKEN_MAX_AGE, 15 * 60);
    }

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn extract_cookie_reads_named_value() {
        let h = headers_with_cookie("auth_token=xyz; refresh_token=abc123; session_id=999");
        assert_eq!(
            extract_cookie(&h, "refresh_token"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_cookie(&h, "auth_token"), Some("xyz".to_string()));
    }

    #[test]
    fn extract_cookie_trims_surrounding_whitespace() {
        let h = headers_with_cookie("  refresh_token=abc123  ");
        assert_eq!(
            extract_cookie(&h, "refresh_token"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extract_cookie_does_not_match_name_as_substring() {
        // A cookie whose name merely contains the target as a substring/suffix
        // must not match (the `=` boundary after the full name is required).
        let h = headers_with_cookie("xrefresh_token=nope; my_auth_token=nope");
        assert_eq!(extract_cookie(&h, "refresh_token"), None);
        assert_eq!(extract_cookie(&h, "auth_token"), None);
    }

    #[test]
    fn extract_cookie_returns_none_when_absent_or_no_header() {
        assert_eq!(extract_cookie(&HeaderMap::new(), "refresh_token"), None);
        let h = headers_with_cookie("auth_token=xyz; session_id=999");
        assert_eq!(extract_cookie(&h, "refresh_token"), None);
    }

    #[test]
    fn oauth_state_cookie_is_scoped_http_only_and_short_lived() {
        let c = create_oauth_state_cookie("nonce-abc").unwrap();
        let s = c.to_str().unwrap();
        assert!(s.contains("oauth_state=nonce-abc"));
        assert!(s.contains("Path=/api/v1/auth/oauth"));
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("SameSite=Lax"));
        assert!(s.contains(&format!("Max-Age={OAUTH_STATE_MAX_AGE}")));
        assert!(s.contains("Secure"));
    }

    #[test]
    fn clear_oauth_state_cookie_expires_immediately_and_is_secure() {
        let s = clear_oauth_state_cookie();
        let v = s.to_str().unwrap();
        assert!(v.contains("oauth_state="));
        assert!(v.contains("Path=/api/v1/auth/oauth"));
        assert!(v.contains("HttpOnly"));
        assert!(v.contains("Max-Age=0"));
        assert!(v.contains("Secure"));
    }

    // A token containing a control character can't form a valid HeaderValue, so
    // the builders return `None` rather than panicking. This exercises the `.ok()`
    // error branch shared by every `create_*` builder.
    #[test]
    fn builders_return_none_for_invalid_header_characters() {
        let bad = "abc\ndef";
        assert!(create_jwt_cookie(bad, 3600).is_none());
        assert!(create_refresh_cookie(bad, 3600).is_none());
        assert!(create_csrf_cookie(bad).is_none());
        assert!(create_oauth_state_cookie(bad).is_none());
        assert!(create_oauth_device_fingerprint_cookie(bad).is_none());
    }

    #[test]
    fn extract_cookie_returns_none_for_non_utf8_header() {
        // A Cookie header with non-ASCII bytes fails `to_str()` and yields None.
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            HeaderValue::from_bytes(b"refresh_token=\xff").unwrap(),
        );
        assert_eq!(extract_cookie(&h, "refresh_token"), None);
    }

    #[test]
    fn extract_cookie_returns_empty_string_for_valueless_cookie() {
        // `name=` with no value is a present cookie whose value is empty.
        let h = headers_with_cookie("oauth_state=; other=1");
        assert_eq!(extract_cookie(&h, "oauth_state"), Some(String::new()));
    }
}
