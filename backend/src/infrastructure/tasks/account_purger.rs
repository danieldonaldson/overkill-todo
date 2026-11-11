//! Background loop that finalizes deletion for accounts past their grace period.
//!
//! A user schedules deletion → after the 14-day grace window this loop tears down
//! their external resources and anonymizes the row. Runs under leader
//! election so exactly one replica drives it.

use std::sync::Arc;
use std::time::Duration;

use super::job::{Job, JobMetrics};
use crate::application::use_cases::PurgeScheduledAccountsUseCase;

/// The job's name in spans (`job.account_purger`) and metric labels.
pub const JOB_NAME: &str = "account_purger";

/// Run the account purger until the process exits.
pub async fn run(
    purge: Arc<PurgeScheduledAccountsUseCase>,
    interval: Duration,
    metrics: JobMetrics,
) {
    tracing::info!(interval_secs = interval.as_secs(), "Account purger started");
    let job = Job::new(JOB_NAME, interval, metrics);
    loop {
        job.run_once(|| async {
            let purged = purge.run_once().await?;
            if purged > 0 {
                tracing::info!(purged, "Account purge pass finalized accounts");
            }
            Ok::<_, anyhow::Error>(())
        })
        .await;
        tokio::time::sleep(job.interval()).await;
    }
}
