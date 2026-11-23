//! Browser error and performance reports (`POST /api/v1/client-telemetry`,
//! OT-006 Decision 3).
//!
//! Public and outside CSRF: `navigator.sendBeacon` can't set the CSRF header,
//! and the endpoint changes no state and returns nothing, so a forged report
//! can at most add noise, which the recording budget bounds. The session cookie
//! is read only to attribute the report; a missing, expired or invalid one
//! makes it anonymous, never a 401.
//!
//! Not traced, and not instrumented: the route is on the span skip list
//! (`telemetry::http::UNTRACED_ROUTES`). Bad input is answered with the standard
//! 400 and logged nowhere, so junk can't flood the logs.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header::USER_AGENT},
};
use validator::Validate;

use crate::infrastructure::api::{
    error::AppError, extractors::ApiJson, middleware::OptionalAuthUser,
};
use application::{dto::ClientReportBatch, ports::incoming::RecordClientTelemetry};

/// Largest request body accepted, in bytes.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Dependencies for the browser-report handler.
pub struct ClientTelemetryHandlerDeps {
    pub record: Arc<dyn RecordClientTelemetry>,
}

/// Record browser error reports.
#[utoipa::path(
    post,
    path = "/client-telemetry",
    tag = "Telemetry",
    request_body = ClientReportBatch,
    responses(
        (status = 204, description = "Accepted. Reports over the recording budget are counted, not recorded, and get this answer too"),
        (status = 400, description = "Not a valid report"),
        (status = 413, description = "Body larger than 16 KB")
    )
)]
/// # Errors
/// Returns a 400 for a body that isn't a valid report and a 413 for one over
/// [`MAX_BODY_BYTES`].
pub async fn record_client_telemetry(
    State(deps): State<Arc<ClientTelemetryHandlerDeps>>,
    OptionalAuthUser(user): OptionalAuthUser,
    headers: HeaderMap,
    ApiJson(batch): ApiJson<ClientReportBatch>,
) -> Result<StatusCode, AppError> {
    // Validated here rather than through `ValidatedJson`, which logs every
    // rejection at error level.
    batch
        .validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    deps.record
        .record(batch, user.map(|u| u.user_id), user_agent);
    Ok(StatusCode::NO_CONTENT)
}
