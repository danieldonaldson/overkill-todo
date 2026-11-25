//! Add a todo at the bottom of a list, idempotently by the client's key.

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListWrite};
use crate::use_cases::lists::{ListError, MutationOutcome};
use domain::entities::Todo;
use domain::value_objects::{ListId, TodoTitle, UserId};
use uuid::Uuid;

/// Adds a todo to one of the caller's lists.
pub struct AddTodoUseCase {
    lists: Arc<dyn ListRepository>,
}

impl AddTodoUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`] for a missing or foreign list,
    /// [`ListError::Domain`] for invalid input or a repository failure.
    #[tracing::instrument(name = "usecase.add_todo", skip_all, fields(user_id = %user_id, list_id = %list_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        key: Uuid,
        title: &str,
    ) -> Result<MutationOutcome<Todo>, ListError> {
        let title = TodoTitle::new(title)?;
        match self.lists.add_todo(user_id, list_id, key, &title).await? {
            ListWrite::Applied(todo) | ListWrite::Repeated(todo) => {
                Ok(MutationOutcome::Applied(todo))
            }
            ListWrite::Stale(todo) => Ok(MutationOutcome::Stale(todo)),
            ListWrite::LimitReached(limit) => Ok(MutationOutcome::LimitReached(limit)),
            ListWrite::ListNotFound => Err(ListError::ListNotFound),
            ListWrite::TodoNotFound => Err(ListError::TodoNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::list_repository::MockListRepository;
    use domain::entities::{POSITION_GAP, TodoParts};
    use domain::errors::DomainError;
    use domain::value_objects::TodoId;

    fn todo(list_id: ListId) -> Todo {
        Todo::from_persistence(TodoParts {
            id: TodoId::new(),
            list_id,
            title: TodoTitle::new("Buy milk").unwrap(),
            version: 1,
            position: Some(POSITION_GAP),
            completed_at: None,
            created_at: jiff::Timestamp::now(),
        })
        .unwrap()
    }

    fn repo(user_id: UserId, list_id: ListId, outcome: ListWrite<Todo>) -> MockListRepository {
        let mut repo = MockListRepository::new();
        repo.expect_add_todo()
            .withf(move |u, l, _, title| {
                *u == user_id && *l == list_id && title.as_str() == "Buy milk"
            })
            .times(1)
            .returning(move |_, _, _, _| Ok(outcome.clone()));
        repo
    }

    #[tokio::test]
    async fn adds_the_todo() {
        let (user_id, list_id) = (UserId::new(), ListId::new());
        let repo = repo(user_id, list_id, ListWrite::Applied(todo(list_id)));
        let outcome = AddTodoUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, Uuid::now_v7(), "Buy milk")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_repeated_key_returns_the_existing_todo() {
        let (user_id, list_id) = (UserId::new(), ListId::new());
        let repo = repo(user_id, list_id, ListWrite::Repeated(todo(list_id)));
        let outcome = AddTodoUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, Uuid::now_v7(), "Buy milk")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_limit_is_an_outcome() {
        let (user_id, list_id) = (UserId::new(), ListId::new());
        let repo = repo(
            user_id,
            list_id,
            ListWrite::LimitReached(crate::ports::outgoing::LimitKind::Todos),
        );
        let outcome = AddTodoUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, Uuid::now_v7(), "Buy milk")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::LimitReached(_)));
    }

    #[tokio::test]
    async fn a_missing_or_foreign_list_is_not_found() {
        let (user_id, list_id) = (UserId::new(), ListId::new());
        let repo = repo(user_id, list_id, ListWrite::ListNotFound);
        let err = AddTodoUseCase::new(Arc::new(repo))
            .execute(&user_id, &list_id, Uuid::now_v7(), "Buy milk")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
    }

    #[tokio::test]
    async fn invalid_input_never_reaches_the_repository() {
        let repo = MockListRepository::new();
        let err = AddTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), Uuid::now_v7(), "")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::Domain(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_add_todo()
            .returning(|_, _, _, _| Err(DomainError::Persistence("db down".into())));
        let err = AddTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), Uuid::now_v7(), "Buy milk")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
