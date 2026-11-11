//! Metric instruments (design Decision 5).
//!
//! Metrics are pushed over OTLP/HTTP with cumulative temporality. Without an
//! endpoint the meter provider has no reader, so instruments are cheap no-ops.
//! Instruments are built from an explicit [`Meter`] so tests can point them at
//! an in-memory exporter; production code takes it from [`global_meter`],
//! which [`super::init`] wires to the real provider before anything else runs.

use super::{ServiceInfo, resource};
use std::sync::Arc;

use crate::infrastructure::persistence::DatabaseHealth;
use opentelemetry::{
    KeyValue,
    metrics::{Histogram, Meter},
};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, exporter::PushMetricExporter};

/// Instrumentation scope name for the backend's own metrics.
pub const METER_NAME: &str = "overkill-todo";

/// `http.server.request.duration` bucket bounds in seconds: 5 ms … 10 s.
const HTTP_DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Build a meter provider exporting through `reader`. `None` gives a provider
/// with no reader: instruments work but nothing is collected or sent.
#[must_use]
pub fn meter_provider<E>(
    service: &ServiceInfo,
    reader: Option<PeriodicReader<E>>,
) -> SdkMeterProvider
where
    E: PushMetricExporter,
{
    let builder = SdkMeterProvider::builder().with_resource(resource(service));
    match reader {
        Some(r) => builder.with_reader(r).build(),
        None => builder.build(),
    }
}

/// The meter from the globally installed provider (set by [`super::init`]).
#[must_use]
pub fn global_meter() -> Meter {
    opentelemetry::global::meter(METER_NAME)
}

/// Request metrics, recorded once per request by
/// [`super::http::record_request_metrics`].
#[derive(Clone)]
pub struct HttpMetrics {
    duration: Histogram<f64>,
}

impl HttpMetrics {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        let duration = meter
            .f64_histogram("http.server.request.duration")
            .with_unit("s")
            .with_description("Duration of HTTP server requests")
            .with_boundaries(HTTP_DURATION_BUCKETS.to_vec())
            .build();
        Self { duration }
    }

    /// Instruments on the global meter.
    #[must_use]
    pub fn from_global() -> Self {
        Self::new(&global_meter())
    }

    /// Record one request. `route` must be a route template (or
    /// [`super::http::UNMATCHED_ROUTE`]), never a raw path, so the label set
    /// stays bounded.
    pub fn record(&self, route: &str, method: &str, status: u16, seconds: f64) {
        self.duration.record(
            seconds,
            &[
                KeyValue::new("http.route", route.to_owned()),
                KeyValue::new("http.request.method", method.to_owned()),
                KeyValue::new("status_class", status_class(status)),
            ],
        );
    }
}

/// `2xx` … `5xx` (`1xx` too); anything out of range is `other`.
#[must_use]
pub const fn status_class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => "other",
    }
}

/// Register `db.client.connection.count` (`state` = `idle`/`used`) and
/// `db.client.connection.max` as observable gauges read from `pool` at each
/// collection. The callbacks live as long as the meter provider.
pub fn register_db_pool_gauges(meter: &Meter, db: &Arc<impl DatabaseHealth + 'static>) {
    let counts = Arc::clone(db);
    meter
        .u64_observable_gauge("db.client.connection.count")
        .with_unit("{connection}")
        .with_description("Connections in the pool, by state")
        .with_callback(move |observer| {
            let stats = counts.pool_stats();
            let size = u64::from(stats.size);
            let idle = u64::try_from(stats.idle).unwrap_or(u64::MAX).min(size);
            observer.observe(idle, &[KeyValue::new("state", "idle")]);
            observer.observe(size - idle, &[KeyValue::new("state", "used")]);
        })
        .build();

    let max = db.max_connections();
    meter
        .u64_observable_gauge("db.client.connection.max")
        .with_unit("{connection}")
        .with_description("Maximum connections the pool may open")
        .with_callback(move |observer| observer.observe(u64::from(max), &[]))
        .build();
}

/// Helpers for reading metrics back in tests.
#[cfg(test)]
pub(crate) mod testing {
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        data::{AggregatedMetrics, Metric, MetricData, ScopeMetrics},
    };

    use super::meter_provider;
    use crate::infrastructure::telemetry::ServiceInfo;

    /// A provider backed by an in-memory exporter. Call
    /// `provider.force_flush()` before reading the exporter.
    pub fn in_memory() -> (SdkMeterProvider, InMemoryMetricExporter) {
        let service = ServiceInfo {
            name: "t".into(),
            version: "t".into(),
            environment: "test".into(),
        };
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        (meter_provider(&service, Some(reader)), exporter)
    }

    /// Attributes of one data point, sorted by key.
    pub type Attrs = Vec<(String, String)>;

    /// Run `f` on the most recently exported metric called `name`.
    fn with_metric<T>(
        exporter: &InMemoryMetricExporter,
        name: &str,
        f: impl FnOnce(&Metric) -> T,
    ) -> Option<T> {
        let batches = exporter.get_finished_metrics().unwrap();
        batches
            .iter()
            .rev()
            .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
            .find(|m| m.name() == name)
            .map(f)
    }

    fn sorted_attrs<'a>(kvs: impl Iterator<Item = &'a KeyValue>) -> Attrs {
        let mut attrs: Attrs = kvs
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect();
        attrs.sort();
        attrs
    }

    /// Unit of the metric called `name`, if exported.
    pub fn unit(exporter: &InMemoryMetricExporter, name: &str) -> Option<String> {
        with_metric(exporter, name, |m| m.unit().to_owned())
    }

    /// `(attributes, count)` for each data point of the `f64` histogram `name`.
    pub fn f64_histogram_points(
        exporter: &InMemoryMetricExporter,
        name: &str,
    ) -> Vec<(Attrs, u64)> {
        with_metric(exporter, name, |m| {
            let AggregatedMetrics::F64(MetricData::Histogram(hist)) = m.data() else {
                panic!("{name} is not an f64 histogram");
            };
            hist.data_points()
                .map(|dp| (sorted_attrs(dp.attributes()), dp.count()))
                .collect()
        })
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;

    use super::testing::{f64_histogram_points, in_memory, unit};
    use super::*;

    #[test]
    fn status_classes() {
        assert_eq!(status_class(101), "1xx");
        assert_eq!(status_class(200), "2xx");
        assert_eq!(status_class(304), "3xx");
        assert_eq!(status_class(404), "4xx");
        assert_eq!(status_class(503), "5xx");
        assert_eq!(status_class(600), "other");
    }

    #[test]
    fn recorded_value_reaches_the_exporter() {
        let (provider, exporter) = in_memory();
        let metrics = HttpMetrics::new(&provider.meter(METER_NAME));
        metrics.record("/items/{id}", "GET", 200, 0.012);
        provider.force_flush().unwrap();

        let name = "http.server.request.duration";
        assert_eq!(unit(&exporter, name).as_deref(), Some("s"));
        let points = f64_histogram_points(&exporter, name);
        assert_eq!(points.len(), 1);
        assert_eq!(
            points[0].0,
            vec![
                ("http.request.method".into(), "GET".into()),
                ("http.route".into(), "/items/{id}".into()),
                ("status_class".into(), "2xx".into()),
            ]
        );
        assert_eq!(points[0].1, 1);
    }

    #[test]
    fn provider_without_reader_accepts_records() {
        let service = ServiceInfo {
            name: "t".into(),
            version: "t".into(),
            environment: "test".into(),
        };
        let provider =
            meter_provider::<opentelemetry_sdk::metrics::InMemoryMetricExporter>(&service, None);
        HttpMetrics::new(&provider.meter(METER_NAME)).record("/x", "GET", 200, 0.001);
        provider.shutdown().unwrap();
    }
}
