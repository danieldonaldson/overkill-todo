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
    metrics::{Counter, Histogram, Meter},
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

/// Which credential or code check an [`AuthMetrics`] measurement is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFlow {
    PasswordSignIn,
    EmailCode,
    ResetCode,
    MagicLink,
}

impl AuthFlow {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PasswordSignIn => "password_sign_in",
            Self::EmailCode => "email_code",
            Self::ResetCode => "reset_code",
            Self::MagicLink => "magic_link",
        }
    }
}

/// How a credential or code check ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The credential or code was accepted.
    Success,
    /// A wrong credential or code (including the guess that locks the code).
    Failure,
    /// Refused without checking, because the code is locked.
    Locked,
    /// The code had expired.
    Expired,
}

impl AuthOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Locked => "locked",
            Self::Expired => "expired",
        }
    }
}

/// Code flows that have an attempt cap, for `auth.code.lockouts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockableCode {
    EmailCode,
    ResetCode,
}

impl LockableCode {
    #[must_use]
    pub const fn flow(self) -> AuthFlow {
        match self {
            Self::EmailCode => AuthFlow::EmailCode,
            Self::ResetCode => AuthFlow::ResetCode,
        }
    }
}

/// Forms protected by the bot check, for `auth.bot_check.rejections`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotCheckForm {
    Register,
    SignIn,
    ResendVerification,
    ForgotPassword,
}

impl BotCheckForm {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::SignIn => "sign_in",
            Self::ResendVerification => "resend_verification",
            Self::ForgotPassword => "forgot_password",
        }
    }
}

/// Why the bot check rejected a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotCheckReason {
    /// No token, or an empty one.
    Missing,
    /// Turnstile said the token is not valid.
    Invalid,
}

impl BotCheckReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }
}

/// Signs that a session may have been stolen, for `auth.session.anomalies`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAnomaly {
    /// A refresh token that was already rotated or revoked, presented again.
    TokenReuse,
    /// A refresh from a different device than the session was issued to.
    DeviceMismatch,
}

impl SessionAnomaly {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TokenReuse => "token_reuse",
            Self::DeviceMismatch => "device_mismatch",
        }
    }
}

/// Authentication outcome counters (OT-007 D3), recorded by the auth and
/// verification handlers once the use case has returned.
///
/// Every label value comes from a closed enum above, never from the request,
/// so no measurement can carry an email, user ID, code or token, and the
/// number of series is fixed.
#[derive(Clone)]
pub struct AuthMetrics {
    attempts: Counter<u64>,
    lockouts: Counter<u64>,
    bot_check_rejections: Counter<u64>,
    session_anomalies: Counter<u64>,
}

impl AuthMetrics {
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            attempts: meter
                .u64_counter("auth.attempts")
                .with_unit("{attempt}")
                .with_description("Credential and code checks, by flow and outcome")
                .build(),
            lockouts: meter
                .u64_counter("auth.code.lockouts")
                .with_unit("{lockout}")
                .with_description("Codes that became locked by reaching the attempt cap")
                .build(),
            bot_check_rejections: meter
                .u64_counter("auth.bot_check.rejections")
                .with_unit("{rejection}")
                .with_description("Requests the bot check rejected, by form and reason")
                .build(),
            session_anomalies: meter
                .u64_counter("auth.session.anomalies")
                .with_unit("{anomaly}")
                .with_description("Refresh-token reuse and device mismatches, by kind")
                .build(),
        }
    }

    /// Instruments on the global meter.
    #[must_use]
    pub fn from_global() -> Self {
        Self::new(&global_meter())
    }

    /// Count one check of `flow` that ended in `outcome`.
    pub fn attempt(&self, flow: AuthFlow, outcome: AuthOutcome) {
        self.attempts.add(
            1,
            &[
                KeyValue::new("flow", flow.as_str()),
                KeyValue::new("outcome", outcome.as_str()),
            ],
        );
    }

    /// Count one bot-check rejection on `form`.
    pub fn bot_check_rejection(&self, form: BotCheckForm, reason: BotCheckReason) {
        self.bot_check_rejections.add(
            1,
            &[
                KeyValue::new("form", form.as_str()),
                KeyValue::new("reason", reason.as_str()),
            ],
        );
    }

    /// Count one session anomaly of `kind`.
    pub fn session_anomaly(&self, kind: SessionAnomaly) {
        self.session_anomalies
            .add(1, &[KeyValue::new("kind", kind.as_str())]);
    }

    /// Count a wrong code guess. The guess is a `failure`; if it left no
    /// attempts it also locked the code, which is counted once here.
    pub fn code_failure(&self, code: LockableCode, attempts_remaining: i32) {
        self.attempt(code.flow(), AuthOutcome::Failure);
        if attempts_remaining <= 0 {
            self.lockouts
                .add(1, &[KeyValue::new("flow", code.flow().as_str())]);
        }
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

    /// `(attributes, bucket counts)` for each data point of the `f64`
    /// histogram `name`, sorted by attributes, with its bucket boundaries.
    pub fn f64_histogram_buckets(
        exporter: &InMemoryMetricExporter,
        name: &str,
    ) -> (Vec<f64>, Vec<(Attrs, Vec<u64>)>) {
        with_metric(exporter, name, |m| {
            let AggregatedMetrics::F64(MetricData::Histogram(hist)) = m.data() else {
                panic!("{name} is not an f64 histogram");
            };
            let mut bounds = Vec::new();
            let mut points: Vec<(Attrs, Vec<u64>)> = hist
                .data_points()
                .map(|dp| {
                    bounds = dp.bounds().collect();
                    (sorted_attrs(dp.attributes()), dp.bucket_counts().collect())
                })
                .collect();
            points.sort();
            (bounds, points)
        })
        .unwrap_or_default()
    }

    /// `(attributes, value)` for each data point of the `u64` sum `name`,
    /// sorted by attributes.
    pub fn u64_sum_points(exporter: &InMemoryMetricExporter, name: &str) -> Vec<(Attrs, u64)> {
        let mut points = with_metric(exporter, name, |m| {
            let AggregatedMetrics::U64(MetricData::Sum(sum)) = m.data() else {
                panic!("{name} is not a u64 sum");
            };
            sum.data_points()
                .map(|dp| (sorted_attrs(dp.attributes()), dp.value()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        points.sort();
        points
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;

    use super::testing::{Attrs, f64_histogram_points, in_memory, u64_sum_points, unit};
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

    fn attrs(pairs: &[(&str, &str)]) -> Attrs {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn auth_attempts_are_counted_by_flow_and_outcome() {
        let (provider, exporter) = in_memory();
        let metrics = AuthMetrics::new(&provider.meter(METER_NAME));
        metrics.attempt(AuthFlow::PasswordSignIn, AuthOutcome::Failure);
        metrics.attempt(AuthFlow::PasswordSignIn, AuthOutcome::Failure);
        metrics.attempt(AuthFlow::PasswordSignIn, AuthOutcome::Success);
        metrics.attempt(AuthFlow::ResetCode, AuthOutcome::Expired);
        provider.force_flush().unwrap();

        assert_eq!(
            unit(&exporter, "auth.attempts").as_deref(),
            Some("{attempt}")
        );
        assert_eq!(
            u64_sum_points(&exporter, "auth.attempts"),
            vec![
                (
                    attrs(&[("flow", "password_sign_in"), ("outcome", "failure")]),
                    2
                ),
                (
                    attrs(&[("flow", "password_sign_in"), ("outcome", "success")]),
                    1
                ),
                (attrs(&[("flow", "reset_code"), ("outcome", "expired")]), 1),
            ]
        );
    }

    /// Only the guess that leaves no attempts counts as a lockout; every wrong
    /// guess is a failure.
    #[test]
    fn the_locking_guess_is_counted_once() {
        let (provider, exporter) = in_memory();
        let metrics = AuthMetrics::new(&provider.meter(METER_NAME));
        for remaining in (0..5).rev() {
            metrics.code_failure(LockableCode::EmailCode, remaining);
        }
        provider.force_flush().unwrap();

        assert_eq!(
            u64_sum_points(&exporter, "auth.attempts"),
            vec![(attrs(&[("flow", "email_code"), ("outcome", "failure")]), 5)]
        );
        assert_eq!(
            unit(&exporter, "auth.code.lockouts").as_deref(),
            Some("{lockout}")
        );
        assert_eq!(
            u64_sum_points(&exporter, "auth.code.lockouts"),
            vec![(attrs(&[("flow", "email_code")]), 1)]
        );
    }

    /// The series count is fixed by the label enums: many calls with every
    /// combination never yield more than 4 × 4 attempt series and 2 lockout
    /// series.
    #[test]
    fn series_do_not_grow_with_calls() {
        let (provider, exporter) = in_memory();
        let metrics = AuthMetrics::new(&provider.meter(METER_NAME));
        let flows = [
            AuthFlow::PasswordSignIn,
            AuthFlow::EmailCode,
            AuthFlow::ResetCode,
            AuthFlow::MagicLink,
        ];
        let outcomes = [
            AuthOutcome::Success,
            AuthOutcome::Failure,
            AuthOutcome::Locked,
            AuthOutcome::Expired,
        ];
        for i in 0..1_000 {
            metrics.attempt(flows[i % 4], outcomes[(i / 4) % 4]);
            let code = if i % 2 == 0 {
                LockableCode::EmailCode
            } else {
                LockableCode::ResetCode
            };
            metrics.code_failure(code, 0);
        }
        provider.force_flush().unwrap();

        assert_eq!(u64_sum_points(&exporter, "auth.attempts").len(), 16);
        assert_eq!(u64_sum_points(&exporter, "auth.code.lockouts").len(), 2);
    }
}
