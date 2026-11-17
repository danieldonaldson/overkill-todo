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

use jiff::{SignedDuration, Timestamp};
use overkill_todo::application::ports::{PasswordResetRepository, UserRepository};
use overkill_todo::domain::entities::attempt_lockout::{
    AttemptOutcome, IssueOutcome, MAX_ATTEMPTS,
};
use overkill_todo::domain::entities::{PasswordResetCode, User};
use overkill_todo::domain::value_objects::{Email, ResetCodeId, UserId};
use overkill_todo::infrastructure::persistence::postgres::{
    Database, PostgresPasswordResetRepository, PostgresUserRepository,
};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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
#[sqlx::test(migrations = "./migrations")]
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
