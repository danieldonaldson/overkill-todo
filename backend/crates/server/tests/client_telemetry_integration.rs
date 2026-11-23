//! `POST /api/v1/client-telemetry` through the real router (OT-006 slices 1
//! and 2): attribution from the session only, the standard 400 and the 413, the
//! recording budget, no CSRF, no trace while the request is still counted, page
//! loads and errors counted with allowlisted labels, and boundary and API
//! errors logged with their island, route, status and trace ID.
//!
//! Each test installs the production subscriber stack as the thread default,
//! with an in-memory span exporter and a captured JSON log writer, and builds
//! the app with the real `TelemetryClientReportSink` on an in-memory meter.
//! `#[sqlx::test(migrations = "../../migrations")]` runs on a current-thread runtime, so the subscriber sees
//! everything the request does.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use api::cookies::AUTH_COOKIE;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData, ScopeMetrics, SumDataPoint},
};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor};
use platform::telemetry::{
    self, LogFormat, ServiceInfo, client_reports::TelemetryClientReportSink, metrics::METER_NAME,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ROUTE: &str = "/api/v1/client-telemetry";
const PAGE_LOAD_ID: &str = "0123456789abcdef0123456789abcdef";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The global meter provider, installed once per test binary: the router's
/// request metrics are built from it. Tests filter its points by route.
fn global_metrics() -> &'static (SdkMeterProvider, InMemoryMetricExporter) {
    static METRICS: OnceLock<(SdkMeterProvider, InMemoryMetricExporter)> = OnceLock::new();
    METRICS.get_or_init(|| {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        (provider, exporter)
    })
}

struct Harness {
    app: Router,
    spans: InMemorySpanExporter,
    tracer: SdkTracerProvider,
    logs: LogBuffer,
    meter: SdkMeterProvider,
    metrics: InMemoryMetricExporter,
    _guard: tracing::subscriber::DefaultGuard,
}

impl Harness {
    fn new(pool: PgPool, max_events_per_sec: u32) -> Self {
        global_metrics();
        let service = ServiceInfo {
            name: "overkill-todo-backend".into(),
            version: "test".into(),
            environment: "test".into(),
        };
        let spans = InMemorySpanExporter::default();
        let tracer =
            telemetry::tracer_provider(&service, Some(SimpleSpanProcessor::new(spans.clone())));
        let logs = LogBuffer::default();
        let subscriber = telemetry::subscriber(
            &tracer,
            telemetry::targets_filter(server::APP_TARGETS, "debug", &["tower_http=debug"]),
            telemetry::export_filter(
                server::APP_TARGETS,
                &[persistence_postgres::STATEMENT_LOG_DIRECTIVE],
            ),
            LogFormat::Json,
            logs.clone(),
            service,
        );
        let guard = tracing::subscriber::set_default(subscriber);

        let metrics = InMemoryMetricExporter::default();
        let meter = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(metrics.clone()).build())
            .build();
        let sink = Arc::new(TelemetryClientReportSink::new(&meter.meter(METER_NAME)));
        let app = common::create_test_app_with_client_telemetry(pool, sink, max_events_per_sec);
        Self {
            app,
            spans,
            tracer,
            logs,
            meter,
            metrics,
            _guard: guard,
        }
    }

    /// POST `body` with the given extra headers; returns status and body text.
    async fn post(&self, body: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
        let mut request = Request::builder()
            .method("POST")
            .uri(ROUTE)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            );
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn post_json(&self, body: &Value, headers: &[(&str, &str)]) -> (StatusCode, String) {
        self.post(&body.to_string(), headers).await
    }

    fn log_lines(&self) -> Vec<Value> {
        String::from_utf8(self.logs.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn client_errors(&self) -> Vec<Value> {
        self.log_lines()
            .into_iter()
            .filter(|line| line["event"] == "client_error")
            .collect()
    }

    /// `(sorted attributes, value)` for each point of the `u64` counter `name`.
    fn counter(&self, name: &str) -> Vec<(Vec<(String, String)>, u64)> {
        self.meter.force_flush().unwrap();
        let batches = self.metrics.get_finished_metrics().unwrap();
        let mut points = batches
            .iter()
            .rev()
            .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
            .find(|m| m.name() == name)
            .map_or_else(Vec::new, |m| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = m.data() else {
                    panic!("{name} is not a u64 sum");
                };
                sum.data_points()
                    .map(|dp: &SumDataPoint<u64>| {
                        let mut attrs: Vec<(String, String)> = dp
                            .attributes()
                            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                            .collect();
                        attrs.sort();
                        (attrs, dp.value())
                    })
                    .collect()
            });
        points.sort();
        points
    }

    /// `(sorted attributes, count)` for each point of the `f64` histogram `name`.
    fn histogram(&self, name: &str) -> Vec<(Vec<(String, String)>, u64)> {
        self.meter.force_flush().unwrap();
        let batches = self.metrics.get_finished_metrics().unwrap();
        let mut points = batches
            .iter()
            .rev()
            .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
            .find(|m| m.name() == name)
            .map_or_else(Vec::new, |m| {
                let AggregatedMetrics::F64(MetricData::Histogram(hist)) = m.data() else {
                    panic!("{name} is not an f64 histogram");
                };
                hist.data_points()
                    .map(|dp| {
                        let mut attrs: Vec<(String, String)> = dp
                            .attributes()
                            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                            .collect();
                        attrs.sort();
                        (attrs, dp.count())
                    })
                    .collect()
            });
        points.sort();
        points
    }

    /// Sum of the `browser.report.dropped` counter.
    fn dropped(&self) -> u64 {
        self.counter("browser.report.dropped")
            .iter()
            .map(|(_, v)| v)
            .sum()
    }
}

fn error_event(message: &str) -> Value {
    json!({
        "type": "error",
        "kind": "error",
        "message": message,
        "stack": format!("Error: {message}\n    at f (https://overkill.example/_astro/a.js:1:2)"),
        "count": 1
    })
}

fn batch(page: &str, events: &[Value]) -> Value {
    json!({
        "page_load_id": PAGE_LOAD_ID,
        "release": "abc123",
        "page": page,
        "events": events
    })
}

fn session_cookie(token: &str) -> String {
    format!("{AUTH_COOKIE}={token}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_visitor_report_is_logged_with_no_user_id_and_an_empty_204(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let (status, body) = h
        .post_json(&batch("/login", &[error_event("x is undefined")]), &[])
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, "", "the response holds nothing");

    let errors = h.client_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let event = &errors[0];
    assert_eq!(event["level"], "WARN");
    assert_eq!(event["page"], "/login");
    assert_eq!(event["release"], "abc123");
    assert_eq!(event["kind"], "error");
    assert_eq!(event["error_message"], "x is undefined");
    assert_eq!(event["page_load_id"], PAGE_LOAD_ID);
    assert_eq!(event["browser_family"], "chrome");
    assert_eq!(event["device_class"], "desktop");
    assert!(event["error_fingerprint"].is_string());
    assert!(event.get("user_id").is_none(), "{event}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_session_cookie_attributes_the_report_with_no_csrf_header(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let user = Uuid::new_v4();
    let cookie = session_cookie(&common::mint_session_jwt(user, true, "user", 15));
    // A signed-in POST with no CSRF header or cookie: `sendBeacon` sends neither.
    let (status, _) = h
        .post_json(
            &batch("/dashboard/settings", &[error_event("boom")]),
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let errors = h.client_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["user_id"], user.to_string());
    assert_eq!(errors[0]["page"], "/dashboard/settings");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_expired_or_invalid_session_is_anonymous_never_a_401(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let expired = session_cookie(&common::mint_session_jwt(Uuid::new_v4(), true, "user", -10));
    let garbage = session_cookie("not-a-token");
    for cookie in [&expired, &garbage] {
        let (status, _) = h
            .post_json(
                &batch("/dashboard", &[error_event("boom")]),
                &[("cookie", cookie)],
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{cookie}");
    }
    let errors = h.client_errors();
    assert_eq!(errors.len(), 2);
    for event in errors {
        assert!(event.get("user_id").is_none(), "{event}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_user_id_in_the_body_is_ignored(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let claimed = Uuid::new_v4().to_string();
    let session_user = Uuid::new_v4();
    let cookie = session_cookie(&common::mint_session_jwt(session_user, true, "user", 15));

    let mut event = error_event("boom");
    event["user_id"] = json!(claimed);
    let mut body = batch("/dashboard", &[event]);
    body["user_id"] = json!(claimed);

    let (anonymous, _) = h.post_json(&body, &[]).await;
    let (signed_in, _) = h.post_json(&body, &[("cookie", &cookie)]).await;
    assert_eq!(anonymous, StatusCode::NO_CONTENT);
    assert_eq!(signed_in, StatusCode::NO_CONTENT);

    let errors = h.client_errors();
    assert_eq!(errors.len(), 2);
    assert!(errors[0].get("user_id").is_none(), "{}", errors[0]);
    assert_eq!(errors[1]["user_id"], session_user.to_string());
    let logs = String::from_utf8(h.logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        !logs.contains(&claimed),
        "the claimed user ID reached the logs"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn malformed_input_gets_the_standard_400_and_is_logged_nowhere(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let mut bad_id = batch("/login", &[error_event("boom")]);
    bad_id["page_load_id"] = json!("not-hex");
    let mut bad_kind = batch("/login", &[error_event("boom")]);
    bad_kind["events"][0]["kind"] = json!("warning");
    let too_many = batch("/login", &vec![error_event("boom"); 31]);

    for body in [
        bad_id.to_string(),
        bad_kind.to_string(),
        too_many.to_string(),
        "{not json".to_string(),
    ] {
        let (status, response) = h.post(&body, &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let error: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(error["code"], "validation_error");
        assert!(error["message"].is_string());
    }
    assert_eq!(h.client_errors(), Vec::<Value>::new());
    let loud: Vec<Value> = h
        .log_lines()
        .into_iter()
        .filter(|line| line["level"] == "ERROR" || line["level"] == "WARN")
        .collect();
    assert!(loud.is_empty(), "bad input was logged: {loud:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_body_over_16_kb_gets_a_413_and_is_not_recorded(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let mut body = batch("/login", &[error_event("boom")]);
    body["padding"] = json!("x".repeat(16 * 1024));
    let (status, response) = h.post_json(&body, &[]).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let error: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(error["code"], "payload_too_large");
    assert_eq!(h.client_errors(), Vec::<Value>::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn reports_over_the_budget_get_a_204_and_are_counted_not_recorded(pool: PgPool) {
    let h = Harness::new(pool, 2);
    let events: Vec<Value> = (0..5).map(|i| error_event(&format!("e{i}"))).collect();
    let (status, body) = h.post_json(&batch("/login", &events), &[]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, "");
    assert_eq!(h.client_errors().len(), 2);
    assert_eq!(h.dropped(), 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_report_gets_no_span_but_is_counted_in_request_metrics(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let (status, _) = h
        .post_json(&batch("/login", &[error_event("boom")]), &[])
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    h.tracer.force_flush().unwrap();
    let spans = h.spans.get_finished_spans().unwrap();
    assert!(
        spans.is_empty(),
        "spans exported for a browser report: {:?}",
        spans.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
    );

    let (provider, exporter) = global_metrics();
    provider.force_flush().unwrap();
    let batches = exporter.get_finished_metrics().unwrap();
    let counted = batches
        .iter()
        .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
        .filter(|m| m.name() == "http.server.request.duration")
        .any(|m| {
            let AggregatedMetrics::F64(MetricData::Histogram(hist)) = m.data() else {
                panic!("http.server.request.duration is not an f64 histogram");
            };
            hist.data_points().any(|dp| {
                dp.count() > 0
                    && dp
                        .attributes()
                        .any(|kv| kv.key.as_str() == "http.route" && kv.value.as_str() == ROUTE)
            })
        });
    assert!(counted, "no request duration recorded for {ROUTE}");
}

fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[sqlx::test(migrations = "../../migrations")]
async fn page_loads_are_counted_as_signed_in_only_from_the_session(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let cookie = session_cookie(&common::mint_session_jwt(Uuid::new_v4(), true, "user", 15));
    let load = json!({ "type": "page_load" });
    for headers in [vec![("cookie", cookie.as_str())], vec![]] {
        let (status, _) = h
            .post_json(&batch("/dashboard", std::slice::from_ref(&load)), &headers)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    // An unknown page is counted as `other`, not as a new series.
    h.post_json(&batch("/made-up", std::slice::from_ref(&load)), &[])
        .await;
    assert_eq!(
        h.counter("browser.page_load"),
        [
            (
                attrs(&[
                    ("authenticated", "false"),
                    ("device_class", "desktop"),
                    ("page", "/dashboard")
                ]),
                1
            ),
            (
                attrs(&[
                    ("authenticated", "false"),
                    ("device_class", "desktop"),
                    ("page", "other")
                ]),
                1
            ),
            (
                attrs(&[
                    ("authenticated", "true"),
                    ("device_class", "desktop"),
                    ("page", "/dashboard")
                ]),
                1
            ),
        ]
    );
    assert!(h.client_errors().is_empty(), "page loads aren't errors");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_boundary_error_is_logged_with_its_island(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let mut known = error_event("panel crashed");
    known["kind"] = json!("boundary");
    known["island"] = json!("SettingsPanel");
    let mut made_up = known.clone();
    made_up["island"] = json!("NotARealPanel");
    h.post_json(&batch("/dashboard/settings", &[known, made_up]), &[])
        .await;

    let errors = h.client_errors();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["kind"], "boundary");
    assert_eq!(errors[0]["island"], "SettingsPanel");
    assert_eq!(errors[1]["island"], "other");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_api_error_is_logged_with_route_status_and_trace_id_and_counted(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let trace_id = "abcdef0123456789abcdef0123456789";
    let api = json!({
        "type": "error",
        "kind": "api",
        "message": "PATCH /users/me failed",
        "api_route": "/users/me",
        "status": 500,
        "trace_id": trace_id,
        "count": 2
    });
    let unreachable = json!({
        "type": "error",
        "kind": "api",
        "message": "POST /auth/register failed",
        "api_route": "/auth/register",
        "network_failed": true,
        "count": 1
    });
    h.post_json(&batch("/dashboard/settings", &[api]), &[])
        .await;
    h.post_json(&batch("/register", &[unreachable]), &[]).await;

    let errors = h.client_errors();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["api_route"], "/users/me");
    assert_eq!(errors[0]["status"], 500);
    assert_eq!(errors[0]["client_trace_id"], trace_id);
    assert_eq!(errors[1]["api_route"], "/auth/register");
    assert_eq!(errors[1]["network_failed"], true);
    assert!(errors[1].get("client_trace_id").is_none());
    assert!(errors[1].get("status").is_none());

    assert_eq!(
        h.counter("browser.error"),
        [
            (
                attrs(&[
                    ("kind", "api"),
                    ("page", "/dashboard/settings"),
                    ("release", "abc123")
                ]),
                2
            ),
            (
                attrs(&[
                    ("kind", "api"),
                    ("page", "/register"),
                    ("release", "abc123")
                ]),
                1
            ),
        ]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_external_error_is_logged_but_not_counted(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let external = json!({
        "type": "error",
        "kind": "error",
        "message": "Script error.",
        "source": "external",
        "count": 3
    });
    h.post_json(&batch("/login", &[external]), &[]).await;
    assert_eq!(h.client_errors().len(), 1);
    assert_eq!(h.client_errors()[0]["source"], "external");
    assert_eq!(h.counter("browser.error"), Vec::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn vitals_are_recorded_per_page_and_poor_ones_logged(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let cookie = session_cookie(&common::mint_session_jwt(Uuid::new_v4(), true, "user", 15));
    let (status, body) = h
        .post_json(
            &batch(
                "/dashboard/settings",
                &[
                    json!({ "type": "vital", "name": "INP", "value": 640.0, "rating": "poor",
                            "attribution": "button#save" }),
                    json!({ "type": "vital", "name": "CLS", "value": 0.02, "rating": "good" }),
                ],
            ),
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, "");
    h.post_json(
        &batch(
            "/made-up",
            &[json!({ "type": "vital", "name": "LCP", "value": 1200.0, "rating": "good" })],
        ),
        &[],
    )
    .await;

    assert_eq!(
        h.histogram("browser.web_vital"),
        [
            (
                attrs(&[
                    ("authenticated", "false"),
                    ("device_class", "desktop"),
                    ("name", "LCP"),
                    ("page", "other"),
                    ("rating", "good"),
                ]),
                1
            ),
            (
                attrs(&[
                    ("authenticated", "true"),
                    ("device_class", "desktop"),
                    ("name", "INP"),
                    ("page", "/dashboard/settings"),
                    ("rating", "poor"),
                ]),
                1
            ),
        ]
    );
    assert_eq!(
        h.histogram("browser.web_vital.cls"),
        [(
            attrs(&[
                ("authenticated", "true"),
                ("device_class", "desktop"),
                ("page", "/dashboard/settings"),
                ("rating", "good"),
            ]),
            1
        )]
    );
    let poor: Vec<Value> = h
        .log_lines()
        .into_iter()
        .filter(|line| line["event"] == "client_vital_poor")
        .collect();
    assert_eq!(poor.len(), 1, "{poor:?}");
    assert_eq!(poor[0]["attribution"], "button#save");
    assert_eq!(poor[0]["page"], "/dashboard/settings");
    assert!(h.client_errors().is_empty(), "vitals aren't errors");
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_vital_names_and_ratings_get_a_400(pool: PgPool) {
    let h = Harness::new(pool, 200);
    for (name, rating) in [("FID", "good"), ("LCP", "great")] {
        let (status, _) = h
            .post_json(
                &batch(
                    "/login",
                    &[json!({ "type": "vital", "name": name, "value": 1.0, "rating": rating })],
                ),
                &[],
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name} {rating}");
    }
    assert_eq!(h.histogram("browser.web_vital").len(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn navigations_are_timed_counted_and_labelled_consistently(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let (status, body) = h
        .post_json(
            &batch(
                "/dashboard/settings",
                &[
                    json!({ "type": "navigation", "nav_type": "soft", "view_transition": "ran",
                            "outcome": "completed", "total_ms": 420.0, "fetch_ms": 120.0,
                            "swap_ms": 8.0, "transition_ms": 250.0 }),
                ],
            ),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, "");
    h.post_json(
        &batch(
            "/dashboard/admin",
            &[
                json!({ "type": "navigation", "nav_type": "soft", "view_transition": "skipped",
                      "outcome": "aborted", "total_ms": 50.0 }),
            ],
        ),
        &[],
    )
    .await;
    // A full load claiming a transition is labelled `none`; a made-up page is `other`.
    h.post_json(
        &batch(
            "/made-up",
            &[
                json!({ "type": "navigation", "nav_type": "hard", "view_transition": "ran",
                      "outcome": "completed", "total_ms": 900.0, "fetch_ms": 1.0 }),
            ],
        ),
        &[],
    )
    .await;

    assert_eq!(
        h.histogram("browser.navigation.duration"),
        [
            (
                attrs(&[
                    ("device_class", "desktop"),
                    ("page", "/dashboard/settings"),
                    ("type", "soft"),
                    ("view_transition", "ran"),
                ]),
                1
            ),
            (
                attrs(&[
                    ("device_class", "desktop"),
                    ("page", "other"),
                    ("type", "hard"),
                    ("view_transition", "none"),
                ]),
                1
            ),
        ]
    );
    assert_eq!(
        h.histogram("browser.navigation.phase.duration"),
        ["fetch", "swap", "transition"]
            .iter()
            .map(|phase| (
                attrs(&[
                    ("device_class", "desktop"),
                    ("page", "/dashboard/settings"),
                    ("phase", phase),
                ]),
                1
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        h.counter("browser.navigation"),
        [
            (
                attrs(&[("outcome", "aborted"), ("page", "/dashboard/admin")]),
                1
            ),
            (
                attrs(&[("outcome", "completed"), ("page", "/dashboard/settings")]),
                1
            ),
        ]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn island_timings_are_recorded_and_made_up_panels_are_other(pool: PgPool) {
    let h = Harness::new(pool, 200);
    let (status, _) = h
        .post_json(
            &batch(
                "/dashboard/admin",
                &[
                    json!({ "type": "island", "name": "AdminPanel", "stage": "interactive",
                            "nav_type": "soft", "ms": 60.0 }),
                    json!({ "type": "island", "name": "AdminPanel", "stage": "ready",
                            "nav_type": "soft", "ms": 340.0 }),
                    json!({ "type": "island", "name": "MadeUpPanel", "stage": "ready",
                            "nav_type": "hard", "ms": 10.0 }),
                ],
            ),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let labels = |island: &str, stage: &str, nav_type: &str| {
        attrs(&[
            ("device_class", "desktop"),
            ("island", island),
            ("nav_type", nav_type),
            ("page", "/dashboard/admin"),
            ("stage", stage),
        ])
    };
    assert_eq!(
        h.histogram("browser.island.duration"),
        [
            (labels("AdminPanel", "interactive", "soft"), 1),
            (labels("AdminPanel", "ready", "soft"), 1),
            (labels("other", "ready", "hard"), 1),
        ]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_navigation_kinds_transitions_outcomes_and_stages_get_a_400(pool: PgPool) {
    let h = Harness::new(pool, 200);
    for event in [
        json!({ "type": "navigation", "nav_type": "reload", "view_transition": "ran", "outcome": "completed" }),
        json!({ "type": "navigation", "nav_type": "soft", "view_transition": "animated", "outcome": "completed" }),
        json!({ "type": "navigation", "nav_type": "soft", "view_transition": "ran", "outcome": "failed" }),
        json!({ "type": "island", "name": "HomePanel", "stage": "hydrated", "nav_type": "hard", "ms": 1.0 }),
    ] {
        let (status, _) = h
            .post_json(&batch("/dashboard", std::slice::from_ref(&event)), &[])
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{event}");
    }
    assert_eq!(h.histogram("browser.navigation.duration").len(), 0);
    assert_eq!(h.histogram("browser.island.duration").len(), 0);
    assert_eq!(h.counter("browser.navigation").len(), 0);
}
