//! Database pool gauges (`db.client.connection.count` / `.max`) read back
//! through the SDK's in-memory metric exporter against a real `PgPool`.

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData, ScopeMetrics},
};
use overkill_todo::infrastructure::persistence::Database;
use overkill_todo::infrastructure::telemetry::metrics::{METER_NAME, register_db_pool_gauges};
use sqlx::{PgPool, postgres::PgPoolOptions};

/// `(state attribute or "", value)` for each point of the `u64` gauge `name`
/// in the latest export.
fn gauge_points(exporter: &InMemoryMetricExporter, name: &str) -> Vec<(String, u64)> {
    let batches = exporter.get_finished_metrics().unwrap();
    let metric = batches
        .iter()
        .rev()
        .flat_map(|rm| rm.scope_metrics().flat_map(ScopeMetrics::metrics))
        .find(|m| m.name() == name)
        .unwrap_or_else(|| panic!("{name} not exported"));
    let AggregatedMetrics::U64(MetricData::Gauge(gauge)) = metric.data() else {
        panic!("{name} is not a u64 gauge");
    };
    let mut points: Vec<_> = gauge
        .data_points()
        .map(|dp| {
            let state = dp
                .attributes()
                .find(|kv| kv.key.as_str() == "state")
                .map(|kv| kv.value.to_string())
                .unwrap_or_default();
            (state, dp.value())
        })
        .collect();
    points.sort();
    points
}

#[sqlx::test(migrations = "./migrations")]
async fn pool_gauges_report_connection_use_and_max(test_pool: PgPool) {
    // A pool of our own with a known maximum, on the test database.
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .min_connections(2)
        .connect_with((*test_pool.connect_options()).clone())
        .await
        .unwrap();
    let held = pool.acquire().await.unwrap();

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    register_db_pool_gauges(
        &provider.meter(METER_NAME),
        &Database::from_pool(pool.clone()),
    );
    provider.force_flush().unwrap();

    assert_eq!(
        gauge_points(&exporter, "db.client.connection.max"),
        vec![(String::new(), 5)]
    );

    let counts = gauge_points(&exporter, "db.client.connection.count");
    let states: Vec<&str> = counts.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(states, ["idle", "used"]);
    let used = counts[1].1;
    let total: u64 = counts.iter().map(|(_, v)| v).sum();
    assert!(used >= 1, "the held connection counts as used: {counts:?}");
    assert_eq!(total, u64::from(pool.size()), "{counts:?}");

    drop(held);
    provider.shutdown().unwrap();
}
