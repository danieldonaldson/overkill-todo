//! Rename a todo at the version the Member saw (design D1/D5).

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListWrite};
use crate::use_cases::lists::{ListError, MutationOutcome};
use domain::entities::Todo;
use domain::value_objects::{ListId, TodoId, TodoTitle, UserId};

/// Renames one of the caller's todos.
pub struct RenameTodoUseCase {
    lists: Arc<dyn ListRepository>,
}

impl RenameTodoUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`]/[`ListError::TodoNotFound`] for a missing or
    /// foreign ID, [`ListError::Domain`] for invalid input or a repository
    /// failure.
    #[tracing::instrument(name = "usecase.rename_todo", skip_all, fields(user_id = %user_id, list_id = %list_id, todo_id = %todo_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        todo_id: &TodoId,
        version: i64,
        title: &str,
    ) -> Result<MutationOutcome<Todo>, ListError> {
        let title = TodoTitle::new(title)?;
        match self
            .lists
            .rename_todo(user_id, list_id, todo_id, version, &title)
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

    fn open_todo(list_id: ListId, title: &str) -> Todo {
        Todo::from_persistence(TodoParts {
            id: TodoId::new(),
            list_id,
            title: TodoTitle::new(title).unwrap(),
            version: 1,
            position: Some(POSITION_GAP),
            completed_at: None,
            created_at: jiff::Timestamp::now(),
        })
        .unwrap()
    }

    fn repo(list_id: ListId, outcome: ListWrite<Todo>) -> MockListRepository {
        let mut repo = MockListRepository::new();
        repo.expect_rename_todo()
            .withf(move |_, l, _, version, title| {
                *l == list_id && *version == 1 && title.as_str() == "Renamed"
            })
            .times(1)
            .returning(move |_, _, _, _, _| Ok(outcome.clone()));
        repo
    }

    #[tokio::test]
    async fn applies_the_rename() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::Applied(open_todo(list_id, "Renamed")));
        let outcome = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, " Renamed ")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_repeated_title_succeeds() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::Repeated(open_todo(list_id, "Renamed")));
        let outcome = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, "Renamed")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Applied(_)));
    }

    #[tokio::test]
    async fn a_stale_rename_returns_the_current_todo() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::Stale(open_todo(list_id, "Other")));
        let outcome = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, "Renamed")
            .await
            .unwrap();
        assert!(matches!(outcome, MutationOutcome::Stale(_)));
    }

    #[tokio::test]
    async fn a_missing_todo_is_not_found() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::TodoNotFound);
        let err = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, "Renamed")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::TodoNotFound));
    }

    #[tokio::test]
    async fn a_missing_list_is_not_found() {
        let list_id = ListId::new();
        let repo = repo(list_id, ListWrite::ListNotFound);
        let err = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &list_id, &TodoId::new(), 1, "Renamed")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
    }

    #[tokio::test]
    async fn invalid_input_never_reaches_the_repository() {
        let repo = MockListRepository::new();
        let err = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), &TodoId::new(), 1, "a\nb")
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::Domain(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_rename_todo()
            .returning(|_, _, _, _, _| Err(DomainError::Persistence("db down".into())));
        let err = RenameTodoUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new(), &TodoId::new(), 1, "Renamed")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
