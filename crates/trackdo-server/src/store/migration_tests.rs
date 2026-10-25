use sqlx::SqlitePool;
use sqlx::migrate::Migrate;
use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{Op, OpId, Seq};
use trackdo_core::task::{TaskChange, TaskId, TaskStatus};
use trackdo_core::time_entry::{TimeEntryChange, TimeEntryId, Timestamp};
use trackdo_core::title::Title;

use super::{Store, UserId, memory_pool};

const UUID: u128 = 0x0707_0707_0707_0707_0707_0707_0707_0707;
const ID: [u8; 16] = UUID.to_be_bytes();

/// One `ops` row by its columns, every id being [`ID`] when present.
#[derive(Clone, Copy)]
struct Row {
    target: &'static str,
    task_id: bool,
    entry_id: bool,
    tag_id: bool,
    field: &'static str,
    value_text: Option<&'static str>,
    value_int: Option<i64>,
    value_id: bool,
}

const fn task(
    field: &'static str,
    value_text: Option<&'static str>,
    value_int: Option<i64>,
) -> Row {
    Row {
        target: "task",
        task_id: true,
        entry_id: false,
        tag_id: false,
        field,
        value_text,
        value_int,
        value_id: false,
    }
}

const fn tag(field: &'static str, value_text: Option<&'static str>, value_int: Option<i64>) -> Row {
    Row {
        target: "tag",
        task_id: false,
        tag_id: true,
        ..task(field, value_text, value_int)
    }
}

const fn entry(field: &'static str, value_int: Option<i64>) -> Row {
    Row {
        target: "time_entry",
        entry_id: true,
        ..task(field, None, value_int)
    }
}

/// A database with only the first migration applied, as servers up to v0.2
/// left it.
async fn v1_pool() -> SqlitePool {
    let pool = memory_pool().await.unwrap();
    let migrator = sqlx::migrate!();
    let first = migrator.iter().next().unwrap();
    assert_eq!(first.version, 1);
    let mut conn = pool.acquire().await.unwrap();
    conn.ensure_migrations_table().await.unwrap();
    conn.apply(first).await.unwrap();
    drop(conn);
    pool
}

async fn migrated_pool() -> SqlitePool {
    let pool = v1_pool().await;
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

async fn insert_v1(pool: &SqlitePool, seq: i64, row: Row) {
    sqlx::query(
        "INSERT INTO ops (user_id, seq, op_id, hlc_wall_ms, hlc_counter, hlc_device, \
                          target, task_id, entry_id, field, value_text, value_int) \
         VALUES (1, ?, ?, 1000, 0, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(seq)
    .bind(op_id(seq))
    .bind(ID.to_vec())
    .bind(row.target)
    .bind(ID.to_vec())
    .bind(row.entry_id.then(|| ID.to_vec()))
    .bind(row.field)
    .bind(row.value_text)
    .bind(row.value_int)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert(pool: &SqlitePool, seq: i64, row: Row) -> Result<(), sqlx::Error> {
    let id = |present: bool| present.then(|| ID.to_vec());
    sqlx::query(
        "INSERT INTO ops (user_id, seq, op_id, hlc_wall_ms, hlc_counter, hlc_device, \
                          target, task_id, entry_id, tag_id, field, value_text, value_int, \
                          value_id) \
         VALUES (1, ?, ?, 1000, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(seq)
    .bind(op_id(seq))
    .bind(ID.to_vec())
    .bind(row.target)
    .bind(id(row.task_id))
    .bind(id(row.entry_id))
    .bind(id(row.tag_id))
    .bind(row.field)
    .bind(row.value_text)
    .bind(row.value_int)
    .bind(id(row.value_id))
    .execute(pool)
    .await
    .map(|_| ())
}

fn op_id(seq: i64) -> Vec<u8> {
    u128::try_from(seq).unwrap().to_be_bytes().to_vec()
}

const V1_ROWS: [Row; 7] = [
    task("title", Some("a"), None),
    task("status", Some("todo"), None),
    task("status", Some("done"), None),
    task("deleted", None, Some(1)),
    entry("started_at", Some(5)),
    entry("stopped_at", None),
    entry("deleted", Some(0)),
];

#[tokio::test]
async fn keeps_the_ops_stored_before_the_migration() {
    let pool = v1_pool().await;
    for (seq, row) in (1..).zip(V1_ROWS) {
        insert_v1(&pool, seq, row).await;
    }

    sqlx::migrate!().run(&pool).await.unwrap();

    let hlc = Hlc::new(1_000, 0, DeviceId(UUID));
    let task = |seq: u64, change| {
        (
            Seq(seq),
            Op::task(OpId(seq.into()), TaskId(UUID), hlc, change),
        )
    };
    let entry = |seq: u64, change| {
        let op = Op::time_entry(
            OpId(seq.into()),
            TimeEntryId(UUID),
            TaskId(UUID),
            hlc,
            change,
        );
        (Seq(seq), op)
    };
    let store = Store { pool };
    assert_eq!(
        store.ops_after(UserId::DEFAULT, None, 100).await.unwrap(),
        [
            task(1, TaskChange::Title(Title::new("a").unwrap())),
            task(2, TaskChange::Status(TaskStatus::Todo)),
            task(3, TaskChange::Status(TaskStatus::Done)),
            task(4, TaskChange::Deleted(true)),
            entry(5, TimeEntryChange::StartedAt(Timestamp(5))),
            entry(6, TimeEntryChange::StoppedAt(None)),
            entry(7, TimeEntryChange::Deleted(false)),
        ]
    );
}

const fn with_tag(row: Row) -> Row {
    Row {
        tag_id: true,
        ..row
    }
}

const fn with_value_id(row: Row) -> Row {
    Row {
        value_id: true,
        ..row
    }
}

const fn without_task(row: Row) -> Row {
    Row {
        task_id: false,
        ..row
    }
}

/// Every shape a valid op is stored as.
const ACCEPTED: [Row; 23] = [
    V1_ROWS[0],
    V1_ROWS[1],
    V1_ROWS[2],
    V1_ROWS[3],
    V1_ROWS[4],
    V1_ROWS[5],
    V1_ROWS[6],
    task("priority", Some("none"), None),
    task("priority", Some("low"), None),
    task("priority", Some("medium"), None),
    task("priority", Some("high"), None),
    task("note", Some(""), None),
    task("note", Some("two\nlines"), None),
    task("due", Some("2024-02-29"), None),
    task("due", None, None),
    task("scheduled_at", None, Some(5)),
    task("scheduled_at", None, None),
    with_value_id(task("parent", None, None)),
    task("parent", None, None),
    with_tag(task("tag", None, Some(1))),
    with_tag(task("tag", None, Some(0))),
    tag("name", Some("work"), None),
    tag("deleted", None, Some(1)),
];

const REJECTED: [Row; 18] = [
    without_task(task("title", Some("a"), None)),
    with_tag(task("title", Some("a"), None)),
    with_value_id(task("title", Some("a"), None)),
    without_task(entry("started_at", Some(5))),
    with_tag(entry("started_at", Some(5))),
    task("priority", Some("urgent"), None),
    task("priority", None, None),
    task("note", None, None),
    task("due", Some("2023-02-29"), None),
    task("due", Some("2024-2-1"), None),
    task("due", None, Some(5)),
    task("scheduled_at", Some("5"), None),
    task("parent", None, Some(5)),
    task("tag", None, Some(1)),
    with_tag(task("tag", None, Some(2))),
    with_tag(Row {
        task_id: true,
        ..tag("name", Some("work"), None)
    }),
    tag("name", None, None),
    tag("deleted", None, Some(2)),
];

#[tokio::test]
async fn accepts_every_kind_of_op() {
    let pool = migrated_pool().await;
    for (seq, row) in (1..).zip(ACCEPTED) {
        let result = insert(&pool, seq, row).await;

        assert!(result.is_ok(), "row {seq}: {result:?}");
    }
}

#[tokio::test]
async fn rejects_rows_no_op_is_stored_as() {
    let pool = migrated_pool().await;
    for (seq, row) in (1..).zip(REJECTED) {
        let error = insert(&pool, seq, row).await.err();

        let message = error
            .as_ref()
            .and_then(|e| e.as_database_error())
            .map(|e| e.message());
        assert!(
            message.is_some_and(|m| m.starts_with("CHECK constraint failed")),
            "row {seq}: {error:?}"
        );
    }
}

#[tokio::test]
async fn still_finds_the_first_op_of_an_entry_by_index() {
    let pool = migrated_pool().await;

    let indexes: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'ops' AND sql IS NOT NULL")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert_eq!(indexes, ["ops_entry"]);
}
