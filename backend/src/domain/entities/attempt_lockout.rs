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
/// `last_sent_at`, for a user with no earlier lockouts.
///
/// Bounds the brute-force lockout so a locked code can be replaced via resend
/// after a short cooldown rather than being clearable immediately (which would
/// reset the attempt counter and defeat the lockout). Each earlier lockout
/// doubles it, up to [`MAX_LOCK_DURATION_MINUTES`].
pub const LOCK_DURATION_MINUTES: i64 = 15;

/// Upper bound on the escalating lock window (24 hours).
///
/// With the cap reached after 7 lockouts, a sustained campaign gets about 35
/// guesses in its first day and 5 a day after that, against 10^6 possible codes.
pub const MAX_LOCK_DURATION_MINUTES: i64 = 24 * 60;

/// Days a once-locked code row is kept after its last send.
///
/// Lets the `lockouts` count outlive the code's own expiry: an attacker who waits
/// for expiry cleanup must not get the short first lock window back.
pub const LOCKOUT_MEMORY_DAYS: i32 = 30;

/// Shared lockout/cooldown arithmetic over an attempt count, the time the code
/// was last sent, and how many earlier codes were replaced while locked.
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
    lockouts: i32,
}

impl AttemptLockout {
    /// Creates a lockout view over an attempt count, last-sent timestamp, and
    /// the number of earlier lockouts.
    #[must_use]
    pub const fn new(attempts: i32, last_sent_at: DateTime<Utc>, lockouts: i32) -> Self {
        Self {
            attempts,
            last_sent_at,
            lockouts,
        }
    }

    /// The lock window for the current code: [`LOCK_DURATION_MINUTES`] doubled
    /// once per earlier lockout, capped at [`MAX_LOCK_DURATION_MINUTES`].
    #[must_use]
    pub fn lock_duration(&self) -> Duration {
        // 2^7 * 15 min already exceeds the cap, so clamping the exponent keeps
        // the shift in range without changing the result.
        let doublings = self.lockouts.clamp(0, 7).unsigned_abs();
        let minutes = (LOCK_DURATION_MINUTES << doublings).min(MAX_LOCK_DURATION_MINUTES);
        Duration::minutes(minutes)
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
        let unlock_at = self.last_sent_at + self.lock_duration();
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

    /// Whether a replacement code may be issued now.
    ///
    /// Issuing upserts a fresh row with `attempts = 0`, so every issuing path
    /// must ask this first or it would clear an active lockout. A lock takes
    /// precedence over the cooldown: it is the longer wait and the one the user
    /// needs to hear about.
    #[must_use]
    pub fn reissue(&self) -> Reissue {
        let seconds_until_unlock = self.seconds_until_unlock();
        if seconds_until_unlock > 0 {
            Reissue::Locked {
                seconds_remaining: seconds_until_unlock,
            }
        } else if !self.can_resend() {
            Reissue::Cooldown {
                seconds_remaining: self.seconds_until_resend(),
            }
        } else {
            Reissue::Allowed
        }
    }
}

/// Whether a replacement one-time code may be issued, and if not, why.
///
/// See [`AttemptLockout::reissue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reissue {
    /// No lock or cooldown applies; a new code may be issued.
    Allowed,
    /// The current code is at the attempt cap and inside its lock window.
    Locked {
        /// Seconds until the lock window ends.
        seconds_remaining: i64,
    },
    /// The current code was sent within the resend cooldown.
    Cooldown {
        /// Seconds until the cooldown ends (may be 0 in its final second).
        seconds_remaining: i64,
    },
}

impl Reissue {
    /// Whether a new code may be issued now.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
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
        AttemptLockout::new(
            attempts,
            Utc::now() - Duration::minutes(sent_minutes_ago),
            0,
        )
    }

    fn after_lockouts(lockouts: i32) -> AttemptLockout {
        AttemptLockout::new(MAX_ATTEMPTS, Utc::now(), lockouts)
    }

    #[test]
    fn lock_duration_doubles_per_earlier_lockout() {
        assert_eq!(after_lockouts(0).lock_duration(), Duration::minutes(15));
        assert_eq!(after_lockouts(1).lock_duration(), Duration::minutes(30));
        assert_eq!(after_lockouts(2).lock_duration(), Duration::minutes(60));
        assert_eq!(after_lockouts(6).lock_duration(), Duration::minutes(960));
    }

    #[test]
    fn lock_duration_is_capped() {
        let cap = Duration::minutes(MAX_LOCK_DURATION_MINUTES);
        assert_eq!(after_lockouts(7).lock_duration(), cap);
        assert_eq!(after_lockouts(1_000).lock_duration(), cap);
    }

    #[test]
    fn escalated_lock_outlasts_base_window() {
        let l = AttemptLockout::new(
            MAX_ATTEMPTS,
            Utc::now() - Duration::minutes(LOCK_DURATION_MINUTES + 1),
            2,
        );
        assert!(
            l.seconds_until_unlock() > 0,
            "third lock lasts an hour, not 15 min"
        );
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
    fn reissue_states() {
        assert_eq!(
            lockout(0, RESEND_COOLDOWN_MINUTES + 1).reissue(),
            Reissue::Allowed
        );
        assert!(matches!(
            lockout(0, 0).reissue(),
            Reissue::Cooldown { seconds_remaining } if seconds_remaining > 0
        ));
        assert!(matches!(
            lockout(MAX_ATTEMPTS, 0).reissue(),
            Reissue::Locked { seconds_remaining } if seconds_remaining > 0
        ));
    }

    /// Past the cooldown but inside the lock window: the lock is reported.
    #[test]
    fn reissue_lock_outranks_cooldown() {
        let l = lockout(MAX_ATTEMPTS, RESEND_COOLDOWN_MINUTES + 1);
        assert!(l.can_resend());
        assert!(matches!(l.reissue(), Reissue::Locked { .. }));
    }

    /// A locked code whose window has elapsed may be replaced.
    #[test]
    fn reissue_allowed_once_lock_elapses() {
        assert_eq!(
            lockout(MAX_ATTEMPTS, LOCK_DURATION_MINUTES + 1).reissue(),
            Reissue::Allowed
        );
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
