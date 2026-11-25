//! Shared brute-force attempt-lockout policy for one-time 6-digit codes.
//!
//! The email-verification and password-reset flows enforce an identical
//! brute-force policy: cap failed guesses at [`MAX_ATTEMPTS`], lock a code for
//! [`LOCK_DURATION_MINUTES`] once the cap is hit, and gate resends behind a
//! [`RESEND_COOLDOWN_MINUTES`] cooldown. Only the *code expiry* differs between
//! the two flows (24h vs 15min) and stays on each entity. This module owns the
//! shared arithmetic so the two flows cannot drift.

use jiff::{SignedDuration, Timestamp};

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
    last_sent_at: Timestamp,
    lockouts: i32,
}

impl AttemptLockout {
    /// Creates a lockout view over an attempt count, last-sent timestamp, and
    /// the number of earlier lockouts.
    #[must_use]
    pub const fn new(attempts: i32, last_sent_at: Timestamp, lockouts: i32) -> Self {
        Self {
            attempts,
            last_sent_at,
            lockouts,
        }
    }

    /// The lock window for the current code: [`LOCK_DURATION_MINUTES`] doubled
    /// once per earlier lockout, capped at [`MAX_LOCK_DURATION_MINUTES`].
    #[must_use]
    pub const fn lock_duration(&self) -> SignedDuration {
        SignedDuration::from_mins(lock_window_minutes(self.lockouts))
    }

    /// Whether the code is at or beyond the failed-attempt cap.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        at_cap(self.attempts)
    }

    /// Seconds until a locked code may be replaced via resend, measured from
    /// `last_sent_at`, as of `now`. Returns 0 when the code is not locked or
    /// once the lock window has elapsed.
    #[must_use]
    pub fn seconds_until_unlock(&self, now: Timestamp) -> i64 {
        seconds_until_unlock_at(
            self.attempts,
            self.lockouts,
            self.last_sent_at.as_microsecond(),
            now.as_microsecond(),
        )
    }

    /// Whether a new code can be sent as of `now` (resend cooldown elapsed).
    #[must_use]
    pub fn can_resend(&self, now: Timestamp) -> bool {
        can_resend_at(self.last_sent_at.as_microsecond(), now.as_microsecond())
    }

    /// Seconds until a resend is allowed as of `now`; 0 if a resend is already
    /// allowed.
    #[must_use]
    pub fn seconds_until_resend(&self, now: Timestamp) -> i64 {
        seconds_until_resend_at(self.last_sent_at.as_microsecond(), now.as_microsecond())
    }

    /// Whether a replacement code may be issued as of `now`. Callers pass
    /// `Timestamp::now()`; taking it as an argument lets the proofs cover every time.
    ///
    /// Issuing upserts a fresh row with `attempts = 0`, so every issuing path
    /// must ask this first or it would clear an active lockout. A lock takes
    /// precedence over the cooldown: it is the longer wait and the one the user
    /// needs to hear about.
    #[must_use]
    pub fn reissue(&self, now: Timestamp) -> Reissue {
        reissue_at(
            self.attempts,
            self.lockouts,
            self.last_sent_at.as_microsecond(),
            now.as_microsecond(),
        )
    }
}

// The lockout policy over plain integers: times are whole microseconds since
// the Unix epoch (`Timestamp::as_microsecond`, exact for every time jiff can
// represent). The `AttemptLockout` methods are thin conversions onto these, so
// the Kani proofs (`verification` below) cover the real decision logic; proving
// it through jiff's timestamp arithmetic directly doesn't finish (design D5).

/// The single definition of "at the failed-attempt cap", shared by
/// [`AttemptLockout::is_locked`] and the integer policy below.
const fn at_cap(attempts: i32) -> bool {
    attempts >= MAX_ATTEMPTS
}

/// Whether a one-time code with expiry `expires_at` has expired as of `now`.
///
/// At its expiry instant it no longer works (threat model P-19). Both code
/// entities call it, and refresh tokens share `expired_at`. Times compare to
/// the microsecond, Postgres's precision.
#[must_use]
pub fn expired(expires_at: Timestamp, now: Timestamp) -> bool {
    expired_at(expires_at.as_microsecond(), now.as_microsecond())
}

/// Whether a magic link with expiry `expires_at` can't be redeemed at `now`.
///
/// There is none, or it has expired. Only the link's own expiry counts, so a
/// resend, which moves the code's expiry, never revives it.
#[must_use]
pub fn magic_link_expired(expires_at: Option<Timestamp>, now: Timestamp) -> bool {
    magic_link_expired_at(
        expires_at.map(Timestamp::as_microsecond),
        now.as_microsecond(),
    )
}

/// The expiry rule over microseconds: expired from the expiry instant on.
pub(crate) const fn expired_at(expires_us: i64, now_us: i64) -> bool {
    now_us >= expires_us
}

const fn magic_link_expired_at(expires_us: Option<i64>, now_us: i64) -> bool {
    match expires_us {
        Some(expires_us) => expired_at(expires_us, now_us),
        None => true,
    }
}

const MICROS_PER_SECOND: i64 = 1_000_000;
const MICROS_PER_MINUTE: i64 = 60 * MICROS_PER_SECOND;

/// The lock window in minutes for a code with `lockouts` earlier lockouts.
const fn lock_window_minutes(lockouts: i32) -> i64 {
    // 2^7 * 15 min already exceeds the cap, so clamping the exponent keeps the
    // shift in range without changing the result.
    let doublings = if lockouts < 0 {
        0
    } else if lockouts > 7 {
        7
    } else {
        lockouts.unsigned_abs()
    };
    let minutes = LOCK_DURATION_MINUTES << doublings;
    if minutes < MAX_LOCK_DURATION_MINUTES {
        minutes
    } else {
        MAX_LOCK_DURATION_MINUTES
    }
}

/// The lock window in minutes by earlier lockout count: entry `n` is the
/// window after `n` lockouts, and the last entry holds for every count beyond
/// it (a negative count reads entry 0).
///
/// The repositories' issuing upsert passes this table to SQL, so the guard
/// that refuses to replace a locked code (P-11) uses the same windows as
/// [`AttemptLockout::reissue`] rather than restating the doubling rule. Kani
/// proves the table and [`lock_window_minutes`] agree for every `i32`
/// (`lock_window_table_matches_the_policy`).
pub const LOCK_WINDOWS_MINUTES: [i64; 8] = {
    let mut table = [0; 8];
    let mut index = 0;
    let mut lockouts = 0;
    while index < table.len() {
        table[index] = lock_window_minutes(lockouts);
        index += 1;
        lockouts += 1;
    }
    table
};

/// Seconds from `now_us` until `until_us`, rounded up, so any time left
/// counts as a second; 0 once `until_us` has passed.
///
/// Rounding up matters: rounding down reported a lock with under a second left
/// as over, so a locked code could be replaced up to a second early (found by
/// `locked_inside_the_window_is_never_reissued`, OT-010 slice 3).
const fn whole_seconds_until(until_us: i64, now_us: i64) -> i64 {
    let remaining = until_us.saturating_sub(now_us);
    if remaining <= 0 {
        0
    } else {
        (remaining - 1) / MICROS_PER_SECOND + 1
    }
}

const fn seconds_until_unlock_at(attempts: i32, lockouts: i32, sent_us: i64, now_us: i64) -> i64 {
    if !at_cap(attempts) {
        return 0;
    }
    let unlock_us = sent_us.saturating_add(lock_window_minutes(lockouts) * MICROS_PER_MINUTE);
    whole_seconds_until(unlock_us, now_us)
}

const fn resend_allowed_at(sent_us: i64) -> i64 {
    sent_us.saturating_add(RESEND_COOLDOWN_MINUTES * MICROS_PER_MINUTE)
}

const fn can_resend_at(sent_us: i64, now_us: i64) -> bool {
    resend_allowed_at(sent_us) <= now_us
}

const fn seconds_until_resend_at(sent_us: i64, now_us: i64) -> i64 {
    whole_seconds_until(resend_allowed_at(sent_us), now_us)
}

/// The reissue decision: see [`AttemptLockout::reissue`].
const fn reissue_at(attempts: i32, lockouts: i32, sent_us: i64, now_us: i64) -> Reissue {
    let seconds_until_unlock = seconds_until_unlock_at(attempts, lockouts, sent_us, now_us);
    if seconds_until_unlock > 0 {
        Reissue::Locked {
            seconds_remaining: seconds_until_unlock,
        }
    } else if !can_resend_at(sent_us, now_us) {
        Reissue::Cooldown {
            seconds_remaining: seconds_until_resend_at(sent_us, now_us),
        }
    } else {
        Reissue::Allowed
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

/// What happened to a newly issued one-time code when it was stored.
///
/// The issuing paths decide with [`AttemptLockout::reissue`] on a row they
/// read earlier. A wrong guess can lock that row in between, or a concurrent
/// request can issue a code first, so the store re-checks the whole reissue
/// rule in the same statement as the write (P-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueOutcome {
    /// The code is stored, replacing any earlier one.
    Issued,
    /// Nothing was written: at the new code's send time, the stored code was
    /// locked inside its window or still inside the resend cooldown.
    Withheld,
}

/// Kani proofs (OT-010, threat model P-10 / P-13). Run with `just verify`.
#[cfg(kani)]
pub(crate) mod verification {
    use super::*;

    /// The lock window for a lockout count, in minutes. `last_sent_at` and
    /// `attempts` don't affect it.
    fn window_minutes(lockouts: i32) -> i64 {
        AttemptLockout::new(MAX_ATTEMPTS, Timestamp::UNIX_EPOCH, lockouts)
            .lock_duration()
            .as_mins()
    }

    /// For every `lockouts: i32` (all 2^32 values, including negative and
    /// corrupt counts): `lock_duration` doesn't panic or overflow, and the
    /// window is between 15 minutes and 24 hours. The bounds are the spec's
    /// literal values, not the constants, so a changed constant fails here.
    #[kani::proof]
    fn lock_window_is_between_15_minutes_and_24_hours() {
        let minutes = window_minutes(kani::any());
        assert!(minutes >= 15, "lock window shorter than 15 minutes");
        assert!(minutes <= 24 * 60, "lock window longer than 24 hours");
    }

    /// 2000-01-01T00:00:00Z and 2200-01-01T00:00:00Z in Unix microseconds: the
    /// bounds on every symbolic timestamp below (design D5).
    const FROM_US: i64 = 946_684_800 * MICROS_PER_SECOND;
    const UNTIL_US: i64 = 7_258_118_400 * MICROS_PER_SECOND;

    /// A symbolic timestamp in [2000, 2200), to the microsecond.
    pub(crate) fn any_time() -> i64 {
        let us: i64 = kani::any();
        kani::assume((FROM_US..UNTIL_US).contains(&us));
        us
    }

    /// Symbolic `attempts` and `lockouts` (any `i32`), and send and now times
    /// in [2000, 2200) in either order (a clock behind the send time is
    /// covered). Returns them with the lock window in microseconds.
    fn any_case() -> (i32, i32, i64, i64, i64) {
        let lockouts: i32 = kani::any();
        let window_us = lock_window_minutes(lockouts) * MICROS_PER_MINUTE;
        (kani::any(), lockouts, any_time(), any_time(), window_us)
    }

    /// Spec: a locked code can't be replaced at any instant inside its lock
    /// window, and the response says how long is left (> 0).
    #[kani::proof]
    fn locked_inside_the_window_is_never_reissued() {
        let (attempts, lockouts, sent, now, window) = any_case();
        kani::assume(attempts >= 5);
        kani::assume(now < sent + window);
        assert!(matches!(
            reissue_at(attempts, lockouts, sent, now),
            Reissue::Locked { seconds_remaining } if seconds_remaining > 0
        ));
    }

    /// Spec: a code that isn't locked can't be replaced within 5 minutes of the
    /// last send, and the response says how long is left (> 0).
    #[kani::proof]
    fn unlocked_inside_the_cooldown_is_refused() {
        let (attempts, lockouts, sent, now, _) = any_case();
        kani::assume(attempts < 5);
        kani::assume(now < sent + 5 * 60 * MICROS_PER_SECOND);
        assert!(matches!(
            reissue_at(attempts, lockouts, sent, now),
            Reissue::Cooldown { seconds_remaining } if seconds_remaining > 0
        ));
    }

    /// Spec: once the lock window (if locked) and the 5-minute cooldown have
    /// both passed, a replacement is issued.
    #[kani::proof]
    fn reissued_once_every_wait_has_passed() {
        let (attempts, lockouts, sent, now, window) = any_case();
        kani::assume(attempts < 5 || now >= sent + window);
        kani::assume(now >= sent + 5 * 60 * MICROS_PER_SECOND);
        assert_eq!(reissue_at(attempts, lockouts, sent, now), Reissue::Allowed);
    }

    /// Spec: every reported wait is a whole number of seconds that is never
    /// negative, and a code below the cap is never reported locked. Over every
    /// input, with no assumption on the order of the send and now times.
    #[kani::proof]
    fn reported_waits_are_never_negative() {
        let (attempts, lockouts, sent, now, _) = any_case();
        match reissue_at(attempts, lockouts, sent, now) {
            Reissue::Allowed => {}
            Reissue::Locked { seconds_remaining } => {
                assert!(attempts >= 5, "reported locked below the cap");
                assert!(seconds_remaining >= 0);
            }
            Reissue::Cooldown { seconds_remaining } => assert!(seconds_remaining >= 0),
        }
    }

    /// For every `lockouts: i32`: the window is exactly the spec's schedule,
    /// written out with literal numbers rather than the constants. 15 minutes
    /// for no (or a corrupt negative) earlier lockout, doubling per earlier
    /// lockout (15, 30, 60 … 960 minutes for 0 to 6), and 24 hours from the
    /// 7th on, where the doubling first passes the cap.
    #[kani::proof]
    fn lock_window_doubles_per_earlier_lockout_up_to_24_hours() {
        let lockouts: i32 = kani::any();
        let expected = match lockouts {
            i32::MIN..=0 => 15,
            1 => 30,
            2 => 60,
            3 => 120,
            4 => 240,
            5 => 480,
            6 => 960,
            7..=i32::MAX => 1440,
        };
        assert_eq!(window_minutes(lockouts), expected);
    }

    /// For every `i32` lockout count: the table the issuing upsert's SQL
    /// guard reads, indexed the way that SQL indexes it (clamped to the table),
    /// gives the same window as the policy.
    #[kani::proof]
    fn lock_window_table_matches_the_policy() {
        let lockouts: i32 = kani::any();
        let last = LOCK_WINDOWS_MINUTES.len() - 1;
        let index = usize::try_from(lockouts.max(0)).map_or(last, |i| i.min(last));
        assert_eq!(LOCK_WINDOWS_MINUTES[index], lock_window_minutes(lockouts));
    }

    /// P-19, for every expiry and current time in [2000, 2200), to the
    /// microsecond: a code is expired exactly from its expiry instant on,
    /// whatever its attempts (`expired` takes none). Both code entities'
    /// `is_expired`, which every entry path checks first, call this rule.
    #[kani::proof]
    fn a_code_is_refused_from_its_expiry_instant() {
        let (expires, now) = (any_time(), any_time());
        assert_eq!(expired_at(expires, now), now >= expires);
        if now < expires {
            assert!(!expired_at(expires, now), "refused before expiry");
        }
    }

    /// P-19, for every link expiry (or none) and current time in [2000,
    /// 2200): a missing link counts as expired, and a link is live exactly
    /// before its own expiry.
    #[kani::proof]
    fn a_magic_link_is_live_only_before_its_own_expiry() {
        let expires: Option<i64> = if kani::any() { Some(any_time()) } else { None };
        let now = any_time();
        let expired = magic_link_expired_at(expires, now);
        match expires {
            None => assert!(expired, "a missing link was live"),
            Some(at) => assert_eq!(expired, now >= at),
        }
    }

    /// For every pair `a <= b` of `i32` lockout counts: more earlier lockouts
    /// never give a shorter window.
    #[kani::proof]
    fn lock_window_never_shrinks_as_lockouts_grow() {
        let a: i32 = kani::any();
        let b: i32 = kani::any();
        kani::assume(a <= b);
        assert!(window_minutes(a) <= window_minutes(b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One fixed instant, so the tests never depend on the wall clock.
    fn now() -> Timestamp {
        Timestamp::from_second(1_800_000_000).expect("valid timestamp")
    }

    fn lockout(attempts: i32, sent_minutes_ago: i64) -> AttemptLockout {
        AttemptLockout::new(
            attempts,
            now() - SignedDuration::from_mins(sent_minutes_ago),
            0,
        )
    }

    fn after_lockouts(lockouts: i32) -> AttemptLockout {
        AttemptLockout::new(MAX_ATTEMPTS, now(), lockouts)
    }

    #[test]
    fn lock_duration_doubles_per_earlier_lockout() {
        assert_eq!(
            after_lockouts(0).lock_duration(),
            SignedDuration::from_mins(15)
        );
        assert_eq!(
            after_lockouts(1).lock_duration(),
            SignedDuration::from_mins(30)
        );
        assert_eq!(
            after_lockouts(2).lock_duration(),
            SignedDuration::from_mins(60)
        );
        assert_eq!(
            after_lockouts(6).lock_duration(),
            SignedDuration::from_mins(960)
        );
    }

    #[test]
    fn lock_duration_is_capped() {
        let cap = SignedDuration::from_mins(MAX_LOCK_DURATION_MINUTES);
        assert_eq!(after_lockouts(7).lock_duration(), cap);
        assert_eq!(after_lockouts(1_000).lock_duration(), cap);
    }

    #[test]
    fn escalated_lock_outlasts_base_window() {
        let l = AttemptLockout::new(
            MAX_ATTEMPTS,
            now() - SignedDuration::from_mins(LOCK_DURATION_MINUTES + 1),
            2,
        );
        assert!(
            l.seconds_until_unlock(now()) > 0,
            "third lock lasts an hour, not 15 min"
        );
    }

    #[test]
    fn not_locked_below_cap() {
        assert!(!lockout(MAX_ATTEMPTS - 1, 0).is_locked());
        assert_eq!(lockout(MAX_ATTEMPTS - 1, 0).seconds_until_unlock(now()), 0);
    }

    #[test]
    fn locked_at_cap_within_window() {
        let l = lockout(MAX_ATTEMPTS, 0);
        assert!(l.is_locked());
        let secs = l.seconds_until_unlock(now());
        assert!(secs > 0 && secs <= LOCK_DURATION_MINUTES * 60);
    }

    #[test]
    fn locked_but_window_elapsed() {
        let l = lockout(MAX_ATTEMPTS, LOCK_DURATION_MINUTES + 1);
        assert!(l.is_locked());
        assert_eq!(l.seconds_until_unlock(now()), 0);
    }

    #[test]
    fn reissue_states() {
        assert_eq!(
            lockout(0, RESEND_COOLDOWN_MINUTES + 1).reissue(now()),
            Reissue::Allowed
        );
        assert!(matches!(
            lockout(0, 0).reissue(now()),
            Reissue::Cooldown { seconds_remaining } if seconds_remaining > 0
        ));
        assert!(matches!(
            lockout(MAX_ATTEMPTS, 0).reissue(now()),
            Reissue::Locked { seconds_remaining } if seconds_remaining > 0
        ));
    }

    /// Past the cooldown but inside the lock window: the lock is reported.
    #[test]
    fn reissue_lock_outranks_cooldown() {
        let l = lockout(MAX_ATTEMPTS, RESEND_COOLDOWN_MINUTES + 1);
        assert!(l.can_resend(now()));
        assert!(matches!(l.reissue(now()), Reissue::Locked { .. }));
    }

    /// A locked code whose window has elapsed may be replaced.
    #[test]
    fn reissue_allowed_once_lock_elapses() {
        assert_eq!(
            lockout(MAX_ATTEMPTS, LOCK_DURATION_MINUTES + 1).reissue(now()),
            Reissue::Allowed
        );
    }

    /// Regression (found by Kani, OT-010 slice 3): with under a second of
    /// the lock left, the code is still locked and the wait rounds up to 1 s.
    #[test]
    fn a_lock_with_under_a_second_left_still_holds() {
        let now = now();
        let sent = now - SignedDuration::from_mins(LOCK_DURATION_MINUTES)
            + SignedDuration::from_millis(500);
        let l = AttemptLockout::new(MAX_ATTEMPTS, sent, 0);
        assert_eq!(
            l.reissue(now),
            Reissue::Locked {
                seconds_remaining: 1
            }
        );
    }

    #[test]
    fn resend_cooldown() {
        assert!(!lockout(0, 0).can_resend(now()));
        assert!(lockout(0, 0).seconds_until_resend(now()) > 0);
        assert!(lockout(0, RESEND_COOLDOWN_MINUTES + 1).can_resend(now()));
        assert_eq!(
            lockout(0, RESEND_COOLDOWN_MINUTES + 1).seconds_until_resend(now()),
            0
        );
    }
}
