//! Background loop that finalizes deletion for accounts past their grace period.
//!
//! A user schedules deletion → after the 14-day grace window this loop tears down
//! their external resources and anonymizes the row. Runs under leader
//! election so exactly one replica drives it.

use std::sync::Arc;
use std::time::Duration;

use crate::application::use_cases::PurgeScheduledAccountsUseCase;

/// Run the account purger until the process exits.
pub async fn run(purge: Arc<PurgeScheduledAccountsUseCase>, interval: Duration) {
    tracing::info!(interval_secs = interval.as_secs(), "Account purger started");
    loop {
        match purge.run_once().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(purged = n, "Account purge pass finalized accounts"),
            Err(e) => tracing::error!(error = %e, "Account purge pass failed"),
        }
        tokio::time::sleep(interval).await;
    }
}
