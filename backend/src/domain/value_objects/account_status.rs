//! Where a user account is in its deletion lifecycle.

use chrono::{DateTime, Utc};

/// Where a user account is in its deletion lifecycle.
///
/// ```text
///            schedule                 grace period elapses
///   Active ───────────▶ PendingDeletion ─────────────────▶ Deleted
///      ▲                     │                               (terminal)
///      └──────── cancel ─────┘
/// ```
///
/// Persisted as two nullable columns (`deleted_at`, `deletion_scheduled_at`);
/// this type is the only place that decides what a pair of them means, so a
/// deleted account can never also be pending deletion.
///
/// Generic over the timestamp so the Kani proofs (`verification` below) can
/// run the same transitions over integer times; the app uses the default,
/// `DateTime<Utc>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountStatus<T = DateTime<Utc>> {
    /// A normal, live account.
    #[default]
    Active,
    /// Deletion requested; the account is purged once `scheduled_for` passes
    /// unless the user cancels first.
    PendingDeletion {
        /// When the grace period ends.
        scheduled_for: T,
    },
    /// Anonymized. Terminal: no transition leaves this state.
    Deleted {
        /// When the account was anonymized.
        deleted_at: T,
    },
}

/// A deletion-lifecycle change refused because the account is already
/// deleted, which is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyDeleted;

impl<T: Copy + PartialOrd> AccountStatus<T> {
    /// Builds the status from its two persisted columns.
    ///
    /// `deleted_at` wins when both are set: deletion is terminal, so a schedule
    /// left beside it is stale and is dropped (the next save writes it back as
    /// `NULL`).
    #[must_use]
    pub const fn from_columns(deleted_at: Option<T>, deletion_scheduled_at: Option<T>) -> Self {
        match (deleted_at, deletion_scheduled_at) {
            (Some(deleted_at), _) => Self::Deleted { deleted_at },
            (None, Some(scheduled_for)) => Self::PendingDeletion { scheduled_for },
            (None, None) => Self::Active,
        }
    }

    /// The `deleted_at` column value.
    #[must_use]
    pub const fn deleted_at(self) -> Option<T> {
        match self {
            Self::Deleted { deleted_at } => Some(deleted_at),
            Self::Active | Self::PendingDeletion { .. } => None,
        }
    }

    /// The `deletion_scheduled_at` column value.
    #[must_use]
    pub const fn deletion_scheduled_at(self) -> Option<T> {
        match self {
            Self::PendingDeletion { scheduled_for } => Some(scheduled_for),
            Self::Active | Self::Deleted { .. } => None,
        }
    }

    /// Schedules deletion for `scheduled_for` and returns the date in force.
    /// An account already pending deletion keeps its existing date.
    ///
    /// # Errors
    /// [`AlreadyDeleted`] if the account is deleted; the status is unchanged.
    pub const fn schedule_deletion(&mut self, scheduled_for: T) -> Result<T, AlreadyDeleted> {
        match *self {
            Self::Active => {
                *self = Self::PendingDeletion { scheduled_for };
                Ok(scheduled_for)
            }
            Self::PendingDeletion { scheduled_for } => Ok(scheduled_for),
            Self::Deleted { .. } => Err(AlreadyDeleted),
        }
    }

    /// Cancels a pending deletion; a no-op on an active account.
    ///
    /// # Errors
    /// [`AlreadyDeleted`] if the account is deleted; the status is unchanged.
    pub const fn cancel_deletion(&mut self) -> Result<(), AlreadyDeleted> {
        match *self {
            Self::Active | Self::PendingDeletion { .. } => {
                *self = Self::Active;
                Ok(())
            }
            Self::Deleted { .. } => Err(AlreadyDeleted),
        }
    }

    /// Marks the account deleted as of `now`.
    ///
    /// # Errors
    /// [`AlreadyDeleted`] if it already is; the original deletion time stays.
    pub const fn anonymize(&mut self, now: T) -> Result<(), AlreadyDeleted> {
        match *self {
            Self::Active | Self::PendingDeletion { .. } => {
                *self = Self::Deleted { deleted_at: now };
                Ok(())
            }
            Self::Deleted { .. } => Err(AlreadyDeleted),
        }
    }

    /// Whether a pending deletion's time has come as of `now`. The purge job
    /// selects and writes with the same rule in SQL.
    #[must_use]
    pub fn is_ready_to_purge(self, now: T) -> bool {
        matches!(self, Self::PendingDeletion { scheduled_for } if scheduled_for <= now)
    }
}

/// Kani proofs (OT-010, threat model P-28). Run with `just verify`. Times are
/// any `i64` (the transitions only copy and compare them), so every instant,
/// in any order, is covered.
#[cfg(kani)]
mod verification {
    use super::*;

    /// Any status, with any times.
    fn any_status() -> AccountStatus<i64> {
        match kani::any::<u8>() {
            0 => AccountStatus::Active,
            1 => AccountStatus::PendingDeletion {
                scheduled_for: kani::any(),
            },
            _ => AccountStatus::Deleted {
                deleted_at: kani::any(),
            },
        }
    }

    /// From `Deleted`, scheduling, cancelling and anonymising are each
    /// refused, and the status and its deletion time don't change. From any
    /// other status, anonymising deletes at `now`.
    #[kani::proof]
    fn a_deleted_account_stays_deleted() {
        let start = any_status();
        let deleted = matches!(start, AccountStatus::Deleted { .. });

        let mut status = start;
        let scheduled = status.schedule_deletion(kani::any());
        assert_eq!(scheduled.is_err(), deleted);
        if deleted {
            assert_eq!(status, start);
        }

        let mut status = start;
        assert_eq!(status.cancel_deletion().is_err(), deleted);
        if deleted {
            assert_eq!(status, start);
        }

        let mut status = start;
        let now: i64 = kani::any();
        let anonymized = status.anonymize(now);
        if deleted {
            assert_eq!(anonymized, Err(AlreadyDeleted));
            assert_eq!(status, start);
        } else {
            assert_eq!(status, AccountStatus::Deleted { deleted_at: now });
        }
    }

    /// Scheduling while pending keeps the existing date, and scheduling an
    /// active account sets the requested one.
    #[kani::proof]
    fn rescheduling_keeps_the_original_date() {
        let start = any_status();
        let mut status = start;
        let requested: i64 = kani::any();
        let result = status.schedule_deletion(requested);
        match start {
            AccountStatus::PendingDeletion { scheduled_for } => {
                assert_eq!(result, Ok(scheduled_for));
                assert_eq!(status, start);
            }
            AccountStatus::Active => {
                assert_eq!(result, Ok(requested));
                assert_eq!(status.deletion_scheduled_at(), Some(requested));
            }
            AccountStatus::Deleted { .. } => assert!(result.is_err()),
        }
    }

    /// For every status and time: ready to purge exactly when pending and the
    /// scheduled time has come.
    #[kani::proof]
    fn a_pending_deletion_is_not_purged_early() {
        let status = any_status();
        let now: i64 = kani::any();
        let ready = status.is_ready_to_purge(now);
        match status {
            AccountStatus::PendingDeletion { scheduled_for } => {
                assert_eq!(ready, scheduled_for <= now);
            }
            AccountStatus::Active | AccountStatus::Deleted { .. } => assert!(!ready),
        }
    }

    /// For every pair of columns: a deleted time wins over a schedule, and
    /// every status reads back as itself from its columns.
    #[kani::proof]
    fn the_columns_round_trip_and_deleted_wins() {
        let deleted: Option<i64> = if kani::any() { Some(kani::any()) } else { None };
        let scheduled: Option<i64> = if kani::any() { Some(kani::any()) } else { None };
        let status = AccountStatus::from_columns(deleted, scheduled);
        if let Some(at) = deleted {
            assert_eq!(status, AccountStatus::Deleted { deleted_at: at });
        }
        let any = any_status();
        assert_eq!(
            AccountStatus::from_columns(any.deleted_at(), any.deletion_scheduled_at()),
            any
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_columns_maps_each_combination() {
        let t = Utc::now();
        assert_eq!(
            AccountStatus::<DateTime<Utc>>::from_columns(None, None),
            AccountStatus::Active
        );
        assert_eq!(
            AccountStatus::from_columns(None, Some(t)),
            AccountStatus::PendingDeletion { scheduled_for: t }
        );
        assert_eq!(
            AccountStatus::from_columns(Some(t), None),
            AccountStatus::Deleted { deleted_at: t }
        );
    }

    #[test]
    fn deleted_wins_over_a_stale_schedule() {
        let deleted = Utc::now();
        let scheduled = deleted + chrono::Duration::days(1);
        let status = AccountStatus::from_columns(Some(deleted), Some(scheduled));
        assert_eq!(
            status,
            AccountStatus::Deleted {
                deleted_at: deleted
            }
        );
        assert_eq!(status.deletion_scheduled_at(), None);
    }

    #[test]
    fn columns_round_trip() {
        let t = Utc::now();
        for status in [
            AccountStatus::Active,
            AccountStatus::PendingDeletion { scheduled_for: t },
            AccountStatus::Deleted { deleted_at: t },
        ] {
            assert_eq!(
                AccountStatus::from_columns(status.deleted_at(), status.deletion_scheduled_at()),
                status
            );
        }
    }
}
