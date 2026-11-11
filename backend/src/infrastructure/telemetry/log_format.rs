//! Log output formats with redaction and trace correlation.
//!
//! - [`JsonFormat`] + [`JsonFields`]: one flat JSON object per line (production).
//!   Fields of every enclosing span are merged in (outermost first, then the
//!   event's own fields), and `trace_id` / `span_id` come from the current
//!   span's OpenTelemetry context, so a log line links to its trace.
//! - [`pretty_fields`]: the human-readable dev format, with the same redaction.
//!
//! Sensitive fields are replaced using [`super::redact::is_sensitive_field`].

use std::fmt;

use opentelemetry::trace::TraceContextExt;
use serde_json::{Map, Value};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{
    field::{MakeExt, RecordFields},
    fmt::{
        FmtContext, FormatEvent, FormatFields, FormattedFields,
        format::{Writer, debug_fn},
    },
    registry::LookupSpan,
};

use super::redact::{REDACTED, is_sensitive_field};

/// Service identity stamped on every JSON log line.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub version: String,
    pub environment: String,
}

/// Collects fields into a JSON map, redacting sensitive ones.
struct JsonVisitor<'a>(&'a mut Map<String, Value>);

impl JsonVisitor<'_> {
    fn insert(&mut self, field: &Field, value: Value) {
        let value = if is_sensitive_field(field.name()) {
            Value::from(REDACTED)
        } else {
            value
        };
        self.0.insert(field.name().to_string(), value);
    }
}

impl Visit for JsonVisitor<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, Value::from(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::from(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::from(value));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.insert(field, Value::from(value.to_string()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, Value::from(format!("{value:?}")));
    }
}

/// Formats span fields as a JSON object string (stored per span by the fmt
/// layer), so [`JsonFormat`] can merge them into each event line.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonFields;

impl<'writer> FormatFields<'writer> for JsonFields {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut map = Map::new();
        fields.record(&mut JsonVisitor(&mut map));
        write!(writer, "{}", Value::Object(map))
    }

    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &span::Record<'_>,
    ) -> fmt::Result {
        let mut map = match serde_json::from_str::<Value>(&current.fields) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        fields.record(&mut JsonVisitor(&mut map));
        current.fields = Value::Object(map).to_string();
        Ok(())
    }
}

/// One flat JSON object per event, with trace correlation and service identity.
#[derive(Debug, Clone)]
pub struct JsonFormat {
    service: ServiceInfo,
}

impl JsonFormat {
    #[must_use]
    pub const fn new(service: ServiceInfo) -> Self {
        Self { service }
    }
}

impl<S> FormatEvent<S, JsonFields> for JsonFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, JsonFields>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let mut obj = Map::new();
        obj.insert(
            "timestamp".into(),
            Value::from(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)),
        );
        obj.insert("level".into(), Value::from(meta.level().as_str()));
        obj.insert("target".into(), Value::from(meta.target()));

        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let ext = span.extensions();
                if let Some(fields) = ext.get::<FormattedFields<JsonFields>>()
                    && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&fields.fields)
                {
                    obj.extend(map);
                }
                if let Some(ids) = ext.get::<TraceIds>() {
                    obj.insert("trace_id".into(), Value::from(ids.trace_id.as_str()));
                    obj.insert("span_id".into(), Value::from(ids.span_id.as_str()));
                }
                drop(ext);
                obj.insert("span".into(), Value::from(span.name()));
            }
        }

        event.record(&mut JsonVisitor(&mut obj));

        obj.insert(
            "service.name".into(),
            Value::from(self.service.name.as_str()),
        );
        obj.insert(
            "service.version".into(),
            Value::from(self.service.version.as_str()),
        );
        obj.insert(
            "deployment.environment.name".into(),
            Value::from(self.service.environment.as_str()),
        );

        writeln!(writer, "{}", Value::Object(obj))
    }
}

/// OpenTelemetry trace/span IDs of a span, cached in its extensions by
/// [`TraceIdLayer`] for the log formatter.
#[derive(Debug, Clone)]
struct TraceIds {
    trace_id: String,
    span_id: String,
}

/// Caches each span's OpenTelemetry IDs in its extensions, so [`JsonFormat`]
/// can put `trace_id` / `span_id` on log lines.
///
/// The formatter cannot look them up itself: while an event is being
/// dispatched, `tracing::dispatcher::get_default` returns the no-op dispatcher
/// (re-entrancy guard), so the OpenTelemetry layer is unreachable from there.
/// This layer captures the real dispatcher at registration instead. IDs are
/// resolved at a span's first event (not at creation) so that a parent context
/// set right after creation (incoming `traceparent`) still takes effect.
/// Must sit *inside* the fmt layer in the stack, so it runs first on each event.
#[derive(Debug, Default)]
pub struct TraceIdLayer {
    dispatch: std::sync::OnceLock<tracing::dispatcher::WeakDispatch>,
}

impl<S> tracing_subscriber::Layer<S> for TraceIdLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_register_dispatch(&self, subscriber: &tracing::Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }

    fn on_event(&self, event: &Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let Some(span) = ctx.event_span(event) else {
            return;
        };
        if span.extensions().get::<TraceIds>().is_some() {
            return;
        }
        let Some(dispatch) = self
            .dispatch
            .get()
            .and_then(tracing::dispatcher::WeakDispatch::upgrade)
        else {
            return;
        };
        // No extensions guard may be held here: this locks them internally.
        let Some(cx) = tracing_opentelemetry::get_otel_context(&span.id(), &dispatch) else {
            return;
        };
        let span_ref = cx.span();
        let sc = span_ref.span_context();
        if sc.is_valid() {
            span.extensions_mut().insert(TraceIds {
                trace_id: sc.trace_id().to_string(),
                span_id: sc.span_id().to_string(),
            });
        }
    }
}

/// Human-readable field formatting for dev, with sensitive values redacted.
#[must_use]
pub fn pretty_fields() -> impl for<'w> FormatFields<'w> + Send + Sync + 'static {
    debug_fn(
        |writer: &mut Writer<'_>, field: &Field, value: &dyn fmt::Debug| {
            if is_sensitive_field(field.name()) {
                write!(writer, "{}={REDACTED}", field.name())
            } else if field.name() == "message" {
                write!(writer, "{value:?}")
            } else {
                write!(writer, "{}={value:?}", field.name())
            }
        },
    )
    .delimited(" ")
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor};
    use tracing_subscriber::EnvFilter;

    use super::*;
    use crate::infrastructure::telemetry::{LogFormat, subscriber, tracer_provider};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn service() -> ServiceInfo {
        ServiceInfo {
            name: "svc".into(),
            version: "v1".into(),
            environment: "test".into(),
        }
    }

    fn lines(capture: &Capture) -> Vec<Value> {
        String::from_utf8(capture.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn log_line_in_span_carries_that_spans_trace_id_and_fields() {
        let exporter = InMemorySpanExporter::default();
        let provider =
            tracer_provider(&service(), Some(SimpleSpanProcessor::new(exporter.clone())));
        let capture = Capture::default();
        let sub = subscriber(
            &provider,
            EnvFilter::new("overkill_todo=info"),
            LogFormat::Json,
            capture.clone(),
            service(),
        );

        tracing::subscriber::with_default(sub, || {
            let span = tracing::info_span!("outer", user_id = "u-1", refresh_token = "rt-secret");
            let _guard = span.enter();
            tracing::info!(password = "hunter2", count = 3, "hello");
        });
        provider.force_flush().unwrap();

        let spans = exporter.get_finished_spans().unwrap();
        assert_eq!(spans.len(), 1);
        let trace_id = spans[0].span_context.trace_id().to_string();

        let lines = lines(&capture);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line["trace_id"], trace_id.as_str());
        assert_eq!(
            line["span_id"],
            spans[0].span_context.span_id().to_string().as_str()
        );
        assert_eq!(line["message"], "hello");
        assert_eq!(line["user_id"], "u-1");
        assert_eq!(line["count"], 3);
        assert_eq!(line["span"], "outer");
        assert_eq!(line["service.name"], "svc");
        assert_eq!(line["service.version"], "v1");
        // Redacted in both the span field and the event field.
        assert_eq!(line["refresh_token"], REDACTED);
        assert_eq!(line["password"], REDACTED);
        // ...and in the exported span.
        let attr = spans[0]
            .attributes
            .iter()
            .find(|kv| kv.key.as_str() == "refresh_token")
            .unwrap();
        assert_eq!(attr.value.to_string(), REDACTED);
        let event_attr = spans[0].events.events[0]
            .attributes
            .iter()
            .find(|kv| kv.key.as_str() == "password")
            .unwrap();
        assert_eq!(event_attr.value.to_string(), REDACTED);
    }

    #[test]
    fn log_line_outside_spans_has_no_trace_id() {
        let provider =
            tracer_provider::<SimpleSpanProcessor<InMemorySpanExporter>>(&service(), None);
        let capture = Capture::default();
        let sub = subscriber(
            &provider,
            EnvFilter::new("overkill_todo=info"),
            LogFormat::Json,
            capture.clone(),
            service(),
        );
        tracing::subscriber::with_default(sub, || tracing::info!("startup"));
        let lines = lines(&capture);
        assert_eq!(lines[0]["message"], "startup");
        assert!(lines[0].get("trace_id").is_none());
    }

    #[test]
    fn provider_without_exporter_still_assigns_trace_ids() {
        let provider =
            tracer_provider::<SimpleSpanProcessor<InMemorySpanExporter>>(&service(), None);
        let capture = Capture::default();
        let sub = subscriber(
            &provider,
            EnvFilter::new("overkill_todo=info"),
            LogFormat::Json,
            capture.clone(),
            service(),
        );
        tracing::subscriber::with_default(sub, || {
            let span = tracing::info_span!("req");
            let _guard = span.enter();
            tracing::info!("inside");
        });
        let lines = lines(&capture);
        let id = lines[0]["trace_id"].as_str().unwrap();
        assert_eq!(id.len(), 32);
        assert_ne!(id, "0".repeat(32));
    }
}
