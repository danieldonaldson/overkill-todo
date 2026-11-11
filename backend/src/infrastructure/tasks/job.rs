//! Tracing and metrics for background job runs (design Decisions 4, 5 and 10).
//!
//! Every run of a periodic job is a root span `job.<name>` (never a child of
//! whatever happened to be current) with an `outcome` attribute, `success` or
//! `failure`. A failure is logged at error level inside that span, so the log
//! line carries the run's trace ID. Each run also records:
//!
//! - `job.run.duration`: histogram in seconds, labelled by `job` and `outcome`;
//! - `job.last_success_timestamp`: gauge, Unix seconds, labelled by `job`;
//! - `job.interval_seconds`: gauge, labelled by `job`, so the staleness alert
//!   (`2 × interval + 15m`) doesn't hardcode any job's schedule.
//!
//! `job.last_success_timestamp` starts at the moment the job begins on this
//! replica. The staleness alert then also fires for a job that has never
//! succeeded since the leader started, which would otherwise have no series
//! to alert on.

use std::fmt::Display;
use std::future::Future;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry::{
    KeyValue,
    metrics::{Gauge, Histogram, Meter},
};
use tracing::{Instrument, field::Empty};

use crate::infrastructure::telemetry::metrics::global_meter;

/// `job.run.duration` bucket bounds in seconds: 10 ms … 5 min.
const RUN_DURATION_BUCKETS: &[f64] = &[
    0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
];

/// Job metric instruments. Cheap to clone; build once and share.
#[derive(Clone)]
pub struct JobMetrics {
    duration: Histogram<f64>,
    last_success: Gauge<f64>,
    interval: Gauge<u64>,
}

impl JobMetrics {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            duration: meter
                .f64_histogram("job.run.duration")
                .with_unit("s")
                .with_description("Duration of background job runs")
                .with_boundaries(RUN_DURATION_BUCKETS.to_vec())
                .build(),
            last_success: meter
                .f64_gauge("job.last_success_timestamp")
                .with_unit("s")
                .with_description(
                    "Unix time of the job's last successful run (or when it started on this replica)",
                )
                .build(),
            interval: meter
                .u64_gauge("job.interval_seconds")
                .with_unit("s")
                .with_description("How often the job is scheduled to run")
                .build(),
        }
    }

    /// Instruments on the global meter.
    #[must_use]
    pub fn from_global() -> Self {
        Self::new(&global_meter())
    }
}

/// One periodic background job: its name, schedule and instruments.
pub struct Job {
    name: &'static str,
    interval: Duration,
    metrics: JobMetrics,
}

impl Job {
    /// Register the job's interval and set its last-success baseline to now.
    #[must_use]
    pub fn new(name: &'static str, interval: Duration, metrics: JobMetrics) -> Self {
        let job = Self {
            name,
            interval,
            metrics,
        };
        job.metrics
            .interval
            .record(interval.as_secs(), &job.labels());
        job.record_success_now();
        job
    }

    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    fn labels(&self) -> [KeyValue; 1] {
        [KeyValue::new("job", self.name)]
    }

    fn record_success_now(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        self.metrics.last_success.record(now, &self.labels());
    }

    /// Run once inside a root `job.<name>` span and record the outcome.
    ///
    /// `run` is called inside the span, so spans it creates (including
    /// `#[instrument]`ed async fns, whose span is made at call time) nest
    /// under the job span. On error the error is logged inside the span and
    /// `None` is returned.
    pub async fn run_once<F, Fut, T, E>(&self, run: F) -> Option<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Display,
    {
        let span = tracing::info_span!(
            parent: None,
            "job.run",
            otel.name = %format_args!("job.{}", self.name),
            otel.status_code = Empty,
            job = self.name,
            outcome = Empty,
        );
        let start = Instant::now();
        let result = span.in_scope(run).instrument(span.clone()).await;
        let seconds = start.elapsed().as_secs_f64();

        let outcome = if result.is_ok() { "success" } else { "failure" };
        span.record("outcome", outcome);
        self.metrics.duration.record(
            seconds,
            &[
                KeyValue::new("job", self.name),
                KeyValue::new("outcome", outcome),
            ],
        );
        match result {
            Ok(value) => {
                self.record_success_now();
                Some(value)
            }
            Err(error) => {
                span.record("otel.status_code", "error");
                span.in_scope(|| tracing::error!(job = self.name, %error, "Job run failed"));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporter,
        data::{AggregatedMetrics, MetricData, ScopeMetrics},
    };
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor, SpanData};
    use serde_json::Value;
    use tracing_subscriber::EnvFilter;

    use super::*;
    use crate::infrastructure::tasks::account_purger;
    use crate::infrastructure::telemetry::metrics::testing::{
        Attrs, f64_histogram_points, in_memory,
    };
    use crate::infrastructure::telemetry::{
        LogFormat, ServiceInfo, metrics::METER_NAME, subscriber, tracer_provider,
    };

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

    struct Run {
        spans: Vec<SpanData>,
        logs: Vec<Value>,
        metrics: InMemoryMetricExporter,
    }

    /// Run `result` once as the account purger job, under the production
    /// subscriber stack, and capture spans, log lines and metrics.
    async fn run_purger(result: Result<u64, String>) -> Run {
        let service = ServiceInfo {
            name: "t".into(),
            version: "t".into(),
            environment: "test".into(),
        };
        let span_exporter = InMemorySpanExporter::default();
        let provider = tracer_provider(
            &service,
            Some(SimpleSpanProcessor::new(span_exporter.clone())),
        );
        let capture = Capture::default();
        let sub = subscriber(
            &provider,
            EnvFilter::new("overkill_todo=info"),
            LogFormat::Json,
            capture.clone(),
            service,
        );
        let (meter_provider, metrics) = in_memory();

        {
            let _guard = tracing::subscriber::set_default(sub);
            // An unrelated current span must not become the job's parent.
            let outer = tracing::info_span!("outer");
            let _entered = outer.enter();
            let job = Job::new(
                account_purger::JOB_NAME,
                Duration::from_hours(1),
                JobMetrics::new(&meter_provider.meter(METER_NAME)),
            );
            job.run_once(|| async { result }).await;
        }
        provider.force_flush().unwrap();
        meter_provider.force_flush().unwrap();

        let logs = String::from_utf8(capture.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        Run {
            spans: span_exporter.get_finished_spans().unwrap(),
            logs,
            metrics,
        }
    }

    fn job_span(spans: &[SpanData]) -> &SpanData {
        spans
            .iter()
            .find(|s| s.name == "job.account_purger")
            .unwrap_or_else(|| panic!("no job span in {:?}", spans.iter().map(|s| &s.name)))
    }

    fn attr(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    }

    /// `(attributes, value)` of each point of gauge `name` in the latest export.
    fn gauge<T: Copy>(
        exporter: &InMemoryMetricExporter,
        name: &str,
        pick: impl Fn(&AggregatedMetrics) -> Option<Vec<(Attrs, T)>>,
    ) -> Vec<(Attrs, T)> {
        let batches = exporter.get_finished_metrics().unwrap();
        batches
            .iter()
            .rev()
            .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
            .find(|m| m.name() == name)
            .and_then(|m| pick(m.data()))
            .unwrap_or_else(|| panic!("{name} not exported as expected"))
    }

    fn job_attrs() -> Attrs {
        vec![("job".into(), "account_purger".into())]
    }

    #[tokio::test]
    async fn successful_run_is_a_root_span_with_metrics() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let run = run_purger(Ok(3)).await;

        let span = job_span(&run.spans);
        assert_eq!(attr(span, "outcome").as_deref(), Some("success"));
        assert_eq!(attr(span, "job").as_deref(), Some("account_purger"));
        assert!(
            !span.parent_span_id.to_string().chars().any(|c| c != '0'),
            "job span has a parent"
        );

        let mut outcome_attrs = job_attrs();
        outcome_attrs.push(("outcome".into(), "success".into()));
        assert_eq!(
            f64_histogram_points(&run.metrics, "job.run.duration"),
            vec![(outcome_attrs, 1)]
        );

        let last_success = gauge(&run.metrics, "job.last_success_timestamp", |d| match d {
            AggregatedMetrics::F64(MetricData::Gauge(g)) => Some(
                g.data_points()
                    .map(|p| (sorted(p.attributes()), p.value()))
                    .collect(),
            ),
            _ => None,
        });
        assert_eq!(last_success.len(), 1);
        assert_eq!(last_success[0].0, job_attrs());
        assert!(last_success[0].1 >= before.floor());

        let interval = gauge(&run.metrics, "job.interval_seconds", |d| match d {
            AggregatedMetrics::U64(MetricData::Gauge(g)) => Some(
                g.data_points()
                    .map(|p| (sorted(p.attributes()), p.value()))
                    .collect(),
            ),
            _ => None,
        });
        assert_eq!(interval, vec![(job_attrs(), 3600)]);
    }

    #[tokio::test]
    async fn failed_run_is_marked_and_logged_with_its_trace_id() {
        let run = run_purger(Err("db unavailable".into())).await;

        let span = job_span(&run.spans);
        assert_eq!(attr(span, "outcome").as_deref(), Some("failure"));
        assert!(matches!(
            span.status,
            opentelemetry::trace::Status::Error { .. }
        ));

        let mut outcome_attrs = job_attrs();
        outcome_attrs.push(("outcome".into(), "failure".into()));
        assert_eq!(
            f64_histogram_points(&run.metrics, "job.run.duration"),
            vec![(outcome_attrs, 1)]
        );

        let trace_id = span.span_context.trace_id().to_string();
        let error_line = run
            .logs
            .iter()
            .find(|l| l["level"] == "ERROR")
            .unwrap_or_else(|| panic!("no error line in {:?}", run.logs));
        assert_eq!(error_line["trace_id"], trace_id.as_str());
        assert_eq!(error_line["job"], "account_purger");
    }

    fn sorted<'a>(kvs: impl Iterator<Item = &'a KeyValue>) -> Attrs {
        let mut attrs: Attrs = kvs
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect();
        attrs.sort();
        attrs
    }
}
