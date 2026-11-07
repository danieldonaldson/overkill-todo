//! Integration test for refresh-token rotation under concurrency.
//!
//! The "rotate exactly once" invariant (two racing refreshes of the same token
//! must produce exactly one successor, never two) previously rested only on a
//! code comment. This fires two concurrent `rotate()` calls against the real
//! Postgres repository and asserts exactly one wins while the other is told the
//! token was already rotated.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use sqlx::PgPool;

use app::application::ports::{RefreshTokenRepository, UserRepository};
use app::domain::entities::{RefreshToken, User};
use app::domain::errors::DomainError;
use app::domain::value_objects::Email;
use app::infrastructure::persistence::{
    Database, PostgresRefreshTokenRepository, PostgresUserRepository,
};

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_rotate_lets_exactly_one_winner(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = PostgresUserRepository::new(db.clone());
    let repo = Arc::new(PostgresRefreshTokenRepository::new(db));

    // FK parent + the token both racers try to rotate.
    let user = User::new(
        Email::new("rotate-race@example.com").unwrap(),
        Some("hash".to_string()),
        "Race".to_string(),
        "Condition".to_string(),
    );
    user_repo.save(&user).await.expect("save user");
    let user_id = user.id().as_uuid();

    let expires = Utc::now() + Duration::days(30);
    let original = RefreshToken::new(
        *user.id(),
        "original-hash".to_string(),
        "device-fp".to_string(),
        expires,
    );
    repo.save(&original).await.expect("save original token");
    let old_id = *original.id();

    let successor = |hash: &str| {
        RefreshToken::new(
            *user.id(),
            hash.to_string(),
            "device-fp".to_string(),
            expires,
        )
    };

    // Fire both rotations concurrently; each mints a distinct successor.
    let task_a = {
        let repo = repo.clone();
        let new = successor("successor-a");
        tokio::spawn(async move { repo.rotate(&old_id, &new).await })
    };
    let task_b = {
        let repo = repo.clone();
        let new = successor("successor-b");
        tokio::spawn(async move { repo.rotate(&old_id, &new).await })
    };

    let results = [task_a.await.unwrap(), task_b.await.unwrap()];
    let wins = results.iter().filter(|r| r.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|r| matches!(r, Err(DomainError::Conflict(_))))
        .count();
    assert_eq!(wins, 1, "exactly one rotation must win: {results:?}");
    assert_eq!(conflicts, 1, "the loser must see a Conflict: {results:?}");

    // The original is revoked; exactly one successor exists and is active.
    let total: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await
    .expect("count tokens")
    .unwrap_or(0);
    assert_eq!(
        total, 2,
        "original + one successor only — the loser never inserted"
    );

    let active: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL",
        user_id
    )
    .fetch_one(&pool)
    .await
    .expect("count active tokens")
    .unwrap_or(0);
    assert_eq!(active, 1, "exactly one active token after rotation");

    let original_revoked: Option<chrono::DateTime<Utc>> = sqlx::query_scalar!(
        "SELECT revoked_at FROM refresh_tokens WHERE id = $1",
        old_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .expect("load original token");
    assert!(original_revoked.is_some(), "original token must be revoked");
}
