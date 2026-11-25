//! A Member's own Security activity (OT-008 slice 3): the audit events about
//! their account, with the actor reduced so staff identities never leave the
//! server.

use std::sync::Arc;

use jiff::Timestamp;

use crate::ports::outgoing::{AuditLogRepository, SubjectEntry};
use domain::entities::{Actor, AuditDetails, AuditKind};
use domain::errors::DomainError;
use domain::value_objects::UserId;

/// Who acted, as a Member is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityActor {
    /// The Member themselves.
    You,
    /// An automatic action.
    System,
    /// Another account or the operator, never identified.
    Administrator,
}

impl ActivityActor {
    /// Reduces `actor` for the Member whose account is `subject`.
    #[must_use]
    pub fn for_subject(actor: Actor, subject: UserId) -> Self {
        match actor {
            Actor::User(id) if id == subject => Self::You,
            Actor::System => Self::System,
            Actor::User(_) | Actor::Operator => Self::Administrator,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntry {
    pub id: i64,
    pub occurred_at: Timestamp,
    pub kind: Option<AuditKind>,
    pub actor: ActivityActor,
    pub details: AuditDetails,
}

/// One page, newest first, and where the next one starts.
#[derive(Debug, PartialEq, Eq)]
pub struct ActivityPage {
    pub entries: Vec<ActivityEntry>,
    /// Pass as `before_id` for the next page; `None` when this was the last.
    pub next_before_id: Option<i64>,
}

pub struct ListSecurityActivityUseCase {
    audit_log: Arc<dyn AuditLogRepository>,
}

impl ListSecurityActivityUseCase {
    #[must_use]
    pub const fn new(audit_log: Arc<dyn AuditLogRepository>) -> Self {
        Self { audit_log }
    }

    /// Up to `limit` of `user_id`'s own events, older than `before_id` if given.
    ///
    /// # Errors
    /// Propagates persistence failures.
    #[tracing::instrument(name = "usecase.list_security_activity", skip_all, fields(user_id = %user_id, limit, paged = before_id.is_some()))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<ActivityPage, DomainError> {
        // One extra row tells whether another page exists.
        let mut rows = self
            .audit_log
            .list_for_subject(*user_id, limit + 1, before_id)
            .await?;
        let more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        rows.truncate(usize::try_from(limit).unwrap_or(0));
        let next_before_id = if more {
            rows.last().map(|e| e.id)
        } else {
            None
        };

        let entries = rows
            .into_iter()
            .map(|e: SubjectEntry| ActivityEntry {
                id: e.id,
                occurred_at: e.occurred_at,
                kind: e.kind,
                actor: ActivityActor::for_subject(e.actor, *user_id),
                details: e.details,
            })
            .collect();
        Ok(ActivityPage {
            entries,
            next_before_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::audit_log_repository::MockAuditLogRepository;

    fn entry(id: i64, actor: Actor) -> SubjectEntry {
        SubjectEntry {
            id,
            occurred_at: Timestamp::UNIX_EPOCH,
            kind: Some(AuditKind::PasswordChanged),
            actor,
            details: AuditDetails::None,
        }
    }

    #[test]
    fn actors_are_reduced_without_naming_staff() {
        let me = UserId::new();
        assert_eq!(
            ActivityActor::for_subject(Actor::User(me), me),
            ActivityActor::You
        );
        assert_eq!(
            ActivityActor::for_subject(Actor::System, me),
            ActivityActor::System
        );
        assert_eq!(
            ActivityActor::for_subject(Actor::User(UserId::new()), me),
            ActivityActor::Administrator
        );
        assert_eq!(
            ActivityActor::for_subject(Actor::Operator, me),
            ActivityActor::Administrator
        );
    }

    #[tokio::test]
    async fn pages_by_id_and_says_where_the_next_page_starts() {
        let me = UserId::new();
        let staff = UserId::new();
        let mut repo = MockAuditLogRepository::new();
        repo.expect_list_for_subject()
            .withf(move |subject, limit, before| {
                *subject == me && *limit == 3 && *before == Some(9)
            })
            .returning(move |_, _, _| {
                Ok(vec![
                    entry(8, Actor::User(me)),
                    entry(7, Actor::User(staff)),
                    entry(6, Actor::System),
                ])
            });

        let page = ListSecurityActivityUseCase::new(Arc::new(repo))
            .execute(&me, 2, Some(9))
            .await
            .unwrap();

        let actors: Vec<_> = page.entries.iter().map(|e| (e.id, e.actor)).collect();
        assert_eq!(
            actors,
            vec![(8, ActivityActor::You), (7, ActivityActor::Administrator)]
        );
        assert_eq!(page.next_before_id, Some(7));
    }

    #[tokio::test]
    async fn the_last_page_has_no_next() {
        let me = UserId::new();
        let mut repo = MockAuditLogRepository::new();
        repo.expect_list_for_subject()
            .returning(move |_, _, _| Ok(vec![entry(1, Actor::User(me))]));

        let page = ListSecurityActivityUseCase::new(Arc::new(repo))
            .execute(&me, 20, None)
            .await
            .unwrap();

        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.next_before_id, None);
    }
}
