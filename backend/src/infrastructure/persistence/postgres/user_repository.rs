use super::connection::Database;
use super::error::persist_err;
use super::models::UserRow;
use crate::application::ports::outgoing::{
    AccountLifecycleRepository, SessionAuthorizationState, UserRepository,
};
use crate::domain::{
    entities::{User, UserParts},
    errors::DomainError,
    value_objects::{Email, UserId, UserRole},
};
use async_trait::async_trait;
use std::sync::Arc;

pub struct PostgresUserRepository {
    db: Arc<Database>,
}

impl PostgresUserRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn row_to_user(row: UserRow) -> Result<User, DomainError> {
        let email = Email::new(&row.email)?;
        let pending_email = row.pending_email.map(|e| Email::new(&e)).transpose()?;
        Ok(User::from_persistence(UserParts {
            id: UserId::from_uuid(row.id),
            email,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email_verified: row.email_verified,
            pending_email,
            pending_email_requested_at: row.pending_email_requested_at,
            deleted_at: row.deleted_at,
            deletion_scheduled_at: row.deletion_scheduled_at,
            role: row.role,
            created_at: row.created_at,
            updated_at: row.updated_at,
            version: row.version,
        }))
    }
}

#[async_trait]
impl UserRepository for PostgresUserRepository {
    #[tracing::instrument(name = "repo.user.save", skip(self, user), fields(user_id = %user.id()))]
    async fn save(&self, user: &User) -> Result<(), DomainError> {
        // Optimistic concurrency: a fresh user INSERTs at version 0; an update
        // takes the `ON CONFLICT` branch and applies ONLY when the persisted
        // `version` still equals the value this entity was loaded at, bumping it
        // by one. A concurrent writer that already advanced the row makes the
        // `WHERE` fail, so the statement touches 0 rows and we surface a
        // `Conflict` instead of silently clobbering the other write.
        let result = sqlx::query!(
            r#"
            INSERT INTO users (id, email, password_hash, first_name, last_name, email_verified, pending_email, pending_email_requested_at, deleted_at, deletion_scheduled_at, role, created_at, updated_at, version)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (id) DO UPDATE SET
                email = EXCLUDED.email,
                password_hash = EXCLUDED.password_hash,
                first_name = EXCLUDED.first_name,
                last_name = EXCLUDED.last_name,
                email_verified = EXCLUDED.email_verified,
                pending_email = EXCLUDED.pending_email,
                pending_email_requested_at = EXCLUDED.pending_email_requested_at,
                deleted_at = EXCLUDED.deleted_at,
                deletion_scheduled_at = EXCLUDED.deletion_scheduled_at,
                role = EXCLUDED.role,
                updated_at = EXCLUDED.updated_at,
                version = users.version + 1
            WHERE users.version = $14
            "#,
            user.id().as_uuid(),
            user.email().as_str(),
            user.password_hash(),
            user.first_name(),
            user.last_name(),
            user.email_verified(),
            user.pending_email().map(Email::as_str),
            user.pending_email_requested_at(),
            user.deleted_at(),
            user.deletion_scheduled_at(),
            user.role() as _,
            user.created_at(),
            user.updated_at(),
            user.version(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        if result.rows_affected() == 0 {
            return Err(DomainError::Conflict(
                "User was modified concurrently; reload and retry".into(),
            ));
        }

        Ok(())
    }

    #[tracing::instrument(name = "repo.user.find_by_id", skip(self), fields(user_id = %id))]
    async fn find_by_id(&self, id: &UserId) -> Result<Option<User>, DomainError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, email, password_hash, first_name, last_name, email_verified, pending_email, pending_email_requested_at, deleted_at, deletion_scheduled_at, role as "role: _", created_at, updated_at, version
            FROM users
            WHERE id = $1
            "#,
            id.as_uuid()
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        row.map(Self::row_to_user).transpose()
    }

    #[tracing::instrument(name = "repo.user.find_by_email", skip(self), fields(email = %email))]
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, DomainError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, email, password_hash, first_name, last_name, email_verified, pending_email, pending_email_requested_at, deleted_at, deletion_scheduled_at, role as "role: _", created_at, updated_at, version
            FROM users
            WHERE email = $1
            "#,
            email
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        row.map(Self::row_to_user).transpose()
    }

    #[tracing::instrument(name = "repo.user.find_by_verified_email", skip(self), fields(email = %email))]
    async fn find_by_verified_email(&self, email: &str) -> Result<Option<User>, DomainError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, email, password_hash, first_name, last_name, email_verified, pending_email, pending_email_requested_at, deleted_at, deletion_scheduled_at, role as "role: _", created_at, updated_at, version
            FROM users
            WHERE email = $1 AND email_verified = true
            "#,
            email
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        row.map(Self::row_to_user).transpose()
    }

    #[tracing::instrument(name = "repo.user.find_all", skip(self))]
    async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<User>, DomainError> {
        let rows = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, email, password_hash, first_name, last_name, email_verified, pending_email, pending_email_requested_at, deleted_at, deletion_scheduled_at, role as "role: _", created_at, updated_at, version
            FROM users
            WHERE deleted_at IS NULL
            ORDER BY created_at DESC, id DESC
            LIMIT $1 OFFSET $2
            "#,
            limit,
            offset
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(persist_err)?;

        rows.into_iter().map(Self::row_to_user).collect()
    }

    #[tracing::instrument(name = "repo.user.save_role_change", skip(self, user), fields(user_id = %user.id(), removing_super_admin))]
    async fn save_role_change(
        &self,
        user: &User,
        removing_super_admin: bool,
    ) -> Result<(), DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;

        if removing_super_admin {
            // Serialize every super-admin demotion. The lock is held until the
            // transaction commits, so two concurrent requests cannot both
            // count the same two admins and remove each one.
            sqlx::query!("SELECT pg_advisory_xact_lock($1)", 0x5457_524f_4c45_i64)
                .execute(&mut *tx)
                .await
                .map_err(persist_err)?;

            let row = sqlx::query!(
                r#"SELECT COUNT(*) AS "count!" FROM users WHERE role = $1 AND deleted_at IS NULL"#,
                UserRole::SuperAdmin as _,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(persist_err)?;

            if row.count <= 1 {
                return Err(DomainError::Forbidden(
                    "Cannot demote the last super-admin".into(),
                ));
            }
        }

        let result = sqlx::query!(
            r#"
            UPDATE users
            SET role = $1, tokens_valid_after = NOW(), updated_at = NOW(), version = version + 1
            WHERE id = $2 AND version = $3
            "#,
            user.role() as _,
            user.id().as_uuid(),
            user.version(),
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        if result.rows_affected() == 0 {
            return Err(DomainError::Conflict(
                "User was modified concurrently; reload and retry".into(),
            ));
        }

        tx.commit().await.map_err(persist_err)?;
        Ok(())
    }

    #[tracing::instrument(name = "repo.user.session_authorization_state", skip(self), fields(user_id = %id))]
    async fn session_authorization_state(
        &self,
        id: &UserId,
    ) -> Result<Option<SessionAuthorizationState>, DomainError> {
        let row = sqlx::query!(
            r#"SELECT role AS "role: UserRole", tokens_valid_after FROM users WHERE id = $1"#,
            id.as_uuid(),
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(row.map(|r| SessionAuthorizationState {
            role: r.role,
            tokens_valid_after: r.tokens_valid_after,
        }))
    }

    #[tracing::instrument(name = "repo.user.mark_email_verified", skip(self), fields(user_id = %id))]
    async fn mark_email_verified(&self, id: &UserId) -> Result<(), DomainError> {
        let result = sqlx::query!(
            r#"
            UPDATE users
            SET email_verified = true, version = version + 1, updated_at = NOW()
            WHERE id = $1
            "#,
            id.as_uuid()
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        // A 0-row update means the user was deleted/anonymized between load and
        // write. Surface it as NotFound instead of a silent success that hides the
        // fact that nothing was verified.
        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound("User not found".into()));
        }

        Ok(())
    }

    #[tracing::instrument(name = "repo.user.update_password", skip(self, password_hash), fields(user_id = %id))]
    async fn update_password(&self, id: &UserId, password_hash: &str) -> Result<(), DomainError> {
        let result = sqlx::query!(
            r#"
            UPDATE users
            SET password_hash = $2, version = version + 1, updated_at = NOW()
            WHERE id = $1
            "#,
            id.as_uuid(),
            password_hash
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        // See mark_email_verified: a vanished row must not look like a successful
        // password change.
        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound("User not found".into()));
        }

        Ok(())
    }
}

#[async_trait]
impl AccountLifecycleRepository for PostgresUserRepository {
    #[tracing::instrument(name = "repo.user.find_ready_for_deletion", skip(self))]
    async fn find_ready_for_deletion(&self) -> Result<Vec<User>, DomainError> {
        let rows = sqlx::query_as!(
            UserRow,
            r#"
            SELECT id, email, password_hash, first_name, last_name, email_verified, pending_email, pending_email_requested_at, deleted_at, deletion_scheduled_at, role as "role: _", created_at, updated_at, version
            FROM users
            WHERE deletion_scheduled_at IS NOT NULL
              AND deletion_scheduled_at <= NOW()
              AND deleted_at IS NULL
            "#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(persist_err)?;

        rows.into_iter().map(Self::row_to_user).collect()
    }

    // INVARIANT: tables holding the only durable handle to a live external
    // resource (a provisioned workload, a billable API key) should reference
    // `users` with ON DELETE RESTRICT, so a hard delete that skipped external
    // teardown fails loudly instead of orphaning the resource. Account removal
    // goes through `anonymize_account`, which scrubs PII and secrets while
    // leaving audit/financial history intact. This method is not used in
    // production.
    #[tracing::instrument(name = "repo.user.delete", skip(self), fields(user_id = %id))]
    async fn delete(&self, id: &UserId) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            DELETE FROM users
            WHERE id = $1
            "#,
            id.as_uuid()
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.user.anonymize_account", skip(self, user), fields(user_id = %user.id()))]
    async fn anonymize_account(&self, user: &User) -> Result<(), DomainError> {
        // INVARIANT: every step below runs in ONE transaction. The user
        // row is scrubbed *and* all secret-bearing child rows (refresh tokens,
        // OAuth links, verification/reset codes) are deleted
        // atomically, or nothing is. A partial commit would leave a user that
        // looks anonymized while live credentials/PII survive in child tables —
        // so never split this into separate transactions or move a DELETE out.
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;

        let user_id = user.id().as_uuid();

        // 1. Update user record with anonymized data
        sqlx::query!(
            r#"
            UPDATE users
            SET email = $2,
                password_hash = NULL,
                first_name = $3,
                last_name = $4,
                pending_email = NULL,
                pending_email_requested_at = NULL,
                deleted_at = $5,
                deletion_scheduled_at = NULL,
                version = version + 1,
                updated_at = $6
            WHERE id = $1
            "#,
            user_id,
            user.email().as_str(),
            user.first_name(),
            user.last_name(),
            user.deleted_at(),
            user.updated_at(),
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        // 2. Delete refresh tokens
        sqlx::query!(r#"DELETE FROM refresh_tokens WHERE user_id = $1"#, user_id)
            .execute(&mut *tx)
            .await
            .map_err(persist_err)?;

        // 3. Delete OAuth accounts
        sqlx::query!(
            r#"DELETE FROM user_oauth_accounts WHERE user_id = $1"#,
            user_id
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        // 4. Delete email verification codes
        sqlx::query!(
            r#"DELETE FROM email_verification_codes WHERE user_id = $1"#,
            user_id
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        // 5. Delete password reset codes
        sqlx::query!(
            r#"DELETE FROM password_reset_codes WHERE user_id = $1"#,
            user_id
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        tx.commit().await.map_err(persist_err)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::UserRole;
    use crate::infrastructure::persistence::postgres::models::UserRow;
    use chrono::Utc;
    use proptest::prelude::*;
    use uuid::Uuid;

    // ==================== Strategies ====================

    /// Strategy for valid email strings (lowercase with @ and .)
    fn valid_email_strategy() -> impl Strategy<Value = String> {
        "[a-z]{1,8}@[a-z]{2,6}\\.[a-z]{2,4}".prop_map(String::from)
    }

    /// Strategy for invalid email strings (missing @ or .)
    fn invalid_email_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            // Missing @
            "[a-z]{3,10}\\.[a-z]{2,4}".prop_map(String::from),
            // Missing .
            "[a-z]{1,8}@[a-z]{3,10}".prop_map(String::from),
            // Empty
            Just(String::new()),
        ]
    }

    /// Creates a valid `UserRow` with the given email.
    fn create_user_row(email: String) -> UserRow {
        let now = Utc::now();
        UserRow {
            id: Uuid::new_v4(),
            email,
            password_hash: Some("fake_hash".to_string()),
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            email_verified: true,
            pending_email: None,
            pending_email_requested_at: None,
            deleted_at: None,
            deletion_scheduled_at: None,
            role: UserRole::User,
            created_at: now,
            updated_at: now,
            version: 0,
        }
    }

    // ==================== Example-Based Tests ====================

    #[test]
    fn test_row_to_user_valid() {
        let row = create_user_row("test@example.com".to_string());
        let result = PostgresUserRepository::row_to_user(row);
        assert!(result.is_ok());
        let user = result.unwrap();
        assert_eq!(user.email().as_str(), "test@example.com");
    }

    #[test]
    fn test_row_to_user_invalid_email() {
        let row = create_user_row("invalid-email".to_string());
        let result = PostgresUserRepository::row_to_user(row);
        assert!(result.is_err());
    }

    // ==================== Property-Based Tests ====================

    proptest! {
        /// Valid rows should convert successfully
        #[test]
        fn prop_valid_row_converts_successfully(
            email in valid_email_strategy(),
            first_name in "\\PC{1,20}",
            last_name in "\\PC{1,20}",
        ) {
            let now = Utc::now();
            let row = UserRow {
                id: Uuid::new_v4(),
                email: email.clone(),
                password_hash: Some("fake_hash".to_string()),
                first_name,
                last_name,
                email_verified: true,
                pending_email: None,
                pending_email_requested_at: None,
                deleted_at: None,
                deletion_scheduled_at: None,
                role: UserRole::User,
                created_at: now,
                updated_at: now,
                version: 0,
            };

            let result = PostgresUserRepository::row_to_user(row);
            prop_assert!(result.is_ok(), "Valid row failed to convert: {:?}", result.err());

            let user = result.unwrap();
            // Email should be normalized (lowercase, trimmed)
            prop_assert_eq!(user.email().as_str(), email.trim().to_lowercase());
        }

        /// Invalid email strings should cause conversion to fail
        #[test]
        fn prop_invalid_email_fails(
            email in invalid_email_strategy(),
        ) {
            let row = create_user_row(email.clone());
            let result = PostgresUserRepository::row_to_user(row);
            prop_assert!(result.is_err(), "Invalid email '{}' should fail", email);
        }

        /// Pending email validation: valid pending_email should work
        #[test]
        fn prop_valid_pending_email_converts(
            email in valid_email_strategy(),
            pending_email in valid_email_strategy(),
        ) {
            let now = Utc::now();
            let row = UserRow {
                id: Uuid::new_v4(),
                email,
                password_hash: None,
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email_verified: false,
                pending_email: Some(pending_email.clone()),
                pending_email_requested_at: Some(now),
                deleted_at: None,
                deletion_scheduled_at: None,
                role: UserRole::User,
                created_at: now,
                updated_at: now,
                version: 0,
            };

            let result = PostgresUserRepository::row_to_user(row);
            prop_assert!(result.is_ok());

            let user = result.unwrap();
            prop_assert!(user.pending_email().is_some());
            prop_assert_eq!(
                user.pending_email().unwrap().as_str(),
                pending_email.trim().to_lowercase()
            );
        }

        /// Invalid pending_email should cause conversion to fail
        #[test]
        fn prop_invalid_pending_email_fails(
            email in valid_email_strategy(),
            pending_email in invalid_email_strategy(),
        ) {
            let now = Utc::now();
            let row = UserRow {
                id: Uuid::new_v4(),
                email,
                password_hash: None,
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email_verified: false,
                pending_email: Some(pending_email.clone()),
                pending_email_requested_at: Some(now),
                deleted_at: None,
                deletion_scheduled_at: None,
                role: UserRole::User,
                created_at: now,
                updated_at: now,
                version: 0,
            };

            let result = PostgresUserRepository::row_to_user(row);
            prop_assert!(result.is_err(), "Invalid pending_email '{}' should fail", pending_email);
        }

        /// UUID roundtrip: user id should be preserved
        #[test]
        fn prop_user_id_preserved(
            email in valid_email_strategy(),
        ) {
            let original_id = Uuid::new_v4();
            let now = Utc::now();
            let row = UserRow {
                id: original_id,
                email,
                password_hash: None,
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email_verified: true,
                pending_email: None,
                pending_email_requested_at: None,
                deleted_at: None,
                deletion_scheduled_at: None,
                role: UserRole::User,
                created_at: now,
                updated_at: now,
                version: 0,
            };

            let result = PostgresUserRepository::row_to_user(row);
            prop_assert!(result.is_ok());

            let user = result.unwrap();
            prop_assert_eq!(*user.id().as_uuid(), original_id);
        }

        /// Boolean fields should be preserved
        #[test]
        fn prop_boolean_fields_preserved(
            email in valid_email_strategy(),
            email_verified in proptest::bool::ANY,
        ) {
            let now = Utc::now();
            let row = UserRow {
                id: Uuid::new_v4(),
                email,
                password_hash: None,
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email_verified,
                pending_email: None,
                pending_email_requested_at: None,
                deleted_at: None,
                deletion_scheduled_at: None,
                role: UserRole::User,
                created_at: now,
                updated_at: now,
                version: 0,
            };

            let result = PostgresUserRepository::row_to_user(row);
            prop_assert!(result.is_ok());

            let user = result.unwrap();
            prop_assert_eq!(user.email_verified(), email_verified);
        }
    }
}
