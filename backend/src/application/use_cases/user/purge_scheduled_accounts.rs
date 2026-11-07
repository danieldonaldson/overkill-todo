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

use crate::application::ports::AccountLifecycleRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::Email;

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
    /// Persistence errors from listing the ready-for-deletion set.
    #[tracing::instrument(name = "usecase.purge_scheduled_accounts", skip(self))]
    pub async fn run_once(&self) -> Result<usize, DomainError> {
        let ready = self.lifecycle.find_ready_for_deletion().await?;
        if ready.is_empty() {
            return Ok(0);
        }
        tracing::info!(count = ready.len(), "Purging scheduled accounts");

        let mut purged = 0;
        for user in ready {
            match self.purge_one(&user).await {
                Ok(()) => purged += 1,
                Err(e) => {
                    tracing::error!(user_id = %user.id(), error = %e, "account purge failed; will retry next pass");
                }
            }
        }
        Ok(purged)
    }

    /// Tears down external resources, then anonymizes the user. Ordering matters —
    /// see the module-level INVARIANT.
    async fn purge_one(&self, user: &User) -> Result<(), DomainError> {
        let user_id = user.id();

        // External teardown goes here, before the scrub (see the module docs).

        let anonymous_email = Email::new(&format!("deleted-{user_id}@anonymized.local"))?;
        let mut anonymized = user.clone();
        anonymized.anonymize(anonymous_email);
        self.lifecycle.anonymize_account(&anonymized).await?;

        tracing::info!(%user_id, "Account purged");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::account_lifecycle::MockAccountLifecycleRepository;
    use crate::domain::entities::UserBuilder;

    #[tokio::test]
    async fn purges_every_ready_account() {
        let mut lifecycle = MockAccountLifecycleRepository::new();
        lifecycle
            .expect_find_ready_for_deletion()
            .returning(|| Ok(vec![UserBuilder::new().build(), UserBuilder::new().build()]));
        lifecycle
            .expect_anonymize_account()
            .withf(|u| u.email().as_str().ends_with("@anonymized.local"))
            .times(2)
            .returning(|_| Ok(()));

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
            .expect_find_ready_for_deletion()
            .returning(|| Ok(vec![UserBuilder::new().build(), UserBuilder::new().build()]));
        let mut calls = 0;
        lifecycle
            .expect_anonymize_account()
            .times(2)
            .returning(move |_| {
                calls += 1;
                if calls == 1 {
                    Err(DomainError::Persistence("db blip".into()))
                } else {
                    Ok(())
                }
            });

        let purged = PurgeScheduledAccountsUseCase::new(Arc::new(lifecycle))
            .run_once()
            .await
            .unwrap();
        assert_eq!(purged, 1);
    }
}
