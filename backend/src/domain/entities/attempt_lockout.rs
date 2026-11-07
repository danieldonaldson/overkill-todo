//! Shared brute-force attempt-lockout policy for one-time 6-digit codes.
//!
//! The email-verification and password-reset flows enforce an identical
//! brute-force policy: cap failed guesses at [`MAX_ATTEMPTS`], lock a code for
//! [`LOCK_DURATION_MINUTES`] once the cap is hit, and gate resends behind a
//! [`RESEND_COOLDOWN_MINUTES`] cooldown. Only the *code expiry* differs between
//! the two flows (24h vs 15min) and stays on each entity. This module owns the
//! shared arithmetic so the two flows cannot drift.

use chrono::{DateTime, Duration, Utc};

/// Maximum number of failed attempts before a code is locked.
pub const MAX_ATTEMPTS: i32 = 5;

/// Minutes before a new code can be sent (resend cooldown).
pub const RESEND_COOLDOWN_MINUTES: i64 = 5;

/// How long a code stays locked after the attempt cap is hit, measured from
/// `last_sent_at`.
///
/// Bounds the brute-force lockout so a locked code can be replaced via resend
/// after a short cooldown rather than being clearable immediately (which would
/// reset the attempt counter and defeat the lockout).
pub const LOCK_DURATION_MINUTES: i64 = 15;

/// Shared lockout/cooldown arithmetic over an attempt count and the time the
/// code was last sent.
///
/// Built on demand from an entity's `attempts` + `last_sent_at`; holds no
/// identity of its own. [`is_locked`](Self::is_locked) is the single definition
/// of "at the attempt cap", deliberately distinct from the repository's atomic
/// per-guess claim (`register_attempt`), which enforces the same cap under
/// concurrency. The two answer different questions: "is this code already
/// locked?" versus "may this individual guess proceed?".
#[derive(Debug, Clone, Copy)]
pub struct AttemptLockout {
    attempts: i32,
    last_sent_at: DateTime<Utc>,
}

impl AttemptLockout {
    /// Creates a lockout view over an attempt count and last-sent timestamp.
    #[must_use]
    pub const fn new(attempts: i32, last_sent_at: DateTime<Utc>) -> Self {
        Self {
            attempts,
            last_sent_at,
        }
    }

    /// Whether the code is at or beyond the failed-attempt cap.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.attempts >= MAX_ATTEMPTS
    }

    /// Seconds until a locked code may be replaced via resend, measured from
    /// `last_sent_at`. Returns 0 when the code is not locked or once the lock
    /// window has elapsed.
    #[must_use]
    pub fn seconds_until_unlock(&self) -> i64 {
        if !self.is_locked() {
            return 0;
        }
        let unlock_at = self.last_sent_at + Duration::minutes(LOCK_DURATION_MINUTES);
        (unlock_at - Utc::now()).num_seconds().max(0)
    }

    /// Whether a new code can be sent (resend cooldown elapsed).
    #[must_use]
    pub fn can_resend(&self) -> bool {
        self.last_sent_at + Duration::minutes(RESEND_COOLDOWN_MINUTES) <= Utc::now()
    }

    /// Seconds until a resend is allowed; 0 if a resend is already allowed.
    #[must_use]
    pub fn seconds_until_resend(&self) -> i64 {
        let allowed_at = self.last_sent_at + Duration::minutes(RESEND_COOLDOWN_MINUTES);
        (allowed_at - Utc::now()).num_seconds().max(0)
    }
}

/// Outcome of atomically claiming an attempt slot before the expensive code
/// comparison.
///
/// Distinguishes the three states a single claim statement can land in so
/// callers never conflate "locked" with "the code row is gone". The latter is
/// reachable when the row is deleted between loading it and claiming the slot
/// (a concurrent successful verification, or expiry cleanup), and must surface
/// an expired/no-code message rather than a misleading "too many attempts".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Slot claimed; carries the resulting attempt count (`1..=MAX_ATTEMPTS`).
    Claimed(i32),
    /// The code row exists but is already at the attempt cap.
    Locked,
    /// No code row exists for this user (expired or deleted concurrently).
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lockout(attempts: i32, sent_minutes_ago: i64) -> AttemptLockout {
        AttemptLockout::new(attempts, Utc::now() - Duration::minutes(sent_minutes_ago))
    }

    #[test]
    fn not_locked_below_cap() {
        assert!(!lockout(MAX_ATTEMPTS - 1, 0).is_locked());
        assert_eq!(lockout(MAX_ATTEMPTS - 1, 0).seconds_until_unlock(), 0);
    }

    #[test]
    fn locked_at_cap_within_window() {
        let l = lockout(MAX_ATTEMPTS, 0);
        assert!(l.is_locked());
        let secs = l.seconds_until_unlock();
        assert!(secs > 0 && secs <= LOCK_DURATION_MINUTES * 60);
    }

    #[test]
    fn locked_but_window_elapsed() {
        let l = lockout(MAX_ATTEMPTS, LOCK_DURATION_MINUTES + 1);
        assert!(l.is_locked());
        assert_eq!(l.seconds_until_unlock(), 0);
    }

    #[test]
    fn resend_cooldown() {
        assert!(!lockout(0, 0).can_resend());
        assert!(lockout(0, 0).seconds_until_resend() > 0);
        assert!(lockout(0, RESEND_COOLDOWN_MINUTES + 1).can_resend());
        assert_eq!(
            lockout(0, RESEND_COOLDOWN_MINUTES + 1).seconds_until_resend(),
            0
        );
    }
}
