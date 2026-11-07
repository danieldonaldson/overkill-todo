//! Trace ID middleware for request correlation.

use axum::{extract::Request, middleware::Next, response::Response};

pub const TRACE_ID_HEADER: &str = "X-Trace-Id";

pub async fn trace_id_middleware(request: Request, next: Next) -> Response {
    let trace_id = format!("{:032x}", rand::random::<u128>());
    tracing::Span::current().record("trace_id", &trace_id);
    let mut response = next.run(request).await;
    if let Ok(header_value) = trace_id.parse() {
        response.headers_mut().insert(TRACE_ID_HEADER, header_value);
    }
    response
}
