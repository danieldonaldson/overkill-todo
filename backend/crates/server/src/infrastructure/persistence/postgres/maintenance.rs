//! sqlx implementation of [`DatabaseMaintenance`]: bounded deletion of
//! expired rows.
//!
//! Add a step here for every table that accumulates rows with an expiry or a
//! retention limit (sessions, one-time codes, processed webhook ids, ...), and
//! a matching field on [`MaintenanceReport`].

use super::Database;
use crate::infrastructure::persistence::{
    DatabaseError, DatabaseMaintenance, MAINTENANCE_BATCH_SIZE, MaintenanceReport,
};

// Expired code rows that were ever locked are kept this long after their last
// send so the escalating lockout count isn't reset by cleanup.
use domain::entities::attempt_lockout::LOCKOUT_MEMORY_DAYS;

impl DatabaseMaintenance for Database {
    async fn purge_expired(&self) -> Result<MaintenanceReport, DatabaseError> {
        let pool = self.pool();
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
        .await
        .map_err(DatabaseError::new)?
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
        .await
        .map_err(DatabaseError::new)?
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
        .await
        .map_err(DatabaseError::new)?
        .rows_affected();

        // OT-008 retention: audit events are kept for 7 years, then deleted.
        // The append-only trigger allows exactly this delete and refuses any
        // younger row, so the condition here and there must agree.
        let audit_events = sqlx::query!(
            r"
            DELETE FROM audit_events
            WHERE id IN (
                SELECT id FROM audit_events
                WHERE occurred_at < NOW() - INTERVAL '7 years'
                ORDER BY occurred_at
                LIMIT $1
            )
            ",
            MAINTENANCE_BATCH_SIZE
        )
        .execute(pool)
        .await
        .map_err(DatabaseError::new)?
        .rows_affected();

        Ok(MaintenanceReport {
            refresh_tokens,
            email_verification_codes,
            password_reset_codes,
            audit_events,
        })
    }
}
