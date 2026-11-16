//! Repository-level integration coverage.
//!
//! These build each Postgres repository directly from a throwaway `PgPool`
//! (provisioned per test by `#[sqlx::test]`) and exercise the find / insert /
//! revoke / consume / delete paths plus their not-found, expired, and
//! idempotency branches — the cases the HTTP suites don't reach.

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use sqlx::PgPool;

use overkill_todo::application::ports::{
    EmailVerificationRepository, OAuthAccountRepository, PasswordResetRepository,
    RefreshTokenRepository, UserRepository,
};
use overkill_todo::domain::entities::attempt_lockout::IssueOutcome;
use overkill_todo::domain::entities::{
    EmailVerificationCode, OAuthAccount, OAuthProvider, PasswordResetCode, RefreshToken, User,
};
use overkill_todo::domain::value_objects::{
    Email, RefreshTokenId, ResetCodeId, UserId, VerificationCodeId,
};
use overkill_todo::infrastructure::persistence::postgres::{
    Database, PostgresEmailVerificationRepository, PostgresOAuthAccountRepository,
    PostgresPasswordResetRepository, PostgresRefreshTokenRepository, PostgresUserRepository,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Inserts a persisted user (via the real user repo) and returns its id, so the
/// foreign keys on the child tables are satisfied.
/// Moves every stored code's send time 6 minutes back, past the resend
/// cooldown, so the next issue may replace it.
async fn age_codes(pool: &PgPool, table: &str) {
    let sql = match table {
        "email_verification_codes" => {
            "UPDATE email_verification_codes SET last_sent_at = last_sent_at - INTERVAL '6 minutes'"
        }
        "password_reset_codes" => {
            "UPDATE password_reset_codes SET last_sent_at = last_sent_at - INTERVAL '6 minutes'"
        }
        other => panic!("no code table {other}"),
    };
    sqlx::query(sql).execute(pool).await.unwrap();
}

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
// OAuth account repository
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn oauth_account_save_find_count_delete(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresOAuthAccountRepository::new(db.clone());
    let user_id = seed_user(&db, "oauth-repo@example.com").await;

    // No account yet across every lookup path.
    assert!(repo.find_by_user_id(&user_id).await.unwrap().is_empty());
    assert_eq!(repo.count_by_user_id(&user_id).await.unwrap(), 0);
    assert!(
        repo.find_by_provider_user_id(OAuthProvider::Google, "sub-123")
            .await
            .unwrap()
            .is_none()
    );

    // Save one link.
    let account = OAuthAccount::new(
        user_id,
        OAuthProvider::Google,
        "sub-123".to_string(),
        Some("oauth-repo@example.com".to_string()),
    );
    repo.save(&account).await.unwrap();

    // Every lookup path now finds it.
    let by_user = repo.find_by_user_id(&user_id).await.unwrap();
    assert_eq!(by_user.len(), 1);
    assert_eq!(by_user[0].provider(), OAuthProvider::Google);
    assert_eq!(repo.count_by_user_id(&user_id).await.unwrap(), 1);
    let by_provider = repo
        .find_by_provider_user_id(OAuthProvider::Google, "sub-123")
        .await
        .unwrap()
        .expect("found by provider id");
    assert_eq!(by_provider.user_id(), &user_id);
    assert_eq!(by_provider.provider_email(), Some("oauth-repo@example.com"));

    // Deleting a provider that isn't linked is a no-op (false).
    assert!(
        !repo
            .delete_by_user_and_provider(&user_id, OAuthProvider::Apple)
            .await
            .unwrap()
    );

    // Deleting the linked provider reports a row was removed, and is idempotent.
    assert!(
        repo.delete_by_user_and_provider(&user_id, OAuthProvider::Google)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .delete_by_user_and_provider(&user_id, OAuthProvider::Google)
            .await
            .unwrap()
    );
    assert_eq!(repo.count_by_user_id(&user_id).await.unwrap(), 0);
}

// ---------------------------------------------------------------------------
// Refresh token repository
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn refresh_token_save_find_revoke_and_revoke_all(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresRefreshTokenRepository::new(db.clone());
    let user_id = seed_user(&db, "refresh-repo@example.com").await;

    // Unknown hash → None.
    assert!(repo.find_by_hash("missing").await.unwrap().is_none());

    let expires = Utc::now() + Duration::days(30);
    let token = RefreshToken::new(
        user_id,
        "hash-1".to_string(),
        "device-1".to_string(),
        expires,
    );
    repo.save(&token).await.unwrap();

    let found = repo
        .find_by_hash("hash-1")
        .await
        .unwrap()
        .expect("token found");
    assert_eq!(found.id(), token.id());
    assert!(found.revoked_at().is_none());

    // Revoke by id; the row is now revoked. Re-revoking is a harmless no-op.
    repo.revoke(token.id()).await.unwrap();
    let after = repo.find_by_hash("hash-1").await.unwrap().unwrap();
    assert!(after.revoked_at().is_some());
    repo.revoke(token.id()).await.unwrap();

    // revoke_all_for_user only touches still-active tokens.
    let token2 = RefreshToken::new(
        user_id,
        "hash-2".to_string(),
        "device-2".to_string(),
        expires,
    );
    let token3 = RefreshToken::new(
        user_id,
        "hash-3".to_string(),
        "device-3".to_string(),
        expires,
    );
    repo.save(&token2).await.unwrap();
    repo.save(&token3).await.unwrap();
    let revoked = repo.revoke_all_for_user(&user_id).await.unwrap();
    assert_eq!(revoked, 2, "only the two active tokens are revoked");
    // Nothing active left → second call revokes zero.
    assert_eq!(repo.revoke_all_for_user(&user_id).await.unwrap(), 0);
}

/// `revoke_all_for_user_except` (powers "log out other devices") revokes every
/// active token for the user *except* the one whose hash is kept, and counts only
/// the rows it touched.
#[sqlx::test(migrations = "./migrations")]
async fn refresh_token_revoke_all_except_keeps_current(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresRefreshTokenRepository::new(db.clone());
    let user_id = seed_user(&db, "revoke-except@example.com").await;
    let expires = Utc::now() + Duration::days(30);

    let current = RefreshToken::new(
        user_id,
        "keep".to_string(),
        "this-device".to_string(),
        expires,
    );
    let other_a = RefreshToken::new(
        user_id,
        "other-a".to_string(),
        "device-a".to_string(),
        expires,
    );
    let other_b = RefreshToken::new(
        user_id,
        "other-b".to_string(),
        "device-b".to_string(),
        expires,
    );
    repo.save(&current).await.unwrap();
    repo.save(&other_a).await.unwrap();
    repo.save(&other_b).await.unwrap();

    let revoked = repo
        .revoke_all_for_user_except(&user_id, "keep")
        .await
        .unwrap();
    assert_eq!(revoked, 2, "both other devices are revoked");

    // The kept token is still active; the others are revoked.
    assert!(
        repo.find_by_hash("keep")
            .await
            .unwrap()
            .unwrap()
            .revoked_at()
            .is_none(),
        "the current device's token must survive"
    );
    assert!(
        repo.find_by_hash("other-a")
            .await
            .unwrap()
            .unwrap()
            .revoked_at()
            .is_some()
    );
    assert!(
        repo.find_by_hash("other-b")
            .await
            .unwrap()
            .unwrap()
            .revoked_at()
            .is_some()
    );

    // Idempotent: nothing else active to revoke.
    assert_eq!(
        repo.revoke_all_for_user_except(&user_id, "keep")
            .await
            .unwrap(),
        0
    );
}

/// `mark_email_verified` and `update_password` must report `NotFound` when the
/// target row no longer exists (deleted/anonymized) rather than silently
/// succeeding on zero rows.
#[sqlx::test(migrations = "./migrations")]
async fn user_targeted_writes_report_not_found_for_missing_row(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresUserRepository::new(db.clone());
    let ghost = UserId::new();

    let verify = repo.mark_email_verified(&ghost).await;
    assert!(
        matches!(
            verify,
            Err(overkill_todo::domain::errors::DomainError::NotFound(_))
        ),
        "verifying a missing user must be NotFound, got {verify:?}"
    );

    let update = repo.update_password(&ghost, "irrelevant-hash").await;
    assert!(
        matches!(
            update,
            Err(overkill_todo::domain::errors::DomainError::NotFound(_))
        ),
        "updating a missing user's password must be NotFound, got {update:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn refresh_token_delete_expired_removes_only_expired(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresRefreshTokenRepository::new(db.clone());
    let user_id = seed_user(&db, "refresh-expire@example.com").await;

    let active = RefreshToken::new(
        user_id,
        "active-hash".to_string(),
        "device".to_string(),
        Utc::now() + Duration::days(30),
    );
    let expired = RefreshToken::new(
        user_id,
        "expired-hash".to_string(),
        "device".to_string(),
        Utc::now() - Duration::days(1),
    );
    repo.save(&active).await.unwrap();
    repo.save(&expired).await.unwrap();

    let deleted = repo.delete_expired().await.unwrap();
    assert_eq!(deleted, 1, "only the expired token is purged");
    assert!(repo.find_by_hash("active-hash").await.unwrap().is_some());
    assert!(repo.find_by_hash("expired-hash").await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn refresh_token_rotate_missing_token_conflicts(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresRefreshTokenRepository::new(db.clone());
    let user_id = seed_user(&db, "refresh-rotate-missing@example.com").await;

    // Rotating a token id that doesn't exist matches zero rows → Conflict.
    let successor = RefreshToken::new(
        user_id,
        "new-hash".to_string(),
        "device".to_string(),
        Utc::now() + Duration::days(30),
    );
    let result = repo.rotate(&RefreshTokenId::new(), &successor).await;
    assert!(
        matches!(
            result,
            Err(overkill_todo::domain::errors::DomainError::Conflict(_))
        ),
        "rotating a missing token must Conflict: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Password reset repository
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn password_reset_save_upsert_consume(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let repo = PostgresPasswordResetRepository::new(db.clone());
    let user_id = seed_user(&db, "reset-repo@example.com").await;

    assert!(repo.find_by_user(&user_id).await.unwrap().is_none());

    let code = PasswordResetCode::new(user_id, "hash-a".to_string());
    repo.save(&code).await.unwrap();
    let found = repo.find_by_user(&user_id).await.unwrap().unwrap();
    assert_eq!(found.code_hash(), "hash-a");

    // Inside the resend cooldown the store withholds a new code (P-11)...
    let code2 = PasswordResetCode::new(user_id, "hash-b".to_string());
    assert_eq!(repo.save(&code2).await.unwrap(), IssueOutcome::Withheld);
    assert_eq!(
        repo.find_by_user(&user_id)
            .await
            .unwrap()
            .unwrap()
            .code_hash(),
        "hash-a"
    );

    // ...and once it has passed, saving again upserts on the unique user_id
    // (replaces the code).
    age_codes(&pool, "password_reset_codes").await;
    assert_eq!(repo.save(&code2).await.unwrap(), IssueOutcome::Issued);
    let found = repo.find_by_user(&user_id).await.unwrap().unwrap();
    assert_eq!(found.code_hash(), "hash-b");

    // Consuming with the wrong code id removes nothing.
    assert!(!repo.consume(&user_id, &ResetCodeId::new()).await.unwrap());
    // Consuming the real code id removes the row; a second consume is a no-op.
    assert!(repo.consume(&user_id, found.id()).await.unwrap());
    assert!(!repo.consume(&user_id, found.id()).await.unwrap());
    assert!(repo.find_by_user(&user_id).await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn password_reset_release_attempt_and_delete_expired(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresPasswordResetRepository::new(db.clone());
    let user_id = seed_user(&db, "reset-release@example.com").await;

    // Persist a code that already has attempts so release can decrement it.
    let code = PasswordResetCode::from_persistence(
        ResetCodeId::new(),
        user_id,
        "hash".to_string(),
        Utc::now() + Duration::hours(1),
        3,
        Utc::now(),
        Utc::now(),
    );
    repo.save(&code).await.unwrap();
    // A release naming another code id (one since replaced) changes nothing.
    repo.release_attempt(&user_id, &ResetCodeId::new())
        .await
        .unwrap();
    assert_eq!(
        repo.find_by_user(&user_id)
            .await
            .unwrap()
            .unwrap()
            .attempts(),
        3
    );
    repo.release_attempt(&user_id, code.id()).await.unwrap();
    assert_eq!(
        repo.find_by_user(&user_id)
            .await
            .unwrap()
            .unwrap()
            .attempts(),
        2
    );

    // release on a non-existent row is harmless.
    repo.release_attempt(&UserId::new(), &ResetCodeId::new())
        .await
        .unwrap();

    // An expired reset code (regardless of user) is purged by delete_expired.
    let expired_user = seed_user(&db, "reset-expired@example.com").await;
    let expired = PasswordResetCode::from_persistence(
        ResetCodeId::new(),
        expired_user,
        "old".to_string(),
        Utc::now() - Duration::hours(1),
        0,
        Utc::now() - Duration::hours(2),
        Utc::now() - Duration::hours(2),
    );
    repo.save(&expired).await.unwrap();
    let deleted = repo.delete_expired().await.unwrap();
    assert_eq!(deleted, 1, "only the expired reset code is purged");
    assert!(repo.find_by_user(&expired_user).await.unwrap().is_none());
    assert!(repo.find_by_user(&user_id).await.unwrap().is_some());
}

// ---------------------------------------------------------------------------
// Email verification repository
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn email_verification_magic_token_lifecycle(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresEmailVerificationRepository::new(db.clone());
    let user_id = seed_user(&db, "verify-repo@example.com").await;

    // Unknown user / magic token return None.
    assert!(repo.find_by_user(&user_id).await.unwrap().is_none());
    assert!(
        repo.find_by_magic_token_hash("nope")
            .await
            .unwrap()
            .is_none()
    );

    // Save a code carrying a magic token.
    let code = EmailVerificationCode::new_with_magic_token(
        user_id,
        "code-hash".to_string(),
        "magic-hash".to_string(),
    );
    repo.save(&code).await.unwrap();

    // Found by both user and magic token hash.
    assert!(repo.find_by_user(&user_id).await.unwrap().is_some());
    let by_magic = repo
        .find_by_magic_token_hash("magic-hash")
        .await
        .unwrap()
        .expect("found by magic token");
    assert_eq!(by_magic.user_id(), &user_id);

    // save_magic_token replaces the stored magic token.
    repo.save_magic_token(&user_id, "magic-hash-2".to_string())
        .await
        .unwrap();
    assert!(
        repo.find_by_magic_token_hash("magic-hash-2")
            .await
            .unwrap()
            .is_some()
    );

    // delete_for_user removes the row entirely; repeating is harmless.
    repo.delete_for_user(&user_id).await.unwrap();
    assert!(repo.find_by_user(&user_id).await.unwrap().is_none());
    repo.delete_for_user(&user_id).await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn email_verification_save_upsert_preserves_magic_token(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let repo = PostgresEmailVerificationRepository::new(db.clone());
    let user_id = seed_user(&db, "verify-upsert@example.com").await;

    // First save carries a magic token.
    repo.save(&EmailVerificationCode::new_with_magic_token(
        user_id,
        "c1".to_string(),
        "magic-keep".to_string(),
    ))
    .await
    .unwrap();

    // A routine code-send (no magic token), after the resend cooldown, upserts
    // on user_id without clobbering the previously issued magic link
    // (COALESCE branch).
    age_codes(&pool, "email_verification_codes").await;
    repo.save(&EmailVerificationCode::new(user_id, "c2".to_string()))
        .await
        .unwrap();

    let found = repo.find_by_user(&user_id).await.unwrap().unwrap();
    assert_eq!(found.code_hash(), "c2");
    assert!(
        repo.find_by_magic_token_hash("magic-keep")
            .await
            .unwrap()
            .is_some(),
        "the magic link must survive a plain code-send upsert"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn email_verification_delete_expired(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresEmailVerificationRepository::new(db.clone());
    let user_id = seed_user(&db, "verify-expired@example.com").await;

    // Persist an already-expired code.
    let expired = EmailVerificationCode::from_persistence(
        overkill_todo::domain::value_objects::VerificationCodeId::new(),
        user_id,
        "hash".to_string(),
        Utc::now() - Duration::hours(1),
        0,
        Utc::now() - Duration::hours(2),
        Utc::now() - Duration::hours(2),
        None,
        None,
    );
    repo.save(&expired).await.unwrap();
    assert!(repo.find_by_user(&user_id).await.unwrap().is_some());

    let deleted = repo.delete_expired().await.unwrap();
    assert_eq!(deleted, 1);
    assert!(repo.find_by_user(&user_id).await.unwrap().is_none());
}

/// P-11: releasing the resend cooldown after a failed email moves only the
/// code whose email failed, and never a locked one, since `last_sent_at` also
/// starts the lock window.
#[sqlx::test(migrations = "./migrations")]
async fn set_last_sent_at_moves_only_the_named_unlocked_code(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool));
    let repo = PostgresEmailVerificationRepository::new(db.clone());
    let user_id = seed_user(&db, "release-cooldown@example.com").await;
    let earlier = Utc::now() - Duration::minutes(10);
    let sent_at = |code: Option<EmailVerificationCode>| code.unwrap().last_sent_at();

    for attempts in [0, 5] {
        let code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "hash".to_string(),
            Utc::now() + Duration::hours(1),
            attempts,
            Utc::now(),
            Utc::now(),
            None,
            None,
        );
        repo.save(&code).await.unwrap();
        let before = sent_at(repo.find_by_user(&user_id).await.unwrap());

        // Another code id (one since replaced): nothing moves.
        repo.set_last_sent_at(&user_id, &VerificationCodeId::new(), earlier)
            .await
            .unwrap();
        assert_eq!(sent_at(repo.find_by_user(&user_id).await.unwrap()), before);

        // The named code moves only while it is below the cap.
        repo.set_last_sent_at(&user_id, code.id(), earlier)
            .await
            .unwrap();
        let after = sent_at(repo.find_by_user(&user_id).await.unwrap());
        if attempts < 5 {
            assert!((after - earlier).num_milliseconds().abs() < 1);
        } else {
            assert_eq!(after, before, "a locked code's window must not move");
        }
        repo.delete_for_user(&user_id).await.unwrap();
    }
}
