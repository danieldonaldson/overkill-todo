//! Telemetry through the real router and database: `X-Trace-Id` correlation,
//! request → repository span nesting, and the end-to-end leak test that keeps
//! secrets and email addresses out of spans and logs.
//!
//! Each test installs the production subscriber stack (`telemetry::subscriber`)
//! as the thread-default, with an in-memory span exporter and a captured log
//! writer. `#[sqlx::test(migrations = "../../migrations")]` runs on a current-thread runtime, so everything the
//! request does is recorded by that subscriber.

mod common;

use std::fmt::Write as _;
use std::io::Write;
use std::sync::{Arc, Mutex};

use application::ports::EmailService;
use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode, header},
};
use domain::errors::DomainError;
use domain::value_objects::UserRole;
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SdkTracerProvider, SimpleSpanProcessor, SpanData,
};
use persistence_postgres::STATEMENT_LOG_DIRECTIVE;
use persistence_postgres::connection::with_statement_logging;
use persistence_postgres::testing;
use platform::telemetry::{self, LogFormat, ServiceInfo};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

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
            telemetry::targets_filter(
                server::APP_TARGETS,
                "debug",
                &["tower_http=debug", "sqlx=debug"],
            ),
            telemetry::export_filter(server::APP_TARGETS, &[STATEMENT_LOG_DIRECTIVE]),
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

/// The `#[sqlx::test(migrations = "../../migrations")]` database with production statement logging, so SQL
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

/// Inserts an account with `role` and returns its ID.
async fn insert_account(pool: &PgPool, role: &str) -> uuid::Uuid {
    let user_role = match role {
        "user" => UserRole::User,
        "admin" => UserRole::Admin,
        "super_admin" => UserRole::SuperAdmin,
        other => panic!("unknown role: {other}"),
    };
    let email = format!("telemetry-{}@example.com", uuid::Uuid::now_v7());
    testing::insert_verified_user(pool, &email, user_role).await
}

/// A Super Admin's role change to `admin` for `target`, with session and CSRF.
fn role_change_request(actor: uuid::Uuid, target: uuid::Uuid) -> Request<Body> {
    let token = common::mint_session_jwt(actor, true, "super_admin", 15);
    let csrf = common::mint_csrf_token(&actor.to_string());
    Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/admin/users/{target}/role"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("__Host-auth_token={token}; __Host-csrf_token={csrf}"),
        )
        .header("x-csrf-token", csrf)
        .body(Body::from(json!({ "role": "admin" }).to_string()))
        .unwrap()
}

/// OT-008 task 1.10: an audit event is stored with its request's trace ID, and
/// an event that can't be stored fails the action with a 5xx (which the 5xx
/// alert counts) and an error log line carrying that trace ID.
#[sqlx::test(migrations = "../../migrations")]
async fn audit_events_carry_the_trace_id_and_insert_failures_are_logged(pool: PgPool) {
    let telemetry = Captured::install();
    let app = common::create_test_app(pool.clone());
    let actor = insert_account(&pool, "super_admin").await;

    let (resp, _) = send(
        &app,
        role_change_request(actor, insert_account(&pool, "user").await),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let stored = testing::latest_audit_event_trace_id(&pool).await;
    assert_eq!(stored, Some(trace_id_header(&resp)));

    testing::refuse_role_changed_audit_events(&pool).await;
    let (resp, body) = send(
        &app,
        role_change_request(actor, insert_account(&pool, "user").await),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let trace_id = trace_id_header(&resp);

    let logs = telemetry.log_text();
    let failure = logs
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|l| {
            l["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("audit event insert failed"))
        })
        .unwrap_or_else(|| panic!("no audit failure log line:\n{logs}"));
    assert_eq!(failure["level"], "ERROR");
    assert_eq!(failure["trace_id"], trace_id.as_str());
    assert_eq!(failure["audit.kind"], "role_changed");
}

/// The `X-Trace-Id` header is the trace ID of the request's exported server
/// span, and log lines written during the request carry the same ID.
#[sqlx::test(migrations = "../../migrations")]
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
#[sqlx::test(migrations = "../../migrations")]
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
#[sqlx::test(migrations = "../../migrations")]
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
#[sqlx::test(migrations = "../../migrations")]
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
    let auth_token = common::cookie_value(&resp, "__Host-auth_token").expect("auth cookie");
    let refresh_token =
        common::cookie_value(&resp, "__Host-refresh_token").expect("refresh cookie");
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    secrets.extend([auth_token.clone(), refresh_token.clone(), csrf.clone()]);

    // Refresh the session (token arrives as a cookie, plus the CSRF header).
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/refresh")
        .header(
            header::COOKIE,
            format!(
                "__Host-auth_token={auth_token}; __Host-refresh_token={refresh_token}; \
                 __Host-csrf_token={csrf}"
            ),
        )
        .header("x-csrf-token", &csrf)
        .header("x-device-fingerprint", FINGERPRINT)
        .body(Body::empty())
        .unwrap();
    let (resp, body) = send(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK, "refresh failed: {body}");
    if let Some(rotated) = common::cookie_value(&resp, "__Host-refresh_token") {
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

/// Lists and todos: names and titles never reach traces or logs, on success,
/// refusal (invalid or stale) or a failed save (OT-013 design D9).
#[sqlx::test(migrations = "../../migrations")]
#[allow(
    clippy::too_many_lines,
    reason = "one flow over every new route; splitting hides which step leaked"
)]
async fn list_and_todo_content_never_reaches_telemetry(pool: PgPool) {
    let telemetry = Captured::install();
    let app = common::create_test_app(production_pool(&pool).await);
    let email = "test-telemetry-lists@example.com";
    let marker = uuid::Uuid::now_v7();
    let list_name = format!("LIST-NAME-{marker}");
    let todo_title = format!("TODO-TITLE-{marker}");
    let second_title = format!("SECOND-TITLE-{marker}");

    let (resp, _) = send(
        &app,
        post_json("/api/v1/auth/register", &json!({ "email": email })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let signup_token = body["signup_token"].as_str().unwrap().to_string();
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "hunter2hunter2" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let auth_token = common::cookie_value(&resp, "__Host-auth_token").expect("auth cookie");
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    let cookies = format!("__Host-auth_token={auth_token}; __Host-csrf_token={csrf}");

    let authed = |method: &str, uri: &str, body: Option<Value>| {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, &cookies)
            .header("x-csrf-token", &csrf);
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        builder
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .unwrap()
    };

    let (resp, list) = send(
        &app,
        authed(
            "POST",
            "/api/v1/lists",
            Some(json!({ "key": uuid::Uuid::now_v7(), "name": list_name })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let list_id = list["id"].as_str().unwrap().to_string();

    let (resp, todo) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": todo_title })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let todo_id = todo["id"].as_str().unwrap().to_string();

    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
            Some(json!({ "version": 1, "done": true })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Refused: too long, a control character, and a stale version with content.
    let (resp, _) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": second_title })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (resp, _) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": "bad\nLIST-CONTENT-REFUSED" })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
            Some(json!({ "version": 1, "done": false })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // Moves: refused for a done todo, and for too many neighbours.
    let neighbours: Vec<_> = (0..21).map(|_| uuid::Uuid::now_v7()).collect();
    for (above, status) in [
        (&neighbours[..1], StatusCode::CONFLICT),
        (&neighbours[..], StatusCode::BAD_REQUEST),
    ] {
        let (resp, _) = send(
            &app,
            authed(
                "POST",
                &format!("/api/v1/lists/{list_id}/todos/{todo_id}/move"),
                Some(json!({ "above": above, "below": [] })),
            ),
        )
        .await;
        assert_eq!(resp.status(), status);
    }

    // Renames and deletes, accepted and refused.
    let renamed_list_name = format!("LIST-RENAMED-{marker}");
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            Some(json!({ "version": 1, "name": renamed_list_name })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            Some(json!({ "version": 1, "name": format!("LIST-CONFLICT-{marker}") })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            Some(json!({ "version": 2, "name": format!("LIST-INVALID-{marker}\nmore") })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let renamed_todo_title = format!("TODO-RENAMED-{marker}");
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
            Some(json!({ "version": 2, "title": renamed_todo_title })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
            Some(json!({ "version": 1, "title": format!("TODO-CONFLICT-{marker}") })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}?version=99"),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let deleted_list_name = format!("LIST-DELETED-{marker}");
    let (resp, doomed_list) = send(
        &app,
        authed(
            "POST",
            "/api/v1/lists",
            Some(json!({ "key": uuid::Uuid::now_v7(), "name": deleted_list_name })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let doomed_list_id = doomed_list["id"].as_str().unwrap().to_string();
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/lists/{doomed_list_id}?version=1&todo_count=0"),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}?version=2&todo_count=99"),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let deleted_todo_title = format!("TODO-DELETED-{marker}");
    let (resp, doomed_todo) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": deleted_todo_title })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let doomed_todo_id = doomed_todo["id"].as_str().unwrap().to_string();
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}/todos/{doomed_todo_id}?version=1"),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let (resp, _) = send(
        &app,
        authed("GET", &format!("/api/v1/lists/{list_id}"), None),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let (resp, export) = send(&app, authed("GET", "/api/v1/users/me/export", None)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(export["lists"][0]["todos"].as_array().unwrap().len() >= 2);

    // A failed save: the table is gone, so the repository errors with content
    // in the request body only.
    testing::drop_todos_table(&pool).await;
    let (resp, _) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": "FAILED-SAVE-TITLE" })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let failed_save_name = format!("LIST-FAILED-SAVE-{marker}");
    let (resp, _) = send(
        &app,
        authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            Some(json!({ "version": 2, "name": failed_save_name })),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}?version=2&todo_count=2"),
            None,
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let everything = telemetry.everything();
    assert_ne!(everything.len(), 0, "nothing was captured");
    for marker in [
        list_name.as_str(),
        todo_title.as_str(),
        second_title.as_str(),
        renamed_list_name.as_str(),
        renamed_todo_title.as_str(),
        deleted_list_name.as_str(),
        deleted_todo_title.as_str(),
        failed_save_name.as_str(),
        "LIST-CONTENT-REFUSED",
        "LIST-CONFLICT",
        "LIST-INVALID",
        "TODO-CONFLICT",
        "FAILED-SAVE-TITLE",
    ] {
        assert!(
            !everything.contains(marker),
            "telemetry contains content: {marker}"
        );
    }
}
