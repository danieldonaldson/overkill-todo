//! Integration coverage for the atomic brute-force attempt cap.
//!
//! These tests pin the two properties the lockout has historically oscillated
//! on, so a future refactor can't silently regress either:
//!   - P1: the per-guess cap holds under concurrency (parallel claims that all
//!     start from `attempts = 0` cannot collectively exceed `MAX_ATTEMPTS`).
//!   - finding #2: a claim against a missing row reports `NotFound`, not a
//!     misleading lockout.
//!   - OT-010 slice 4: a claim names the code it will be compared against, so
//!     a guess whose code was replaced reports `NotFound` and takes no slot on
//!     the new code.

use std::sync::Arc;

use application::ports::{EmailVerificationRepository, PasswordResetRepository, UserRepository};
use domain::entities::attempt_lockout::{
    AttemptOutcome, IssueOutcome, LOCK_WINDOWS_MINUTES, MAX_ATTEMPTS, RESEND_COOLDOWN_MINUTES,
};
use domain::entities::{EmailVerificationCode, PasswordResetCode, User};
use domain::value_objects::{Email, ResetCodeId, UserId, VerificationCodeId};
use jiff::{SignedDuration, Timestamp};
use persistence_postgres::{
    Database, PostgresEmailVerificationRepository, PostgresPasswordResetRepository,
    PostgresUserRepository,
};
use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn register_attempt_caps_under_concurrency(pool: PgPool) {
    // More parallel claims than the cap allows; they all observe attempts = 0.
    const PARALLEL: i32 = 25;

    let db = Arc::new(Database::from_pool(pool));
    let user_repo = PostgresUserRepository::new(db.clone());
    let reset_repo = Arc::new(PostgresPasswordResetRepository::new(db));

    // Seed a user with a fresh reset code (attempts = 0).
    let user = User::new(
        Email::new("race@example.com").unwrap(),
        Some("password_hash".to_string()),
        "Race".to_string(),
        "Tester".to_string(),
    );
    user_repo.save(&user).await.unwrap();
    let user_id = *user.id();
    let code = PasswordResetCode::new(user_id, "hashed_code".to_string());
    reset_repo.save(&code).await.unwrap();
    let code_id = *code.id();

    // Fire many more parallel claims than the cap allows.
    let mut handles = Vec::new();
    for _ in 0..PARALLEL {
        let repo = reset_repo.clone();
        handles.push(tokio::spawn(async move {
            repo.register_attempt(&user_id, &code_id, MAX_ATTEMPTS)
                .await
                .unwrap()
        }));
    }

    let mut claimed = 0;
    let mut locked = 0;
    let mut not_found = 0;
    for handle in handles {
        match handle.await.unwrap() {
            AttemptOutcome::Claimed(_) => claimed += 1,
            AttemptOutcome::Locked => locked += 1,
            AttemptOutcome::NotFound => not_found += 1,
        }
    }

    // P1: exactly the cap is admitted no matter how many raced.
    assert_eq!(claimed, MAX_ATTEMPTS, "exactly the cap should be admitted");
    assert_eq!(locked, PARALLEL - MAX_ATTEMPTS);
    assert_eq!(not_found, 0);

    // The persisted counter lands exactly on the cap.
    let after = reset_repo.find_by_user(&user_id).await.unwrap().unwrap();
    assert_eq!(after.attempts(), MAX_ATTEMPTS);
    assert!(after.is_locked());
}

#[sqlx::test(migrations = "../../migrations")]
async fn register_attempt_reports_not_found_without_row(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let reset_repo = PostgresPasswordResetRepository::new(db);

    // No reset code row exists for this user → NotFound, never Locked.
    let outcome = reset_repo
        .register_attempt(&UserId::new(), &ResetCodeId::new(), MAX_ATTEMPTS)
        .await
        .unwrap();
    assert_eq!(outcome, AttemptOutcome::NotFound);
}

/// A guess read code A, then a reissue replaced it with code B. Its claim
/// names A, so it reports `NotFound` (the no-code answer) and leaves B's
/// attempts alone, even with B at the cap (no misleading "locked"). Claiming
/// by user alone let it take a slot on B and be compared against A, past A's
/// cap (`models/attempt_claim.qnt`, module `claim_by_user`).
#[sqlx::test(migrations = "../../migrations")]
async fn claim_naming_a_replaced_code_finds_no_code(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let user_repo = PostgresUserRepository::new(db.clone());
    let reset_repo = PostgresPasswordResetRepository::new(db);
    let user = User::new(
        Email::new("replaced@example.com").unwrap(),
        Some("password_hash".to_string()),
        "Replaced".to_string(),
        "Code".to_string(),
    );
    user_repo.save(&user).await.unwrap();
    let user_id = *user.id();

    // Code A was sent 10 minutes ago, so B may replace it (past the cooldown).
    let sent_a = Timestamp::now() - SignedDuration::from_mins(10);
    let read = PasswordResetCode::from_persistence(
        ResetCodeId::new(),
        user_id,
        "code_a".to_string(),
        sent_a + SignedDuration::from_mins(15),
        0,
        sent_a,
        sent_a,
    );
    assert_eq!(reset_repo.save(&read).await.unwrap(), IssueOutcome::Issued);
    let current = PasswordResetCode::new(user_id, "code_b".to_string());
    assert_eq!(
        reset_repo.save(&current).await.unwrap(),
        IssueOutcome::Issued
    );

    for expected_attempts in [0, MAX_ATTEMPTS] {
        if expected_attempts > 0 {
            for _ in 0..MAX_ATTEMPTS {
                reset_repo
                    .register_attempt(&user_id, current.id(), MAX_ATTEMPTS)
                    .await
                    .unwrap();
            }
        }
        let outcome = reset_repo
            .register_attempt(&user_id, read.id(), MAX_ATTEMPTS)
            .await
            .unwrap();
        assert_eq!(outcome, AttemptOutcome::NotFound);
        let stored = reset_repo.find_by_user(&user_id).await.unwrap().unwrap();
        assert_eq!(stored.id(), current.id());
        assert_eq!(stored.attempts(), expected_attempts);
    }
}

fn expected_lock_window_minutes(lockouts: i32) -> i64 {
    let max_idx = i32::try_from(LOCK_WINDOWS_MINUTES.len() - 1).unwrap();
    let idx = usize::try_from(lockouts.clamp(0, max_idx)).unwrap();
    LOCK_WINDOWS_MINUTES[idx]
}

fn reset_code_at(user_id: UserId, sent_at: Timestamp) -> PasswordResetCode {
    PasswordResetCode::from_persistence(
        ResetCodeId::new(),
        user_id,
        "hash".to_string(),
        sent_at + SignedDuration::from_mins(15),
        0,
        sent_at,
        sent_at,
    )
}

fn verification_code_at(user_id: UserId, sent_at: Timestamp) -> EmailVerificationCode {
    EmailVerificationCode::from_persistence(
        VerificationCodeId::new(),
        user_id,
        "hash".to_string(),
        sent_at + SignedDuration::from_hours(24),
        0,
        sent_at,
        sent_at,
        None,
        Some(sent_at + SignedDuration::from_hours(24)),
    )
}

/// Pins the SQL P-11 lockout and cooldown guards in both code repositories at
/// exact `±1s` boundaries and across edge `lockouts` values (`-1`, `0`, `1`,
/// `6`, `7`, `8`, `1000`), verifying 1-based Postgres array clamping matches
/// `LOCK_WINDOWS_MINUTES`.
#[sqlx::test(migrations = "../../migrations")]
async fn sql_lockout_and_cooldown_guards_hold_at_exact_boundaries(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = PostgresUserRepository::new(db.clone());
    let reset_repo = PostgresPasswordResetRepository::new(db.clone());
    let verify_repo = PostgresEmailVerificationRepository::new(db);

    let user = User::new(
        Email::new("boundaries@example.com").unwrap(),
        Some("password_hash".to_string()),
        "Bound".to_string(),
        "Test".to_string(),
    );
    user_repo.save(&user).await.unwrap();
    let user_id = *user.id();

    let base = Timestamp::from_second(1_700_000_000).unwrap();
    let cooldown = SignedDuration::from_mins(RESEND_COOLDOWN_MINUTES);
    let one_sec = SignedDuration::from_secs(1);

    // 1. Cooldown boundaries (attempts = 0, unlocked) for both repositories.
    assert_eq!(
        reset_repo
            .save(&reset_code_at(user_id, base))
            .await
            .unwrap(),
        IssueOutcome::Issued
    );
    assert_eq!(
        reset_repo
            .save(&reset_code_at(user_id, base + cooldown - one_sec))
            .await
            .unwrap(),
        IssueOutcome::Withheld,
        "reset code must be withheld at cooldown - 1s"
    );
    assert_eq!(
        reset_repo
            .save(&reset_code_at(user_id, base + cooldown))
            .await
            .unwrap(),
        IssueOutcome::Issued,
        "reset code must be issued at exact cooldown"
    );
    assert_eq!(
        reset_repo
            .save(&reset_code_at(
                user_id,
                base + cooldown + cooldown + one_sec
            ))
            .await
            .unwrap(),
        IssueOutcome::Issued,
        "reset code must be issued at cooldown + 1s"
    );

    assert_eq!(
        verify_repo
            .save(&verification_code_at(user_id, base))
            .await
            .unwrap(),
        IssueOutcome::Issued
    );
    assert_eq!(
        verify_repo
            .save(&verification_code_at(user_id, base + cooldown - one_sec))
            .await
            .unwrap(),
        IssueOutcome::Withheld,
        "verification code must be withheld at cooldown - 1s"
    );
    assert_eq!(
        verify_repo
            .save(&verification_code_at(user_id, base + cooldown))
            .await
            .unwrap(),
        IssueOutcome::Issued,
        "verification code must be issued at exact cooldown"
    );
    assert_eq!(
        verify_repo
            .save(&verification_code_at(
                user_id,
                base + cooldown + cooldown + one_sec
            ))
            .await
            .unwrap(),
        IssueOutcome::Issued,
        "verification code must be issued at cooldown + 1s"
    );

    // 2. Negative `lockouts` is rejected by the schema's CHECK constraint on both tables.
    for sql in [
        "UPDATE password_reset_codes SET lockouts = -1 WHERE user_id = $1",
        "UPDATE email_verification_codes SET lockouts = -1 WHERE user_id = $1",
    ] {
        let err = sqlx::query(sql)
            .bind(user_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("lockouts_check"),
            "expected lockouts CHECK constraint violation for `{sql}`, got {err}"
        );
    }

    // 3. Lockout window boundaries (attempts = MAX_ATTEMPTS) across edge lockouts.
    for lockouts in [0, 1, 6, 7, 8, 1000] {
        let window = SignedDuration::from_mins(expected_lock_window_minutes(lockouts));

        for is_reset in [true, false] {
            let label = if is_reset {
                "password_reset_codes"
            } else {
                "email_verification_codes"
            };
            let update_sql = if is_reset {
                "UPDATE password_reset_codes SET attempts = $1, lockouts = $2, last_sent_at = to_timestamp($3) WHERE user_id = $4"
            } else {
                "UPDATE email_verification_codes SET attempts = $1, lockouts = $2, last_sent_at = to_timestamp($3) WHERE user_id = $4"
            };
            let select_sql = if is_reset {
                "SELECT lockouts FROM password_reset_codes WHERE user_id = $1"
            } else {
                "SELECT lockouts FROM email_verification_codes WHERE user_id = $1"
            };

            let set_locked = async |pool: &PgPool| {
                sqlx::query(update_sql)
                    .bind(MAX_ATTEMPTS)
                    .bind(lockouts)
                    .bind(f64::from(i32::try_from(base.as_second()).unwrap()))
                    .bind(user_id.as_uuid())
                    .execute(pool)
                    .await
                    .unwrap();
            };

            let save_at = async |t: Timestamp| {
                if is_reset {
                    reset_repo.save(&reset_code_at(user_id, t)).await.unwrap()
                } else {
                    verify_repo
                        .save(&verification_code_at(user_id, t))
                        .await
                        .unwrap()
                }
            };

            set_locked(&pool).await;
            assert_eq!(
                save_at(base + window - one_sec).await,
                IssueOutcome::Withheld,
                "{label}: lockouts={lockouts} must be withheld at window - 1s"
            );
            assert_eq!(
                save_at(base + window).await,
                IssueOutcome::Issued,
                "{label}: lockouts={lockouts} must be issued at exact window"
            );
            let stored_lockouts: i32 = sqlx::query_scalar(select_sql)
                .bind(user_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                stored_lockouts,
                lockouts + 1,
                "{label}: replacing locked row must increment lockouts"
            );

            set_locked(&pool).await;
            assert_eq!(
                save_at(base + window + one_sec).await,
                IssueOutcome::Issued,
                "{label}: lockouts={lockouts} must be issued at window + 1s"
            );
        }
    }
}
