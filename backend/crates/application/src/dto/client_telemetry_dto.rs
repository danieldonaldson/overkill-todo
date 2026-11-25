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

use super::schema_enums::{
    ClientErrorKind, IslandStage, NavOutcome, NavType, Rating, ViewTransitionResult, VitalName,
};

/// Most events one batch may carry.
pub const MAX_EVENTS_PER_BATCH: usize = 30;
/// Longest error message accepted, in characters.
pub const MAX_MESSAGE_CHARS: usize = 500;
/// Longest stack accepted, in characters.
pub const MAX_STACK_CHARS: usize = 4_000;
/// Longest island (panel) name accepted, in characters.
pub const MAX_ISLAND_CHARS: usize = 100;
/// Longest Web Vital attribution (tag name and selector) accepted, in characters.
pub const MAX_ATTRIBUTION_CHARS: usize = 100;
/// Largest Web Vital value accepted: 10 minutes in ms, far above any real
/// measurement and any CLS score.
pub const MAX_VITAL_VALUE: f64 = 600_000.0;
/// Longest navigation or panel duration accepted, in ms: 10 minutes, like
/// [`MAX_VITAL_VALUE`].
pub const MAX_DURATION_MS: f64 = 600_000.0;

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
    /// The page finished loading, by a full load or an in-site navigation: the
    /// denominator of the error rate. Never sampled.
    PageLoad,
    /// One Web Vital: LCP, FCP or TTFB for a full load, INP or CLS for a page
    /// visit. Sampled.
    Vital(ClientVitalEvent),
    /// A full page load or an in-site navigation, with its timings (Decision
    /// 6a). Sampled.
    Navigation(ClientNavigationEvent),
    /// When an island became interactive or ready, from the start of the
    /// navigation that showed it (Decision 6b). Sampled.
    Island(ClientIslandEvent),
}

impl Validate for ClientEvent {
    fn validate(&self) -> Result<(), ValidationErrors> {
        match self {
            Self::Error(event) => event.validate(),
            Self::PageLoad => Ok(()),
            Self::Vital(event) => event.validate(),
            Self::Navigation(event) => event.validate(),
            Self::Island(event) => event.validate(),
        }
    }
}

/// A Web Vital measurement (Decision 6).
// `Serialize` for the same reason as `ClientErrorEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct ClientVitalEvent {
    pub name: VitalName,

    /// Milliseconds, or the unitless score for CLS.
    #[schema(minimum = 0, maximum = 600_000)]
    #[validate(range(min = 0.0, max = 600_000.0, message = "value must be 0-600000"))]
    pub value: f64,

    pub rating: Rating,

    /// For a poor rating: the element responsible, as its tag name and the
    /// selector `web-vitals` gives. Logged only for poor ratings, never a
    /// metric label.
    #[schema(max_length = 100)]
    #[validate(length(max = 100, message = "attribution must be at most 100 characters"))]
    #[serde(default)]
    pub attribution: Option<String>,
}

/// A navigation to the batch's page (Decision 6a). The field is `nav_type`
/// because the event's own tag is `type`.
///
/// Any combination the schema allows is accepted; the server labels it
/// consistently: a `hard` load is always `view_transition=none` with no phases,
/// an `aborted` navigation has no durations, and a navigation without the
/// timings it needs is counted but not timed.
// `Serialize` for the same reason as `ClientErrorEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct ClientNavigationEvent {
    pub nav_type: NavType,
    pub view_transition: ViewTransitionResult,
    pub outcome: NavOutcome,

    /// From the start of the navigation (0 for a full load) to the page being
    /// usable and any transition having finished.
    #[schema(minimum = 0, maximum = 600_000)]
    #[validate(range(min = 0.0, max = 600_000.0, message = "total_ms must be 0-600000"))]
    #[serde(default)]
    pub total_ms: Option<f64>,

    /// Fetching the new page.
    #[schema(minimum = 0, maximum = 600_000)]
    #[validate(range(min = 0.0, max = 600_000.0, message = "fetch_ms must be 0-600000"))]
    #[serde(default)]
    pub fetch_ms: Option<f64>,

    /// Swapping it into the document.
    #[schema(minimum = 0, maximum = 600_000)]
    #[validate(range(min = 0.0, max = 600_000.0, message = "swap_ms must be 0-600000"))]
    #[serde(default)]
    pub swap_ms: Option<f64>,

    /// The view transition's animation, when it ran.
    #[schema(minimum = 0, maximum = 600_000)]
    #[validate(range(min = 0.0, max = 600_000.0, message = "transition_ms must be 0-600000"))]
    #[serde(default)]
    pub transition_ms: Option<f64>,
}

/// One island timing mark on the batch's page (Decision 6b).
// `Serialize` for the same reason as `ClientErrorEvent`.
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct ClientIslandEvent {
    /// The island's `TelemetryBoundary` name. Mapped to the island allowlist
    /// on the server; anything unknown is `other`.
    #[schema(max_length = 100)]
    #[validate(length(max = 100, message = "name must be at most 100 characters"))]
    pub name: String,

    pub stage: IslandStage,

    /// The kind of navigation that showed the island.
    pub nav_type: NavType,

    /// From the start of that navigation, in ms.
    #[schema(minimum = 0, maximum = 600_000)]
    #[validate(range(min = 0.0, max = 600_000.0, message = "ms must be 0-600000"))]
    pub ms: f64,
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

    /// For `boundary` errors: the island whose boundary caught it. Mapped to
    /// the island allowlist on the server; anything unknown is `other`.
    #[schema(max_length = 100)]
    #[validate(length(max = 100, message = "island must be at most 100 characters"))]
    #[serde(default)]
    pub island: Option<String>,

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
        assert!(parse(with_field("type", json!("navigate"))).is_err());
    }

    #[test]
    fn accepts_a_page_load_next_to_errors() {
        let batch = parse(batch_with(&[json!({ "type": "page_load" }), error_event()])).unwrap();
        assert!(matches!(batch.events[0], ClientEvent::PageLoad));
    }

    #[test]
    fn island_is_capped_at_100_characters() {
        assert!(parse(with_field("island", json!("SettingsPanel"))).is_ok());
        assert!(parse(with_field("island", json!("x".repeat(MAX_ISLAND_CHARS)))).is_ok());
        assert!(
            parse(with_field(
                "island",
                json!("x".repeat(MAX_ISLAND_CHARS + 1))
            ))
            .is_err()
        );
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

    fn vital(name: &str, value: f64, rating: &str) -> Value {
        json!({ "type": "vital", "name": name, "value": value, "rating": rating })
    }

    #[test]
    fn accepts_every_vital_and_rating() {
        let events: Vec<Value> = [
            ("LCP", 2400.0, "good"),
            ("FCP", 1900.5, "needs-improvement"),
            ("TTFB", 0.0, "good"),
            ("INP", 640.0, "poor"),
            ("CLS", 0.31, "poor"),
        ]
        .into_iter()
        .map(|(n, v, r)| vital(n, v, r))
        .collect();
        let batch = parse(batch_with(&events)).unwrap();
        let ClientEvent::Vital(cls) = &batch.events[4] else {
            panic!("not a vital");
        };
        assert_eq!(cls.name, VitalName::Cls);
        assert_eq!(cls.rating, Rating::Poor);
    }

    #[test]
    fn rejects_an_unknown_vital_name_or_rating() {
        for (name, rating) in [
            ("FID", "good"),
            ("lcp", "good"),
            ("LCP", "bad"),
            ("LCP", "Poor"),
        ] {
            assert!(
                parse(batch_with(&[vital(name, 100.0, rating)])).is_err(),
                "{name} {rating} accepted"
            );
        }
    }

    #[test]
    fn rejects_a_negative_or_huge_vital_value() {
        assert!(parse(batch_with(&[vital("LCP", MAX_VITAL_VALUE, "poor")])).is_ok());
        assert!(parse(batch_with(&[vital("LCP", -1.0, "good")])).is_err());
        assert!(parse(batch_with(&[vital("LCP", MAX_VITAL_VALUE + 1.0, "poor")])).is_err());
    }

    #[test]
    fn attribution_is_capped_at_100_characters() {
        let mut event = vital("INP", 700.0, "poor");
        event["attribution"] = json!("x".repeat(MAX_ATTRIBUTION_CHARS));
        assert!(parse(batch_with(&[event.clone()])).is_ok());
        event["attribution"] = json!("x".repeat(MAX_ATTRIBUTION_CHARS + 1));
        assert!(parse(batch_with(&[event])).is_err());
    }

    fn navigation(nav_type: &str, view_transition: &str, outcome: &str) -> Value {
        json!({
            "type": "navigation",
            "nav_type": nav_type,
            "view_transition": view_transition,
            "outcome": outcome,
            "total_ms": 420.5,
            "fetch_ms": 120.0,
            "swap_ms": 8.0,
            "transition_ms": 250.0
        })
    }

    fn island(name: &str, stage: &str, nav_type: &str) -> Value {
        json!({ "type": "island", "name": name, "stage": stage, "nav_type": nav_type, "ms": 310.0 })
    }

    #[test]
    fn accepts_navigations_and_island_timings() {
        let batch = parse(batch_with(&[
            navigation("soft", "ran", "completed"),
            navigation("soft", "unsupported", "aborted"),
            json!({ "type": "navigation", "nav_type": "hard", "view_transition": "none",
                    "outcome": "completed", "total_ms": 900.0 }),
            island("SettingsPanel", "interactive", "soft"),
            island("HomePanel", "ready", "hard"),
        ]))
        .unwrap();
        let ClientEvent::Navigation(nav) = &batch.events[0] else {
            panic!("not a navigation");
        };
        assert_eq!(nav.nav_type, NavType::Soft);
        assert_eq!(nav.view_transition, ViewTransitionResult::Ran);
        assert_eq!(nav.outcome, NavOutcome::Completed);
        assert_eq!(nav.total_ms, Some(420.5));
        let ClientEvent::Navigation(hard) = &batch.events[2] else {
            panic!("not a navigation");
        };
        assert_eq!(hard.fetch_ms, None);
        let ClientEvent::Island(ready) = &batch.events[4] else {
            panic!("not an island");
        };
        assert_eq!(ready.stage, IslandStage::Ready);
        assert_eq!(ready.nav_type, NavType::Hard);
    }

    #[test]
    fn rejects_unknown_navigation_kinds_results_outcomes_and_stages() {
        for bad in [
            navigation("reload", "ran", "completed"),
            navigation("Soft", "ran", "completed"),
            navigation("soft", "finished", "completed"),
            navigation("soft", "ran", "failed"),
            island("SettingsPanel", "hydrated", "soft"),
            island("SettingsPanel", "ready", "back"),
        ] {
            assert!(
                parse(batch_with(std::slice::from_ref(&bad))).is_err(),
                "{bad} accepted"
            );
        }
        // A navigation's kind is `nav_type`; `type` is the event's tag.
        let mut tagged = navigation("soft", "ran", "completed");
        tagged.as_object_mut().unwrap().remove("nav_type");
        assert!(parse(batch_with(&[tagged])).is_err());
    }

    #[test]
    fn rejects_negative_or_huge_durations() {
        for field in ["total_ms", "fetch_ms", "swap_ms", "transition_ms"] {
            let mut ok = navigation("soft", "ran", "completed");
            ok[field] = json!(MAX_DURATION_MS);
            assert!(parse(batch_with(&[ok])).is_ok(), "{field}");
            for bad in [-1.0, MAX_DURATION_MS + 1.0] {
                let mut nav = navigation("soft", "ran", "completed");
                nav[field] = json!(bad);
                assert!(parse(batch_with(&[nav])).is_err(), "{field} {bad}");
            }
        }
        let mut slow = island("HomePanel", "ready", "hard");
        slow["ms"] = json!(MAX_DURATION_MS + 1.0);
        assert!(parse(batch_with(&[slow])).is_err());
        let mut negative = island("HomePanel", "ready", "hard");
        negative["ms"] = json!(-0.5);
        assert!(parse(batch_with(&[negative])).is_err());
    }

    #[test]
    fn island_timing_name_is_capped_at_100_characters() {
        let long = "x".repeat(MAX_ISLAND_CHARS);
        assert!(parse(batch_with(&[island(&long, "ready", "soft")])).is_ok());
        let longer = "x".repeat(MAX_ISLAND_CHARS + 1);
        assert!(parse(batch_with(&[island(&longer, "ready", "soft")])).is_err());
    }
}
