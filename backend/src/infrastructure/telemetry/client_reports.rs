//! The [`ClientReportSink`] adapter (OT-006 Decision 2): browser errors become
//! `client_error` log events, shipped to the log store like every other log
//! line, and dropped reports a counter.
//!
//! Its instruments are no-ops exactly when the OTLP exporter is (dev only), and
//! the log events always go to stdout, so there is no no-op variant to refuse
//! in production.

use opentelemetry::metrics::{Counter, Meter};

use super::metrics::global_meter;
use crate::application::ports::outgoing::{ClientErrorReport, ClientReportSink};

/// Writes browser reports to the logs and metrics.
#[derive(Clone)]
pub struct TelemetryClientReportSink {
    dropped: Counter<u64>,
}

impl TelemetryClientReportSink {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            dropped: meter
                .u64_counter("browser.report.dropped")
                .with_unit("{event}")
                .with_description("Browser report events over the recording budget, not recorded")
                .build(),
        }
    }

    /// Instruments on the global meter.
    #[must_use]
    pub fn from_global() -> Self {
        Self::new(&global_meter())
    }
}

impl ClientReportSink for TelemetryClientReportSink {
    fn error(&self, report: &ClientErrorReport) {
        // `Option` fields are left out of the line when `None`.
        let user_id = report.user_id.map(|id| id.to_string());
        tracing::warn!(
            event = "client_error",
            page = report.page,
            release = %report.release,
            kind = report.kind.as_str(),
            error_fingerprint = %report.fingerprint,
            count = report.count,
            error_message = %report.message,
            stack = report.stack.as_deref(),
            source = report.source.as_deref(),
            api_route = report.api_route.as_deref(),
            status = report.status,
            network_failed = report.network_failed,
            client_trace_id = report.client_trace_id.as_deref(),
            page_load_id = %report.page_load_id,
            browser_family = report.browser_family,
            device_class = report.device_class,
            user_id = user_id.as_deref(),
            "browser error"
        );
    }

    fn dropped(&self, n: u64) {
        self.dropped.add(n, &[]);
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use serde_json::{Value, json};

    use super::*;
    use crate::application::dto::ClientErrorKind;
    use crate::application::use_cases::auth::log_capture::LogCapture;
    use crate::domain::value_objects::UserId;
    use crate::infrastructure::telemetry::metrics::{METER_NAME, testing};

    fn report(user_id: Option<UserId>) -> ClientErrorReport {
        ClientErrorReport {
            page: "/dashboard/settings",
            release: "abc123".into(),
            kind: ClientErrorKind::Api,
            fingerprint: "0badc0de".into(),
            count: 2,
            message: "boom".into(),
            stack: Some("Error: boom\n    at f (https://h/_astro/a.js:1:2)".into()),
            source: Some("https://h/_astro/a.js".into()),
            api_route: Some("/users/me".into()),
            status: Some(500),
            network_failed: Some(false),
            client_trace_id: Some("abcdef0123456789abcdef0123456789".into()),
            page_load_id: "0123456789abcdef0123456789abcdef".into(),
            browser_family: "chrome",
            device_class: "desktop",
            user_id,
        }
    }

    fn logged(report: &ClientErrorReport) -> Value {
        let logs = LogCapture::install();
        TelemetryClientReportSink::new(&opentelemetry::global::meter("t")).error(report);
        let mut events = logs.events("client_error");
        assert_eq!(events.len(), 1);
        events.pop().unwrap()
    }

    #[test]
    fn an_error_is_logged_at_warn_with_every_field() {
        let user = UserId::new();
        let event = logged(&report(Some(user)));
        assert_eq!(event["level"], "WARN");
        let expected = json!({
            "page": "/dashboard/settings",
            "release": "abc123",
            "kind": "api",
            "error_fingerprint": "0badc0de",
            "count": 2,
            "error_message": "boom",
            "stack": "Error: boom\n    at f (https://h/_astro/a.js:1:2)",
            "source": "https://h/_astro/a.js",
            "api_route": "/users/me",
            "status": 500,
            "network_failed": false,
            "client_trace_id": "abcdef0123456789abcdef0123456789",
            "page_load_id": "0123456789abcdef0123456789abcdef",
            "browser_family": "chrome",
            "device_class": "desktop",
            "user_id": user.to_string(),
        });
        for (field, value) in expected.as_object().unwrap() {
            assert_eq!(&event[field], value, "field {field}");
        }
    }

    #[test]
    fn absent_fields_and_a_visitors_user_id_are_left_out() {
        let mut visitor = report(None);
        visitor.stack = None;
        visitor.status = None;
        visitor.client_trace_id = None;
        let event = logged(&visitor);
        for field in ["user_id", "stack", "status", "client_trace_id"] {
            assert!(event.get(field).is_none(), "{field} present: {event}");
        }
        assert_eq!(event["page"], "/dashboard/settings");
    }

    #[test]
    fn dropped_events_are_counted() {
        let (provider, exporter) = testing::in_memory();
        let sink = TelemetryClientReportSink::new(&provider.meter(METER_NAME));
        sink.dropped(3);
        sink.dropped(4);
        provider.force_flush().unwrap();
        assert_eq!(
            testing::u64_sum_points(&exporter, "browser.report.dropped"),
            [(vec![], 7)]
        );
        assert_eq!(
            testing::unit(&exporter, "browser.report.dropped").as_deref(),
            Some("{event}")
        );
    }
}
