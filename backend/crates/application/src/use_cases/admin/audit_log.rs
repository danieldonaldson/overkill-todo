//! Reading the audit log (OT-008, Super Admins only).

use std::sync::Arc;

use crate::application::ports::outgoing::{AuditEntry, AuditFilter, AuditLogRepository};
use crate::domain::entities::AuditKind;
use crate::domain::errors::DomainError;

/// One page of events plus the total matching the filter.
#[derive(Debug)]
pub struct AuditPage {
    pub events: Vec<AuditEntry>,
    pub total: i64,
}

/// Lists audit events, newest first, for the audit log page.
pub struct ListAuditEventsUseCase {
    audit_log: Arc<dyn AuditLogRepository>,
}

impl ListAuditEventsUseCase {
    #[must_use]
    pub const fn new(audit_log: Arc<dyn AuditLogRepository>) -> Self {
        Self { audit_log }
    }

    /// # Errors
    /// Propagates persistence failures.
    #[tracing::instrument(
        name = "usecase.admin.list_audit_events",
        skip_all,
        fields(limit, offset, filtered_by_account = filter.account.is_some(), kind = filter.kind.map(AuditKind::as_str))
    )]
    pub async fn execute(
        &self,
        filter: AuditFilter,
        limit: i64,
        offset: i64,
    ) -> Result<AuditPage, DomainError> {
        let (events, total) = tokio::try_join!(
            self.audit_log.list(filter, limit, offset),
            self.audit_log.count(filter),
        )?;
        Ok(AuditPage { events, total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::audit_log_repository::MockAuditLogRepository;
    use crate::domain::value_objects::UserId;

    #[tokio::test]
    async fn passes_the_filter_and_page_through() {
        let account = UserId::new();
        let filter = AuditFilter {
            account: Some(account),
            kind: Some(AuditKind::RoleChanged),
        };
        let mut repo = MockAuditLogRepository::new();
        repo.expect_list()
            .withf(move |f, limit, offset| *f == filter && *limit == 50 && *offset == 100)
            .returning(|_, _, _| Ok(Vec::new()));
        repo.expect_count()
            .withf(move |f| *f == filter)
            .returning(|_| Ok(7));

        let page = ListAuditEventsUseCase::new(Arc::new(repo))
            .execute(filter, 50, 100)
            .await
            .unwrap();
        assert_eq!(page.events, Vec::new());
        assert_eq!(page.total, 7);
    }
}
