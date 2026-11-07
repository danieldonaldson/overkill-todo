//! Integration test for the GDPR account-anonymization transaction.
//!
//! Seeds a user plus a row in every child table the anonymize transaction is
//! responsible for, runs the real Postgres `anonymize_account`, and asserts the
//! PII is scrubbed and every child row is gone. This is the multi-table
//! transactional purge that previously had only in-memory coverage.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use app::application::ports::{
    AccountLifecycleRepository, EmailVerificationRepository, RefreshTokenRepository, UserRepository,
};
use app::domain::entities::{EmailVerificationCode, RefreshToken, User};
use app::domain::value_objects::Email;
use app::infrastructure::persistence::{
    Database, PostgresEmailVerificationRepository, PostgresRefreshTokenRepository,
    PostgresUserRepository,
};

/// Total rows across every child table the anonymize transaction must purge,
/// for the given user. Compile-checked per-table counts summed in code so the
/// queries stay `sqlx::query!`-validated.
async fn child_row_count(pool: &PgPool, user_id: Uuid) -> i64 {
    let refresh = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count refresh_tokens")
    .unwrap_or(0);
    let oauth = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM user_oauth_accounts WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count user_oauth_accounts")
    .unwrap_or(0);
    let verification = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM email_verification_codes WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count email_verification_codes")
    .unwrap_or(0);
    let reset = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM password_reset_codes WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count password_reset_codes")
    .unwrap_or(0);
    refresh + oauth + verification + reset
}

#[sqlx::test(migrations = "./migrations")]
async fn anonymize_scrubs_pii_and_purges_child_rows(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));

    let user_repo = PostgresUserRepository::new(db.clone());
    let refresh_repo = PostgresRefreshTokenRepository::new(db.clone());
    let verification_repo = PostgresEmailVerificationRepository::new(db);

    // --- Seed: a complete user with a row in every anonymize-managed child table.
    let mut user = User::new(
        Email::new("real-person@example.com").unwrap(),
        Some("argon2-hash".to_string()),
        "Real".to_string(),
        "Person".to_string(),
    );
    user.mark_email_verified();
    user_repo.save(&user).await.expect("save user");
    let user_id = *user.id().as_uuid();

    refresh_repo
        .save(&RefreshToken::new(
            *user.id(),
            "token-hash".to_string(),
            "device-fp".to_string(),
            Utc::now() + Duration::days(30),
        ))
        .await
        .expect("seed refresh token");

    verification_repo
        .save(&EmailVerificationCode::new(
            *user.id(),
            "code-hash".to_string(),
        ))
        .await
        .expect("seed verification code");

    sqlx::query!(
        "INSERT INTO user_oauth_accounts (user_id, provider, provider_user_id) \
         VALUES ($1, 'google', $2)",
        user_id,
        "google-sub-123",
    )
    .execute(&pool)
    .await
    .expect("seed oauth account");

    sqlx::query!(
        "INSERT INTO password_reset_codes (user_id, code_hash, expires_at) VALUES ($1, $2, $3)",
        user_id,
        "reset-hash",
        Utc::now() + Duration::hours(1),
    )
    .execute(&pool)
    .await
    .expect("seed reset code");

    // Sanity: one row in each of the four child tables before deletion.
    assert_eq!(
        child_row_count(&pool, user_id).await,
        4,
        "seeding should create one row per child table",
    );

    // --- Act: anonymize (the data-scrub half of account deletion).
    let mut anonymized = user.clone();
    let anon_email = Email::new(&format!("deleted-{user_id}@anonymized.local")).unwrap();
    anonymized.anonymize(anon_email);
    user_repo
        .anonymize_account(&anonymized)
        .await
        .expect("anonymize");

    // --- Assert: PII scrubbed on the user row.
    let row = sqlx::query!(
        "SELECT email, password_hash, deleted_at, deletion_scheduled_at FROM users WHERE id = $1",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .expect("load user");

    assert_eq!(row.email, format!("deleted-{user_id}@anonymized.local"));
    assert!(row.password_hash.is_none(), "password hash must be cleared");
    assert!(row.deleted_at.is_some(), "deleted_at must be set");
    assert!(
        row.deletion_scheduled_at.is_none(),
        "deletion_scheduled_at must be cleared",
    );
    assert!(
        !row.email.contains("real-person"),
        "original email must not survive",
    );

    // --- Assert: every child table is empty for this user.
    assert_eq!(
        child_row_count(&pool, user_id).await,
        0,
        "all child rows must be purged",
    );
}
