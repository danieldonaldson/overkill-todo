//! Audit log persistence (OT-008).
//!
//! An event is never written on its own by an audited action: the adapter
//! method that makes the action's primary write calls [`insert_audit_event`]
//! inside its own transaction, so the event exists if and only if the action
//! took effect.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::connection::Database;
use super::error::persist_err;
use super::time::ToDomain;
use application::ports::outgoing::{
    AuditAccount, AuditActorView, AuditEntry, AuditFilter, AuditLogRepository, SubjectEntry,
};
use domain::entities::{Actor, AuditDetails, AuditEvent, AuditKind, OAuthProvider};
use domain::errors::DomainError;
use domain::value_objects::{UserId, UserRole};
use platform::telemetry::current_trace_id;

fn details_json(details: AuditDetails) -> Value {
    match details {
        AuditDetails::None => json!({}),
        AuditDetails::RoleChanged { from, to } => {
            json!({ "from": from.to_db_str(), "to": to.to_db_str() })
        }
        AuditDetails::SignInMethod { provider } => json!({ "provider": provider.as_str() }),
    }
}

/// Reads stored details back for `kind`. Anything that doesn't fit (an older
/// or newer release's shape) reads as no details rather than an error.
fn details_from_json(kind: Option<AuditKind>, details: &Value) -> AuditDetails {
    let role = |field: &str| details.get(field)?.as_str().and_then(UserRole::from_db_str);
    match kind {
        Some(AuditKind::RoleChanged) => match (role("from"), role("to")) {
            (Some(from), Some(to)) => AuditDetails::RoleChanged { from, to },
            _ => AuditDetails::None,
        },
        Some(AuditKind::SignInMethodLinked | AuditKind::SignInMethodUnlinked) => details
            .get("provider")
            .and_then(Value::as_str)
            .and_then(OAuthProvider::parse)
            .map_or(AuditDetails::None, |provider| AuditDetails::SignInMethod {
                provider,
            }),
        Some(
            AuditKind::PasswordChanged
            | AuditKind::PasswordSet
            | AuditKind::PasswordReset
            | AuditKind::DeletionRequested
            | AuditKind::DeletionCancelled
            | AuditKind::AccountDeleted
            | AuditKind::DataExported
            | AuditKind::SessionsRevokedAfterReuse,
        )
        | None => AuditDetails::None,
    }
}

/// Inserts `event` on `conn`, which must be the transaction making the
/// action's primary write. The trace ID comes from the current span.
///
/// # Errors
/// A failed insert is returned so the caller's transaction rolls back and the
/// action doesn't take effect without its event.
pub(super) async fn insert_audit_event(
    conn: &mut PgConnection,
    event: &AuditEvent,
) -> Result<(), DomainError> {
    let actor = event.actor();
    let subject = event.subject();
    let details = details_json(event.details());
    let trace_id = current_trace_id();
    sqlx::query!(
        r#"
        INSERT INTO audit_events (kind, subject_id, actor_kind, actor_id, details, trace_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
        event.kind().as_str(),
        subject.as_uuid(),
        actor.kind_str(),
        actor.user_id().map(|id| *id.as_uuid()),
        details,
        trace_id,
    )
    .execute(conn)
    .await
    .map_err(|e| {
        tracing::error!(
            audit.kind = event.kind().as_str(),
            error = %e,
            "audit event insert failed; the action is rolled back"
        );
        persist_err(e)
    })?;
    Ok(())
}

pub struct PostgresAuditLogRepository {
    db: Arc<Database>,
}

impl PostgresAuditLogRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

const fn account(id: Uuid, email: Option<String>) -> AuditAccount {
    AuditAccount {
        id: UserId::from_uuid(id),
        email,
    }
}

#[async_trait]
impl AuditLogRepository for PostgresAuditLogRepository {
    #[tracing::instrument(name = "repo.audit_log.record", skip_all, fields(db.system = "postgresql", audit.kind = event.kind().as_str()))]
    async fn record(&self, event: &AuditEvent) -> Result<(), DomainError> {
        let mut conn = self.db.pool().acquire().await.map_err(persist_err)?;
        insert_audit_event(&mut conn, event).await
    }

    #[tracing::instrument(name = "repo.audit_log.list", skip_all, fields(db.system = "postgresql", limit, offset))]
    async fn list(
        &self,
        filter: AuditFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AuditEntry>, DomainError> {
        // The page is picked on IDs alone (an index-only scan when unfiltered),
        // then joined, so a deep page doesn't join every row it skips.
        // Emails come from today's account rows, never from the event, and are
        // hidden once the account is deleted (anonymised) or gone.
        let rows = sqlx::query!(
            r#"
            WITH page AS (
                SELECT id FROM audit_events
                WHERE ($1::uuid IS NULL OR subject_id = $1 OR actor_id = $1)
                  AND ($2::text IS NULL OR kind = $2)
                ORDER BY id DESC
                LIMIT $3 OFFSET $4
            )
            SELECT e.id, e.occurred_at, e.kind, e.subject_id, e.actor_kind, e.actor_id, e.details,
                   CASE WHEN s.deleted_at IS NULL THEN s.email END AS subject_email,
                   CASE WHEN a.deleted_at IS NULL THEN a.email END AS actor_email
            FROM page
            JOIN audit_events e ON e.id = page.id
            LEFT JOIN users s ON s.id = e.subject_id
            LEFT JOIN users a ON a.id = e.actor_id
            ORDER BY e.id DESC
            "#,
            filter.account.map(|id| *id.as_uuid()),
            filter.kind.map(AuditKind::as_str),
            limit,
            offset,
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let kind = AuditKind::parse(&row.kind);
                let actor = match (row.actor_kind.as_str(), row.actor_id) {
                    ("user", Some(id)) => AuditActorView::User(account(id, row.actor_email)),
                    ("operator", _) => AuditActorView::Operator,
                    _ => AuditActorView::System,
                };
                AuditEntry {
                    id: row.id,
                    occurred_at: row.occurred_at.to_domain(),
                    kind,
                    subject: account(row.subject_id, row.subject_email),
                    actor,
                    details: details_from_json(kind, &row.details),
                }
            })
            .collect())
    }

    #[tracing::instrument(name = "repo.audit_log.count", skip_all, fields(db.system = "postgresql"))]
    async fn count(&self, filter: AuditFilter) -> Result<i64, DomainError> {
        sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!" FROM audit_events e
            WHERE ($1::uuid IS NULL OR e.subject_id = $1 OR e.actor_id = $1)
              AND ($2::text IS NULL OR e.kind = $2)
            "#,
            filter.account.map(|id| *id.as_uuid()),
            filter.kind.map(AuditKind::as_str),
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(persist_err)
    }

    #[tracing::instrument(name = "repo.audit_log.list_for_subject", skip_all, fields(db.system = "postgresql", limit, paged = before_id.is_some()))]
    async fn list_for_subject(
        &self,
        subject: UserId,
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<Vec<SubjectEntry>, DomainError> {
        // Served by the (subject_id, id DESC) index.
        let rows = sqlx::query!(
            r#"
            SELECT id, occurred_at, kind, actor_kind, actor_id, details
            FROM audit_events
            WHERE subject_id = $1 AND ($2::bigint IS NULL OR id < $2)
            ORDER BY id DESC
            LIMIT $3
            "#,
            subject.as_uuid(),
            before_id,
            limit,
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let kind = AuditKind::parse(&row.kind);
                let actor = match (row.actor_kind.as_str(), row.actor_id) {
                    ("user", Some(id)) => Actor::User(UserId::from_uuid(id)),
                    ("operator", _) => Actor::Operator,
                    _ => Actor::System,
                };
                SubjectEntry {
                    id: row.id,
                    occurred_at: row.occurred_at.to_domain(),
                    kind,
                    actor,
                    details: details_from_json(kind, &row.details),
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_change_details_round_trip() {
        let details = AuditDetails::RoleChanged {
            from: UserRole::User,
            to: UserRole::SuperAdmin,
        };
        let kind = Some(AuditKind::RoleChanged);
        assert_eq!(details_from_json(kind, &details_json(details)), details);
    }

    #[test]
    fn sign_in_method_details_round_trip() {
        let details = AuditDetails::SignInMethod {
            provider: OAuthProvider::Google,
        };
        for kind in [
            AuditKind::SignInMethodLinked,
            AuditKind::SignInMethodUnlinked,
        ] {
            assert_eq!(
                details_from_json(Some(kind), &details_json(details)),
                details
            );
        }
    }

    #[test]
    fn unreadable_details_read_as_none() {
        let kind = Some(AuditKind::RoleChanged);
        assert_eq!(details_from_json(kind, &json!({})), AuditDetails::None);
        assert_eq!(
            details_from_json(kind, &json!({ "from": "wizard", "to": "admin" })),
            AuditDetails::None
        );
        assert_eq!(
            details_from_json(None, &json!({ "from": "user", "to": "admin" })),
            AuditDetails::None
        );
    }
}
