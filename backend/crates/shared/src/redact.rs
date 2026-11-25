//! Redaction rules for secrets in field names and URL queries (OT-001).
//!
//! Applied at every telemetry exit by `platform::telemetry::redact`, and
//! to browser reports by the application layer (OT-006). Dependency-free.

/// Replacement for every redacted value.
pub const REDACTED: &str = "[REDACTED]";

/// A field whose (lowercased) name contains any of these is redacted.
const SENSITIVE_FIELD_PARTS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "code",
    "otp",
    "session",
    "cookie",
    "authorization",
    "csrf",
];

/// Exact field names that match [`SENSITIVE_FIELD_PARTS`] but are known safe.
const ALLOWED_FIELDS: &[&str] = &[
    "status_code",
    "http.response.status_code",
    "otel.status_code",
];

/// Field-name prefixes that are known safe (OpenTelemetry source-location
/// attributes such as `code.file.path`, `code.line.number`).
const ALLOWED_PREFIXES: &[&str] = &["code."];

/// Query parameters whose values are always redacted, in addition to any name
/// that [`is_sensitive_field`] matches.
const SENSITIVE_QUERY_PARAMS: &[&str] = &["token", "code", "password", "secret", "key", "state"];

/// Whether a span attribute or log field with this name must be redacted.
#[must_use]
pub fn is_sensitive_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if ALLOWED_FIELDS.contains(&lower.as_str())
        || ALLOWED_PREFIXES.iter().any(|p| lower.starts_with(p))
    {
        return false;
    }
    SENSITIVE_FIELD_PARTS
        .iter()
        .any(|part| lower.contains(part))
}

fn is_sensitive_query_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_QUERY_PARAMS.contains(&lower.as_str()) || is_sensitive_field(&lower)
}

/// Redact the values of sensitive parameters in a URL query string (without the
/// leading `?`). Keys and order are kept: `a=1&token=x` → `a=1&token=[REDACTED]`.
#[must_use]
pub fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_sensitive_query_param(key) => format!("{key}={REDACTED}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Redact a full URL or path-with-query: only the query part is touched.
#[must_use]
pub fn redact_url(url: &str) -> String {
    match url.split_once('?') {
        Some((base, query)) => format!("{base}?{}", redact_query(query)),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_name_is_sensitive() {
        for name in [
            "password",
            "new_password",
            "password_hash",
            "passwd",
            "client_secret",
            "token",
            "refresh_token",
            "access_token",
            "magic_token_hash",
            "code",
            "reset_code",
            "otp",
            "session",
            "session_id",
            "cookie",
            "set_cookie",
            "authorization",
            "http.request.header.authorization",
            "csrf",
            "x_csrf_token",
            "PASSWORD",
            "Refresh-Token",
        ] {
            assert!(is_sensitive_field(name), "{name} must be redacted");
        }
    }

    #[test]
    fn known_safe_names_are_kept() {
        for name in [
            "status_code",
            "http.response.status_code",
            "otel.status_code",
            "code.file.path",
            "code.line.number",
            "user_id",
            "email_domain",
            "http.route",
            "message",
            "latency_ms",
        ] {
            assert!(!is_sensitive_field(name), "{name} must not be redacted");
        }
    }

    #[test]
    fn query_values_are_redacted_by_param_name() {
        assert_eq!(
            redact_query("token=abc&page=2&code=123456&state=xyz&key=k&password=p&secret=s"),
            "token=[REDACTED]&page=2&code=[REDACTED]&state=[REDACTED]&key=[REDACTED]&password=[REDACTED]&secret=[REDACTED]"
        );
        assert_eq!(redact_query("magic_token=abc"), "magic_token=[REDACTED]");
        assert_eq!(redact_query("page=2&sort=asc"), "page=2&sort=asc");
        assert_eq!(redact_query("flag"), "flag");
    }

    #[test]
    fn url_redaction_only_touches_the_query() {
        assert_eq!(
            redact_url("/api/v1/auth/verify-magic?token=abc"),
            "/api/v1/auth/verify-magic?token=[REDACTED]"
        );
        assert_eq!(redact_url("/api/v1/auth/me"), "/api/v1/auth/me");
    }
}
