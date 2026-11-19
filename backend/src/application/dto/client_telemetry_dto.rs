//! Browser error and performance reports (`POST /api/v1/client-telemetry`,
//! OT-006 Decision 2).
//!
//! The shape is closed: fixed value sets are enums, free text has a length cap,
//! and identifiers have a fixed format. The server re-scrubs every text field
//! and maps `page` onto its allowlist (`RecordClientTelemetryUseCase`), so none
//! of these values reaches a log line or metric label unchecked. There is no
//! user ID field: attribution comes only from the session.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError, ValidationErrors};

use super::schema_enums::ClientErrorKind;

/// Most events one batch may carry.
pub const MAX_EVENTS_PER_BATCH: usize = 30;
/// Longest error message accepted, in characters.
pub const MAX_MESSAGE_CHARS: usize = 500;
/// Longest stack accepted, in characters.
pub const MAX_STACK_CHARS: usize = 4_000;

/// `true` when `value` is exactly `len` lowercase hex characters.
fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn validate_page_load_id(value: &str) -> Result<(), ValidationError> {
    if is_lower_hex(value, 32) {
        Ok(())
    } else {
        Err(ValidationError::new("page_load_id")
            .with_message("page_load_id must be 32 lowercase hex characters".into()))
    }
}

fn validate_trace_id(value: &str) -> Result<(), ValidationError> {
    if is_lower_hex(value, 32) {
        Ok(())
    } else {
        Err(ValidationError::new("trace_id")
            .with_message("trace_id must be 32 lowercase hex characters".into()))
    }
}

/// A release is a commit SHA or `dev`: 1-64 characters of `[0-9A-Za-z._-]`.
fn validate_release(value: &str) -> Result<(), ValidationError> {
    if (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(ValidationError::new("release")
            .with_message("release must be 1-64 characters of [0-9A-Za-z._-]".into()))
    }
}

/// One batch of browser reports from one page load.
#[derive(Debug, Clone, Deserialize, Validate, ToSchema)]
pub struct ClientReportBatch {
    /// Random ID of the page load, regenerated on sign-in and sign-out.
    #[schema(pattern = "^[0-9a-f]{32}$", min_length = 32, max_length = 32)]
    #[validate(custom(function = "validate_page_load_id"))]
    pub page_load_id: String,

    /// Release of the site the browser is running (commit SHA, or `dev`).
    #[schema(pattern = "^[0-9A-Za-z._-]{1,64}$", min_length = 1, max_length = 64)]
    #[validate(custom(function = "validate_release"))]
    pub release: String,

    /// `location.pathname` without a trailing slash. Mapped to a page template
    /// on the server; anything unknown is recorded as `other`.
    #[schema(max_length = 200)]
    #[validate(length(max = 200, message = "page must be at most 200 characters"))]
    pub page: String,

    /// The events, at most 30.
    #[schema(min_items = 1, max_items = 30)]
    #[validate(
        length(min = 1, max = 30, message = "a batch carries 1-30 events"),
        nested
    )]
    pub events: Vec<ClientEvent>,
}

/// One browser report.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    /// A script error, unhandled rejection, boundary error or failed API call.
    Error(ClientErrorEvent),
}

impl Validate for ClientEvent {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            Self::Error(event) => event.validate(),
        }
    }
}

/// A browser error, deduplicated within its page load (`count` repeats).
// `Serialize` only because `validator`'s length rule on `events` records the
// rejected value; nothing serializes these otherwise.
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct ClientErrorEvent {
    pub kind: ClientErrorKind,

    /// The error message, scrubbed in the browser and again on the server.
    #[schema(max_length = 500)]
    #[validate(length(max = 500, message = "message must be at most 500 characters"))]
    pub message: String,

    /// The (minified) stack.
    #[schema(max_length = 4000)]
    #[validate(length(max = 4000, message = "stack must be at most 4000 characters"))]
    #[serde(default)]
    pub stack: Option<String>,

    /// The script the error came from, or `external` for another origin.
    #[schema(max_length = 500)]
    #[validate(length(max = 500, message = "source must be at most 500 characters"))]
    #[serde(default)]
    pub source: Option<String>,

    /// For `api` errors: the route template the call used.
    #[schema(max_length = 200)]
    #[validate(length(max = 200, message = "api_route must be at most 200 characters"))]
    #[serde(default)]
    pub api_route: Option<String>,

    /// For `api` errors: the response status, when the server answered.
    #[schema(minimum = 100, maximum = 599)]
    #[validate(range(min = 100, max = 599, message = "status must be 100-599"))]
    #[serde(default)]
    pub status: Option<u16>,

    /// For `api` errors: the request never reached the server.
    #[serde(default)]
    pub network_failed: Option<bool>,

    /// For `api` errors: the response's `X-Trace-Id`.
    #[schema(pattern = "^[0-9a-f]{32}$", min_length = 32, max_length = 32)]
    #[validate(custom(function = "validate_trace_id"))]
    #[serde(default)]
    pub trace_id: Option<String>,

    /// How many times this error happened in the page load.
    #[schema(minimum = 1, maximum = 10000)]
    #[validate(range(min = 1, max = 10_000, message = "count must be 1-10000"))]
    pub count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn error_event() -> Value {
        json!({
            "type": "error",
            "kind": "error",
            "message": "TypeError: x is undefined",
            "stack": "at f (/_astro/index.js:1:2)",
            "count": 1
        })
    }

    fn batch_with(events: &[Value]) -> Value {
        json!({
            "page_load_id": "0123456789abcdef0123456789abcdef",
            "release": "dev",
            "page": "/login",
            "events": events
        })
    }

    fn parse(value: Value) -> Result<ClientReportBatch, String> {
        let batch: ClientReportBatch =
            serde_json::from_value(value).map_err(|e| format!("deserialize: {e}"))?;
        batch.validate().map_err(|e| format!("validate: {e}"))?;
        Ok(batch)
    }

    fn with_field(field: &str, value: Value) -> Value {
        let mut event = error_event();
        event[field] = value;
        batch_with(&[event])
    }

    #[test]
    fn accepts_a_valid_batch() {
        let mut api = error_event();
        api["kind"] = json!("api");
        api["api_route"] = json!("/users/me");
        api["status"] = json!(500);
        api["trace_id"] = json!("abcdef0123456789abcdef0123456789");
        let batch = parse(batch_with(&[error_event(), api])).unwrap();
        assert_eq!(batch.events.len(), 2);
    }

    #[test]
    fn accepts_thirty_events() {
        assert!(parse(batch_with(&vec![error_event(); MAX_EVENTS_PER_BATCH])).is_ok());
    }

    #[test]
    fn rejects_thirty_one_events() {
        let err = parse(batch_with(&vec![error_event(); MAX_EVENTS_PER_BATCH + 1])).unwrap_err();
        assert!(err.starts_with("validate"), "{err}");
    }

    #[test]
    fn rejects_an_empty_batch() {
        assert!(parse(batch_with(&[])).is_err());
    }

    #[test]
    fn message_is_capped_at_500_characters() {
        // Characters, not bytes: 500 multi-byte characters are fine.
        assert!(parse(with_field("message", json!("é".repeat(MAX_MESSAGE_CHARS)))).is_ok());
        assert!(
            parse(with_field(
                "message",
                json!("x".repeat(MAX_MESSAGE_CHARS + 1))
            ))
            .is_err()
        );
    }

    #[test]
    fn stack_is_capped_at_4000_characters() {
        assert!(parse(with_field("stack", json!("x".repeat(MAX_STACK_CHARS)))).is_ok());
        assert!(parse(with_field("stack", json!("x".repeat(MAX_STACK_CHARS + 1)))).is_err());
    }

    #[test]
    fn rejects_a_malformed_page_load_id() {
        for bad in [
            "",
            "0123456789abcdef",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcdeg",
            "0123456789abcdef0123456789abcdef0",
        ] {
            let mut batch = batch_with(&[error_event()]);
            batch["page_load_id"] = json!(bad);
            assert!(parse(batch).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn rejects_a_malformed_trace_id() {
        for bad in [
            "",
            "abc",
            "ABCDEF0123456789ABCDEF0123456789",
            "00-abcdef0123456789abcdef0123456789-01",
        ] {
            assert!(
                parse(with_field("trace_id", json!(bad))).is_err(),
                "{bad:?} accepted"
            );
        }
    }

    #[test]
    fn rejects_an_unknown_kind_or_event_type() {
        assert!(parse(with_field("kind", json!("warning"))).is_err());
        assert!(parse(with_field("type", json!("vital"))).is_err());
    }

    #[test]
    fn rejects_a_malformed_release() {
        for bad in ["", "has space", &"a".repeat(65)] {
            let mut batch = batch_with(&[error_event()]);
            batch["release"] = json!(bad);
            assert!(parse(batch).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn rejects_a_zero_count_and_an_out_of_range_status() {
        assert!(parse(with_field("count", json!(0))).is_err());
        assert!(parse(with_field("status", json!(99))).is_err());
        assert!(parse(with_field("status", json!(600))).is_err());
    }
}
