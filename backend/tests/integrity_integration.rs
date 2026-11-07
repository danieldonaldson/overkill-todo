//! Concurrency and data-integrity invariants at the repository layer.
//!
//! These build each Postgres repository directly from a throwaway `PgPool`
//! (provisioned per test by `#[sqlx::test]`) and assert invariants the HTTP and
//! basic repository suites don't reach: optimistic-version conflicts, the
//! compare-and-swap status update, the 23505 unique-violation → `Conflict`
//! mapping, and the encrypted-at-rest read paths' refusal to surface wrong
//! plaintext when a ciphertext is corrupted or its AAD no longer binds.
//!
//! The corruption / row-move steps use runtime `sqlx::query` (NOT the
//! compile-checked macros) precisely so that they can write deliberately invalid
//! data and do not need the offline `.sqlx` cache regenerated.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use sqlx::PgPool;

use app::application::ports::{OAuthAccountRepository, RefreshTokenRepository, UserRepository};
use app::domain::entities::{OAuthAccount, OAuthProvider, RefreshToken, User};
use app::domain::errors::DomainError;
use app::domain::value_objects::{Email, UserId};
use app::infrastructure::persistence::{
    Database, PostgresOAuthAccountRepository, PostgresRefreshTokenRepository,
    PostgresUserRepository,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Inserts a persisted user (via the real user repo) and returns its id.
async fn seed_user(db: &Arc<Database>, email: &str) -> UserId {
    let user_repo = PostgresUserRepository::new(db.clone());
    let user = User::new(
        Email::new(email).unwrap(),
        Some("password_hash".to_string()),
        "Test".to_string(),
        "User".to_string(),
    );
    user_repo.save(&user).await.expect("save user");
    *user.id()
}

// ---------------------------------------------------------------------------
// 1. user_repository::save optimistic-version concurrency
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn user_save_optimistic_version_conflict_and_increment(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresUserRepository::new(db.clone());

    // Fresh user: INSERT branch, version starts at 0.
    let user = User::new(
        Email::new("optimistic@example.com").unwrap(),
        Some("hash".to_string()),
        "Opti".to_string(),
        "Mistic".to_string(),
    );
    repo.save(&user).await.expect("initial insert");
    let id = *user.id();

    // Load a fresh copy (version 0), mutate, and save → happy increment to 1.
    let mut loaded = repo.find_by_id(&id).await.unwrap().expect("user present");
    assert_eq!(loaded.version(), 0, "freshly inserted user is at version 0");
    loaded.update_name("Edited".to_string(), "Once".to_string());
    repo.save(&loaded).await.expect("first update succeeds");

    let after = repo.find_by_id(&id).await.unwrap().unwrap();
    assert_eq!(after.version(), 1, "a successful update bumps the version");
    assert_eq!(after.first_name(), "Edited");

    // Now save AGAIN from the STALE original entity (still at version 0). The
    // row is already at version 1, so the WHERE version = 0 matches no rows and
    // the repo must surface a Conflict rather than clobber the winning write.
    let mut stale = loaded; // this entity was loaded at version 0
    stale.update_name("Stale".to_string(), "Write".to_string());
    let result = repo.save(&stale).await;
    assert!(
        matches!(result, Err(DomainError::Conflict(ref m)) if m.contains("modified concurrently")),
        "a stale save must Conflict: {result:?}"
    );

    // The clobber did NOT happen: the winning write survives unchanged.
    let final_state = repo.find_by_id(&id).await.unwrap().unwrap();
    assert_eq!(final_state.first_name(), "Edited");
    assert_eq!(final_state.version(), 1);
}

// ---------------------------------------------------------------------------
// 2. user_agent_repository: update_if_status CAS + create duplicate-active
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 3. 23505 unique-violation mapping, end-to-end through real repos
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn oauth_account_duplicate_link_maps_23505_to_conflict(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresOAuthAccountRepository::new(db.clone());
    let user_id = seed_user(&db, "oauth-dup@example.com").await;

    let account = OAuthAccount::new(
        user_id,
        OAuthProvider::Google,
        "sub-xyz".to_string(),
        Some("oauth-dup@example.com".to_string()),
    );
    repo.save(&account).await.expect("first link");

    // Linking the same (provider, provider_user_id) again is a 23505 unique
    // violation that persist_err must turn into a Conflict.
    let dup = OAuthAccount::new(
        user_id,
        OAuthProvider::Google,
        "sub-xyz".to_string(),
        Some("oauth-dup@example.com".to_string()),
    );
    let err = repo.save(&dup).await.unwrap_err();
    assert!(
        matches!(err, DomainError::Conflict(_)),
        "a duplicate OAuth link must Conflict (23505 arm): {err:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn refresh_token_duplicate_hash_maps_23505_to_conflict(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresRefreshTokenRepository::new(db.clone());
    let user_id = seed_user(&db, "refresh-dup@example.com").await;

    let expires = Utc::now() + Duration::days(30);
    let token = RefreshToken::new(
        user_id,
        "dup-hash".to_string(),
        "device".to_string(),
        expires,
    );
    repo.save(&token).await.expect("first token");

    // idx_refresh_tokens_hash is UNIQUE; a second row with the same token_hash
    // trips 23505 → Conflict.
    let dup = RefreshToken::new(
        user_id,
        "dup-hash".to_string(),
        "device-2".to_string(),
        expires,
    );
    let err = repo.save(&dup).await.unwrap_err();
    assert!(
        matches!(err, DomainError::Conflict(_)),
        "a duplicate refresh-token hash must Conflict (23505 arm): {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Decrypt-failure / AAD-binding read paths
// ---------------------------------------------------------------------------
