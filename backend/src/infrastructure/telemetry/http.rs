//! HTTP server spans: one per request, named `"{METHOD} {route}"`.
//!
//! Records only the HTTP semantic-convention attributes below: no headers
//! (except `user-agent`), no bodies, and the query string with sensitive values
//! redacted. Health probes get no span. Must be added with `Router::layer` so
//! axum has already routed the request and [`MatchedPath`] is available.

use std::time::Duration;

use axum::extract::MatchedPath;
use axum::http::{Request, Response, header::USER_AGENT};
use opentelemetry::trace::TraceContextExt;
use tower_http::{
    classify::{ServerErrorsAsFailures, SharedClassifier},
    trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer},
};
use tracing::{Span, field::Empty};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::redact::redact_query;

/// Route label for requests that matched no route (keeps span names and, later,
/// metric labels bounded no matter how many distinct paths are probed).
pub const UNMATCHED_ROUTE: &str = "unmatched";

/// Route templates of the liveness/readiness probes, which are not traced.
const HEALTH_ROUTES: &[&str] = &[
    "/health",
    "/health/ready",
    "/api/v1/health",
    "/api/v1/health/ready",
];

/// Builds the per-request server span.
#[derive(Debug, Clone, Copy, Default)]
pub struct MakeHttpSpan;

impl<B> MakeSpan<B> for MakeHttpSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or(UNMATCHED_ROUTE, MatchedPath::as_str);
        if HEALTH_ROUTES.contains(&route) {
            return Span::none();
        }

        let method = request.method().as_str();
        let user_agent = request
            .headers()
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let span = tracing::info_span!(
            "http.request",
            otel.name = %format_args!("{method} {route}"),
            otel.kind = "server",
            otel.status_code = Empty,
            http.request.method = method,
            http.route = route,
            url.path = request.uri().path(),
            url.query = Empty,
            user_agent.original = user_agent,
            http.response.status_code = Empty,
        );
        if let Some(query) = request.uri().query() {
            span.record("url.query", redact_query(query));
        }
        span
    }
}

/// Records the status code, and marks the span as an error on 5xx.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordHttpResponse;

impl<B> OnResponse<B> for RecordHttpResponse {
    fn on_response(self, response: &Response<B>, _latency: Duration, span: &Span) {
        let status = response.status();
        span.record("http.response.status_code", status.as_u16());
        if status.is_server_error() {
            span.record("otel.status_code", "error");
        }
    }
}

/// The request-tracing layer type installed by `create_router`.
pub type HttpTraceLayer = TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    MakeHttpSpan,
    DefaultOnRequest,
    RecordHttpResponse,
>;

/// The request-tracing layer: server span per request, status recorded.
#[must_use]
pub fn http_trace_layer() -> HttpTraceLayer {
    TraceLayer::new_for_http()
        .make_span_with(MakeHttpSpan)
        .on_response(RecordHttpResponse)
}

/// Trace ID (32 lowercase hex) of the current span, if it has a valid
/// OpenTelemetry context.
#[must_use]
pub fn current_trace_id() -> Option<String> {
    let cx = Span::current().context();
    let span_ref = cx.span();
    let sc = span_ref.span_context();
    sc.is_valid().then(|| sc.trace_id().to_string())
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use opentelemetry::trace::{SpanKind, Status};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor, SpanData};
    use tower::ServiceExt;
    use tracing_subscriber::EnvFilter;

    use super::*;
    use crate::infrastructure::telemetry::{LogFormat, ServiceInfo, subscriber, tracer_provider};

    fn app() -> Router {
        Router::new()
            .route("/items/{id}", get(|| async { "ok" }))
            .route(
                "/boom",
                get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
            )
            .route("/health", get(|| async { "ok" }))
            .layer(http_trace_layer())
    }

    /// Send one request through [`app`] with the production subscriber stack
    /// and return the exported spans.
    async fn spans_for(uri: &str) -> Vec<SpanData> {
        let service = ServiceInfo {
            name: "t".into(),
            version: "t".into(),
            environment: "test".into(),
        };
        let exporter = InMemorySpanExporter::default();
        let provider = tracer_provider(&service, Some(SimpleSpanProcessor::new(exporter.clone())));
        let sub = subscriber(
            &provider,
            EnvFilter::new("off"),
            LogFormat::Json,
            std::io::sink,
            service,
        );
        let _guard = tracing::subscriber::set_default(sub);
        let response = app()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        drop(response);
        provider.force_flush().unwrap();
        exporter.get_finished_spans().unwrap()
    }

    fn attr(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    }

    #[tokio::test]
    async fn templated_route_names_span_and_records_semconv_attributes() {
        let spans = spans_for("/items/42?page=2&token=secret-value").await;
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "GET /items/{id}");
        assert_eq!(span.span_kind, SpanKind::Server);
        assert_eq!(attr(span, "http.route").as_deref(), Some("/items/{id}"));
        assert_eq!(attr(span, "http.request.method").as_deref(), Some("GET"));
        assert_eq!(attr(span, "url.path").as_deref(), Some("/items/42"));
        assert_eq!(
            attr(span, "url.query").as_deref(),
            Some("page=2&token=[REDACTED]")
        );
        assert_eq!(
            attr(span, "http.response.status_code").as_deref(),
            Some("200")
        );
        assert_eq!(span.status, Status::Unset);
    }

    #[tokio::test]
    async fn unmatched_path_uses_a_single_route_label() {
        let spans = spans_for("/no/such/path/123").await;
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "GET unmatched");
        assert_eq!(
            attr(&spans[0], "http.route").as_deref(),
            Some(UNMATCHED_ROUTE)
        );
        assert_eq!(
            attr(&spans[0], "http.response.status_code").as_deref(),
            Some("404")
        );
    }

    #[tokio::test]
    async fn server_error_marks_span_as_error() {
        let spans = spans_for("/boom").await;
        assert_eq!(spans.len(), 1);
        assert_eq!(
            attr(&spans[0], "http.response.status_code").as_deref(),
            Some("500")
        );
        assert!(matches!(spans[0].status, Status::Error { .. }));
    }

    #[tokio::test]
    async fn health_probe_produces_no_span() {
        assert_eq!(spans_for("/health").await.len(), 0);
    }
}
