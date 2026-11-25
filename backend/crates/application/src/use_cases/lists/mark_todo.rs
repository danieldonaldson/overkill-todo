//! Mark a todo done or not done at the version the Member saw.

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListWrite};
use crate::use_cases::lists::{ListError, MutationOutcome};
use domain::entities::Todo;
use domain::value_objects::{ListId, TodoId, UserId};

/// Applies a done-state change to one of the caller's todos.
pub struct MarkTodoUseCase {
    lists: Arc<dyn ListRepository>,
}

impl MarkTodoUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`]/[`ListError::TodoNotFound`] for a missing or
    /// foreign ID, [`ListError::Domain`] for a repository failure.
    #[tracing::instrument(name = "usecase.mark_todo", skip_all, fields(user_id = %user_id, list_id = %list_id, todo_id = %todo_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        todo_id: &TodoId,
        version: i64,
        done: bool,
    ) -> Result<MutationOutcome<Todo>, ListError> {
        match self
            .lists
            .mark_todo(user_id, list_id, todo_id, version, done)
            .await?
        {
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
    use domain::value_objects::TodoTitle;

    fn open_todo(list_id: ListId) -> Todo {
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

    fn repo(list_id: ListId, outcome: ListWrite<Todo>) -> MockListRepository {
        let mut repo = MockListRepository::new();
        repo.expect_mark_todo()
            .withf(move |_, l, _, version, done| *l == list_id && *version == 1 && *done)
            .times(1)
            .returning(move |_, _, _, _, _| Ok(outcome.clone()));
        repo
    }

    #[tokio::test]
    async fn applies_the_change() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::Applied(open_todo(list_id)));
        let outcome = MarkTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, true)
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_stale_change_returns_the_current_todo() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::Stale(open_todo(list_id)));
        let outcome = MarkTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, true)
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Stale(_)));
    }

    #[tokio::test]
    async fn a_missing_todo_is_not_found() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::TodoNotFound);
        let err = MarkTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, true)
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::TodoNotFound));
    }

    #[tokio::test]
    async fn a_missing_list_is_not_found() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::ListNotFound);
        let err = MarkTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, true)
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_mark_todo()
            .returning(|_, _, _, _, _| Err(DomainError::Persistence("db down".into())));
        let err = MarkTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), &TodoId::new(), 1, true)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
