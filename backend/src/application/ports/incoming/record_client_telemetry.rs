//! Accept a batch of browser reports (OT-006).

use crate::application::dto::ClientReportBatch;
use crate::domain::value_objects::UserId;

/// Record a validated batch of browser reports.
///
/// Infallible on purpose: the endpoint answers 204 whatever happens to the
/// reports, including when they're over the budget and dropped.
pub trait RecordClientTelemetry: Send + Sync {
    /// `user` comes from the session (or `None`); `user_agent` is the request's
    /// `User-Agent` header, used only for the browser family and device class.
    fn record(&self, batch: ClientReportBatch, user: Option<UserId>, user_agent: &str);
}
