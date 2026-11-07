use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

/// Liveness probe — always returns 200 if the process is running.
#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    responses(
        (status = 200, description = "Service is running", body = Value)
    )
)]
#[tracing::instrument(name = "health_check")]
pub async fn health_check() -> Json<Value> {
    // Intentionally omits the build version: this endpoint is unauthenticated, so
    // disclosing the exact version only helps an attacker fingerprint the build
    // for CVE matching. The container self-healthcheck only asserts the 200.
    Json(json!({
        "status": "ok",
        "service": "app"
    }))
}

/// Readiness probe — checks DB connectivity before returning 200.
#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "Health",
    responses(
        (status = 200, description = "Service is ready", body = Value),
        (status = 503, description = "Service is not ready", body = Value)
    )
)]
#[tracing::instrument(name = "health_ready", skip(pool))]
pub async fn health_ready(State(pool): State<PgPool>) -> (StatusCode, Json<Value>) {
    match sqlx::query!("SELECT 1 as ping").fetch_one(&pool).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "status": "ready", "db": "ok" })),
        ),
        Err(e) => {
            tracing::error!(error = %e, "Readiness check: DB ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "not ready", "db": "error" })),
            )
        }
    }
}
