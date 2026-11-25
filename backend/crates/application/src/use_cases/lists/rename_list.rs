//! Rename a list at the version the Member saw (design D1/D5).

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListSummary, ListWrite};
use crate::use_cases::lists::{ListError, MutationOutcome};
use domain::value_objects::{ListId, ListName, UserId};

/// Renames one of the caller's lists.
pub struct RenameListUseCase {
    lists: Arc<dyn ListRepository>,
}

impl RenameListUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`] for a missing or foreign list,
    /// [`ListError::Domain`] for invalid input or a repository failure.
    #[tracing::instrument(name = "usecase.rename_list", skip_all, fields(user_id = %user_id, list_id = %list_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        version: i64,
        name: &str,
    ) -> Result<MutationOutcome<ListSummary>, ListError> {
        let name = ListName::new(name)?;
        match self
            .lists
            .rename_list(user_id, list_id, version, &name)
            .await?
        {
            ListWrite::Applied(list) | ListWrite::Repeated(list) => {
                Ok(MutationOutcome::Applied(list))
            }
            ListWrite::Stale(list) => Ok(MutationOutcome::Stale(list)),
            ListWrite::LimitReached(limit) => Ok(MutationOutcome::LimitReached(limit)),
            ListWrite::ListNotFound | ListWrite::TodoNotFound => Err(ListError::ListNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::list_repository::MockListRepository;
    use domain::entities::{List, ListParts};
    use domain::errors::DomainError;

    fn summary(user_id: UserId, version: i64) -> ListSummary {
        ListSummary {
            list: List::from_persistence(ListParts {
                id: ListId::new(),
                user_id,
                name: ListName::new("Job").unwrap(),
                version,
                created_at: jiff::Timestamp::now(),
            }),
            open_count: 0,
            todo_count: 0,
        }
    }

    fn repo(
        user_id: UserId,
        list_id: ListId,
        outcome: ListWrite<ListSummary>,
    ) -> MockListRepository {
        let mut repo = MockListRepository::new();
        repo.expect_rename_list()
            .withf(move |u, l, version, name| {
                *u == user_id && *l == list_id && *version == 1 && name.as_str() == "Job"
            })
            .times(1)
            .returning(move |_, _, _, _| Ok(outcome.clone()));
        repo
    }

    #[tokio::test]
    async fn applies_the_rename() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::Applied(summary(user_id, 2)));
        let outcome = RenameListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, "  Job ")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_repeated_name_succeeds() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::Repeated(summary(user_id, 1)));
        let outcome = RenameListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, "Job")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_stale_rename_returns_the_current_list() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::Stale(summary(user_id, 3)));
        let outcome = RenameListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, "Job")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Stale(_)));
    }

    #[tokio::test]
    async fn a_missing_list_is_not_found() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::ListNotFound);
        let err = RenameListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, "Job")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
    }

    #[tokio::test]
    async fn invalid_input_never_reaches_the_repository() {
        let repo = MockListRepository::new();
        let err = RenameListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), 1, " \n ")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::Domain(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_rename_list()
            .returning(|_, _, _, _| Err(DomainError::Persistence("db down".into())));
        let err = RenameListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), 1, "Job")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
