//! HTTP server spans and request metrics: one span per request, named
//! `"{METHOD} {route}"`, and one `http.server.request.duration` sample labelled
//! with the same route.
//!
//! Records only the HTTP semantic-convention attributes below: no headers
//! (except `user-agent`), no bodies, and the query string with sensitive values
//! redacted. Health probes get no span and no metric. Both layers must be added
//! with `Router::layer` so axum has already routed the request and
//! [`MatchedPath`] is available.
//!
//! The request span continues the W3C trace context in the incoming
//! `traceparent` (design Decision 3). That's safe only because the backend is
//! reachable solely through the edge, which strips any client-supplied context
//! and starts the trace itself.

use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, State};
use axum::http::{HeaderMap, Request, Response, header::USER_AGENT};
use axum::{body::Body, middleware::Next};
use opentelemetry::propagation::{Extractor, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tower_http::{
    classify::{ServerErrorsAsFailures, SharedClassifier},
    trace::{DefaultOnRequest, MakeSpan, OnResponse, TraceLayer},
};
use tracing::{Span, field::Empty};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::metrics::HttpMetrics;
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

/// The route template a request matched, or [`UNMATCHED_ROUTE`].
fn route_of<B>(request: &Request<B>) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED_ROUTE, MatchedPath::as_str)
}

impl<B> MakeSpan<B> for MakeHttpSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let route = route_of(request);
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
        let parent = TraceContextPropagator::new().extract(&HeaderExtractor(request.headers()));
        if parent.span().span_context().is_valid() {
            // Only fails when no OpenTelemetry layer is installed, in which case
            // there is no trace to join.
            let _ = span.set_parent(parent);
        }
        span
    }
}

/// Reads W3C trace-context headers from an HTTP request.
struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
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

/// Middleware recording `http.server.request.duration` for every request
/// except health probes. Install with
/// `middleware::from_fn_with_state(HttpMetrics::from_global(), record_request_metrics)`.
pub async fn record_request_metrics(
    State(metrics): State<HttpMetrics>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let route = route_of(&request).to_owned();
    if HEALTH_ROUTES.contains(&route.as_str()) {
        return next.run(request).await;
    }
    let method = request.method().clone();
    let start = Instant::now();
    let response = next.run(request).await;
    metrics.record(
        &route,
        method.as_str(),
        response.status().as_u16(),
        start.elapsed().as_secs_f64(),
    );
    response
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
        spans_for_request(Request::builder().uri(uri).body(Body::empty()).unwrap()).await
    }

    /// Like [`spans_for`], for a fully built request.
    async fn spans_for_request(request: Request<Body>) -> Vec<SpanData> {
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
        let response = app().oneshot(request).await.unwrap();
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
    async fn incoming_traceparent_becomes_the_parent() {
        let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
        let parent_span_id = "00f067aa0ba902b7";
        let request = Request::builder()
            .uri("/items/1")
            .header("traceparent", format!("00-{trace_id}-{parent_span_id}-01"))
            .body(Body::empty())
            .unwrap();
        let spans = spans_for_request(request).await;
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.span_context.trace_id().to_string(), trace_id);
        assert_eq!(span.parent_span_id.to_string(), parent_span_id);
        assert!(span.parent_span_is_remote);
    }

    #[tokio::test]
    async fn malformed_traceparent_starts_a_new_trace() {
        let request = Request::builder()
            .uri("/items/1")
            .header("traceparent", "not-a-traceparent")
            .body(Body::empty())
            .unwrap();
        let spans = spans_for_request(request).await;
        assert_eq!(spans.len(), 1);
        assert!(spans[0].span_context.is_valid());
        assert!(!spans[0].parent_span_is_remote);
    }

    #[tokio::test]
    async fn health_probe_produces_no_span() {
        assert_eq!(spans_for("/health").await.len(), 0);
    }
}

#[cfg(test)]
mod metrics_tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
    };
    use opentelemetry::metrics::MeterProvider as _;
    use tower::ServiceExt;

    use super::*;
    use crate::infrastructure::telemetry::metrics::{
        METER_NAME,
        testing::{Attrs, f64_histogram_points, in_memory},
    };

    const DURATION: &str = "http.server.request.duration";

    /// Send each URI through a router with the metrics layer, then return the
    /// exported histogram points.
    async fn points_for(uris: &[&str]) -> Vec<(Attrs, u64)> {
        let (provider, exporter) = in_memory();
        let metrics = HttpMetrics::new(&provider.meter(METER_NAME));
        let app = Router::new()
            .route("/items/{id}", get(|| async { "ok" }))
            .route(
                "/boom",
                get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
            )
            .route("/health", get(|| async { "ok" }))
            .route("/health/ready", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                metrics,
                record_request_metrics,
            ));
        for uri in uris {
            let request = Request::builder().uri(*uri).body(Body::empty()).unwrap();
            drop(app.clone().oneshot(request).await.unwrap());
        }
        provider.force_flush().unwrap();
        f64_histogram_points(&exporter, DURATION)
    }

    fn attrs(route: &str, status_class: &str) -> Attrs {
        vec![
            ("http.request.method".into(), "GET".into()),
            ("http.route".into(), route.into()),
            ("status_class".into(), status_class.into()),
        ]
    }

    #[tokio::test]
    async fn templated_route_is_recorded_under_its_template() {
        let points = points_for(&["/items/1", "/items/2", "/items/3", "/boom"]).await;
        assert_eq!(points.len(), 2, "{points:?}");
        assert!(
            points.contains(&(attrs("/items/{id}", "2xx"), 3)),
            "{points:?}"
        );
        assert!(points.contains(&(attrs("/boom", "5xx"), 1)), "{points:?}");
    }

    #[tokio::test]
    async fn distinct_unknown_paths_share_one_unmatched_series() {
        let uris: Vec<String> = (0..1000).map(|i| format!("/probe/{i}/x")).collect();
        let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
        let points = points_for(&uris).await;
        assert_eq!(points, vec![(attrs(UNMATCHED_ROUTE, "4xx"), 1000)]);
    }

    #[tokio::test]
    async fn health_probes_record_nothing() {
        assert_eq!(points_for(&["/health", "/health/ready"]).await, vec![]);
    }
}
