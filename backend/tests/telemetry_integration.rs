//! Telemetry through the real router and database: `X-Trace-Id` correlation,
//! request → repository span nesting, and the end-to-end leak test that keeps
//! secrets and email addresses out of spans and logs.
//!
//! Each test installs the production subscriber stack (`telemetry::subscriber`)
//! as the thread-default, with an in-memory span exporter and a captured log
//! writer. `#[sqlx::test]` runs on a current-thread runtime, so everything the
//! request does is recorded by that subscriber.

mod common;

use std::fmt::Write as _;
use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode, header},
};
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor, SpanData,
};
use overkill_todo::{
    application::ports::EmailService,
    domain::errors::DomainError,
    infrastructure::{
        persistence::postgres::connection::with_statement_logging,
        telemetry::{self, LogFormat, ServiceInfo},
    },
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// Capture harness
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

struct Captured {
    exporter: InMemorySpanExporter,
    provider: SdkTracerProvider,
    logs: LogCapture,
    _guard: tracing::subscriber::DefaultGuard,
}

impl Captured {
    /// Production stack, with debug-level logs so the leak test sees the most.
    fn install() -> Self {
        let service = ServiceInfo {
            name: "overkill-todo-backend".into(),
            version: "test".into(),
            environment: "test".into(),
        };
        let exporter = InMemorySpanExporter::default();
        let provider =
            telemetry::tracer_provider(&service, Some(SimpleSpanProcessor::new(exporter.clone())));
        let logs = LogCapture::default();
        let subscriber = telemetry::subscriber(
            &provider,
            EnvFilter::new("overkill_todo=debug,tower_http=debug,sqlx=debug"),
            LogFormat::Json,
            logs.clone(),
            service,
        );
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            exporter,
            provider,
            logs,
            _guard: guard,
        }
    }

    fn spans(&self) -> Vec<SpanData> {
        self.provider.force_flush().unwrap();
        self.exporter.get_finished_spans().unwrap()
    }

    fn log_text(&self) -> String {
        String::from_utf8(self.logs.0.lock().unwrap().clone()).unwrap()
    }

    /// Every span (name, attributes, events, status) and every log line, as one
    /// searchable string.
    fn everything(&self) -> String {
        let mut out = self.log_text();
        for span in self.spans() {
            let _ = write!(
                out,
                "\n{} {:?} {:?} {:?}",
                span.name, span.attributes, span.events.events, span.status
            );
        }
        out
    }
}

/// The `#[sqlx::test]` database with production statement logging, so SQL
/// text reaches the spans exactly as it does in a deployed backend.
async fn production_pool(pool: &PgPool) -> PgPool {
    let options = with_statement_logging((*pool.connect_options()).clone());
    PgPoolOptions::new().connect_with(options).await.unwrap()
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

const FINGERPRINT: &str = "test-device-abc123";

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", FINGERPRINT)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (Response<Body>, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (Response::from_parts(parts, Body::empty()), json)
}

fn trace_id_header(resp: &Response<Body>) -> String {
    resp.headers()
        .get("x-trace-id")
        .expect("X-Trace-Id header")
        .to_str()
        .unwrap()
        .to_string()
}

fn server_span<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans.iter().find(|s| s.name == name).unwrap_or_else(|| {
        panic!(
            "no span named {name:?}; got {:?}",
            spans.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
        )
    })
}

/// Email service that records what it would send, without logging anything.
#[derive(Default)]
struct CapturingEmail {
    magic_links: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl EmailService for CapturingEmail {
    async fn send_verification_email(
        &self,
        _to: &str,
        _code: &str,
        _first_name: &str,
        magic_link: Option<String>,
    ) -> Result<(), DomainError> {
        if let Some(link) = magic_link {
            self.magic_links.lock().unwrap().push(link);
        }
        Ok(())
    }
    async fn send_password_reset_email(
        &self,
        _to: &str,
        _code: &str,
        _first_name: &str,
    ) -> Result<(), DomainError> {
        Ok(())
    }
    async fn notify_account_exists(&self, _to: &str, _first_name: &str) -> Result<(), DomainError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// X-Trace-Id correlation
// ---------------------------------------------------------------------------

/// The `X-Trace-Id` header is the trace ID of the request's exported server
/// span, and log lines written during the request carry the same ID.
#[sqlx::test(migrations = "./migrations")]
async fn x_trace_id_matches_exported_trace_and_logs(pool: PgPool) {
    let telemetry = Captured::install();
    let app = common::create_test_app(pool);

    let (resp, _) = send(
        &app,
        post_json(
            "/api/v1/auth/login",
            &json!({ "email": "nobody@example.com", "password": "wrong-password-1" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let trace_id = trace_id_header(&resp);
    assert_eq!(trace_id.len(), 32);
    assert!(
        trace_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );

    let spans = telemetry.spans();
    let server = server_span(&spans, "POST /api/v1/auth/login");
    assert_eq!(server.span_context.trace_id().to_string(), trace_id);

    let logs = telemetry.log_text();
    let correlated = logs
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|l| l["trace_id"] == trace_id.as_str())
        .count();
    assert!(
        correlated > 0,
        "no log line carried the request's trace id:\n{logs}"
    );
}

/// The backend continues the trace the edge started: `X-Trace-Id` and every
/// span carry the `traceparent`'s trace ID. Clients can't use this to pick a
/// trace ID, because the edge strips their `traceparent` before starting its own
/// span. That half is checked on the full stack (tasks 3.2, 3.8).
#[sqlx::test(migrations = "./migrations")]
async fn edge_traceparent_is_continued(pool: PgPool) {
    let telemetry = Captured::install();
    let app = common::create_test_app(pool);
    let edge_trace = "4bf92f3577b34da6a3ce929d0e0e4736";
    let edge_span = "00f067aa0ba902b7";

    let req = Request::builder()
        .uri("/api/v1/auth/me")
        .header("traceparent", format!("00-{edge_trace}-{edge_span}-01"))
        .body(Body::empty())
        .unwrap();
    let (resp, _) = send(&app, req).await;

    assert_eq!(trace_id_header(&resp), edge_trace);
    let spans = telemetry.spans();
    for span in &spans {
        assert_eq!(span.span_context.trace_id().to_string(), edge_trace);
    }
    let server = server_span(&spans, "GET /api/v1/auth/me");
    assert_eq!(server.parent_span_id.to_string(), edge_span);
    assert!(server.parent_span_is_remote);
}

// ---------------------------------------------------------------------------
// Database calls nest under the request
// ---------------------------------------------------------------------------

/// A sign-in records at least one `repo.*` span inside the request's trace,
/// descending from the server span.
#[sqlx::test(migrations = "./migrations")]
async fn login_trace_contains_repository_spans(pool: PgPool) {
    let telemetry = Captured::install();
    let app = common::create_test_app(production_pool(&pool).await);

    let (resp, _) = send(
        &app,
        post_json(
            "/api/v1/auth/login",
            &json!({ "email": "nobody@example.com", "password": "wrong-password-1" }),
        ),
    )
    .await;
    let trace_id = trace_id_header(&resp);

    let spans = telemetry.spans();
    let server = server_span(&spans, "POST /api/v1/auth/login");
    let in_trace: Vec<_> = spans
        .iter()
        .filter(|s| s.span_context.trace_id().to_string() == trace_id)
        .collect();
    let repo = in_trace
        .iter()
        .find(|s| s.name.starts_with("repo."))
        .expect("a repo.* span in the login trace");
    assert!(
        repo.attributes
            .iter()
            .any(|kv| kv.key.as_str() == "db.system" && kv.value.as_str() == "postgresql")
    );

    // Walk parents up to the server span.
    let mut parent = repo.parent_span_id;
    let mut reached_server = false;
    for _ in 0..in_trace.len() {
        if parent == server.span_context.span_id() {
            reached_server = true;
            break;
        }
        match in_trace.iter().find(|s| s.span_context.span_id() == parent) {
            Some(p) => parent = p.parent_span_id,
            None => break,
        }
    }
    assert!(
        reached_server,
        "repo span does not descend from the server span"
    );
    // Each statement's SQL text is an event on its repository span.
    assert!(
        in_trace
            .iter()
            .filter(|s| s.name.starts_with("repo."))
            .flat_map(|s| s.events.events.iter())
            .any(|e| e
                .attributes
                .iter()
                .any(|kv| kv.key.as_str() == "db.statement")),
        "no db.statement event on any repo.* span"
    );
}

// ---------------------------------------------------------------------------
// Leak test
// ---------------------------------------------------------------------------

/// Register → verify by code → set password → sign in → refresh → forgot /
/// reset password → magic link: no telemetry contains the email address, the
/// passwords, the codes, the tokens, the CSRF token or any cookie value.
#[sqlx::test(migrations = "./migrations")]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end flow; splitting it would hide which step leaked"
)]
async fn auth_flows_leave_no_secrets_in_telemetry(pool: PgPool) {
    let telemetry = Captured::install();
    let email_service = Arc::new(CapturingEmail::default());
    let app = common::create_test_app_with_email_service(
        production_pool(&pool).await,
        email_service.clone(),
    );

    // Test-pattern emails get the fixed dev code (see `RandomCodeGenerator`).
    let email = "test-leakcheck@example.com";
    let password = "Leak-Check-Password-77";
    let new_password = "Leak-Check-Reset-88";
    let code = "123456";
    let mut secrets: Vec<String> = vec![
        email.into(),
        "test-leakcheck".into(),
        password.into(),
        new_password.into(),
    ];

    // Register (sends the verification email, with a magic link).
    let (resp, _) = send(
        &app,
        post_json("/api/v1/auth/register", &json!({ "email": email })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify by code.
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": code }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let signup_token = body["signup_token"].as_str().unwrap().to_string();
    secrets.push(signup_token.clone());

    // Set the password.
    let (resp, _) = send(
        &app,
        post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": password }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Sign in.
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": password }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let auth_token = common::cookie_value(&resp, "auth_token").expect("auth cookie");
    let refresh_token = common::cookie_value(&resp, "refresh_token").expect("refresh cookie");
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    secrets.extend([auth_token.clone(), refresh_token.clone(), csrf.clone()]);

    // Refresh the session (token arrives as a cookie, plus the CSRF header).
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/refresh")
        .header(
            header::COOKIE,
            format!("refresh_token={refresh_token}; csrf_token={csrf}"),
        )
        .header("x-csrf-token", &csrf)
        .header("x-device-fingerprint", FINGERPRINT)
        .header(header::AUTHORIZATION, format!("Bearer {auth_token}"))
        .body(Body::empty())
        .unwrap();
    let (resp, body) = send(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK, "refresh failed: {body}");
    if let Some(rotated) = common::cookie_value(&resp, "refresh_token") {
        secrets.push(rotated);
    }
    if let Some(t) = body["csrf_token"].as_str() {
        secrets.push(t.into());
    }

    // Forgot / verify reset code / reset password.
    let (resp, _) = send(
        &app,
        post_json("/api/v1/auth/forgot-password", &json!({ "email": email })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": code }),
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "verify-reset-code failed: {body}"
    );
    let reset_token = body["reset_token"].as_str().unwrap().to_string();
    secrets.push(reset_token.clone());
    let (resp, _) = send(
        &app,
        post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": reset_token, "new_password": new_password }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Magic link: register a second account and follow its link.
    let magic_email = "test-leakmagic@example.com";
    secrets.push(magic_email.into());
    let (resp, _) = send(
        &app,
        post_json("/api/v1/auth/register", &json!({ "email": magic_email })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let link = email_service
        .magic_links
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("a magic link was sent");
    let magic_token = link.split("token=").nth(1).unwrap().to_string();
    secrets.push(magic_token.clone());
    // Legacy GET link (token in the query string), then the POST confirmation.
    let req = Request::builder()
        .uri(format!("/api/v1/auth/verify-magic?token={magic_token}"))
        .body(Body::empty())
        .unwrap();
    let (resp, _) = send(&app, req).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-magic",
            &json!({ "token": magic_token }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    if let Some(signup_token) = body.get("signup_token").and_then(|t| t.as_str()) {
        secrets.push(signup_token.to_string());
    }

    // The code is too short to search for safely on its own (it could appear in
    // a timestamp), so look for it only as a field value.
    let everything = telemetry.everything();
    assert_ne!(everything.len(), 0, "nothing was captured");
    for secret in &secrets {
        assert!(
            !everything.contains(secret.as_str()),
            "telemetry contains a secret ({}…):\n{everything}",
            &secret[..secret.len().min(12)]
        );
    }
    for needle in [
        format!("\"{code}\""),
        format!("={code}"),
        format!("String(\"{code}\")"),
    ] {
        assert!(
            !everything.contains(&needle),
            "telemetry contains the code: {needle}"
        );
    }
}
