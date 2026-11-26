//! Move an open todo to another place among its list's open todos.

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, TodoMove};
use crate::use_cases::lists::ListError;
use domain::entities::Todo;
use domain::value_objects::{ListId, TodoId, UserId};

/// The outcome of a move: the resulting order, or the todo as it is now done.
#[derive(Debug)]
pub enum MoveOutcome {
    /// The list's open todos in order.
    Moved(Vec<TodoId>),
    Stale(Todo),
}

/// Moves one of the caller's open todos.
pub struct MoveTodoUseCase {
    lists: Arc<dyn ListRepository>,
}

impl MoveTodoUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`]/[`ListError::TodoNotFound`] for a missing or
    /// foreign ID, [`ListError::Domain`] for a repository failure. A neighbour
    /// that is missing or foreign is ignored, never an error.
    #[tracing::instrument(name = "usecase.move_todo", skip_all, fields(user_id = %user_id, list_id = %list_id, todo_id = %todo_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        todo_id: &TodoId,
        above: &[TodoId],
        below: &[TodoId],
    ) -> Result<MoveOutcome, ListError> {
        match self
            .lists
            .move_todo(user_id, list_id, todo_id, above, below)
            .await?
        {
            TodoMove::Moved(order) => Ok(MoveOutcome::Moved(order)),
            TodoMove::Done(todo) => Ok(MoveOutcome::Stale(todo)),
            TodoMove::ListNotFound => Err(ListError::ListNotFound),
            TodoMove::TodoNotFound => Err(ListError::TodoNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::list_repository::MockListRepository;
    use domain::entities::TodoParts;
    use domain::errors::DomainError;
    use domain::value_objects::TodoTitle;

    async fn run(outcome: Result<TodoMove, DomainError>) -> Result<MoveOutcome, ListError> {
        let (user, list, todo) = (UserId::new(), ListId::new(), TodoId::new());
        let (above, below) = ([TodoId::new()], [TodoId::new(), TodoId::new()]);
        let mut repo = MockListRepository::new();
        repo.expect_move_todo()
            .withf(move |u, l, t, a, b| {
                *u == user && *l == list && *t == todo && a == above && b == below
            })
            .times(1)
            .return_once(move |_, _, _, _, _| outcome);
        MoveTodoUseCase::new(Arc::new(repo))
            .execute(&user, &list, &todo, &above, &below)
            .await
    }

    #[tokio::test]
    async fn a_move_returns_the_order() {
        let order = vec![TodoId::new(), TodoId::new()];
        let outcome = run(Ok(TodoMove::Moved(order.clone()))).await.unwrap();
        assert!(matches!(outcome, MoveOutcome::Moved(ids) if ids == order));
    }

    #[tokio::test]
    async fn moving_a_done_todo_is_stale() {
        let done = Todo::from_persistence(TodoParts {
            id: TodoId::new(),
            list_id: ListId::new(),
            title: TodoTitle::new("Buy milk").unwrap(),
            version: 2,
            position: None,
            completed_at: Some(jiff::Timestamp::now()),
            created_at: jiff::Timestamp::now(),
        })
        .unwrap();
        let outcome = run(Ok(TodoMove::Done(done.clone()))).await.unwrap();
        assert!(matches!(outcome, MoveOutcome::Stale(todo) if todo == done));
    }

    #[tokio::test]
    async fn missing_ids_are_not_found() {
        let err = run(Ok(TodoMove::ListNotFound)).await.unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
        let err = run(Ok(TodoMove::TodoNotFound)).await.unwrap_err();
        assert!(matches!(err, ListError::TodoNotFound));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let err = run(Err(DomainError::Persistence("db down".into())))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
