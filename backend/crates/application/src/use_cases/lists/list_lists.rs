//! List the caller's lists. This slice adds no filtering or paging: a Member
//! has at most 100 lists.

use std::sync::Arc;

use crate::ports::outgoing::{ListRepository, ListSummary};
use domain::errors::DomainError;
use domain::value_objects::UserId;

/// Lists a Member's lists, oldest first.
pub struct ListListsUseCase {
    lists: Arc<dyn ListRepository>,
}

impl ListListsUseCase {
    #[must_use]
    pub fn new(lists: Arc<dyn ListRepository>) -> Self {
        Self { lists }
    }

    /// # Errors
    /// Propagates a repository failure.
    #[tracing::instrument(name = "usecase.list_lists", skip_all, fields(user_id = %user_id))]
    pub async fn execute(&self, user_id: &UserId) -> Result<Vec<ListSummary>, DomainError> {
        self.lists.list_summaries(user_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::list_repository::MockListRepository;
    use domain::entities::{List, ListParts};
    use domain::value_objects::ListName;

    fn summary(user_id: UserId) -> ListSummary {
        ListSummary {
            list: List::from_persistence(ListParts {
                id: domain::value_objects::ListId::new(),
                user_id,
                name: ListName::new("Work").unwrap(),
                version: 1,
                created_at: jiff::Timestamp::now(),
            }),
            open_count: 2,
            todo_count: 3,
        }
    }

    #[tokio::test]
    async fn returns_the_repositorys_lists() {
        let user_id = UserId::new();
        let mut repo = MockListRepository::new();
        repo.expect_list_summaries()
            .withf(move |id| *id == user_id)
            .returning(move |_| Ok(vec![summary(user_id)]));

        let result = ListListsUseCase::new(Arc::new(repo))
            .execute(&user_id)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn propagates_a_repository_failure() {
        let mut repo = MockListRepository::new();
        repo.expect_list_summaries()
            .returning(|_| Err(DomainError::Persistence("db down".into())));

        let err = ListListsUseCase::new(Arc::new(repo))
            .execute(&UserId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Persistence(_)));
    }
}
