//! List and todo schema invariants (OT-013 tasks 1.2/1.5).
//!
//! The deliberately invalid writes use runtime `sqlx::query` (not the
//! compile-checked macros) so they can target constraints directly.

use std::sync::Arc;

use application::ports::outgoing::{LimitKind, ListRepository, ListWrite};
use domain::value_objects::{ListId, ListName, TodoId, TodoTitle, UserId};
use jiff::Timestamp;
use persistence_postgres::time::ToDb;
use persistence_postgres::{Database, PostgresListRepository};
use sqlx::PgPool;
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
async fn concurrent_list_creation_at_the_limit_saves_one(pool: PgPool) {
    let (user_id, user) = user(&pool, "list-race@example.com").await;
    for i in 0..99 {
        insert_list(&pool, user_id, &format!("List {i}")).await;
    }
    let repo = Arc::new(repo(&pool));
    let (name_a, name_b) = (name("A"), name("B"));
    let (a, b) = tokio::join!(
        repo.create_list(&user, Uuid::now_v7(), &name_a),
        repo.create_list(&user, Uuid::now_v7(), &name_b),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    let applied = outcomes
        .iter()
        .filter(|o| matches!(o, ListWrite::Applied(_)))
        .count();
    let limited = outcomes
        .iter()
        .filter(|o| matches!(o, ListWrite::LimitReached(LimitKind::Lists)))
        .count();
    assert_eq!((applied, limited), (1, 1));
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
async fn concurrent_todo_additions_at_the_limit_save_one(pool: PgPool) {
    let (user_id, user) = user(&pool, "todo-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    seed_list_todos(&pool, list, 999).await;
    let repo = Arc::new(repo(&pool));
    let (title_a, title_b) = (title("a"), title("b"));
    let (a, b) = tokio::join!(
        repo.add_todo(&user, &list_id, Uuid::now_v7(), &title_a),
        repo.add_todo(&user, &list_id, Uuid::now_v7(), &title_b),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, ListWrite::Applied(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, ListWrite::LimitReached(LimitKind::Todos)))
            .count(),
        1
    );
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
async fn a_concurrent_add_makes_the_list_delete_stale_or_waits_until_after_it(pool: PgPool) {
    let (user_id, user) = user(&pool, "delete-race@example.com").await;
    let list = insert_list(&pool, user_id, "Work").await;
    let list_id = ListId::from_uuid(list);
    let repo = Arc::new(repo(&pool));
    let added_title = title("added elsewhere");
    let added = repo.add_todo(&user, &list_id, Uuid::now_v7(), &added_title);
    let deleted = repo.delete_list(&user, &list_id, 1, 0);
    let (added, deleted) = tokio::join!(added, deleted);

    match (added.unwrap(), deleted.unwrap()) {
        // The add went first: the delete saw a different count.
        (ListWrite::Applied(_), ListWrite::Stale(current)) => {
            assert_eq!(current.todo_count, 1);
            assert_eq!(count_lists(&pool, user_id).await, 1);
            assert_eq!(count_todos(&pool, list).await, 1);
        }
        // The delete went first: nothing was added to a deleted list.
        (ListWrite::ListNotFound, ListWrite::Applied(_)) => {
            assert_eq!(count_lists(&pool, user_id).await, 0);
        }
        (added, deleted) => panic!("inconsistent outcomes: {added:?}, {deleted:?}"),
    }
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
