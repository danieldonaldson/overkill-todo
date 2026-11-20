//! Where accepted browser reports go (OT-006 Decision 2).

use crate::application::dto::{
    ClientErrorKind, IslandStage, NavOutcome, NavType, Rating, ViewTransitionResult, VitalName,
};
use crate::domain::value_objects::UserId;

/// A browser error, already validated, scrubbed and normalised by
/// `RecordClientTelemetryUseCase`. Every `&'static str` comes from a fixed
/// list, so it is safe as a metric label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientErrorReport {
    /// Page template from the allowlist, or `other`.
    pub page: &'static str,
    /// The release the browser reported, for the log line.
    pub release: String,
    /// The release as a metric label: `release`, or `other` once the replica
    /// has labelled its 5 distinct releases (Decision 7).
    pub release_label: String,
    pub kind: ClientErrorKind,
    pub fingerprint: String,
    pub count: u32,
    pub message: String,
    pub stack: Option<String>,
    pub source: Option<String>,
    /// From another origin (an extension or a third-party script): logged, but
    /// not counted as a site error.
    pub external: bool,
    /// Island template from the allowlist, or `other`, for boundary errors.
    pub island: Option<&'static str>,
    pub api_route: Option<String>,
    pub status: Option<u16>,
    pub network_failed: Option<bool>,
    /// The `X-Trace-Id` of the failed API call, if the browser got one.
    pub client_trace_id: Option<String>,
    pub page_load_id: String,
    pub browser_family: &'static str,
    pub device_class: &'static str,
    /// From the session, never from the report.
    pub user_id: Option<UserId>,
}

/// A finished page load: the denominator of the browser error rate, and the
/// sign that a release is in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPageLoadReport {
    /// Page template from the allowlist, or `other`.
    pub page: &'static str,
    pub device_class: &'static str,
    /// Whether the session (never the report) names a signed-in user.
    pub authenticated: bool,
    /// The release as a metric label, capped like `ClientErrorReport`'s.
    pub release_label: String,
}

/// One Web Vital measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientVitalReport {
    pub name: VitalName,
    /// Milliseconds, or the unitless score for CLS.
    pub value: f64,
    pub rating: Rating,
    /// Page template from the allowlist, or `other`.
    pub page: &'static str,
    pub device_class: &'static str,
    /// Whether the session (never the report) names a signed-in user.
    pub authenticated: bool,
    /// The element responsible, scrubbed; only ever set for a poor rating.
    pub attribution: Option<String>,
}

/// A navigation to `page` (Decision 6a).
///
/// The use case has already made its labels consistent: a `hard` load has `view_transition=None` and no phases, an
/// aborted navigation has no durations, and a duration is `None` whenever it
/// must not be recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientNavigationReport {
    /// Destination page template from the allowlist, or `other`.
    pub page: &'static str,
    pub device_class: &'static str,
    pub nav_type: NavType,
    pub view_transition: ViewTransitionResult,
    pub outcome: NavOutcome,
    pub total_ms: Option<f64>,
    pub fetch_ms: Option<f64>,
    pub swap_ms: Option<f64>,
    pub transition_ms: Option<f64>,
}

/// One island timing mark (Decision 6b).
#[derive(Debug, Clone, PartialEq)]
pub struct ClientIslandReport {
    /// Island name from the allowlist, or `other`.
    pub island: &'static str,
    /// Page template from the allowlist, or `other`.
    pub page: &'static str,
    pub stage: IslandStage,
    pub nav_type: NavType,
    pub device_class: &'static str,
    pub ms: f64,
}

/// Records browser reports: errors as log events; counts, Web
/// Vitals, navigations and island timings as metrics.
///
/// # Implementors
///
/// - [`crate::infrastructure::telemetry::client_reports::TelemetryClientReportSink`]
pub trait ClientReportSink: Send + Sync {
    /// Record one browser error.
    fn error(&self, report: &ClientErrorReport);
    /// Count one page load, and mark its release as seen.
    fn page_load(&self, report: &ClientPageLoadReport);
    /// Record one Web Vital measurement.
    fn vital(&self, report: &ClientVitalReport);
    /// Record one navigation: counted if in-site, timed where it has timings.
    fn navigation(&self, report: &ClientNavigationReport);
    /// Record one island timing mark.
    fn island(&self, report: &ClientIslandReport);
    /// Count `n` events that were over the recording budget and not recorded.
    fn dropped(&self, n: u64);
}
