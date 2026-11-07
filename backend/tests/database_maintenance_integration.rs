//! Integration coverage for bounded expiry and retention maintenance.

use overkill_todo::infrastructure::tasks::database_maintenance;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn removes_stale_rows_and_preserves_current_rows(pool: PgPool) {
    let user_id = Uuid::now_v7();
    let old_email_user = Uuid::now_v7();
    let old_reset_user = Uuid::now_v7();
    for (id, email) in [
        (user_id, "maintenance@example.com"),
        (old_email_user, "maintenance-old-email@example.com"),
        (old_reset_user, "maintenance-old-reset@example.com"),
    ] {
        sqlx::query("INSERT INTO users (id, email) VALUES ($1, $2)")
            .bind(id)
            .bind(email)
            .execute(&pool)
            .await
            .unwrap();
    }

    for expired in [true, false] {
        let suffix = if expired { "old" } else { "current" };
        let expiry = if expired { "-1 hour" } else { "1 hour" };

        sqlx::query(
            "INSERT INTO refresh_tokens
             (id, user_id, token_hash, device_fingerprint, expires_at)
             VALUES ($1, $2, $3, $4, NOW() + $5::interval)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(format!("token-{suffix}"))
        .bind(format!("device-{suffix}"))
        .bind(expiry)
        .execute(&pool)
        .await
        .unwrap();
    }

    for (owner, suffix, expiry) in [
        (old_email_user, "old", "-1 hour"),
        (user_id, "current", "1 hour"),
    ] {
        sqlx::query(
            "INSERT INTO email_verification_codes
             (id, user_id, code_hash, expires_at)
             VALUES ($1, $2, $3, NOW() + $4::interval)",
        )
        .bind(Uuid::now_v7())
        .bind(owner)
        .bind(format!("email-{suffix}"))
        .bind(expiry)
        .execute(&pool)
        .await
        .unwrap();
    }

    for (owner, suffix, expiry) in [
        (old_reset_user, "old", "-1 hour"),
        (user_id, "current", "1 hour"),
    ] {
        sqlx::query(
            "INSERT INTO password_reset_codes
             (id, user_id, code_hash, expires_at)
             VALUES ($1, $2, $3, NOW() + $4::interval)",
        )
        .bind(Uuid::now_v7())
        .bind(owner)
        .bind(format!("reset-{suffix}"))
        .bind(expiry)
        .execute(&pool)
        .await
        .unwrap();
    }

    let report = database_maintenance::run_once(&pool).await.unwrap();
    assert_eq!(report.refresh_tokens, 1);
    assert_eq!(report.email_verification_codes, 1);
    assert_eq!(report.password_reset_codes, 1);

    for (table, count_sql) in [
        ("refresh_tokens", "SELECT COUNT(*) FROM refresh_tokens"),
        (
            "email_verification_codes",
            "SELECT COUNT(*) FROM email_verification_codes",
        ),
        (
            "password_reset_codes",
            "SELECT COUNT(*) FROM password_reset_codes",
        ),
    ] {
        let count: i64 = sqlx::query_scalar(count_sql)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "unexpected retained row count for {table}");
    }
}

/// Expired code rows that were ever locked are kept for the lockout memory
/// window, so waiting for cleanup doesn't reset the escalating lock. Once idle
/// past that window they go like any other expired row.
#[sqlx::test(migrations = "./migrations")]
async fn keeps_expired_code_rows_with_lockouts_until_idle(pool: PgPool) {
    // (lockouts, last sent) → kept?
    let cases = [
        (0, "-1 hour", false),
        (2, "-1 hour", true),
        (2, "-31 days", false),
    ];
    // sqlx only takes static SQL, so each table spells out its own statements.
    let tables = [
        (
            "email_verification_codes",
            "INSERT INTO email_verification_codes (id, user_id, code_hash, expires_at, attempts, lockouts, last_sent_at)
             VALUES ($1, $2, 'h', NOW() - INTERVAL '1 minute', 5, $3, NOW() + $4::interval)",
            "SELECT user_id FROM email_verification_codes ORDER BY user_id",
        ),
        (
            "password_reset_codes",
            "INSERT INTO password_reset_codes (id, user_id, code_hash, expires_at, attempts, lockouts, last_sent_at)
             VALUES ($1, $2, 'h', NOW() - INTERVAL '1 minute', 5, $3, NOW() + $4::interval)",
            "SELECT user_id FROM password_reset_codes ORDER BY user_id",
        ),
    ];
    for (table, insert_sql, select_sql) in tables {
        let mut kept = Vec::new();
        for (i, (lockouts, sent, want_kept)) in cases.into_iter().enumerate() {
            let user = Uuid::now_v7();
            sqlx::query("INSERT INTO users (id, email) VALUES ($1, $2)")
                .bind(user)
                .bind(format!("lockout-memory-{table}-{i}@example.com"))
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(insert_sql)
                .bind(Uuid::now_v7())
                .bind(user)
                .bind(lockouts)
                .bind(sent)
                .execute(&pool)
                .await
                .unwrap();
            if want_kept {
                kept.push(user);
            }
        }

        database_maintenance::run_once(&pool).await.unwrap();

        let remaining: Vec<Uuid> = sqlx::query_scalar(select_sql)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, kept, "{table}");
    }
}
