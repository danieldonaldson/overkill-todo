//! Telemetry: tracing subscriber, OpenTelemetry trace export, redaction.
//!
//! The application and domain layers only use `tracing` macros; everything
//! OpenTelemetry-specific stays in this module. The "real vs no-op adapter"
//! choice is made by [`init`]: with `OTEL_EXPORTER_OTLP_ENDPOINT` set, spans are
//! exported over OTLP/HTTP; without it (dev only, see `Settings::from_env`) a
//! provider with no exporter still assigns real trace IDs, so `X-Trace-Id` and
//! log correlation work the same everywhere.

pub mod http;
pub mod log_format;
pub mod redact;

use std::time::Duration;

use opentelemetry::{KeyValue, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider, SpanProcessor},
};
use opentelemetry_semantic_conventions::resource::{
    DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_NAME, SERVICE_VERSION,
};
use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter, Layer, fmt::MakeWriter, layer::SubscriberExt, registry::LookupSpan,
    util::SubscriberInitExt,
};

use crate::infrastructure::config::TelemetrySettings;
pub use log_format::ServiceInfo;
use log_format::{JsonFields, JsonFormat, TraceIdLayer, pretty_fields};
use redact::RedactingSpanProcessor;

/// Instrumentation scope name for spans created through `tracing`.
const TRACER_NAME: &str = "overkill-todo";

/// Spans exported to the collector: our own code, `tower_http`, and sqlx's
/// per-query events (SQL text only, never bound parameters), which attach to
/// the enclosing repository span.
const EXPORT_FILTER: &str = "overkill_todo=info,tower_http=info,sqlx::query=info";

/// How log lines are written to stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Flat JSON, one object per line, with `trace_id` (production).
    Json,
    /// Human-readable (dev).
    Pretty,
}

/// Flushes and shuts down the trace pipeline. Call [`TelemetryGuard::shutdown`]
/// at the end of `main`, after the server has drained.
#[must_use = "dropping the guard without calling shutdown may lose buffered spans"]
pub struct TelemetryGuard {
    provider: SdkTracerProvider,
}

impl TelemetryGuard {
    /// Flush buffered spans and stop the exporter (bounded by the SDK timeout).
    pub async fn shutdown(self) {
        let provider = self.provider;
        // The batch exporter blocks on its worker thread; keep that off the runtime.
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(e) = provider.shutdown() {
                eprintln!("telemetry shutdown: {e}");
            }
        })
        .await;
    }
}

/// The `service.*` / `deployment.*` resource attached to every span.
#[must_use]
pub fn resource(service: &ServiceInfo) -> Resource {
    Resource::builder_empty()
        .with_attributes([
            KeyValue::new(SERVICE_NAME, service.name.clone()),
            KeyValue::new(SERVICE_VERSION, service.version.clone()),
            KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, service.environment.clone()),
        ])
        .build()
}

/// Build a tracer provider whose spans pass through redaction before `processor`.
/// `None` gives a provider with no exporter (trace IDs, no export).
pub fn tracer_provider<P>(service: &ServiceInfo, processor: Option<P>) -> SdkTracerProvider
where
    P: SpanProcessor + 'static,
{
    let builder = SdkTracerProvider::builder().with_resource(resource(service));
    match processor {
        Some(p) => builder
            .with_span_processor(RedactingSpanProcessor::new(p))
            .build(),
        None => builder.build(),
    }
}

/// Assemble the subscriber: stdout logs plus the OpenTelemetry layer.
///
/// The log layer is filtered by `log_filter`, the OpenTelemetry layer by
/// [`EXPORT_FILTER`]. Exposed so tests capture spans and logs with exactly the
/// production stack.
pub fn subscriber<W>(
    provider: &SdkTracerProvider,
    log_filter: EnvFilter,
    format: LogFormat,
    writer: W,
    service: ServiceInfo,
) -> impl Subscriber + for<'a> LookupSpan<'a> + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let otel = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer(TRACER_NAME))
        .with_filter(EnvFilter::new(EXPORT_FILTER));

    let fmt_layer = match format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .fmt_fields(JsonFields)
            .event_format(JsonFormat::new(service))
            .with_writer(writer)
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer()
            .fmt_fields(pretty_fields())
            .with_writer(writer)
            .boxed(),
    }
    .with_filter(log_filter);

    // Order matters: on each event the inner layers run first, so the trace-ID
    // cache is filled before the fmt layer formats the line.
    tracing_subscriber::registry()
        .with(TraceIdLayer::default())
        .with(fmt_layer)
        .with(otel)
}

/// Default stdout log filter: debug in dev, info in prod, plus OpenTelemetry
/// SDK warnings (export failures). Overridable via `RUST_LOG`.
fn default_log_filter(dev_mode: bool) -> EnvFilter {
    let default = if dev_mode {
        "overkill_todo=debug,tower_http=debug,opentelemetry_sdk=warn,opentelemetry_otlp=warn"
    } else {
        "overkill_todo=info,tower_http=info,opentelemetry_sdk=warn,opentelemetry_otlp=warn"
    };
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
}

/// Install the global subscriber and trace pipeline. Call once, early in `main`.
///
/// # Errors
///
/// Returns an error if the OTLP exporter cannot be built (e.g. a malformed
/// endpoint URL) or a global subscriber is already installed.
pub fn init(settings: &TelemetrySettings, dev_mode: bool) -> anyhow::Result<TelemetryGuard> {
    let service = ServiceInfo {
        name: settings.service_name.clone(),
        version: settings.version.clone(),
        environment: settings.environment.clone(),
    };

    let processor = settings
        .otlp_endpoint
        .as_deref()
        .map(|endpoint| -> anyhow::Result<_> {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(traces_url(endpoint))
                .with_timeout(Duration::from_secs(5))
                .build()?;
            // Bounded queue: when the collector is down, spans are dropped
            // rather than piling up in memory or slowing requests.
            let config = BatchConfigBuilder::default()
                .with_max_queue_size(4096)
                .with_scheduled_delay(Duration::from_secs(2))
                .build();
            Ok(BatchSpanProcessor::builder(exporter)
                .with_batch_config(config)
                .build())
        })
        .transpose()?;

    let provider = tracer_provider(&service, processor);
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let format = if settings.log_json.unwrap_or(!dev_mode) {
        LogFormat::Json
    } else {
        LogFormat::Pretty
    };
    subscriber(
        &provider,
        default_log_filter(dev_mode),
        format,
        std::io::stdout,
        service,
    )
    .try_init()?;

    if let Some(endpoint) = settings.otlp_endpoint.as_deref() {
        tracing::info!(endpoint, "Telemetry export enabled");
        tokio::spawn(check_collector_reachable(endpoint.to_string()));
    } else {
        tracing::warn!("OTEL_EXPORTER_OTLP_ENDPOINT not set — telemetry export disabled");
    }

    Ok(TelemetryGuard { provider })
}

/// `{base}/v1/traces`, tolerating a trailing slash on the base URL.
fn traces_url(endpoint: &str) -> String {
    format!("{}/v1/traces", endpoint.trim_end_matches('/'))
}

/// Startup probe: warn (never fail) when the collector cannot be reached, so
/// the backend keeps serving while telemetry is not being delivered.
async fn check_collector_reachable(endpoint: String) {
    let client = reqwest::Client::new();
    // Any HTTP response (even 404/405) means the collector is listening.
    if let Err(e) = client
        .get(&endpoint)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        tracing::warn!(
            endpoint,
            error = %e,
            "Telemetry collector unreachable — telemetry is not being delivered"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_url_appends_signal_path() {
        assert_eq!(
            traces_url("http://collector:4318"),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            traces_url("http://collector:4318/"),
            "http://collector:4318/v1/traces"
        );
    }
}
