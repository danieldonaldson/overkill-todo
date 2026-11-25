//! Postgres list and todo repository (OT-013 design D3/D4).
//!
//! Every write opens a transaction and locks its list (or, for a list creation,
//! the account row) first. One lock gives ownership, the limits and one move at
//! a time, and the lock order is always users → lists → todos so a purge and a
//! create can't deadlock.

use std::sync::Arc;

use application::ports::outgoing::{
    LimitKind, ListRepository, ListSummary, ListWithTodos, ListWrite,
};
use async_trait::async_trait;
use domain::entities::{
    List, ListParts, MAX_LISTS_PER_USER, MAX_TODOS_PER_LIST, Todo, TodoParts, append_position,
};
use domain::errors::DomainError;
use domain::value_objects::{ListId, ListName, TodoId, TodoTitle, UserId};
use uuid::Uuid;

use crate::connection::Database;
use crate::error::persist_err;
use crate::models::{ListRow, ListSummaryRow, TodoRow};
use crate::time::ToDomain;

pub struct PostgresListRepository {
    db: Arc<Database>,
}

impl PostgresListRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn row_to_list(row: &ListRow) -> List {
        List::from_persistence(ListParts {
            id: ListId::from_uuid(row.id),
            user_id: UserId::from_uuid(row.user_id),
            name: ListName::new(&row.name).expect("stored list name is valid"),
            version: i64::from(row.version),
            created_at: row.created_at.to_domain(),
        })
    }

    fn summary_to_list(row: &ListSummaryRow) -> List {
        List::from_persistence(ListParts {
            id: ListId::from_uuid(row.id),
            user_id: UserId::from_uuid(row.user_id),
            name: ListName::new(&row.name).expect("stored list name is valid"),
            version: i64::from(row.version),
            created_at: row.created_at.to_domain(),
        })
    }

    fn row_to_todo(row: &TodoRow) -> Result<Todo, DomainError> {
        let title = TodoTitle::new(&row.title)
            .map_err(|_| DomainError::Persistence("stored todo title is invalid".into()))?;
        Todo::from_persistence(TodoParts {
            id: TodoId::from_uuid(row.id),
            list_id: ListId::from_uuid(row.list_id),
            title,
            version: i64::from(row.version),
            position: row.position,
            completed_at: row.completed_at.to_domain(),
            created_at: row.created_at.to_domain(),
        })
    }

    /// Locks one of `user_id`'s lists for the rest of the transaction.
    ///
    /// `Ok(false)` covers a missing list and another Member's list alike.
    async fn lock_list(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<bool, DomainError> {
        let row = sqlx::query!(
            r#"
            SELECT id FROM lists WHERE id = $1 AND user_id = $2 FOR UPDATE
            "#,
            list_id.as_uuid(),
            user_id.as_uuid(),
        )
        .fetch_optional(&mut **tx)
        .await
        .map_err(persist_err)?;
        Ok(row.is_some())
    }

    async fn list_summary_on(
        executor: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<ListSummary, DomainError> {
        let row = sqlx::query_as!(
            ListSummaryRow,
            r#"
            SELECT l.id, l.user_id, l.name, l.version, l.created_at,
                   COUNT(t.id) FILTER (WHERE t.completed_at IS NULL) AS "open_count!",
                   COUNT(t.id) AS "todo_count!"
            FROM lists l
            LEFT JOIN todos t ON t.list_id = l.id
            WHERE l.id = $1 AND l.user_id = $2
            GROUP BY l.id
            "#,
            list_id.as_uuid(),
            user_id.as_uuid(),
        )
        .fetch_one(&mut **executor)
        .await
        .map_err(persist_err)?;
        Ok(ListSummary {
            list: Self::summary_to_list(&row),
            open_count: row.open_count,
            todo_count: row.todo_count,
        })
    }

    async fn todo_row(
        executor: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        list_id: &ListId,
        todo_id: &TodoId,
    ) -> Result<Option<TodoRow>, DomainError> {
        sqlx::query_as!(
            TodoRow,
            r#"
            SELECT id, list_id, title, version, position, completed_at, created_at
            FROM todos
            WHERE id = $1 AND list_id = $2
            "#,
            todo_id.as_uuid(),
            list_id.as_uuid(),
        )
        .fetch_optional(&mut **executor)
        .await
        .map_err(persist_err)
    }

    async fn by_creation_key(
        executor: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        list_id: &ListId,
        key: Uuid,
    ) -> Result<Option<TodoRow>, DomainError> {
        sqlx::query_as!(
            TodoRow,
            r#"
            SELECT id, list_id, title, version, position, completed_at, created_at
            FROM todos
            WHERE list_id = $1 AND creation_key = $2
            "#,
            list_id.as_uuid(),
            key,
        )
        .fetch_optional(&mut **executor)
        .await
        .map_err(persist_err)
    }

    async fn last_open_position(
        executor: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        list_id: &ListId,
    ) -> Result<Option<i64>, DomainError> {
        sqlx::query_scalar!(
            r#"
            SELECT MAX(position) FROM todos
            WHERE list_id = $1 AND position IS NOT NULL
            "#,
            list_id.as_uuid(),
        )
        .fetch_one(&mut **executor)
        .await
        .map_err(persist_err)
    }
}

#[async_trait]
impl ListRepository for PostgresListRepository {
    #[tracing::instrument(name = "repo.list.list_summaries", skip_all, fields(db.system = "postgresql", user_id = %user_id))]
    async fn list_summaries(&self, user_id: &UserId) -> Result<Vec<ListSummary>, DomainError> {
        let rows = sqlx::query_as!(
            ListSummaryRow,
            r#"
            SELECT l.id, l.user_id, l.name, l.version, l.created_at,
                   COUNT(t.id) FILTER (WHERE t.completed_at IS NULL) AS "open_count!",
                   COUNT(t.id) AS "todo_count!"
            FROM lists l
            LEFT JOIN todos t ON t.list_id = l.id
            WHERE l.user_id = $1
            GROUP BY l.id
            ORDER BY l.created_at, l.id
            "#,
            user_id.as_uuid(),
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(rows
            .iter()
            .map(|row| ListSummary {
                list: Self::summary_to_list(row),
                open_count: row.open_count,
                todo_count: row.todo_count,
            })
            .collect())
    }

    #[tracing::instrument(name = "repo.list.find_list", skip_all, fields(db.system = "postgresql", user_id = %user_id, list_id = %list_id))]
    async fn find_list(
        &self,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<Option<ListWithTodos>, DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;
        let list_row = sqlx::query_as!(
            ListRow,
            r#"
            SELECT id, user_id, name, version, created_at
            FROM lists
            WHERE id = $1 AND user_id = $2
            "#,
            list_id.as_uuid(),
            user_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(persist_err)?;

        let Some(list_row) = list_row else {
            tx.rollback().await.map_err(persist_err)?;
            return Ok(None);
        };

        let todo_rows = sqlx::query_as!(
            TodoRow,
            r#"
            SELECT id, list_id, title, version, position, completed_at, created_at
            FROM todos
            WHERE list_id = $1
            ORDER BY position ASC NULLS LAST, completed_at DESC NULLS LAST, id
            "#,
            list_id.as_uuid(),
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(persist_err)?;
        tx.commit().await.map_err(persist_err)?;

        let todos = todo_rows
            .into_iter()
            .map(|row| Self::row_to_todo(&row))
            .collect::<Result<Vec<_>, _>>()?;
        let open_count = todos.iter().filter(|todo| !todo.is_done()).count();
        Ok(Some(ListWithTodos {
            list: Self::row_to_list(&list_row),
            open_count: i64::try_from(open_count).unwrap_or(i64::MAX),
            todo_count: i64::try_from(todos.len()).unwrap_or(i64::MAX),
            todos,
        }))
    }

    #[tracing::instrument(name = "repo.list.create_list", skip_all, fields(db.system = "postgresql", user_id = %user_id))]
    async fn create_list(
        &self,
        user_id: &UserId,
        key: Uuid,
        name: &ListName,
    ) -> Result<ListWrite<ListSummary>, DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;

        // Lock order: users first. A deleted account's token can't create a list,
        // even before it expires (design D4).
        let account = sqlx::query!(
            r#"
            SELECT id FROM users WHERE id = $1 AND deleted_at IS NULL FOR UPDATE
            "#,
            user_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(persist_err)?;
        if account.is_none() {
            tx.rollback().await.map_err(persist_err)?;
            return Err(DomainError::Authorization(
                "account is not available".into(),
            ));
        }

        // A repeated key is checked before the limit, so a retry after the
        // hundredth list still succeeds.
        let existing = sqlx::query_as!(
            ListRow,
            r#"
            SELECT id, user_id, name, version, created_at
            FROM lists WHERE user_id = $1 AND creation_key = $2
            "#,
            user_id.as_uuid(),
            key,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(persist_err)?;
        if let Some(row) = existing {
            let summary =
                Self::list_summary_on(&mut tx, user_id, &ListId::from_uuid(row.id)).await?;
            tx.commit().await.map_err(persist_err)?;
            return Ok(ListWrite::Repeated(summary));
        }

        let count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM lists WHERE user_id = $1",
            user_id.as_uuid(),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persist_err)?;
        if usize::try_from(count.unwrap_or(0)).unwrap_or(usize::MAX) >= MAX_LISTS_PER_USER {
            tx.rollback().await.map_err(persist_err)?;
            return Ok(ListWrite::LimitReached(LimitKind::Lists));
        }

        let id = ListId::new();
        let row = sqlx::query_as!(
            ListRow,
            r#"
            INSERT INTO lists (id, user_id, creation_key, name)
            VALUES ($1, $2, $3, $4)
            RETURNING id, user_id, name, version, created_at
            "#,
            id.as_uuid(),
            user_id.as_uuid(),
            key,
            name.as_str(),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persist_err)?;
        tx.commit().await.map_err(persist_err)?;

        Ok(ListWrite::Applied(ListSummary {
            list: Self::row_to_list(&row),
            open_count: 0,
            todo_count: 0,
        }))
    }

    #[tracing::instrument(name = "repo.list.add_todo", skip_all, fields(db.system = "postgresql", user_id = %user_id, list_id = %list_id))]
    async fn add_todo(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        key: Uuid,
        title: &TodoTitle,
    ) -> Result<ListWrite<Todo>, DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;
        if !Self::lock_list(&mut tx, user_id, list_id).await? {
            tx.rollback().await.map_err(persist_err)?;
            return Ok(ListWrite::ListNotFound);
        }

        if let Some(row) = Self::by_creation_key(&mut tx, list_id, key).await? {
            let todo = Self::row_to_todo(&row)?;
            tx.commit().await.map_err(persist_err)?;
            return Ok(ListWrite::Repeated(todo));
        }

        let count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM todos WHERE list_id = $1",
            list_id.as_uuid()
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persist_err)?;
        if usize::try_from(count.unwrap_or(0)).unwrap_or(usize::MAX) >= MAX_TODOS_PER_LIST {
            tx.rollback().await.map_err(persist_err)?;
            return Ok(ListWrite::LimitReached(LimitKind::Todos));
        }

        let last = Self::last_open_position(&mut tx, list_id).await?;
        let id = TodoId::new();
        let row = sqlx::query_as!(
            TodoRow,
            r#"
            INSERT INTO todos (id, list_id, creation_key, title, position)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, list_id, title, version, position, completed_at, created_at
            "#,
            id.as_uuid(),
            list_id.as_uuid(),
            key,
            title.as_str(),
            append_position(last),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persist_err)?;
        tx.commit().await.map_err(persist_err)?;
        Ok(ListWrite::Applied(Self::row_to_todo(&row)?))
    }

    #[tracing::instrument(name = "repo.list.mark_todo", skip_all, fields(db.system = "postgresql", user_id = %user_id, list_id = %list_id, todo_id = %todo_id))]
    async fn mark_todo(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        todo_id: &TodoId,
        version: i64,
        done: bool,
    ) -> Result<ListWrite<Todo>, DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;
        if !Self::lock_list(&mut tx, user_id, list_id).await? {
            tx.rollback().await.map_err(persist_err)?;
            return Ok(ListWrite::ListNotFound);
        }

        let Some(row) = Self::todo_row(&mut tx, list_id, todo_id).await? else {
            tx.rollback().await.map_err(persist_err)?;
            return Ok(ListWrite::TodoNotFound);
        };
        let current = Self::row_to_todo(&row)?;

        // Asking for the state already stored succeeds without changing the
        // version, whatever version the client claimed (design D1).
        if row.completed_at.is_some() == done {
            tx.commit().await.map_err(persist_err)?;
            return Ok(ListWrite::Repeated(current));
        }
        if i64::from(row.version) != version {
            tx.commit().await.map_err(persist_err)?;
            return Ok(ListWrite::Stale(current));
        }

        let updated = if done {
            sqlx::query_as!(
                TodoRow,
                r#"
                UPDATE todos
                SET position = NULL,
                    completed_at = clock_timestamp(),
                    version = version + 1
                WHERE id = $1 AND list_id = $2
                RETURNING id, list_id, title, version, position, completed_at, created_at
                "#,
                todo_id.as_uuid(),
                list_id.as_uuid(),
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(persist_err)?
        } else {
            let last = Self::last_open_position(&mut tx, list_id).await?;
            sqlx::query_as!(
                TodoRow,
                r#"
                UPDATE todos
                SET position = $3,
                    completed_at = NULL,
                    version = version + 1
                WHERE id = $1 AND list_id = $2
                RETURNING id, list_id, title, version, position, completed_at, created_at
                "#,
                todo_id.as_uuid(),
                list_id.as_uuid(),
                append_position(last),
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(persist_err)?
        };
        tx.commit().await.map_err(persist_err)?;
        Ok(ListWrite::Applied(Self::row_to_todo(&updated)?))
    }

    #[tracing::instrument(name = "repo.list.list_todos", skip_all, fields(db.system = "postgresql", user_id = %user_id, list_id = %list_id))]
    async fn list_todos(
        &self,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<Vec<Todo>, DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;
        let list = sqlx::query!(
            "SELECT id FROM lists WHERE id = $1 AND user_id = $2",
            list_id.as_uuid(),
            user_id.as_uuid(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(persist_err)?;
        if list.is_none() {
            tx.rollback().await.map_err(persist_err)?;
            return Err(DomainError::NotFound("list not found".into()));
        }
        let rows = sqlx::query_as!(
            TodoRow,
            r#"
            SELECT id, list_id, title, version, position, completed_at, created_at
            FROM todos
            WHERE list_id = $1
            ORDER BY position ASC NULLS LAST, completed_at DESC NULLS LAST, id
            "#,
            list_id.as_uuid(),
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(persist_err)?;
        tx.commit().await.map_err(persist_err)?;
        rows.into_iter()
            .map(|row| Self::row_to_todo(&row))
            .collect()
    }
}
