//! List and todo schema invariants (OT-013 tasks 1.2/1.5).
//!
//! The deliberately invalid writes use runtime `sqlx::query` (not the
//! compile-checked macros) so they can target constraints directly.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use application::ports::outgoing::{
    AccountLifecycleRepository, LimitKind, ListRepository, ListWrite, TodoMove, UserRepository,
};
use domain::entities::AuditEvent;
use domain::errors::DomainError;
use domain::value_objects::{Email, ListId, ListName, TodoId, TodoTitle, UserId};
use jiff::Timestamp;
use persistence_postgres::time::ToDb;
use persistence_postgres::{Database, PostgresListRepository, PostgresUserRepository};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::task::JoinHandle;
use uuid::Uuid;

fn repo(pool: &PgPool) -> PostgresListRepository {
    PostgresListRepository::new(Arc::new(Database::from_pool(pool.clone())))
}

fn name(text: &str) -> ListName {
    ListName::new(text).unwrap()
}

fn title(text: &str) -> TodoTitle {
    TodoTitle::new(text).unwrap()
}

async fn user(pool: &PgPool, email: &str) -> (Uuid, UserId) {
    let id = sqlx::query_scalar!(
        "INSERT INTO users (id, email, role, email_verified)
         VALUES (gen_random_uuid(), $1, 'user', true) RETURNING id",
        email,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (id, UserId::from_uuid(id))
}

async fn insert_list(pool: &PgPool, user_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO lists (id, user_id, creation_key, name)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), $2) RETURNING id",
        user_id,
        name,
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_todo(
    pool: &PgPool,
    list_id: Uuid,
    title: &str,
    position: Option<i64>,
    completed_at: Option<Timestamp>,
) -> Uuid {
    sqlx::query_scalar!(
        "INSERT INTO todos (id, list_id, creation_key, title, position, completed_at)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), $2, $3, $4) RETURNING id",
        list_id,
        title,
        position,
        completed_at.to_db(),
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count_lists(pool: &PgPool, user_id: Uuid) -> i64 {
    sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE user_id = $1", user_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap_or(0)
}

async fn count_todos(pool: &PgPool, list_id: Uuid) -> i64 {
    sqlx::query_scalar!("SELECT COUNT(*) FROM todos WHERE list_id = $1", list_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap_or(0)
}

async fn seed_list_todos(pool: &PgPool, list_id: Uuid, count: i32) {
    sqlx::query!(
        "INSERT INTO todos (id, list_id, creation_key, title, position)
         SELECT gen_random_uuid(), $1, gen_random_uuid(), 't' || n, n::bigint * 1048576
         FROM generate_series(1, $2) AS n",
        list_id,
        count,
    )
    .execute(pool)
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// Schema constraints (task 1.2)
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_todo_cannot_be_both_open_and_done_or_neither(pool: PgPool) {
    let (user_id, _) = user(&pool, "constraint@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;

    for (position, completed_at) in [
        (Some(1_i64), Some(Timestamp::now())),
        (Some(1), None),
        (None, Some(Timestamp::now())),
        (None, None),
    ] {
        let valid = position.is_some() != completed_at.is_some();
        let result = sqlx::query(
            "INSERT INTO todos (id, list_id, creation_key, title, position, completed_at)
             VALUES (gen_random_uuid(), $1, gen_random_uuid(), 't', $2, $3)",
        )
        .bind(list)
        .bind(position)
        .bind(completed_at.to_db())
        .execute(&pool)
        .await;
        assert_eq!(
            result.is_err(),
            !valid,
            "position={position:?} completed_at={completed_at:?}"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn positions_are_unique_per_list_only(pool: PgPool) {
    let (user_id, _) = user(&pool, "positions@example.com").await;
    let first = insert_list(&pool, user_id, "First").await;
    let second = insert_list(&pool, user_id, "Second").await;

    insert_todo(&pool, first, "a", Some(1), None).await;
    let refused = sqlx::query(
        "INSERT INTO todos (id, list_id, creation_key, title, position)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'c', 1)",
    )
    .bind(first)
    .execute(&pool)
    .await;
    assert!(refused.is_err());
    insert_todo(&pool, second, "c", Some(1), None).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn one_statement_can_swap_a_lists_positions(pool: PgPool) {
    let (user_id, _) = user(&pool, "renumber@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let a = insert_todo(&pool, list, "a", Some(100), None).await;
    let b = insert_todo(&pool, list, "b", Some(200), None).await;

    sqlx::query(
        "UPDATE todos SET position = CASE id WHEN $1 THEN 200 WHEN $2 THEN 100 END
         WHERE id IN ($1, $2)",
    )
    .bind(a)
    .bind(b)
    .execute(&pool)
    .await
    .expect("the deferrable unique constraint allows one statement to swap positions");

    let positions: Vec<(Uuid, i64)> =
        sqlx::query_as("SELECT id, position FROM todos WHERE list_id = $1 ORDER BY position")
            .bind(list)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(positions, vec![(b, 100), (a, 200)]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_list_deletes_its_todos(pool: PgPool) {
    let (user_id, _) = user(&pool, "cascade@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    insert_todo(&pool, list, "a", Some(1), None).await;
    insert_todo(&pool, list, "b", None, Some(Timestamp::now())).await;

    sqlx::query("DELETE FROM lists WHERE id = $1")
        .bind(list)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count_todos(&pool, list).await, 0);
}

// ---------------------------------------------------------------------------
// Repository (task 1.5)
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn lists_are_oldest_first_with_open_and_total_counts(pool: PgPool) {
    let (_user_id, user) = user(&pool, "counts@example.com").await;
    let repo = repo(&pool);

    let first = repo
        .create_list(&user, Uuid::now_v7(), &name("Work"))
        .await
        .unwrap()
        .into_applied()
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = repo
        .create_list(&user, Uuid::now_v7(), &name("Home"))
        .await
        .unwrap()
        .into_applied()
        .unwrap();

    insert_todo(&pool, *first.list.id().as_uuid(), "a", Some(1), None).await;
    insert_todo(&pool, *first.list.id().as_uuid(), "b", Some(2), None).await;
    insert_todo(
        &pool,
        *first.list.id().as_uuid(),
        "c",
        None,
        Some(Timestamp::now()),
    )
    .await;

    let lists = repo.list_summaries(&user).await.unwrap();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[0].list.name().as_str(), "Work");
    assert_eq!(lists[1].list.name().as_str(), "Home");
    assert_eq!(lists[0].open_count, 2);
    assert_eq!(lists[0].todo_count, 3);
    assert_eq!(lists[1].open_count, 0);
    assert_eq!(lists[1].todo_count, 0);
    assert_eq!(second.list.name().as_str(), "Home");
}

#[sqlx::test(migrations = "../../migrations")]
async fn done_todos_come_back_most_recently_completed_first(pool: PgPool) {
    let (user_id, user) = user(&pool, "done-order@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    insert_todo(&pool, list, "open", Some(1), None).await;
    let older = Timestamp::now() - jiff::SignedDuration::from_secs(60);
    insert_todo(&pool, list, "older", None, Some(older)).await;
    insert_todo(&pool, list, "newer", None, Some(Timestamp::now())).await;

    let detail = repo(&pool)
        .find_list(&user, &list_id)
        .await
        .unwrap()
        .unwrap();
    let titles: Vec<_> = detail.todos.iter().map(|t| t.title().as_str()).collect();
    assert_eq!(titles, vec!["open", "newer", "older"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_repeated_creation_key_returns_the_existing_list_also_at_the_limit(pool: PgPool) {
    let (user_id, user) = user(&pool, "list-limit@example.com").await;
    let repo = repo(&pool);
    for i in 0..99 {
        repo.create_list(&user, Uuid::now_v7(), &name(&format!("List {i}")))
            .await
            .unwrap();
    }
    let key = Uuid::now_v7();
    assert!(matches!(
        repo.create_list(&user, key, &name("Hundredth"))
            .await
            .unwrap(),
        ListWrite::Applied(_)
    ));
    assert!(matches!(
        repo.create_list(&user, key, &name("Retry")).await.unwrap(),
        ListWrite::Repeated(_)
    ));
    assert!(matches!(
        repo.create_list(&user, Uuid::now_v7(), &name("Over"))
            .await
            .unwrap(),
        ListWrite::LimitReached(LimitKind::Lists)
    ));
    assert_eq!(count_lists(&pool, user_id).await, 100);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_repeated_todo_key_returns_the_existing_todo_also_at_the_limit(pool: PgPool) {
    let (user_id, user) = user(&pool, "todo-limit@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    seed_list_todos(&pool, list, 999).await;
    let repo = repo(&pool);

    let key = Uuid::now_v7();
    let applied = repo
        .add_todo(&user, &list_id, key, &title("last"))
        .await
        .unwrap();
    let created = match applied {
        ListWrite::Applied(todo) => todo,
        other => panic!("expected Applied, got {other:?}"),
    };
    let repeated = repo
        .add_todo(&user, &list_id, key, &title("retry"))
        .await
        .unwrap();
    match repeated {
        ListWrite::Repeated(todo) => assert_eq!(todo.id(), created.id()),
        other => panic!("expected Repeated, got {other:?}"),
    }
    assert!(matches!(
        repo.add_todo(&user, &list_id, Uuid::now_v7(), &title("over"))
            .await
            .unwrap(),
        ListWrite::LimitReached(LimitKind::Todos)
    ));
    assert_eq!(count_todos(&pool, list).await, 1000);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_mark_is_refused_with_the_current_row(pool: PgPool) {
    let (user_id, user) = user(&pool, "stale@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let todo = insert_todo(&pool, list, "Buy milk", Some(1), None).await;
    let repo = repo(&pool);

    let outcome = repo
        .mark_todo(
            &user,
            &ListId::from_uuid(list),
            &TodoId::from_uuid(todo),
            9,
            true,
        )
        .await
        .unwrap();
    match outcome {
        ListWrite::Stale(current) => {
            assert_eq!(current.version(), 1);
            assert!(!current.is_done());
            assert_eq!(current.title().as_str(), "Buy milk");
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_mark_already_in_place_succeeds_without_changing_the_version(pool: PgPool) {
    let (user_id, user) = user(&pool, "same-state@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let todo = insert_todo(&pool, list, "Buy milk", None, Some(Timestamp::now())).await;
    let repo = repo(&pool);

    let outcome = repo
        .mark_todo(
            &user,
            &ListId::from_uuid(list),
            &TodoId::from_uuid(todo),
            1,
            true,
        )
        .await
        .unwrap();
    match outcome {
        ListWrite::Repeated(current) => {
            assert!(current.is_done());
            assert_eq!(current.version(), 1);
        }
        other => panic!("expected Repeated, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_done_and_not_done_moves_the_todo_as_specified(pool: PgPool) {
    let (user_id, user) = user(&pool, "mark@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    insert_todo(&pool, list, "first", Some(1), None).await;
    let todo = insert_todo(&pool, list, "second", Some(2), None).await;
    let todo_id = TodoId::from_uuid(todo);
    let repo = repo(&pool);

    let done = repo
        .mark_todo(&user, &list_id, &todo_id, 1, true)
        .await
        .unwrap()
        .into_applied()
        .unwrap();
    assert!(done.is_done());
    assert_eq!(done.version(), 2);
    let detail = repo.find_list(&user, &list_id).await.unwrap().unwrap();
    assert_eq!(detail.todos.last().unwrap().id(), &todo_id);

    let open_again = repo
        .mark_todo(&user, &list_id, &todo_id, 2, false)
        .await
        .unwrap()
        .into_applied()
        .unwrap();
    assert!(!open_again.is_done());
    assert_eq!(open_again.version(), 3);
    let detail = repo.find_list(&user, &list_id).await.unwrap().unwrap();
    assert_eq!(detail.todos.last().unwrap().id(), &todo_id);
    assert_eq!(detail.open_count, 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_members_list_and_key_are_invisible(pool: PgPool) {
    let (_, alice) = user(&pool, "alice@example.com").await;
    let (bob_id, bob) = user(&pool, "bob@example.com").await;
    let repo = repo(&pool);
    let key = Uuid::now_v7();
    let alice_list = repo
        .create_list(&alice, key, &name("Alice's"))
        .await
        .unwrap()
        .into_applied()
        .unwrap();
    let alice_list_id = *alice_list.list.id();
    let alice_todo_key = Uuid::now_v7();
    let alice_todo = repo
        .add_todo(&alice, &alice_list_id, alice_todo_key, &title("secret"))
        .await
        .unwrap()
        .into_applied()
        .unwrap();

    assert!(
        repo.find_list(&bob, &alice_list_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        repo.add_todo(&bob, &alice_list_id, Uuid::now_v7(), &title("x"))
            .await
            .unwrap(),
        ListWrite::ListNotFound
    ));
    assert!(matches!(
        repo.mark_todo(&bob, &alice_list_id, alice_todo.id(), 1, true)
            .await
            .unwrap(),
        ListWrite::ListNotFound
    ));

    // Bob may use the same key: it is scoped to his own lists.
    let bob_list = repo
        .create_list(&bob, key, &name("Bob's"))
        .await
        .unwrap()
        .into_applied()
        .unwrap();
    assert_ne!(bob_list.list.id(), &alice_list_id);
    assert_eq!(count_lists(&pool, bob_id).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_account_cannot_create_a_list(pool: PgPool) {
    let (user_id, user) = user(&pool, "deleted@example.com").await;
    sqlx::query("UPDATE users SET deleted_at = NOW() WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let err = repo(&pool)
        .create_list(&user, Uuid::now_v7(), &name("Work"))
        .await
        .unwrap_err();
    assert!(matches!(err, domain::errors::DomainError::Authorization(_)));
    assert_eq!(count_lists(&pool, user_id).await, 0);
}

// ---------------------------------------------------------------------------
// Rename and delete (task 3.1)
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn a_stale_list_rename_is_refused_with_the_current_list(pool: PgPool) {
    let (user_id, user) = user(&pool, "rename-list@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let repo = repo(&pool);

    let outcome = repo
        .rename_list(&user, &list_id, 9, &name("Job"))
        .await
        .unwrap();
    match outcome {
        ListWrite::Stale(current) => {
            assert_eq!(current.list.name().as_str(), "Work");
            assert_eq!(current.list.version(), 1);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_list_rename_already_in_place_succeeds_without_changing_the_version(pool: PgPool) {
    let (user_id, user) = user(&pool, "same-name@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let repo = repo(&pool);

    let outcome = repo
        .rename_list(&user, &list_id, 9, &name("Work"))
        .await
        .unwrap();
    match outcome {
        ListWrite::Repeated(current) => {
            assert_eq!(current.list.name().as_str(), "Work");
            assert_eq!(current.list.version(), 1);
        }
        other => panic!("expected Repeated, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn renaming_different_todos_does_not_conflict(pool: PgPool) {
    let (user_id, user) = user(&pool, "rename-todos@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let first = TodoId::from_uuid(insert_todo(&pool, list, "one", Some(1), None).await);
    let second = TodoId::from_uuid(insert_todo(&pool, list, "two", Some(2), None).await);
    let repo = repo(&pool);

    assert!(matches!(
        repo.rename_todo(&user, &list_id, &first, 1, &title("first"))
            .await
            .unwrap(),
        ListWrite::Applied(_)
    ));
    // The second todo is still at version 1, though the list has other writes.
    assert!(matches!(
        repo.rename_todo(&user, &list_id, &second, 1, &title("second"))
            .await
            .unwrap(),
        ListWrite::Applied(_)
    ));
    // The first moved to version 2: a rename at 1 is stale, the second stays.
    match repo
        .rename_todo(&user, &list_id, &first, 1, &title("again"))
        .await
        .unwrap()
    {
        ListWrite::Stale(current) => {
            assert_eq!(current.title().as_str(), "first");
            assert_eq!(current.version(), 2);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
    let detail = repo.find_list(&user, &list_id).await.unwrap().unwrap();
    assert_eq!(detail.todos[1].title().as_str(), "second");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_todo_delete_is_refused_at_a_stale_version(pool: PgPool) {
    let (user_id, user) = user(&pool, "delete-todo@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let todo = TodoId::from_uuid(insert_todo(&pool, list, "Buy milk", Some(1), None).await);
    let repo = repo(&pool);

    match repo.delete_todo(&user, &list_id, &todo, 9).await.unwrap() {
        ListWrite::Stale(current) => {
            assert_eq!(current.title().as_str(), "Buy milk");
            assert_eq!(current.version(), 1);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(count_todos(&pool, list).await, 1);

    assert!(matches!(
        repo.delete_todo(&user, &list_id, &todo, 1).await.unwrap(),
        ListWrite::Applied(_)
    ));
    assert_eq!(count_todos(&pool, list).await, 0);
    assert!(matches!(
        repo.delete_todo(&user, &list_id, &todo, 1).await.unwrap(),
        ListWrite::TodoNotFound
    ));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_list_requires_the_name_and_count_it_was_confirmed_with(pool: PgPool) {
    let (user_id, user) = user(&pool, "delete-list@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    for (i, title) in ["one", "two", "three"].iter().enumerate() {
        insert_todo(
            &pool,
            list,
            title,
            Some(i64::try_from(i).unwrap() + 1),
            None,
        )
        .await;
    }
    let repo = repo(&pool);

    // A count other than the stored one deletes nothing, even at the version.
    match repo.delete_list(&user, &list_id, 1, 2).await.unwrap() {
        ListWrite::Stale(current) => {
            assert_eq!(current.todo_count, 3);
            assert_eq!(current.open_count, 3);
            assert_eq!(current.list.version(), 1);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(count_todos(&pool, list).await, 3);

    // A rename now shows a newer version, and the refusal names it.
    assert!(matches!(
        repo.rename_list(&user, &list_id, 1, &name("Job"))
            .await
            .unwrap(),
        ListWrite::Applied(_)
    ));
    match repo.delete_list(&user, &list_id, 1, 3).await.unwrap() {
        ListWrite::Stale(current) => {
            assert_eq!(current.list.name().as_str(), "Job");
            assert_eq!(current.list.version(), 2);
            assert_eq!(current.todo_count, 3);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(count_lists(&pool, user_id).await, 1);

    assert!(matches!(
        repo.delete_list(&user, &list_id, 2, 3).await.unwrap(),
        ListWrite::Applied(_)
    ));
    assert_eq!(count_lists(&pool, user_id).await, 0);
    assert_eq!(count_todos(&pool, list).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_makes_room_again_at_both_limits(pool: PgPool) {
    let (_user_id, user) = user(&pool, "room-again@example.com").await;
    let repo = repo(&pool);
    for i in 0..100 {
        repo.create_list(&user, Uuid::now_v7(), &name(&format!("List {i}")))
            .await
            .unwrap();
    }
    let lists = repo.list_summaries(&user).await.unwrap();
    let first = lists[0].list.id().to_owned();
    assert!(matches!(
        repo.delete_list(&user, &first, 1, 0).await.unwrap(),
        ListWrite::Applied(_)
    ));
    assert!(matches!(
        repo.create_list(&user, Uuid::now_v7(), &name("Replacement"))
            .await
            .unwrap(),
        ListWrite::Applied(_)
    ));

    let list = *lists[1].list.id().as_uuid();
    let list_id = ListId::from_uuid(list);
    seed_list_todos(&pool, list, 1000).await;
    let todos = repo
        .find_list(&user, &list_id)
        .await
        .unwrap()
        .unwrap()
        .todos;
    let last = todos.last().unwrap().id().to_owned();
    assert!(matches!(
        repo.delete_todo(&user, &list_id, &last, 1).await.unwrap(),
        ListWrite::Applied(_)
    ));
    assert!(matches!(
        repo.add_todo(&user, &list_id, Uuid::now_v7(), &title("room"))
            .await
            .unwrap(),
        ListWrite::Applied(_)
    ));
}

// ---------------------------------------------------------------------------
// Moves (slice 4)
// ---------------------------------------------------------------------------

/// A list with open todos named `a`, `b`, … in order, at the given positions.
async fn list_at(pool: &PgPool, user_id: Uuid, positions: &[i64]) -> (ListId, Vec<TodoId>) {
    let list = insert_list(pool, user_id, "Work").await;
    let mut ids = Vec::new();
    for (index, position) in positions.iter().enumerate() {
        let title = char::from(b'a' + u8::try_from(index).unwrap()).to_string();
        let id = insert_todo(pool, list, &title, Some(*position), None).await;
        ids.push(TodoId::from_uuid(id));
    }
    (ListId::from_uuid(list), ids)
}

/// The list's open todos in stored order.
async fn open_order(pool: &PgPool, list: &ListId) -> Vec<TodoId> {
    sqlx::query_scalar!(
        "SELECT id FROM todos WHERE list_id = $1 AND position IS NOT NULL ORDER BY position",
        list.as_uuid(),
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(TodoId::from_uuid)
    .collect()
}

fn moved(outcome: Result<TodoMove, domain::errors::DomainError>) -> Vec<TodoId> {
    match outcome.unwrap() {
        TodoMove::Moved(order) => order,
        other => panic!("not moved: {other:?}"),
    }
}

/// Sorted, for comparing which todos there are rather than their order.
fn as_set(ids: &[TodoId]) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = ids.iter().map(|id| *id.as_uuid()).collect();
    ids.sort_unstable();
    ids
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_todo_moves_to_the_top_the_middle_and_the_bottom(pool: PgPool) {
    let (user_id, user) = user(&pool, "move@example.com").await;
    let (list, ids) = list_at(&pool, user_id, &[10, 20, 30, 40]).await;
    let [a, b, c, d] = ids[..] else { panic!() };
    let repo = repo(&pool);

    for (todo, above, below, expected) in [
        (c, vec![], vec![a, b], [c, a, b, d]),
        (d, vec![a, c], vec![b], [c, a, d, b]),
        (c, vec![b, d, a], vec![], [a, d, b, c]),
        // Already there.
        (c, vec![b], vec![], [a, d, b, c]),
        // Its own ID is ignored, both ways.
        (b, vec![b, a], vec![b, d], [a, b, d, c]),
        (c, vec![c], vec![c, d], [a, b, c, d]),
    ] {
        let order = moved(repo.move_todo(&user, &list, &todo, &above, &below).await);
        assert_eq!(order, expected);
        assert_eq!(open_order(&pool, &list).await, expected);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn moves_never_change_a_version(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-version@example.com").await;
    // Adjacent, so the move also renumbers.
    let (list, ids) = list_at(&pool, user_id, &[1, 2, 3]).await;
    let repo = repo(&pool);
    moved(
        repo.move_todo(&user, &list, &ids[2], &[ids[0]], &[ids[1]])
            .await,
    );
    let detail = repo.find_list(&user, &list).await.unwrap().unwrap();
    assert!(detail.todos.iter().all(|todo| todo.version() == 1));
    assert_eq!(detail.list.version(), 1);

    // So a rename from a tab that hasn't seen the move still applies.
    let renamed = repo
        .rename_todo(&user, &list, &ids[2], 1, &title("renamed"))
        .await
        .unwrap();
    assert!(matches!(renamed, ListWrite::Applied(_)));
    assert_eq!(
        open_order(&pool, &list).await,
        [ids[0], ids[2], ids[1]],
        "the rename kept the move"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_thousand_moves_between_the_same_two_todos_lose_nothing(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-many@example.com").await;
    let gap = 1 << 20;
    let (list, ids) = list_at(&pool, user_id, &[gap, 2 * gap, 3 * gap, 4 * gap]).await;
    let [top, x, y, bottom] = ids[..] else {
        panic!()
    };
    let repo = repo(&pool);

    // Each move goes directly below `top`, halving the same gap every time.
    let mut renumbered = 0;
    for n in 0..1000 {
        let (todo, other) = if n % 2 == 0 { (y, x) } else { (x, y) };
        let order = moved(repo.move_todo(&user, &list, &todo, &[top], &[other]).await);
        assert_eq!(order, [top, todo, other, bottom], "move {n}");
        assert_eq!(open_order(&pool, &list).await, order, "move {n}");
        let second = sqlx::query_scalar!(
            r#"SELECT position AS "position!" FROM todos WHERE id = $1"#,
            todo.as_uuid(),
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        renumbered += u32::from(second == 2 * gap);
    }
    assert!(renumbered > 10, "renumbered only {renumbered} times");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_move_renumbers_at_either_end_of_the_range(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-ends@example.com").await;
    let (list, ids) = list_at(&pool, user_id, &[i64::MIN, 0, i64::MAX]).await;
    let [a, b, c] = ids[..] else { panic!() };
    let repo = repo(&pool);
    let order = moved(repo.move_todo(&user, &list, &b, &[], &[a]).await);
    assert_eq!(order, [b, a, c]);
    assert_eq!(open_order(&pool, &list).await, order);

    let (list, ids) = list_at(&pool, user_id, &[0, 5, i64::MAX]).await;
    let [a, b, c] = ids[..] else { panic!() };
    let order = moved(repo.move_todo(&user, &list, &a, &[c], &[]).await);
    assert_eq!(order, [b, c, a]);
    assert_eq!(open_order(&pool, &list).await, order);
}

#[sqlx::test(migrations = "../../migrations")]
async fn neighbours_that_are_gone_done_or_foreign_are_ignored(pool: PgPool) {
    let (user_id, member) = user(&pool, "move-gone@example.com").await;
    let (other_id, _) = user(&pool, "move-foreign@example.com").await;
    let (list, ids) = list_at(&pool, user_id, &[10, 20, 30, 40, 50]).await;
    let [a, b, c, d, e] = ids[..] else { panic!() };
    let (_, foreign) = list_at(&pool, other_id, &[10]).await;
    let (_, own_other_list) = list_at(&pool, user_id, &[10]).await;
    let repo = repo(&pool);
    repo.delete_todo(&member, &list, &b, 1).await.unwrap();
    repo.mark_todo(&member, &list, &c, 1, true).await.unwrap();
    let gone = [b, c, foreign[0], own_other_list[0], TodoId::new()];

    // Below the first that remains above.
    let above = [&gone[..], &[a]].concat();
    let order = moved(repo.move_todo(&member, &list, &e, &above, &[d]).await);
    assert_eq!(order, [a, e, d]);
    // None remains above: above the first that remains below.
    let below = [&gone[..], &[a]].concat();
    let order = moved(repo.move_todo(&member, &list, &d, &gone, &below).await);
    assert_eq!(order, [d, a, e]);
    // None remains either way: the bottom.
    let order = moved(repo.move_todo(&member, &list, &d, &gone, &gone).await);
    assert_eq!(order, [a, e, d]);
    assert_eq!(open_order(&pool, &list).await, order);

    // Each kind of absent neighbour gives the same answer as an unknown one.
    for id in gone {
        let order = moved(repo.move_todo(&member, &list, &e, &[id], &[]).await);
        assert_eq!(order, [a, d, e]);
        moved(repo.move_todo(&member, &list, &e, &[a], &[d]).await);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_done_deleted_or_foreign_todo_does_not_move(pool: PgPool) {
    let (user_id, member) = user(&pool, "move-done@example.com").await;
    let (_, stranger) = user(&pool, "move-stranger@example.com").await;
    let (list, ids) = list_at(&pool, user_id, &[10, 20, 30]).await;
    let [a, b, c] = ids[..] else { panic!() };
    let repo = repo(&pool);
    repo.mark_todo(&member, &list, &a, 1, true).await.unwrap();
    repo.delete_todo(&member, &list, &b, 1).await.unwrap();

    match repo.move_todo(&member, &list, &a, &[c], &[]).await.unwrap() {
        TodoMove::Done(todo) => {
            assert_eq!(*todo.id(), a);
            assert!(todo.is_done());
            assert_eq!(todo.version(), 2);
        }
        other => panic!("{other:?}"),
    }
    for (todo, owner, list_id) in [(b, &member, list), (TodoId::new(), &member, list)] {
        assert!(matches!(
            repo.move_todo(owner, &list_id, &todo, &[c], &[])
                .await
                .unwrap(),
            TodoMove::TodoNotFound
        ));
    }
    for (owner, list_id) in [(&stranger, list), (&member, ListId::new())] {
        assert!(matches!(
            repo.move_todo(owner, &list_id, &c, &[], &[]).await.unwrap(),
            TodoMove::ListNotFound
        ));
    }
    assert_eq!(open_order(&pool, &list).await, [c]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_moves_adds_and_completions_keep_every_todo_once(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-race@example.com").await;
    // Adjacent positions, so most rounds renumber while others write.
    let (list, ids) = list_at(&pool, user_id, &[1, 2, 3, 4, 5, 6]).await;
    let [a, b, c, d, e, f] = ids[..] else {
        panic!()
    };
    let repo = repo(&pool);
    let added = title("added");
    let (top, under_b, under_a) = ([a], ([b], [c]), ([a], [b]));
    for round in 0_i32..20 {
        let done = round % 2 == 0;
        let (to_top, same_todo, between, add, mark) = tokio::join!(
            repo.move_todo(&user, &list, &e, &[], &top),
            repo.move_todo(&user, &list, &e, &under_b.0, &under_b.1),
            repo.move_todo(&user, &list, &f, &under_a.0, &under_a.1),
            repo.add_todo(&user, &list, Uuid::now_v7(), &added),
            repo.mark_todo(&user, &list, &d, 1 + i64::from(round), done),
        );
        for outcome in [to_top, same_todo, between] {
            moved(outcome);
        }
        assert!(matches!(add.unwrap(), ListWrite::Applied(_)));
        assert!(matches!(mark.unwrap(), ListWrite::Applied(_)));

        let detail = repo.find_list(&user, &list).await.unwrap().unwrap();
        let all: Vec<TodoId> = detail.todos.iter().map(|todo| *todo.id()).collect();
        let mut distinct = as_set(&all);
        distinct.dedup();
        assert_eq!(distinct.len(), 6 + usize::try_from(round).unwrap() + 1);
        assert!(ids.iter().all(|id| all.contains(id)));
        let open = open_order(&pool, &list).await;
        assert_eq!(open.contains(&d), !done);
        assert_eq!(open[..], all[..open.len()], "one order");
        // The same todo moved twice ended in one of the two places.
        let place = |id: TodoId| open.iter().position(|other| *other == id).unwrap();
        assert!(place(e) < place(a) || place(b) < place(e));
    }
}

/// Overlapping transactions: with the list lock held elsewhere, a move waits,
/// and then sees what was committed meanwhile.
#[sqlx::test(migrations = "../../migrations")]
async fn a_move_waits_for_the_list_lock(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-lock@example.com").await;
    let (list, ids) = list_at(&pool, user_id, &[10, 20, 30]).await;
    let [a, b, c] = ids[..] else { panic!() };
    let mut tx = pool.begin().await.unwrap();
    sqlx::query!(
        "SELECT id FROM lists WHERE id = $1 FOR UPDATE",
        list.as_uuid()
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let repo = Arc::new(repo(&pool));
    let move_repo = Arc::clone(&repo);
    let mut moving =
        tokio::spawn(async move { move_repo.move_todo(&user, &list, &c, &[a], &[b]).await });
    let early = tokio::time::timeout(std::time::Duration::from_millis(300), &mut moving).await;
    assert!(early.is_err(), "the move did not wait for the lock");

    sqlx::query!("DELETE FROM todos WHERE id = $1", a.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(moved(moving.await.unwrap()), [c, b]);
    assert_eq!(open_order(&pool, &list).await, [c, b]);
}

// ---------------------------------------------------------------------------
// Overlapping transactions (task 7.5)
// ---------------------------------------------------------------------------
//
// Model replay calls each repository method once, at its commit, so it can't
// notice a missing lock (design D13). These tests make two requests overlap
// on purpose. A transaction of the test's own holds the rows a request would
// lock (`hold`), and each request is started only once the one before it is
// waiting. With the lock in place, both requests wait for it before reading
// anything; without it, both read, then wait at their writes (a row they
// update, or the row a foreign key checks), so releasing the hold lets them
// write on what they read. Postgres grants a row's waiters in turn, so the
// first request started goes first.
//
// Each was seen to fail with its lock removed on a scratch edit (`FOR UPDATE`
// dropped from `lock_list`, or from the account lock in `create_list`).

/// A repository on a connection of its own, so requests run in separate sessions.
async fn own_repo(pool: &PgPool) -> Arc<PostgresListRepository> {
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    Arc::new(PostgresListRepository::new(Arc::new(Database::from_pool(
        single,
    ))))
}

/// Holds a list row and all its todos, as a request's `lock_list` would.
async fn hold_list(pool: &PgPool, list: Uuid) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM lists WHERE id = $1 FOR UPDATE")
        .bind(list)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM todos WHERE list_id = $1 FOR UPDATE")
        .bind(list)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

/// Holds an account row, as `create_list` and the purge would.
async fn hold_account(pool: &PgPool, user: Uuid) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

/// Starts `request` and returns once it waits for a lock (the `waiting`th
/// session in this database to do so) or has finished without waiting.
async fn start<T, F>(pool: &PgPool, waiting: i64, request: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let task = tokio::spawn(request);
    for _ in 0..1000 {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if count >= waiting || task.is_finished() {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the request neither waited for a lock nor finished within 10 s");
}

/// Releases the hold and returns both requests' results, in the order started.
async fn release<A, B>(
    hold: Transaction<'static, Postgres>,
    a: JoinHandle<A>,
    b: JoinHandle<B>,
) -> (A, B) {
    hold.rollback().await.unwrap();
    (a.await.unwrap(), b.await.unwrap())
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_todo_additions_at_the_limit_save_one(pool: PgPool) {
    let (user_id, user) = user(&pool, "todo-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    seed_list_todos(&pool, list, 999).await;
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_list(&pool, list).await;
    let a = start(&pool, 1, async move {
        first
            .add_todo(&user, &list_id, Uuid::now_v7(), &title("a"))
            .await
    })
    .await;
    let b = start(&pool, 2, async move {
        second
            .add_todo(&user, &list_id, Uuid::now_v7(), &title("b"))
            .await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    assert!(matches!(a.unwrap(), ListWrite::Applied(_)));
    assert!(matches!(
        b.unwrap(),
        ListWrite::LimitReached(LimitKind::Todos)
    ));
    assert_eq!(count_todos(&pool, list).await, 1000);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_list_creation_at_the_limit_saves_one(pool: PgPool) {
    let (user_id, user) = user(&pool, "list-race@example.com").await;
    for i in 0..99 {
        insert_list(&pool, user_id, &format!("List {i}")).await;
    }
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_account(&pool, user_id).await;
    let a = start(&pool, 1, async move {
        first.create_list(&user, Uuid::now_v7(), &name("A")).await
    })
    .await;
    let b = start(&pool, 2, async move {
        second.create_list(&user, Uuid::now_v7(), &name("B")).await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    assert!(matches!(a.unwrap(), ListWrite::Applied(_)));
    assert!(matches!(
        b.unwrap(),
        ListWrite::LimitReached(LimitKind::Lists)
    ));
    assert_eq!(count_lists(&pool, user_id).await, 100);
}

#[sqlx::test(migrations = "../../migrations")]
async fn one_todo_key_sent_twice_at_once_saves_one_todo(pool: PgPool) {
    let (user_id, user) = user(&pool, "todo-key-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let key = Uuid::now_v7();
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_list(&pool, list).await;
    let a = start(&pool, 1, async move {
        first.add_todo(&user, &list_id, key, &title("milk")).await
    })
    .await;
    let b = start(&pool, 2, async move {
        second.add_todo(&user, &list_id, key, &title("milk")).await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    let ListWrite::Applied(added) = a.unwrap() else {
        panic!("the first request should add the todo")
    };
    match b.unwrap() {
        ListWrite::Repeated(repeated) => assert_eq!(repeated.id(), added.id()),
        other => panic!("expected Repeated, got {other:?}"),
    }
    assert_eq!(count_todos(&pool, list).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn one_list_key_sent_twice_at_once_saves_one_list(pool: PgPool) {
    let (user_id, user) = user(&pool, "list-key-race@example.com").await;
    let key = Uuid::now_v7();
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_account(&pool, user_id).await;
    let a = start(&pool, 1, async move {
        first.create_list(&user, key, &name("Work")).await
    })
    .await;
    let b = start(&pool, 2, async move {
        second.create_list(&user, key, &name("Work")).await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    let ListWrite::Applied(created) = a.unwrap() else {
        panic!("the first request should create the list")
    };
    match b.unwrap() {
        ListWrite::Repeated(repeated) => assert_eq!(repeated.list.id(), created.list.id()),
        other => panic!("expected Repeated, got {other:?}"),
    }
    assert_eq!(count_lists(&pool, user_id).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_todo_renames_at_one_version_apply_one(pool: PgPool) {
    let (user_id, user) = user(&pool, "rename-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let todo = TodoId::from_uuid(insert_todo(&pool, list, "milk", Some(1), None).await);
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_list(&pool, list).await;
    let a = start(&pool, 1, async move {
        first
            .rename_todo(&user, &list_id, &todo, 1, &title("A"))
            .await
    })
    .await;
    let b = start(&pool, 2, async move {
        second
            .rename_todo(&user, &list_id, &todo, 1, &title("B"))
            .await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    assert!(matches!(a.unwrap(), ListWrite::Applied(_)));
    match b.unwrap() {
        ListWrite::Stale(current) => {
            assert_eq!(current.title().as_str(), "A");
            assert_eq!(current.version(), 2);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_marks_at_one_version_move_the_version_once(pool: PgPool) {
    let (user_id, user) = user(&pool, "mark-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let todo = TodoId::from_uuid(insert_todo(&pool, list, "milk", Some(1), None).await);
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_list(&pool, list).await;
    let a = start(&pool, 1, async move {
        first.mark_todo(&user, &list_id, &todo, 1, true).await
    })
    .await;
    let b = start(&pool, 2, async move {
        second.mark_todo(&user, &list_id, &todo, 1, true).await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    assert!(matches!(a.unwrap(), ListWrite::Applied(_)));
    // Done in both tabs: the second asks for what is stored (spec: no note).
    match b.unwrap() {
        ListWrite::Repeated(current) => assert_eq!(current.version(), 2),
        other => panic!("expected Repeated, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn two_list_renames_at_one_version_apply_one(pool: PgPool) {
    let (user_id, user) = user(&pool, "list-rename-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    let hold = hold_list(&pool, list).await;
    let a = start(&pool, 1, async move {
        first.rename_list(&user, &list_id, 1, &name("Job")).await
    })
    .await;
    let b = start(&pool, 2, async move {
        second.rename_list(&user, &list_id, 1, &name("Home")).await
    })
    .await;
    let (a, b) = release(hold, a, b).await;

    assert!(matches!(a.unwrap(), ListWrite::Applied(_)));
    match b.unwrap() {
        ListWrite::Stale(current) => {
            assert_eq!(current.list.name().as_str(), "Job");
            assert_eq!(current.list.version(), 2);
        }
        other => panic!("expected Stale, got {other:?}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_concurrent_add_makes_the_list_delete_stale_or_waits_until_after_it(pool: PgPool) {
    // The add first: the delete sees a different count and deletes nothing.
    let (user_id, user) = user(&pool, "delete-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);
    let hold = hold_list(&pool, list).await;
    let added = start(&pool, 1, async move {
        first
            .add_todo(&user, &list_id, Uuid::now_v7(), &title("added elsewhere"))
            .await
    })
    .await;
    let deleted = start(&pool, 2, async move {
        second.delete_list(&user, &list_id, 1, 0).await
    })
    .await;
    let (added, deleted) = release(hold, added, deleted).await;
    assert!(matches!(added.unwrap(), ListWrite::Applied(_)));
    match deleted.unwrap() {
        ListWrite::Stale(current) => assert_eq!(current.todo_count, 1),
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(count_todos(&pool, list).await, 1);

    // The delete first: nothing is added to the deleted list.
    let list = insert_list(&pool, user_id, "Home").await;
    let list_id = ListId::from_uuid(list);
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);
    let hold = hold_list(&pool, list).await;
    let deleted = start(&pool, 1, async move {
        first.delete_list(&user, &list_id, 1, 0).await
    })
    .await;
    let added = start(&pool, 2, async move {
        second
            .add_todo(&user, &list_id, Uuid::now_v7(), &title("added elsewhere"))
            .await
    })
    .await;
    let (deleted, added) = release(hold, deleted, added).await;
    assert!(matches!(deleted.unwrap(), ListWrite::Applied(_)));
    assert!(matches!(added.unwrap(), ListWrite::ListNotFound));
    assert_eq!(count_todos(&pool, list).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_moves_settle_on_one_order(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-race@example.com").await;
    let gap = 1 << 20;
    let (list, ids) = list_at(&pool, user_id, &[gap, 2 * gap, 3 * gap, 4 * gap]).await;
    let list_uuid = *list.as_uuid();
    let [a, b, c, d] = [ids[0], ids[1], ids[2], ids[3]];
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);

    // Both move a todo directly below `a`: from the same read, both would
    // take the midpoint of `a` and `b`.
    let hold = hold_list(&pool, list_uuid).await;
    let one = start(&pool, 1, async move {
        first.move_todo(&user, &list, &c, &[a], &[b]).await
    })
    .await;
    let two = start(&pool, 2, async move {
        second.move_todo(&user, &list, &d, &[a], &[b]).await
    })
    .await;
    let (one, two) = release(hold, one, two).await;

    assert_eq!(moved(one), vec![a, c, b, d]);
    assert_eq!(moved(two), vec![a, d, c, b]);
    assert_eq!(open_order(&pool, &list).await, vec![a, d, c, b]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_move_after_its_todo_was_completed_or_deleted_concurrently_changes_nothing(pool: PgPool) {
    let (user_id, user) = user(&pool, "move-gone-race@example.com").await;
    let gap = 1 << 20;
    let (list, ids) = list_at(&pool, user_id, &[gap, 2 * gap, 3 * gap]).await;
    let list_uuid = *list.as_uuid();
    let [a, b, c] = [ids[0], ids[1], ids[2]];

    // Marked done first: the move finds a done todo.
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);
    let hold = hold_list(&pool, list_uuid).await;
    let marked = start(&pool, 1, async move {
        first.mark_todo(&user, &list, &c, 1, true).await
    })
    .await;
    let moving = start(&pool, 2, async move {
        second.move_todo(&user, &list, &c, &[], &[a]).await
    })
    .await;
    let (marked, moving) = release(hold, marked, moving).await;
    assert!(matches!(marked.unwrap(), ListWrite::Applied(_)));
    assert!(matches!(moving.unwrap(), TodoMove::Done(_)));
    assert_eq!(open_order(&pool, &list).await, vec![a, b]);

    // Deleted first: the move finds no todo.
    let (first, second) = (own_repo(&pool).await, own_repo(&pool).await);
    let hold = hold_list(&pool, list_uuid).await;
    let deleted = start(&pool, 1, async move {
        first.delete_todo(&user, &list, &b, 1).await
    })
    .await;
    let moving = start(&pool, 2, async move {
        second.move_todo(&user, &list, &b, &[], &[a]).await
    })
    .await;
    let (deleted, moving) = release(hold, deleted, moving).await;
    assert!(matches!(deleted.unwrap(), ListWrite::Applied(_)));
    assert!(matches!(moving.unwrap(), TodoMove::TodoNotFound));
    assert_eq!(open_order(&pool, &list).await, vec![a]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_list_created_during_the_purge_is_refused(pool: PgPool) {
    let (user_id, user) = user(&pool, "purge-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    sqlx::query(
        "UPDATE users SET deletion_scheduled_at = NOW() - interval '1 minute' WHERE id = $1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    let accounts = PostgresUserRepository::new(Arc::new(Database::from_pool(pool.clone())));
    let mut anonymized = accounts.find_by_id(&user).await.unwrap().unwrap();
    anonymized
        .anonymize(
            Email::new(&format!("deleted-{user_id}@anonymized.local")).unwrap(),
            Timestamp::now(),
        )
        .unwrap();
    let creator = own_repo(&pool).await;

    // The purge takes the account, then waits for the held list; the
    // creation then waits for the account, and finds it purged.
    let hold = hold_list(&pool, list).await;
    let purge = start(&pool, 1, async move {
        accounts
            .anonymize_account(&anonymized, &AuditEvent::account_deleted(user))
            .await
    })
    .await;
    let created = start(&pool, 2, async move {
        creator
            .create_list(&user, Uuid::now_v7(), &name("Late"))
            .await
    })
    .await;
    let (purge, created) = release(hold, purge, created).await;

    assert!(purge.unwrap());
    assert!(matches!(created, Err(DomainError::Authorization(_))));
    assert_eq!(count_lists(&pool, user_id).await, 0);
}
