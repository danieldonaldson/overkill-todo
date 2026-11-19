//! `POST /api/v1/client-telemetry` through the real router (OT-006 slice 1):
//! attribution from the session only, the standard 400 and the 413, the
//! recording budget, no CSRF, and no trace while the request is still counted.
//!
//! Each test installs the production subscriber stack as the thread default,
//! with an in-memory span exporter and a captured JSON log writer, and builds
//! the app with the real `TelemetryClientReportSink` on an in-memory meter.
//! `#[sqlx::test]` runs on a current-thread runtime, so the subscriber sees
//! everything the request does.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

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
use overkill_todo::infrastructure::api::cookies::AUTH_COOKIE;
use overkill_todo::infrastructure::telemetry::{
    self, LogFormat, ServiceInfo, client_reports::TelemetryClientReportSink, metrics::METER_NAME,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;
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
            EnvFilter::new("overkill_todo=debug,tower_http=debug"),
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

    /// Sum of the `browser.report.dropped` counter.
    fn dropped(&self) -> u64 {
        self.meter.force_flush().unwrap();
        let batches = self.metrics.get_finished_metrics().unwrap();
        batches
            .iter()
            .rev()
            .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
            .find(|m| m.name() == "browser.report.dropped")
            .map_or(0, |m| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = m.data() else {
                    panic!("browser.report.dropped is not a u64 sum");
                };
                sum.data_points().map(SumDataPoint::value).sum()
            })
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
async fn reports_over_the_budget_get_a_204_and_are_counted_not_recorded(pool: PgPool) {
    let h = Harness::new(pool, 2);
    let events: Vec<Value> = (0..5).map(|i| error_event(&format!("e{i}"))).collect();
    let (status, body) = h.post_json(&batch("/login", &events), &[]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, "");
    assert_eq!(h.client_errors().len(), 2);
    assert_eq!(h.dropped(), 3);
}

#[sqlx::test(migrations = "./migrations")]
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
