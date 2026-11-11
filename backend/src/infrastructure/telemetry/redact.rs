//! Redaction of secrets and personal data from telemetry.
//!
//! A `tracing` layer cannot rewrite the values other layers see, so redaction
//! is applied at each exit instead, sharing the rules below:
//!
//! - [`RedactingSpanProcessor`] scrubs span and span-event attributes before
//!   any exporter sees them (OpenTelemetry path).
//! - The log formatters in [`super::log_format`] scrub fields as they write them
//!   (stdout path).
//!
//! The collector applies the same field-name rule again as a second line of
//! defence (`deploy/observability/collector/config.yaml`).

use opentelemetry::{Context, KeyValue, Value};
use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    trace::{Span, SpanData, SpanProcessor},
};
use std::time::Duration;

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

fn redact_attributes(attributes: &mut [KeyValue]) {
    for kv in attributes {
        let key = kv.key.as_str();
        if is_sensitive_field(key) {
            kv.value = Value::from(REDACTED);
        } else if matches!(key, "url.query" | "url.full" | "http.target" | "uri") {
            let redacted = match &kv.value {
                Value::String(s) if key == "url.query" => redact_query(s.as_str()),
                Value::String(s) => redact_url(s.as_str()),
                _ => continue,
            };
            kv.value = Value::from(redacted);
        }
    }
}

/// Wraps another [`SpanProcessor`] and redacts every finished span (attributes
/// and event attributes) before handing it on, so no exporter ever sees a
/// sensitive value.
#[derive(Debug)]
pub struct RedactingSpanProcessor<P> {
    inner: P,
}

impl<P: SpanProcessor> RedactingSpanProcessor<P> {
    pub const fn new(inner: P) -> Self {
        Self { inner }
    }
}

impl<P: SpanProcessor> SpanProcessor for RedactingSpanProcessor<P> {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        self.inner.on_start(span, cx);
    }

    fn on_end(&self, mut span: SpanData) {
        redact_attributes(&mut span.attributes);
        for event in &mut span.events.events {
            redact_attributes(&mut event.attributes);
        }
        self.inner.on_end(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
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

    #[test]
    fn attributes_are_redacted_by_name_and_url_query() {
        let mut attrs = vec![
            KeyValue::new("password", "hunter2"),
            KeyValue::new("user_id", "u-1"),
            KeyValue::new("url.query", "code=123&x=1"),
            KeyValue::new("url.full", "https://h/p?token=t"),
            KeyValue::new("http.response.status_code", 500_i64),
        ];
        redact_attributes(&mut attrs);
        let get = |k: &str| {
            attrs
                .iter()
                .find(|kv| kv.key.as_str() == k)
                .map(|kv| kv.value.to_string())
                .unwrap()
        };
        assert_eq!(get("password"), REDACTED);
        assert_eq!(get("user_id"), "u-1");
        assert_eq!(get("url.query"), "code=[REDACTED]&x=1");
        assert_eq!(get("url.full"), "https://h/p?token=[REDACTED]");
        assert_eq!(get("http.response.status_code"), "500");
    }
}
