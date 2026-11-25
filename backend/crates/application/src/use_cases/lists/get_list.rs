//! Open one list with all its todos.

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListWithTodos};
use crate::use_cases::lists::ListError;
use domain::value_objects::{ListId, UserId};

/// Reads one list of the caller's.
pub struct GetListUseCase {
    lists: Arc<dyn ListRepository>,
}

impl GetListUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// [`ListError::ListNotFound`] for a missing, malformed or foreign ID, and
    /// [`ListError::Domain`] for a repository failure.
    #[tracing::instrument(name = "usecase.get_list", skip_all, fields(user_id = %user_id, list_id = %list_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<ListWithTodos, ListError> {
        self.lists
            .find_list(user_id, list_id)
            .await?
            .ok_or(ListError::ListNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::list_repository::MockListRepository;
    use domain::errors::DomainError;

    #[tokio::test]
    async fn returns_the_list_when_it_exists() {
        let mut repo = MockListRepository::new();
        repo.expect_find_list().returning(|_, id| {
            Ok(Some(ListWithTodos {
                list: domain::entities::List::from_persistence(domain::entities::ListParts {
                    id: *id,
                    user_id: UserId::new(),
                    name: domain::value_objects::ListName::new("Work").unwrap(),
                    version: 1,
                    created_at: jiff::Timestamp::now(),
                }),
                open_count: 0,
                todo_count: 0,
                todos: Vec::new(),
            }))
        });

        let list = GetListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new())
            .await
            .unwrap();
        assert_eq!(list.list.name().as_str(), "Work");
    }

    #[tokio::test]
    async fn a_missing_list_is_not_found() {
        let mut repo = MockListRepository::new();
        repo.expect_find_list().returning(|_, _| Ok(None));

        let err = GetListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ListError::ListNotFound));
    }

    #[tokio::test]
    async fn a_repository_failure_is_domain() {
        let mut repo = MockListRepository::new();
        repo.expect_find_list()
            .returning(|_, _| Err(DomainError::Persistence("db down".into())));

        let err = GetListUseCase::new(Arc::new(repo))
            .execute(&UserId::new(), &ListId::new())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ListError::Domain(DomainError::Persistence(_))
        ));
    }
}
