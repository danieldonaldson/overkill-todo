//! Periodic bounded deletion of expired database rows.
//!
//! Add a step here for every table that accumulates rows with an expiry or a
//! retention limit (sessions, one-time codes, processed webhook ids, ...).
//!
//! Each statement touches at most one batch. This keeps transactions short and
//! avoids a large historical backlog monopolising the database on startup.

use sqlx::PgPool;
use std::time::Duration;

const BATCH_SIZE: i64 = 1_000;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    pub refresh_tokens: u64,
    pub email_verification_codes: u64,
    pub password_reset_codes: u64,
}

impl MaintenanceReport {
    /// Total rows deleted by a pass.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.refresh_tokens + self.email_verification_codes + self.password_reset_codes
    }
}

/// Run one bounded maintenance pass.
///
/// # Errors
/// Returns the first database error encountered; later tables are left for the
/// next hourly pass.
pub async fn run_once(pool: &PgPool) -> Result<MaintenanceReport, sqlx::Error> {
    let refresh_tokens = sqlx::query!(
        r"
        DELETE FROM refresh_tokens
        WHERE id IN (
            SELECT id FROM refresh_tokens
            WHERE expires_at < NOW()
            ORDER BY expires_at
            LIMIT $1
        )
        ",
        BATCH_SIZE
    )
    .execute(pool)
    .await?
    .rows_affected();

    let email_verification_codes = sqlx::query!(
        r"
        DELETE FROM email_verification_codes
        WHERE id IN (
            SELECT id FROM email_verification_codes
            WHERE expires_at < NOW()
            ORDER BY expires_at
            LIMIT $1
        )
        ",
        BATCH_SIZE
    )
    .execute(pool)
    .await?
    .rows_affected();

    let password_reset_codes = sqlx::query!(
        r"
        DELETE FROM password_reset_codes
        WHERE id IN (
            SELECT id FROM password_reset_codes
            WHERE expires_at < NOW()
            ORDER BY expires_at
            LIMIT $1
        )
        ",
        BATCH_SIZE
    )
    .execute(pool)
    .await?
    .rows_affected();

    Ok(MaintenanceReport {
        refresh_tokens,
        email_verification_codes,
        password_reset_codes,
    })
}

/// Run database maintenance until the process exits.
pub async fn run(pool: PgPool, interval: Duration) {
    tracing::info!(
        interval_secs = interval.as_secs(),
        batch_size = BATCH_SIZE,
        "Database maintenance worker started"
    );

    loop {
        match run_once(&pool).await {
            Ok(report) if report.total() > 0 => tracing::info!(
                refresh_tokens = report.refresh_tokens,
                email_verification_codes = report.email_verification_codes,
                password_reset_codes = report.password_reset_codes,
                "Database maintenance pass changed rows"
            ),
            Ok(_) => {}
            Err(error) => tracing::error!(%error, "Database maintenance pass failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_total_sums_all_tables() {
        let report = MaintenanceReport {
            refresh_tokens: 1,
            email_verification_codes: 2,
            password_reset_codes: 3,
        };
        assert_eq!(report.total(), 6);
    }
}
