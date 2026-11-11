//! Trace ID middleware for request correlation.
//!
//! Returns the request's OpenTelemetry trace ID in `X-Trace-Id`, so a client
//! report can be looked up in the trace store and in logs. Requests without a
//! trace (health probes) get a random ID of the same shape.

use axum::{extract::Request, middleware::Next, response::Response};

use crate::infrastructure::telemetry::http::current_trace_id;

pub const TRACE_ID_HEADER: &str = "X-Trace-Id";

pub async fn trace_id_middleware(request: Request, next: Next) -> Response {
    let trace_id = current_trace_id().unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
    let mut response = next.run(request).await;
    if let Ok(header_value) = trace_id.parse() {
        response.headers_mut().insert(TRACE_ID_HEADER, header_value);
    }
    response
}
