//! The [`ClientReportSink`] adapter (OT-006 Decisions 2 and 7).
//!
//! Browser errors become `client_error` log events, shipped to the log store
//! like every other log line, and are counted; page loads and dropped reports
//! are counted; Web Vitals go to histograms, and poor ones are also logged as
//! `client_vital_poor` with their attribution. Every label value comes from an
//! enum or an allowlist.
//!
//! Its instruments are no-ops exactly when the OTLP exporter is (dev only), and
//! the log events always go to stdout, so there is no no-op variant to refuse
//! in production.

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

use super::metrics::global_meter;
use crate::application::dto::Rating;
use crate::application::ports::outgoing::{
    ClientErrorReport, ClientPageLoadReport, ClientReportSink, ClientVitalReport,
};

/// Buckets for the millisecond vitals.
///
/// There is an edge at every good/poor threshold: INP 200/500, TTFB 800/1800,
/// FCP 1800/3000, LCP 2500/4000. The low edges keep quick interactions (8 ms
/// when under the browser's 40 ms entry threshold) and fast local loads from
/// all reading as ~40 ms.
pub const VITAL_MS_BUCKETS: &[f64] = &[
    10.0, 25.0, 50.0, 100.0, 200.0, 300.0, 500.0, 800.0, 1_000.0, 1_500.0, 1_800.0, 2_500.0,
    3_000.0, 4_000.0, 6_000.0, 10_000.0, 20_000.0,
];

/// Buckets for CLS, with edges at its thresholds 0.1 and 0.25.
pub const VITAL_CLS_BUCKETS: &[f64] = &[0.01, 0.05, 0.1, 0.15, 0.25, 0.5, 1.0, 2.0];

/// Writes browser reports to the logs and metrics.
#[derive(Clone)]
pub struct TelemetryClientReportSink {
    errors: Counter<u64>,
    page_loads: Counter<u64>,
    dropped: Counter<u64>,
    /// LCP, FCP, TTFB and INP. One instrument has one unit and one bucket
    /// set, so CLS, a unitless score, has its own.
    vital_ms: Histogram<f64>,
    vital_cls: Histogram<f64>,
    releases_seen: Counter<u64>,
}

impl TelemetryClientReportSink {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            errors: meter
                .u64_counter("browser.error")
                .with_unit("{error}")
                .with_description(
                    "Browser errors from the site's own scripts, repeats included",
                )
                .build(),
            page_loads: meter
                .u64_counter("browser.page_load")
                .with_unit("{load}")
                .with_description(
                    "Finished page loads and in-site navigations: the browser error rate's denominator, not traffic analytics",
                )
                .build(),
            dropped: meter
                .u64_counter("browser.report.dropped")
                .with_unit("{event}")
                .with_description("Browser report events over the recording budget, not recorded")
                .build(),
            vital_ms: meter
                .f64_histogram("browser.web_vital")
                .with_unit("ms")
                .with_description("Web Vitals measured in the browser: LCP, FCP and TTFB per full load, INP per page visit")
                .with_boundaries(VITAL_MS_BUCKETS.to_vec())
                .build(),
            releases_seen: meter
                .u64_counter("browser.release_seen")
                .with_unit("{load}")
                .with_description(
                    "Page loads per release: the first sample of each marks when a release went out",
                )
                .build(),
            vital_cls: meter
                .f64_histogram("browser.web_vital.cls")
                .with_unit("1")
                .with_description("Cumulative Layout Shift per page visit, measured in the browser")
                .with_boundaries(VITAL_CLS_BUCKETS.to_vec())
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
            island = report.island,
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
        // Extensions and third-party scripts aren't the site's errors.
        if !report.external {
            self.errors.add(
                u64::from(report.count),
                &[
                    KeyValue::new("page", report.page),
                    KeyValue::new("kind", report.kind.as_str()),
                    KeyValue::new("release", report.release_label.clone()),
                ],
            );
        }
    }

    fn page_load(&self, report: &ClientPageLoadReport) {
        self.page_loads.add(
            1,
            &[
                KeyValue::new("page", report.page),
                KeyValue::new("device_class", report.device_class),
                KeyValue::new("authenticated", report.authenticated),
            ],
        );
        self.releases_seen
            .add(1, &[KeyValue::new("release", report.release_label.clone())]);
    }

    fn vital(&self, report: &ClientVitalReport) {
        let mut labels = vec![
            KeyValue::new("page", report.page),
            KeyValue::new("rating", report.rating.as_str()),
            KeyValue::new("device_class", report.device_class),
            KeyValue::new("authenticated", report.authenticated),
        ];
        if report.name.is_duration() {
            labels.push(KeyValue::new("name", report.name.as_str()));
            self.vital_ms.record(report.value, &labels);
        } else {
            self.vital_cls.record(report.value, &labels);
        }
        if report.rating == Rating::Poor {
            tracing::info!(
                event = "client_vital_poor",
                name = report.name.as_str(),
                value = report.value,
                page = report.page,
                device_class = report.device_class,
                attribution = report.attribution.as_deref(),
                "poor web vital"
            );
        }
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
    use crate::application::dto::{ClientErrorKind, VitalName};
    use crate::application::use_cases::auth::log_capture::LogCapture;
    use crate::domain::value_objects::UserId;
    use crate::infrastructure::telemetry::metrics::testing::Attrs;
    use crate::infrastructure::telemetry::metrics::{METER_NAME, testing};

    fn report(user_id: Option<UserId>) -> ClientErrorReport {
        ClientErrorReport {
            page: "/dashboard/settings",
            release: "abc123".into(),
            release_label: "abc123".into(),
            kind: ClientErrorKind::Api,
            fingerprint: "0badc0de".into(),
            count: 2,
            message: "boom".into(),
            stack: Some("Error: boom\n    at f (https://h/_astro/a.js:1:2)".into()),
            source: Some("https://h/_astro/a.js".into()),
            external: false,
            island: Some("SettingsPanel"),
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
            "island": "SettingsPanel",
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
        visitor.island = None;
        let event = logged(&visitor);
        for field in ["user_id", "stack", "status", "client_trace_id", "island"] {
            assert!(event.get(field).is_none(), "{field} present: {event}");
        }
        assert_eq!(event["page"], "/dashboard/settings");
    }

    fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn site_errors_are_counted_by_page_kind_and_release_label() {
        let (provider, exporter) = testing::in_memory();
        let sink = TelemetryClientReportSink::new(&provider.meter(METER_NAME));
        let mut forged = report(None);
        forged.release = "forged-release".into();
        forged.release_label = "other".into();
        forged.count = 5;
        let mut external = report(None);
        external.external = true;
        external.source = Some("external".into());
        sink.error(&report(None));
        sink.error(&forged);
        sink.error(&external);
        provider.force_flush().unwrap();
        assert_eq!(
            testing::u64_sum_points(&exporter, "browser.error"),
            [
                (
                    attrs(&[
                        ("kind", "api"),
                        ("page", "/dashboard/settings"),
                        ("release", "abc123")
                    ]),
                    2 // the external error is logged, not counted
                ),
                (
                    attrs(&[
                        ("kind", "api"),
                        ("page", "/dashboard/settings"),
                        ("release", "other")
                    ]),
                    5
                ),
            ]
        );
    }

    #[test]
    fn page_loads_are_counted_by_page_device_and_session() {
        let (provider, exporter) = testing::in_memory();
        let sink = TelemetryClientReportSink::new(&provider.meter(METER_NAME));
        let load = ClientPageLoadReport {
            page: "/dashboard",
            device_class: "mobile",
            authenticated: true,
            release_label: "r1".into(),
        };
        sink.page_load(&load);
        sink.page_load(&load);
        sink.page_load(&ClientPageLoadReport {
            page: "/login",
            device_class: "desktop",
            authenticated: false,
            release_label: "r2".into(),
        });
        provider.force_flush().unwrap();
        assert_eq!(
            testing::u64_sum_points(&exporter, "browser.page_load"),
            [
                (
                    attrs(&[
                        ("authenticated", "false"),
                        ("device_class", "desktop"),
                        ("page", "/login")
                    ]),
                    1
                ),
                (
                    attrs(&[
                        ("authenticated", "true"),
                        ("device_class", "mobile"),
                        ("page", "/dashboard")
                    ]),
                    2
                ),
            ]
        );
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

    fn vital(name: VitalName, value: f64, rating: Rating) -> ClientVitalReport {
        ClientVitalReport {
            name,
            value,
            rating,
            page: "/dashboard",
            device_class: "mobile",
            authenticated: true,
            attribution: None,
        }
    }

    /// Index of the bucket `value` lands in, given upper-inclusive bounds.
    fn bucket_of(bounds: &[f64], value: f64) -> usize {
        bounds
            .iter()
            .position(|b| value <= *b)
            .unwrap_or(bounds.len())
    }

    #[test]
    fn vitals_land_in_the_bucket_their_threshold_puts_them_in() {
        let (provider, exporter) = testing::in_memory();
        let sink = TelemetryClientReportSink::new(&provider.meter(METER_NAME));
        sink.vital(&vital(VitalName::Inp, 200.0, Rating::Good));
        sink.vital(&vital(VitalName::Inp, 201.0, Rating::NeedsImprovement));
        sink.vital(&vital(VitalName::Lcp, 4_100.0, Rating::Poor));
        sink.vital(&vital(VitalName::Cls, 0.1, Rating::Good));
        sink.vital(&vital(VitalName::Cls, 0.3, Rating::Poor));
        provider.force_flush().unwrap();

        let (bounds, points) = testing::f64_histogram_buckets(&exporter, "browser.web_vital");
        assert_eq!(bounds, VITAL_MS_BUCKETS);
        let base = [
            ("authenticated", "true"),
            ("device_class", "mobile"),
            ("page", "/dashboard"),
        ];
        let with = |name: &str, rating: &str| {
            let mut a = attrs(&base);
            a.push(("name".into(), name.into()));
            a.push(("rating".into(), rating.into()));
            a.sort();
            a
        };
        let counts = |a: &Attrs| points.iter().find(|(p, _)| p == a).unwrap().1.clone();
        // Bucket bounds are upper-inclusive: 200 ms is the top of the good INP
        // bucket and 201 ms the start of the next.
        assert_eq!(counts(&with("INP", "good"))[bucket_of(&bounds, 200.0)], 1);
        assert_eq!(bucket_of(&bounds, 200.0) + 1, bucket_of(&bounds, 201.0));
        assert_eq!(
            counts(&with("INP", "needs-improvement"))[bucket_of(&bounds, 201.0)],
            1
        );
        assert_eq!(counts(&with("LCP", "poor"))[bucket_of(&bounds, 4_100.0)], 1);
        assert_eq!(
            testing::unit(&exporter, "browser.web_vital").as_deref(),
            Some("ms")
        );

        let (cls_bounds, cls_points) =
            testing::f64_histogram_buckets(&exporter, "browser.web_vital.cls");
        assert_eq!(cls_bounds, VITAL_CLS_BUCKETS);
        let good = attrs(&[
            ("authenticated", "true"),
            ("device_class", "mobile"),
            ("page", "/dashboard"),
            ("rating", "good"),
        ]);
        let good_counts = &cls_points.iter().find(|(p, _)| *p == good).unwrap().1;
        assert_eq!(good_counts[bucket_of(&cls_bounds, 0.1)], 1);
        // CLS carries no `name` label: the instrument is the name.
        assert!(
            cls_points
                .iter()
                .all(|(a, _)| a.iter().all(|(k, _)| k != "name"))
        );
        assert_eq!(
            testing::unit(&exporter, "browser.web_vital.cls").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn only_poor_vitals_are_logged_with_their_attribution() {
        let logs = LogCapture::install();
        let sink = TelemetryClientReportSink::new(&opentelemetry::global::meter("t"));
        sink.vital(&vital(VitalName::Inp, 120.0, Rating::Good));
        sink.vital(&vital(VitalName::Inp, 300.0, Rating::NeedsImprovement));
        let mut poor = vital(VitalName::Inp, 640.0, Rating::Poor);
        poor.attribution = Some("button#save".into());
        sink.vital(&poor);
        let events = logs.events("client_vital_poor");
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["name"], "INP");
        assert_eq!(events[0]["value"], 640.0);
        assert_eq!(events[0]["page"], "/dashboard");
        assert_eq!(events[0]["attribution"], "button#save");
        assert!(events[0].get("user_id").is_none());
    }

    #[test]
    fn each_release_is_one_release_seen_series() {
        let (provider, exporter) = testing::in_memory();
        let sink = TelemetryClientReportSink::new(&provider.meter(METER_NAME));
        for (page, release) in [("/login", "r1"), ("/dashboard", "r1"), ("/login", "r2")] {
            sink.page_load(&ClientPageLoadReport {
                page,
                device_class: "desktop",
                authenticated: false,
                release_label: release.into(),
            });
        }
        provider.force_flush().unwrap();
        assert_eq!(
            testing::u64_sum_points(&exporter, "browser.release_seen"),
            [
                (attrs(&[("release", "r1")]), 2),
                (attrs(&[("release", "r2")]), 1)
            ]
        );
    }
}
