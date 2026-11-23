//! Shared auth cookie construction.
//!
//! Every auth cookie is built here so their names and attributes stay
//! consistent across handlers. Each name carries the `__Host-` prefix, so a
//! browser accepts the cookie only from this exact host, over HTTPS, with
//! `Secure`, `Path=/` and no `Domain`. An attacker who controls a sibling
//! subdomain, or a plain-HTTP response before HSTS applies, therefore can't
//! plant one, which closes login CSRF by cookie tossing (OT-010 slice 5,
//! threat model P-22). Browsers treat `http://localhost` as secure, so dev
//! works over plain HTTP in Chromium and Firefox.

use axum::http::HeaderMap;
use axum::http::header::{COOKIE, HeaderValue};

/// Session JWT (`HttpOnly`).
pub const AUTH_COOKIE: &str = "__Host-auth_token";
/// Refresh token (`HttpOnly`).
pub const REFRESH_COOKIE: &str = "__Host-refresh_token";
/// CSRF token, readable by the app's script for the double-submit header.
pub const CSRF_COOKIE: &str = "__Host-csrf_token";
/// Nonce binding a Google sign-in to the browser that started it (`HttpOnly`).
pub const OAUTH_STATE_COOKIE: &str = "__Host-oauth_state";
/// Device fingerprint carried across the Google redirect (`HttpOnly`).
pub const OAUTH_DEVICE_FINGERPRINT_COOKIE: &str = "__Host-oauth_device_fingerprint";

/// Scan the request's `Cookie` header for the first cookie named `name` and
/// return its value. Every extractor shares this one implementation. `name` is
/// the full cookie name without the `=` (one of the constants above).
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

/// OAuth fingerprint handoff cookie lifetime in seconds (5 minutes, matches OAuth state).
pub const OAUTH_DEVICE_FINGERPRINT_MAX_AGE: i64 = 5 * 60;

/// OAuth state-binding cookie lifetime in seconds (5 minutes, matches the state JWT).
pub const OAUTH_STATE_MAX_AGE: i64 = 5 * 60;

/// The attributes `__Host-` requires (`Secure`, `Path=/`, no `Domain`), plus
/// `SameSite=Lax`, and `HttpOnly` unless page scripts must read the cookie.
fn cookie(name: &str, value: &str, max_age_seconds: i64, http_only: bool) -> Option<HeaderValue> {
    let http_only = if http_only { "; HttpOnly" } else { "" };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; Secure; SameSite=Lax{http_only}; Max-Age={max_age_seconds}"
    ))
    .ok()
}

/// A cookie that clears `name`. Same attributes as when it was set, or the
/// browser wouldn't match it.
fn cleared(name: &str, http_only: bool) -> HeaderValue {
    cookie(name, "", 0, http_only).expect("a cookie name constant with an empty value is valid")
}

/// Build an `HttpOnly` cookie for the JWT access token.
///
/// Returns `None` only if the token contains invalid header characters (extremely rare).
#[must_use]
pub fn create_jwt_cookie(token: &str, max_age_seconds: i64) -> Option<HeaderValue> {
    cookie(AUTH_COOKIE, token, max_age_seconds, true)
}

/// Build an `HttpOnly` cookie for the refresh token.
///
/// Returns `None` only if the token contains invalid header characters (extremely rare).
#[must_use]
pub fn create_refresh_cookie(token: &str, max_age_seconds: i64) -> Option<HeaderValue> {
    cookie(REFRESH_COOKIE, token, max_age_seconds, true)
}

/// Build a cookie for the CSRF token. `max_age_seconds` is the session's
/// access-token lifetime, so the cookie and the token's validity match.
///
/// This cookie is intentionally NOT httpOnly so JavaScript can read it and echo
/// it back in the `X-CSRF-Token` header (double-submit pattern).
/// Returns `None` only if the token contains invalid header characters (extremely rare).
#[must_use]
pub fn create_csrf_cookie(token: &str, max_age_seconds: i64) -> Option<HeaderValue> {
    cookie(CSRF_COOKIE, token, max_age_seconds, false)
}

/// Build an `HttpOnly` cookie used to carry the dashboard device fingerprint
/// across an OAuth provider redirect.
#[must_use]
pub fn create_oauth_device_fingerprint_cookie(fingerprint: &str) -> Option<HeaderValue> {
    cookie(
        OAUTH_DEVICE_FINGERPRINT_COOKIE,
        fingerprint,
        OAUTH_DEVICE_FINGERPRINT_MAX_AGE,
        true,
    )
}

/// Build an `HttpOnly` cookie binding the OAuth `state` nonce to this browser.
///
/// INVARIANT: the nonce embedded in the state JWT is also stored here at
/// `oauth_start` and compared at the callback, so a valid `code`+`state` captured
/// from an attacker's own flow cannot be replayed against a victim's browser
/// (login CSRF / session fixation). The `__Host-` prefix stops the attacker
/// planting their own nonce here.
#[must_use]
pub fn create_oauth_state_cookie(nonce: &str) -> Option<HeaderValue> {
    cookie(OAUTH_STATE_COOKIE, nonce, OAUTH_STATE_MAX_AGE, true)
}

/// Build a cookie that clears the OAuth state-binding cookie.
#[must_use]
pub fn clear_oauth_state_cookie() -> HeaderValue {
    cleared(OAUTH_STATE_COOKIE, true)
}

/// Build a cookie that clears the JWT access token.
#[must_use]
pub fn clear_jwt_cookie() -> HeaderValue {
    cleared(AUTH_COOKIE, true)
}

/// Build a cookie that clears the refresh token.
#[must_use]
pub fn clear_refresh_cookie() -> HeaderValue {
    cleared(REFRESH_COOKIE, true)
}

/// Build a cookie that clears the CSRF token.
#[must_use]
pub fn clear_csrf_cookie() -> HeaderValue {
    cleared(CSRF_COOKIE, false)
}

/// Build a cookie that clears the OAuth fingerprint handoff cookie.
#[must_use]
pub fn clear_oauth_device_fingerprint_cookie() -> HeaderValue {
    cleared(OAUTH_DEVICE_FINGERPRINT_COOKIE, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cookie this module sets or clears.
    fn every_cookie() -> Vec<HeaderValue> {
        vec![
            create_jwt_cookie("t", 60).unwrap(),
            create_refresh_cookie("t", 60).unwrap(),
            create_csrf_cookie("t", 60).unwrap(),
            create_oauth_state_cookie("n").unwrap(),
            create_oauth_device_fingerprint_cookie("f").unwrap(),
            clear_jwt_cookie(),
            clear_refresh_cookie(),
            clear_csrf_cookie(),
            clear_oauth_state_cookie(),
            clear_oauth_device_fingerprint_cookie(),
        ]
    }

    #[test]
    fn every_cookie_meets_the_host_prefix_rules() {
        for c in every_cookie() {
            let s = c.to_str().unwrap();
            assert!(s.starts_with("__Host-"), "{s}");
            assert!(s.contains("; Path=/;"), "{s}");
            assert!(s.contains("; Secure"), "{s}");
            assert!(s.contains("; SameSite=Lax"), "{s}");
            assert!(!s.to_ascii_lowercase().contains("domain="), "{s}");
        }
    }

    #[test]
    fn only_the_csrf_cookie_is_readable_by_scripts() {
        for c in every_cookie() {
            let s = c.to_str().unwrap();
            let readable = s.starts_with(&format!("{CSRF_COOKIE}="));
            assert_eq!(!s.contains("HttpOnly"), readable, "{s}");
        }
    }

    #[test]
    fn cookies_carry_their_value_and_lifetime() {
        let s = create_jwt_cookie("tok123", 3600).unwrap();
        let s = s.to_str().unwrap();
        assert!(s.starts_with("__Host-auth_token=tok123;"), "{s}");
        assert!(s.ends_with("Max-Age=3600"), "{s}");

        let s = create_refresh_cookie("rtok", 86400).unwrap();
        assert!(
            s.to_str()
                .unwrap()
                .starts_with("__Host-refresh_token=rtok;")
        );

        let s = create_csrf_cookie("csrf456", 900).unwrap();
        let s = s.to_str().unwrap();
        assert!(s.starts_with("__Host-csrf_token=csrf456;") && s.ends_with("Max-Age=900"));

        let s = create_oauth_state_cookie("nonce-abc").unwrap();
        let s = s.to_str().unwrap();
        assert!(s.starts_with("__Host-oauth_state=nonce-abc;"), "{s}");
        assert!(
            s.ends_with(&format!("Max-Age={OAUTH_STATE_MAX_AGE}")),
            "{s}"
        );

        let s = create_oauth_device_fingerprint_cookie("fingerprint123").unwrap();
        let s = s.to_str().unwrap();
        assert!(
            s.starts_with("__Host-oauth_device_fingerprint=fingerprint123;"),
            "{s}"
        );
        assert!(
            s.ends_with(&format!("Max-Age={OAUTH_DEVICE_FINGERPRINT_MAX_AGE}")),
            "{s}"
        );
    }

    #[test]
    fn clear_cookies_expire_immediately() {
        for c in every_cookie().into_iter().skip(5) {
            let s = c.to_str().unwrap();
            assert!(s.contains("=; ") && s.ends_with("Max-Age=0"), "{s}");
        }
    }

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn extract_cookie_reads_named_value() {
        let h = headers_with_cookie(
            "__Host-auth_token=xyz; __Host-refresh_token=abc123; session_id=999",
        );
        assert_eq!(
            extract_cookie(&h, REFRESH_COOKIE),
            Some("abc123".to_string())
        );
        assert_eq!(extract_cookie(&h, AUTH_COOKIE), Some("xyz".to_string()));
    }

    #[test]
    fn extract_cookie_ignores_the_unprefixed_name() {
        // A cookie without the prefix (an old one, or one planted by a sibling
        // host) is a different cookie and must not be read.
        let h = headers_with_cookie("refresh_token=planted; auth_token=planted");
        assert_eq!(extract_cookie(&h, REFRESH_COOKIE), None);
        assert_eq!(extract_cookie(&h, AUTH_COOKIE), None);
    }

    #[test]
    fn extract_cookie_trims_surrounding_whitespace() {
        let h = headers_with_cookie("  __Host-refresh_token=abc123  ");
        assert_eq!(
            extract_cookie(&h, REFRESH_COOKIE),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extract_cookie_does_not_match_name_as_substring() {
        // A cookie whose name merely contains the target as a prefix must not
        // match (the `=` boundary after the full name is required).
        let h = headers_with_cookie("__Host-refresh_tokenx=nope; __Host-auth_token2=nope");
        assert_eq!(extract_cookie(&h, REFRESH_COOKIE), None);
        assert_eq!(extract_cookie(&h, AUTH_COOKIE), None);
    }

    #[test]
    fn extract_cookie_returns_none_when_absent_or_no_header() {
        assert_eq!(extract_cookie(&HeaderMap::new(), REFRESH_COOKIE), None);
        let h = headers_with_cookie("__Host-auth_token=xyz; session_id=999");
        assert_eq!(extract_cookie(&h, REFRESH_COOKIE), None);
    }

    // A token containing a control character can't form a valid HeaderValue, so
    // the builders return `None` rather than panicking.
    #[test]
    fn builders_return_none_for_invalid_header_characters() {
        let bad = "abc\ndef";
        assert!(create_jwt_cookie(bad, 3600).is_none());
        assert!(create_refresh_cookie(bad, 3600).is_none());
        assert!(create_csrf_cookie(bad, 900).is_none());
        assert!(create_oauth_state_cookie(bad).is_none());
        assert!(create_oauth_device_fingerprint_cookie(bad).is_none());
    }

    #[test]
    fn extract_cookie_returns_none_for_non_utf8_header() {
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            HeaderValue::from_bytes(b"__Host-refresh_token=\xff").unwrap(),
        );
        assert_eq!(extract_cookie(&h, REFRESH_COOKIE), None);
    }

    #[test]
    fn extract_cookie_returns_empty_string_for_valueless_cookie() {
        let h = headers_with_cookie("__Host-oauth_state=; other=1");
        assert_eq!(extract_cookie(&h, OAUTH_STATE_COOKIE), Some(String::new()));
    }
}
