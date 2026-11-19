//! Where accepted browser reports go (OT-006 Decision 2).

use crate::application::dto::ClientErrorKind;
use crate::domain::value_objects::UserId;

/// A browser error, already validated, scrubbed and normalised by
/// `RecordClientTelemetryUseCase`. Every `&'static str` comes from a fixed
/// list, so it is safe as a metric label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientErrorReport {
    /// Page template from the allowlist, or `other`.
    pub page: &'static str,
    pub release: String,
    pub kind: ClientErrorKind,
    pub fingerprint: String,
    pub count: u32,
    pub message: String,
    pub stack: Option<String>,
    pub source: Option<String>,
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

/// Records browser reports: errors as log events, counts as metrics.
///
/// # Implementors
///
/// - [`crate::infrastructure::telemetry::client_reports::TelemetryClientReportSink`]
pub trait ClientReportSink: Send + Sync {
    /// Record one browser error.
    fn error(&self, report: &ClientErrorReport);
    /// Count `n` events that were over the recording budget and not recorded.
    fn dropped(&self, n: u64);
}
