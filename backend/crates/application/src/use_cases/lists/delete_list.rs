//! Delete a list, confirmed by its version and todo count (design D5/D10).

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListSummary, ListWrite};
use crate::use_cases::lists::{ListError, MutationOutcome};
use domain::value_objects::{ListId, UserId};

/// Deletes one of the caller's lists.
pub struct DeleteListUseCase {
    lists: Arc<dyn ListRepository>,
}

impl DeleteListUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`] for a missing or foreign list, and
    /// [`ListError::Domain`] for a repository failure.
    #[tracing::instrument(name = "usecase.delete_list", skip_all, fields(user_id = %user_id, list_id = %list_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        version: i64,
        todo_count: i64,
    ) -> Result<MutationOutcome<ListSummary>, ListError> {
        match self
            .lists
            .delete_list(user_id, list_id, version, todo_count)
            .await?
        {
            ListWrite::Applied(list) | ListWrite::Repeated(list) => {
                Ok(MutationOutcome::Applied(list))
            }
            // The refusal carries the current name, version and count, so the
            // Member confirms again without a refetch.
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
    use domain::value_objects::ListName;

    fn summary(user_id: UserId, version: i64, todo_count: i64) -> ListSummary {
        ListSummary {
            list: List::from_persistence(ListParts {
                id: ListId::new(),
                user_id,
                name: ListName::new("Work").unwrap(),
                version,
                created_at: jiff::Timestamp::now(),
            }),
            open_count: todo_count,
            todo_count,
        }
    }

    fn repo(
        user_id: UserId,
        list_id: ListId,
        outcome: ListWrite<ListSummary>,
    ) -> MockListRepository {
        let mut repo = MockListRepository::new();
        repo.expect_delete_list()
            .withf(move |u, l, version, todo_count| {
                *u == user_id && *l == list_id && *version == 1 && *todo_count == 5
            })
            .times(1)
            .returning(move |_, _, _, _| Ok(outcome.clone()));
        repo
    }

    #[tokio::test]
    async fn deletes_the_list() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::Applied(summary(user_id, 1, 5)));
        let outcome = DeleteListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, 5)
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_changed_list_is_a_stale_refusal_with_its_counts() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::Stale(summary(user_id, 2, 6)));
        let outcome = DeleteListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, 5)
            .await
            .unwrap();
        match outcome {
            MutationOutcome::Stale(current) => {
                assert_eq!(current.todo_count, 6);
                assert_eq!(current.list.version(), 2);
            }
            _ => panic!("expected Stale"),
        }
    }

    #[tokio::test]
    async fn a_missing_list_is_not_found() {
        let user_id = UserId::new();
        let list_id = ListId::new();
        let repo = repo(user_id, list_id, ListWrite::ListNotFound);
        let err = DeleteListUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, 1, 5)
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_delete_list()
            .returning(|_, _, _, _| Err(DomainError::Persistence("db down".into())));
        let err = DeleteListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), 1, 5)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
