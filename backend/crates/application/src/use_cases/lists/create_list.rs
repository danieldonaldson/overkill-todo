//! Create a list, idempotently by the client's creation key.

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListSummary, ListWrite};
use crate::use_cases::lists::{ListError, MutationOutcome};
use domain::value_objects::{ListName, UserId};
use uuid::Uuid;

/// Creates a list for the caller.
pub struct CreateListUseCase {
    lists: Arc<dyn ListRepository>,
}

impl CreateListUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::Domain`] for invalid input (`Validation`) or a repository
    /// failure. A repeated key is `Applied` with the existing list.
    #[tracing::instrument(name = "usecase.create_list", skip_all, fields(user_id = %user_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        key: Uuid,
        name: &str,
    ) -> Result<MutationOutcome<ListSummary>, ListError> {
        let name = ListName::new(name)?;
        match self.lists.create_list(user_id, key, &name).await? {
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
    use domain::value_objects::ListId;

    fn summary(user_id: UserId) -> ListSummary {
        ListSummary {
            list: List::from_persistence(ListParts {
                id: ListId::new(),
                user_id,
                name: ListName::new("Work").unwrap(),
                version: 1,
                created_at: jiff::Timestamp::now(),
            }),
            open_count: 0,
            todo_count: 0,
        }
    }

    fn repo_returning(user_id: UserId, outcome: ListWrite<ListSummary>) -> MockListRepository {
        let mut repo = MockListRepository::new();
        repo.expect_create_list()
            .withf(move |id, _, name| *id == user_id && name.as_str() == "Work")
            .times(1)
            .returning(move |_, _, _| Ok(outcome.clone()));
        repo
    }

    #[tokio::test]
    async fn creates_the_list() {
        let user_id = UserId::new();
        let repo = repo_returning(user_id, ListWrite::Applied(summary(user_id)));
        let outcome = CreateListUseCase::new(Arc::new(repo))
            .execute(&user_id, Uuid::now_v7(), "  Work  ")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_repeated_key_returns_the_existing_list() {
        let user_id = UserId::new();
        let repo = repo_returning(user_id, ListWrite::Repeated(summary(user_id)));
        let outcome = CreateListUseCase::new(Arc::new(repo))
            .execute(&user_id, Uuid::now_v7(), "Work")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_limit_is_an_outcome_not_an_error() {
        let user_id = UserId::new();
        let repo = repo_returning(
            user_id,
            ListWrite::LimitReached(crate::ports::outgoing::LimitKind::Lists),
        );
        let outcome = CreateListUseCase::new(Arc::new(repo))
            .execute(&user_id, Uuid::now_v7(), "Work")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::LimitReached(_)));
    }

    #[tokio::test]
    async fn invalid_input_never_reaches_the_repository() {
        let repo = MockListRepository::new();
        let err = CreateListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), Uuid::now_v7(), " \n ")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::Domain(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_create_list()
            .returning(|_, _, _| Err(DomainError::Persistence("db down".into())));
        let err = CreateListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), Uuid::now_v7(), "Work")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
