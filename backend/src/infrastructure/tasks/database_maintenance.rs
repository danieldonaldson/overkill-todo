//! Periodic bounded deletion of expired database rows.
//!
//! The deletion pass itself (which tables, batch size, retention rules) lives
//! in `persistence::postgres::maintenance`; add a step there for every table
//! that accumulates rows with an expiry or a retention limit. This module only
//! schedules it.

use std::sync::Arc;
use std::time::Duration;

use super::job::{Job, JobMetrics};
use crate::infrastructure::persistence::{Database, DatabaseError};

pub use crate::infrastructure::persistence::postgres::maintenance::{
    MAINTENANCE_BATCH_SIZE, MaintenanceReport, run_once,
};

/// The job's name in spans (`job.database_maintenance`) and metric labels.
pub const JOB_NAME: &str = "database_maintenance";

/// Run database maintenance until the process exits.
pub async fn run(db: Arc<Database>, interval: Duration, metrics: JobMetrics) {
    tracing::info!(
        interval_secs = interval.as_secs(),
        batch_size = MAINTENANCE_BATCH_SIZE,
        "Database maintenance worker started"
    );
    let job = Job::new(JOB_NAME, interval, metrics);

    loop {
        job.run_once(|| async {
            let report = run_once(&db).await?;
            if report.total() > 0 {
                tracing::info!(
                    refresh_tokens = report.refresh_tokens,
                    email_verification_codes = report.email_verification_codes,
                    password_reset_codes = report.password_reset_codes,
                    "Database maintenance pass changed rows"
                );
            }
            Ok::<_, DatabaseError>(())
        })
        .await;

        tokio::time::sleep(job.interval()).await;
    }
}
