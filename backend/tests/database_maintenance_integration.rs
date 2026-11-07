//! Integration coverage for bounded expiry and retention maintenance.

use app::infrastructure::tasks::database_maintenance;
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
