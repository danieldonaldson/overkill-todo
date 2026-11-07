//! Integration coverage for the atomic brute-force attempt cap.
//!
//! These tests pin the two properties the lockout has historically oscillated
//! on, so a future refactor can't silently regress either:
//!   - P1: the per-guess cap holds under concurrency (parallel claims that all
//!     start from `attempts = 0` cannot collectively exceed `MAX_ATTEMPTS`).
//!   - finding #2: a claim against a missing row reports `NotFound`, not a
//!     misleading lockout.

use std::sync::Arc;

use app::application::ports::{PasswordResetRepository, UserRepository};
use app::domain::entities::attempt_lockout::{AttemptOutcome, MAX_ATTEMPTS};
use app::domain::entities::{PasswordResetCode, User};
use app::domain::value_objects::{Email, UserId};
use app::infrastructure::persistence::{
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
    reset_repo
        .save(&PasswordResetCode::new(user_id, "hashed_code".to_string()))
        .await
        .unwrap();

    // Fire many more parallel claims than the cap allows.
    let mut handles = Vec::new();
    for _ in 0..PARALLEL {
        let repo = reset_repo.clone();
        handles.push(tokio::spawn(async move {
            repo.register_attempt(&user_id, MAX_ATTEMPTS).await.unwrap()
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
        .register_attempt(&UserId::new(), MAX_ATTEMPTS)
        .await
        .unwrap();
    assert_eq!(outcome, AttemptOutcome::NotFound);
}
