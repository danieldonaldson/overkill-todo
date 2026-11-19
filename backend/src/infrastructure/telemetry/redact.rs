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

// The rules themselves are dependency-free, so the application layer can apply
// them too (browser reports, OT-006).
pub use crate::shared::redact::{REDACTED, is_sensitive_field, redact_query, redact_url};

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
