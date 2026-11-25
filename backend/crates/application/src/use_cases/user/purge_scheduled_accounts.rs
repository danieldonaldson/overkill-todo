//! Use case for purging accounts whose deletion grace period has expired.
//!
//! A user requests deletion → `deletion_scheduled_at` is set 14 days out → after
//! the grace period this worker finalizes it.
//!
//! INVARIANT: tear down external resources BEFORE scrubbing the row. If the user
//! owns anything outside this database (a provisioned resource, a billable key, a
//! live subscription), revoke it first, then anonymize. Guard those tables with
//! `ON DELETE RESTRICT` so a hard-delete can never orphan a live external resource.
//! Each step must be idempotent: a failure leaves the account for the next pass,
//! which simply repeats the sequence.

use std::sync::Arc;

use crate::ports::AccountLifecycleRepository;
use domain::entities::{AuditEvent, User};
use domain::errors::DomainError;
use domain::value_objects::Email;

/// Finalizes deletion for accounts past their grace period.
pub struct PurgeScheduledAccountsUseCase {
    lifecycle: Arc<dyn AccountLifecycleRepository>,
}

impl PurgeScheduledAccountsUseCase {
    #[must_use]
    pub fn new(lifecycle: Arc<dyn AccountLifecycleRepository>) -> Self {
        Self { lifecycle }
    }

    /// Scans once for accounts ready for deletion and finalizes each.
    ///
    /// Returns the number of accounts fully purged. A per-account failure is logged
    /// and left for the next pass rather than aborting the whole scan.
    ///
    /// # Errors
    /// Persistence errors from listing the ready-for-deletion set, or from the sweep
    /// once every due account has been handled.
    #[tracing::instrument(name = "usecase.purge_scheduled_accounts", skip_all)]
    pub async fn run_once(&self) -> Result<usize, DomainError> {
        // Before the early return: a purge by an older release (or a rollback)
        // may have anonymized accounts while leaving their lists behind.
        // A failed sweep is reported at the end: it must not hold up accounts
        // that are due.
        let sweep = self.lifecycle.delete_lists_of_deleted_accounts().await;
        match &sweep {
            Ok(0) => {}
            Ok(rows) => tracing::info!(rows, "Swept lists of already-deleted accounts"),
            Err(e) => {
                tracing::error!(error = %e, "Sweep of deleted accounts' lists failed; will retry next pass");
            }
        }

        let ready = self.lifecycle.find_ready_for_deletion().await?;
        if !ready.is_empty() {
            tracing::info!(count = ready.len(), "Purging scheduled accounts");
        }

        let mut purged = 0;
        for user in ready {
            match self.purge_one(&user).await {
                Ok(true) => purged += 1,
                // Cancelled (or otherwise no longer due) since the scan listed it.
                Ok(false) => {
                    tracing::info!(user_id = %user.id(), "account no longer due for deletion; skipped");
                }
                Err(e) => {
                    tracing::error!(user_id = %user.id(), error = %e, "account purge failed; will retry next pass");
                }
            }
        }
        sweep.map(|_| purged)
    }

    /// Tears down external resources, then anonymizes the user. Ordering matters —
    /// see the module-level INVARIANT.
    ///
    /// Returns `Ok(false)` when the account stopped being due (the user cancelled
    /// after the scan) and nothing was anonymized.
    async fn purge_one(&self, user: &User) -> Result<bool, DomainError> {
        let user_id = user.id();

        // External teardown goes here, before the scrub (see the module docs).

        let anonymous_email = Email::new(&format!("deleted-{user_id}@anonymized.local"))?;
        let mut anonymized = user.clone();
        anonymized.anonymize(anonymous_email, jiff::Timestamp::now())?;
        if !self
            .lifecycle
            .anonymize_account(&anonymized, &AuditEvent::account_deleted(*user_id))
            .await?
        {
            return Ok(false);
        }

        tracing::info!(%user_id, "Account purged");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::account_lifecycle::MockAccountLifecycleRepository;
    use domain::entities::UserBuilder;

    #[tokio::test]
    async fn purges_every_ready_account() {
        let mut lifecycle = MockAccountLifecycleRepository::new();
        lifecycle
            .expect_delete_lists_of_deleted_accounts()
            .returning(|| Ok(0));
        lifecycle
            .expect_find_ready_for_deletion()
            .returning(|| Ok(vec![UserBuilder::new().build(), UserBuilder::new().build()]));
        lifecycle
            .expect_anonymize_account()
            .withf(|u, e| {
                u.email().as_str().ends_with("@anonymized.local")
                    && *e == AuditEvent::account_deleted(*u.id())
            })
            .times(2)
            .returning(|_, _| Ok(true));

        let purged = PurgeScheduledAccountsUseCase::new(Arc::new(lifecycle))
            .run_once()
            .await
            .unwrap();
        assert_eq!(purged, 2);
    }

    #[tokio::test]
    async fn one_failure_does_not_abort_the_scan() {
        let mut lifecycle = MockAccountLifecycleRepository::new();
        lifecycle
            .expect_delete_lists_of_deleted_accounts()
            .returning(|| Ok(0));
        lifecycle
            .expect_find_ready_for_deletion()
            .returning(|| Ok(vec![UserBuilder::new().build(), UserBuilder::new().build()]));
        let mut calls = 0;
        lifecycle
            .expect_anonymize_account()
            .times(2)
            .returning(move |_, _| {
                calls += 1;
                if calls == 1 {
                    Err(DomainError::Persistence("db blip".into()))
                } else {
                    Ok(true)
                }
            });

        let purged = PurgeScheduledAccountsUseCase::new(Arc::new(lifecycle))
            .run_once()
            .await
            .unwrap();
        assert_eq!(purged, 1);
    }

    #[tokio::test]
    async fn account_cancelled_after_the_scan_is_not_counted() {
        let mut lifecycle = MockAccountLifecycleRepository::new();
        lifecycle
            .expect_delete_lists_of_deleted_accounts()
            .returning(|| Ok(0));
        lifecycle
            .expect_find_ready_for_deletion()
            .returning(|| Ok(vec![UserBuilder::new().build()]));
        lifecycle
            .expect_anonymize_account()
            .times(1)
            .returning(|_, _| Ok(false));

        let purged = PurgeScheduledAccountsUseCase::new(Arc::new(lifecycle))
            .run_once()
            .await
            .unwrap();
        assert_eq!(purged, 0);
    }

    #[tokio::test]
    async fn sweeps_lists_even_when_no_account_is_due() {
        let mut lifecycle = MockAccountLifecycleRepository::new();
        lifecycle
            .expect_delete_lists_of_deleted_accounts()
            .times(1)
            .returning(|| Ok(3));
        lifecycle
            .expect_find_ready_for_deletion()
            .times(1)
            .returning(|| Ok(Vec::new()));
        lifecycle.expect_anonymize_account().times(0);

        let purged = PurgeScheduledAccountsUseCase::new(Arc::new(lifecycle))
            .run_once()
            .await
            .unwrap();
        assert_eq!(purged, 0);
    }

    #[tokio::test]
    async fn a_sweep_failure_is_reported_after_due_accounts_are_purged() {
        let mut lifecycle = MockAccountLifecycleRepository::new();
        lifecycle
            .expect_delete_lists_of_deleted_accounts()
            .returning(|| Err(DomainError::Persistence("db down".into())));
        lifecycle
            .expect_find_ready_for_deletion()
            .returning(|| Ok(vec![UserBuilder::new().build()]));
        lifecycle
            .expect_anonymize_account()
            .times(1)
            .returning(|_, _| Ok(true));

        let err = PurgeScheduledAccountsUseCase::new(Arc::new(lifecycle))
            .run_once()
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Persistence(_)));
    }
}
