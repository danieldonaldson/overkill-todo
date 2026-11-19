//! Authentication outcome metrics (OT-007 slice 1), read back through the
//! SDK's in-memory exporter while real sign-in and code flows run over the
//! router and Postgres.
//!
//! Also pins the `verify-reset-code` responses for each outcome, so typing the
//! use case's errors (D2) can't change what clients see.

mod common;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData, ScopeMetrics},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor};
use overkill_todo::infrastructure::telemetry::{
    self, LogFormat, ServiceInfo,
    metrics::{AuthMetrics, METER_NAME},
};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

/// The code the dev code generator gives `e2e-*` addresses.
const DEV_CODE: &str = "123456";
const WRONG_CODE: &str = "000000";
const PASSWORD: &str = "correct-horse-1";

/// Log lines written under the production subscriber stack (JSON, with
/// `trace_id` from the request span), captured in memory.
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

impl LogCapture {
    /// Install the production stack as the thread default. `#[sqlx::test]`
    /// runs on a current-thread runtime, so every request is captured.
    fn install() -> (Self, tracing::subscriber::DefaultGuard) {
        let service = ServiceInfo {
            name: "overkill-todo-backend".into(),
            version: "test".into(),
            environment: "test".into(),
        };
        let provider = telemetry::tracer_provider(
            &service,
            Some(SimpleSpanProcessor::new(InMemorySpanExporter::default())),
        );
        let logs = Self::default();
        let subscriber = telemetry::subscriber(
            &provider,
            EnvFilter::new("overkill_todo=debug"),
            LogFormat::Json,
            logs.clone(),
            service,
        );
        (logs, tracing::subscriber::set_default(subscriber))
    }

    /// Captured lines that carry an `event` field (the investigation events).
    fn events(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|l| l.get("event").is_some())
            .collect()
    }
}

/// An app recording auth metrics on an in-memory exporter.
struct Harness {
    app: Router,
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

impl Harness {
    fn new(pool: PgPool) -> Self {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let metrics = AuthMetrics::new(&provider.meter(METER_NAME));
        Self {
            app: common::create_test_app_with_auth_metrics(pool, metrics),
            provider,
            exporter,
        }
    }

    async fn post(&self, uri: &str, body: &Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-device-fingerprint", "test-device-abc123")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn register(&self, email: &str) {
        let (status, body) = self
            .post("/api/v1/auth/register", &json!({ "email": email }))
            .await;
        assert_eq!(status, StatusCode::OK, "register {email}: {body}");
    }

    async fn verify_email(&self, email: &str, code: &str) -> (StatusCode, Value) {
        self.post(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": code }),
        )
        .await
    }

    /// Register, verify and set a password, leaving a Member who can sign in.
    async fn sign_up(&self, email: &str) {
        self.register(email).await;
        let (status, body) = self.verify_email(email, DEV_CODE).await;
        assert_eq!(status, StatusCode::OK, "verify {email}: {body}");
        let token = body["signup_token"].as_str().unwrap().to_owned();
        let (status, body) = self
            .post(
                "/api/v1/auth/complete-signup",
                &json!({ "signup_token": token, "email": email, "password": PASSWORD }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "complete-signup {email}: {body}");
    }

    async fn login(&self, email: &str, password: &str) -> StatusCode {
        self.post(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": password }),
        )
        .await
        .0
    }

    async fn forgot_password(&self, email: &str) {
        let (status, body) = self
            .post("/api/v1/auth/forgot-password", &json!({ "email": email }))
            .await;
        assert_eq!(status, StatusCode::OK, "forgot-password {email}: {body}");
    }

    async fn verify_reset_code(&self, email: &str, code: &str) -> (StatusCode, Value) {
        self.post(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": code }),
        )
        .await
    }

    /// `(sorted labels, value)` for each point of the counter `name`.
    fn points(&self, name: &str) -> Vec<(Vec<(String, String)>, u64)> {
        self.provider.force_flush().unwrap();
        let batches = self.exporter.get_finished_metrics().unwrap();
        let Some(metric) = batches
            .iter()
            .rev()
            .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
            .find(|m| m.name() == name)
        else {
            return Vec::new();
        };
        let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
            panic!("{name} is not a u64 counter");
        };
        let mut points: Vec<_> = sum
            .data_points()
            .map(|dp| {
                let mut labels: Vec<_> = dp
                    .attributes()
                    .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                    .collect();
                labels.sort();
                (labels, dp.value())
            })
            .collect();
        points.sort();
        points
    }

    /// The `auth.attempts` count for `flow` and `outcome` (0 if never recorded).
    fn attempts(&self, flow: &str, outcome: &str) -> u64 {
        let want = labels(&[("flow", flow), ("outcome", outcome)]);
        self.points("auth.attempts")
            .into_iter()
            .find(|(l, _)| *l == want)
            .map_or(0, |(_, v)| v)
    }

    /// The `auth.code.lockouts` count for `flow` (0 if never recorded).
    fn lockouts(&self, flow: &str) -> u64 {
        let want = labels(&[("flow", flow)]);
        self.points("auth.code.lockouts")
            .into_iter()
            .find(|(l, _)| *l == want)
            .map_or(0, |(_, v)| v)
    }
}

fn labels(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

async fn expire_reset_code(pool: &PgPool, email: &str) {
    sqlx::query(
        "UPDATE password_reset_codes SET expires_at = NOW() - INTERVAL '1 hour' \
         WHERE user_id = (SELECT id FROM users WHERE email = $1)",
    )
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
}

async fn expire_email_code(pool: &PgPool, email: &str) {
    sqlx::query(
        "UPDATE email_verification_codes SET expires_at = NOW() - INTERVAL '1 hour' \
         WHERE user_id = (SELECT id FROM users WHERE email = $1)",
    )
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// Password sign-in
// ---------------------------------------------------------------------------

/// Wrong password and unknown email are both one failure each; a correct
/// password is a success. Responses are the usual 401 / 200.
#[sqlx::test(migrations = "./migrations")]
async fn sign_in_outcomes_are_counted(pool: PgPool) {
    let h = Harness::new(pool);
    let email = "e2e-signin@example.com";
    h.sign_up(email).await;

    assert_eq!(
        h.login(email, "wrong-password-1").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(h.attempts("password_sign_in", "failure"), 1);

    assert_eq!(
        h.login("e2e-nobody@example.com", "wrong-password-1").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.attempts("password_sign_in", "failure"),
        2,
        "an unknown email counts the same as a wrong password"
    );

    assert_eq!(h.login(email, PASSWORD).await, StatusCode::OK);
    assert_eq!(h.attempts("password_sign_in", "success"), 1);
    assert_eq!(h.attempts("password_sign_in", "failure"), 2);
}

// ---------------------------------------------------------------------------
// Email verification code
// ---------------------------------------------------------------------------

/// 5 wrong codes then 3 more while locked: 5 failures, 1 lockout, 3 locked.
#[sqlx::test(migrations = "./migrations")]
async fn email_code_lockout_is_counted_once(pool: PgPool) {
    let h = Harness::new(pool);
    let email = "e2e-lockout@example.com";
    h.register(email).await;

    for remaining in (0..5).rev() {
        let (status, body) = h.verify_email(email, WRONG_CODE).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error_code"], "CODE_INVALID");
        assert_eq!(body["attempts_remaining"], remaining);
    }
    for _ in 0..3 {
        let (status, body) = h.verify_email(email, WRONG_CODE).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error_code"], "CODE_LOCKED");
    }

    assert_eq!(h.attempts("email_code", "failure"), 5);
    assert_eq!(h.lockouts("email_code"), 1);
    assert_eq!(h.attempts("email_code", "locked"), 3);
    assert_eq!(h.lockouts("reset_code"), 0);
}

/// An expired email code is counted as expired, a correct one as a success;
/// unknown and already verified emails aren't guesses and aren't counted.
#[sqlx::test(migrations = "./migrations")]
async fn email_code_expired_and_success_are_counted(pool: PgPool) {
    let h = Harness::new(pool.clone());
    let expired = "e2e-expired@example.com";
    h.register(expired).await;
    expire_email_code(&pool, expired).await;
    let (status, body) = h.verify_email(expired, DEV_CODE).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "CODE_EXPIRED");

    let ok = "e2e-verified@example.com";
    h.register(ok).await;
    assert_eq!(h.verify_email(ok, DEV_CODE).await.0, StatusCode::OK);
    // Already verified, and unknown: not counted.
    assert_eq!(
        h.verify_email(ok, DEV_CODE).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.verify_email("e2e-nobody@example.com", DEV_CODE).await.0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        h.points("auth.attempts"),
        vec![
            (labels(&[("flow", "email_code"), ("outcome", "expired")]), 1),
            (labels(&[("flow", "email_code"), ("outcome", "success")]), 1),
        ]
    );
}

// ---------------------------------------------------------------------------
// Password reset code
// ---------------------------------------------------------------------------

/// Every `verify-reset-code` outcome keeps its pre-OT-007 status and body, and
/// is counted under the reset-code flow: 5 failures, 1 lockout, then locked;
/// expired as expired; no code not at all.
#[sqlx::test(migrations = "./migrations")]
async fn reset_code_outcomes_keep_their_responses_and_are_counted(pool: PgPool) {
    let h = Harness::new(pool.clone());
    let email = "e2e-reset@example.com";
    h.sign_up(email).await;

    let no_code = json!({
        "code": "validation_error",
        "message": "No reset code found. Please request a new one."
    });
    // Unknown email and a Member with no pending code: same answer, not counted.
    assert_eq!(
        h.verify_reset_code("e2e-nobody@example.com", DEV_CODE)
            .await,
        (StatusCode::BAD_REQUEST, no_code.clone())
    );
    assert_eq!(
        h.verify_reset_code(email, DEV_CODE).await,
        (StatusCode::BAD_REQUEST, no_code)
    );
    for outcome in ["success", "failure", "locked", "expired"] {
        assert_eq!(h.attempts("reset_code", outcome), 0, "{outcome}");
    }

    h.forgot_password(email).await;
    for _ in 0..5 {
        assert_eq!(
            h.verify_reset_code(email, WRONG_CODE).await,
            (
                StatusCode::BAD_REQUEST,
                json!({ "code": "validation_error", "message": "Invalid reset code." })
            )
        );
    }
    assert_eq!(
        h.verify_reset_code(email, DEV_CODE).await,
        (
            StatusCode::BAD_REQUEST,
            json!({
                "code": "validation_error",
                "message": "Too many failed attempts. Please request a new code."
            })
        )
    );
    assert_eq!(h.attempts("reset_code", "failure"), 5);
    assert_eq!(h.lockouts("reset_code"), 1);
    assert_eq!(h.attempts("reset_code", "locked"), 1);
    assert_eq!(h.lockouts("email_code"), 0);

    expire_reset_code(&pool, email).await;
    assert_eq!(
        h.verify_reset_code(email, DEV_CODE).await,
        (
            StatusCode::BAD_REQUEST,
            json!({
                "code": "validation_error",
                "message": "Reset code has expired. Please request a new one."
            })
        )
    );
    assert_eq!(h.attempts("reset_code", "expired"), 1);
    assert_eq!(
        h.attempts("reset_code", "failure"),
        5,
        "expired is not a guess"
    );
}

/// A correct reset code is a success and still returns a reset token.
#[sqlx::test(migrations = "./migrations")]
async fn reset_code_success_is_counted(pool: PgPool) {
    let h = Harness::new(pool);
    let email = "e2e-reset-ok@example.com";
    h.sign_up(email).await;
    h.forgot_password(email).await;

    let (status, body) = h.verify_reset_code(email, DEV_CODE).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["reset_token"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(h.attempts("reset_code", "success"), 1);
    for outcome in ["failure", "locked", "expired"] {
        assert_eq!(h.attempts("reset_code", outcome), 0, "{outcome}");
    }
}

// ---------------------------------------------------------------------------
// No personal data
// ---------------------------------------------------------------------------

/// After sign-ins and code checks by several Members, no label value on any
/// auth measurement holds an email, user ID, code or token, and the series
/// are the fixed flow/outcome combinations only.
#[sqlx::test(migrations = "./migrations")]
async fn measurements_hold_no_personal_data(pool: PgPool) {
    let (logs, _guard) = LogCapture::install();
    let h = Harness::new(pool.clone());
    let mut user_ids: Vec<String> = Vec::new();
    let mut secrets: Vec<String> = vec![
        DEV_CODE.into(),
        WRONG_CODE.into(),
        PASSWORD.into(),
        "wrong-password-1".into(),
    ];

    for i in 0..4 {
        let email = format!("e2e-member{i}@example.com");
        h.sign_up(&email).await;
        h.login(&email, PASSWORD).await;
        h.login(&email, "wrong-password-1").await;
        h.forgot_password(&email).await;
        h.verify_reset_code(&email, WRONG_CODE).await;
        let (_, body) = h.verify_reset_code(&email, DEV_CODE).await;
        if let Some(token) = body["reset_token"].as_str() {
            secrets.push(token.to_owned());
        }
        let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
            .bind(&email)
            .fetch_one(&pool)
            .await
            .unwrap();
        secrets.push(id.to_string());
        user_ids.push(id.to_string());
        secrets.push(email);
    }
    for i in 0..3 {
        let email = format!("e2e-pending{i}@example.com");
        h.register(&email).await;
        for _ in 0..6 {
            h.verify_email(&email, WRONG_CODE).await;
        }
        secrets.push(email);
    }

    let attempts = h.points("auth.attempts");
    let lockouts = h.points("auth.code.lockouts");
    for (labels, _) in attempts.iter().chain(&lockouts) {
        for (key, value) in labels {
            assert!(
                key == "flow" || key == "outcome",
                "unexpected label {key}={value}"
            );
            for secret in &secrets {
                assert!(
                    !value.contains(secret.as_str()),
                    "label {key}={value} contains {secret}"
                );
            }
        }
    }

    // 7 Members, but only the fixed combinations.
    assert_eq!(
        attempts.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(),
        vec![
            labels(&[("flow", "email_code"), ("outcome", "failure")]),
            labels(&[("flow", "email_code"), ("outcome", "locked")]),
            labels(&[("flow", "email_code"), ("outcome", "success")]),
            labels(&[("flow", "password_sign_in"), ("outcome", "failure")]),
            labels(&[("flow", "password_sign_in"), ("outcome", "success")]),
            labels(&[("flow", "reset_code"), ("outcome", "failure")]),
            labels(&[("flow", "reset_code"), ("outcome", "success")]),
        ]
    );
    assert_eq!(lockouts, vec![(labels(&[("flow", "email_code")]), 3)]);

    // The investigation events from the same runs (OT-007 D7): they may name
    // the account by user ID, but never hold an email, code, token or
    // password, and each links to its request trace.
    let events = logs.events();
    let count = |name: &str| events.iter().filter(|e| e["event"] == name).count();
    assert_eq!(count("auth.sign_in.failed"), 4);
    assert_eq!(count("auth.code.locked"), 3);
    assert_eq!(count("auth.code.refused_locked"), 3);
    for event in &events {
        let trace_id = event["trace_id"].as_str().unwrap_or_default();
        assert_eq!(trace_id.len(), 32, "no trace_id on {event}");
        // Timestamps and IDs are random digits that could contain a code by
        // chance; everything else is searched.
        let mut fields = event.as_object().unwrap().clone();
        for key in ["timestamp", "trace_id", "span_id"] {
            fields.remove(key);
        }
        let text = Value::Object(fields).to_string();
        for secret in secrets.iter().filter(|s| !user_ids.contains(s)) {
            assert!(!text.contains(secret.as_str()), "{text} contains {secret}");
        }
    }
}
