//! Bounded deletion of expired database rows, run periodically by
//! `tasks::database_maintenance`.
//!
//! Add a step here for every table that accumulates rows with an expiry or a
//! retention limit (sessions, one-time codes, processed webhook ids, ...).
//!
//! Each statement touches at most one batch. This keeps transactions short and
//! avoids a large historical backlog monopolising the database on startup.

use super::{Database, error::DatabaseError};

// Expired code rows that were ever locked are kept this long after their last
// send so the escalating lockout count isn't reset by cleanup.
use crate::domain::entities::attempt_lockout::LOCKOUT_MEMORY_DAYS;

/// Most rows each statement of a maintenance pass deletes.
pub const MAINTENANCE_BATCH_SIZE: i64 = 1_000;

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
pub async fn run_once(db: &Database) -> Result<MaintenanceReport, DatabaseError> {
    let pool = db.pool();

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
        MAINTENANCE_BATCH_SIZE
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
              AND (lockouts = 0 OR last_sent_at < NOW() - make_interval(days => $2))
            ORDER BY expires_at
            LIMIT $1
        )
        ",
        MAINTENANCE_BATCH_SIZE,
        LOCKOUT_MEMORY_DAYS
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
              AND (lockouts = 0 OR last_sent_at < NOW() - make_interval(days => $2))
            ORDER BY expires_at
            LIMIT $1
        )
        ",
        MAINTENANCE_BATCH_SIZE,
        LOCKOUT_MEMORY_DAYS
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
