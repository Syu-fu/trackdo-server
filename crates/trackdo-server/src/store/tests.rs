use proptest::prelude::*;
use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{Op, OpId, Seq};
use trackdo_core::task::{TaskChange, TaskId, TaskStatus};
use trackdo_core::time_entry::{TimeEntryChange, TimeEntryId, Timestamp};
use trackdo_core::title::Title;

use super::{Store, StoreError, UserId};

const USER: UserId = UserId::DEFAULT;
const OTHER: UserId = UserId(2);

fn task_op(id: u128, change: TaskChange) -> Op {
    Op::task(OpId(id), TaskId(7), Hlc::new(1_000, 0, DeviceId(9)), change)
}

fn rename(id: u128, title: &str) -> Op {
    task_op(id, TaskChange::Title(Title::new(title).unwrap()))
}

fn seqs(stored: &[(Seq, Op)]) -> Vec<u64> {
    stored.iter().map(|(Seq(seq), _)| *seq).collect()
}

fn ops(stored: Vec<(Seq, Op)>) -> Vec<Op> {
    stored.into_iter().map(|(_, op)| op).collect()
}

async fn all(store: &Store, user: UserId) -> Vec<(Seq, Op)> {
    store.ops_after(user, None, u32::MAX).await.unwrap()
}

async fn add_user(store: &Store, user: UserId) {
    sqlx::query("INSERT INTO users (id) VALUES (?)")
        .bind(user.0)
        .execute(&store.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn new_log_is_empty() {
    let store = Store::in_memory().await.unwrap();

    assert!(all(&store, USER).await.is_empty());
}

#[tokio::test]
async fn assigns_increasing_seqs_in_arrival_order() {
    let store = Store::in_memory().await.unwrap();
    let first = [rename(1, "a"), rename(2, "b")];
    let second = [rename(3, "c")];

    store.append(USER, &first).await.unwrap();
    store.append(USER, &second).await.unwrap();

    let stored = all(&store, USER).await;
    assert_eq!(seqs(&stored), [1, 2, 3]);
    assert_eq!(ops(stored), [first.as_slice(), &second].concat());
}

#[tokio::test]
async fn acknowledges_every_op_including_retransmissions() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[rename(1, "a")]).await.unwrap();

    let accepted = store
        .append(USER, &[rename(1, "a"), rename(2, "b")])
        .await
        .unwrap();

    assert_eq!(accepted, [OpId(1), OpId(2)]);
}

#[tokio::test]
async fn stores_retransmitted_ops_once() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[rename(1, "a")]).await.unwrap();

    store
        .append(USER, &[rename(1, "a"), rename(2, "b"), rename(2, "b")])
        .await
        .unwrap();

    assert_eq!(seqs(&all(&store, USER).await), [1, 2]);
}

#[tokio::test]
async fn keeps_the_first_copy_of_an_op_id() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[rename(1, "first")]).await.unwrap();

    store.append(USER, &[rename(1, "second")]).await.unwrap();

    assert_eq!(ops(all(&store, USER).await), [rename(1, "first")]);
}

#[tokio::test]
async fn empty_batch_changes_nothing() {
    let store = Store::in_memory().await.unwrap();

    assert!(store.append(USER, &[]).await.unwrap().is_empty());
    assert!(all(&store, USER).await.is_empty());
}

#[tokio::test]
async fn reads_after_a_position_up_to_a_limit() {
    let store = Store::in_memory().await.unwrap();
    let batch: Vec<Op> = (1..=5).map(|id| rename(id, "x")).collect();
    store.append(USER, &batch).await.unwrap();

    let page = store.ops_after(USER, Some(Seq(2)), 2).await.unwrap();

    assert_eq!(seqs(&page), [3, 4]);
    assert_eq!(ops(page), batch[2..4]);
}

#[tokio::test]
async fn reads_nothing_past_the_end() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[rename(1, "a")]).await.unwrap();

    assert!(
        store
            .ops_after(USER, Some(Seq(1)), 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn users_have_separate_logs() {
    let store = Store::in_memory().await.unwrap();
    add_user(&store, OTHER).await;
    store.append(USER, &[rename(1, "a")]).await.unwrap();

    store
        .append(OTHER, &[rename(1, "a"), rename(2, "b")])
        .await
        .unwrap();

    assert_eq!(seqs(&all(&store, USER).await), [1]);
    assert_eq!(seqs(&all(&store, OTHER).await), [1, 2]);
}

#[tokio::test]
async fn rejects_unknown_user() {
    let store = Store::in_memory().await.unwrap();

    assert!(store.append(OTHER, &[rename(1, "a")]).await.is_err());
}

#[tokio::test]
async fn head_is_the_newest_seq_of_the_user() {
    let store = Store::in_memory().await.unwrap();
    add_user(&store, OTHER).await;
    assert_eq!(store.head(USER).await.unwrap(), None);

    store
        .append(USER, &[rename(1, "a"), rename(2, "b")])
        .await
        .unwrap();

    assert_eq!(store.head(USER).await.unwrap(), Some(Seq(2)));
    assert_eq!(store.head(OTHER).await.unwrap(), None);
}

const ENTRY: TimeEntryId = TimeEntryId(0xe);

fn entry_op(id: u128, task: u128) -> Op {
    Op::time_entry(
        OpId(id),
        ENTRY,
        TaskId(task),
        Hlc::new(1_000, 0, DeviceId(9)),
        TimeEntryChange::Deleted(false),
    )
}

fn assert_mismatch(result: Result<Vec<OpId>, StoreError>, op_id: u128, expected: u128) {
    match result {
        Err(StoreError::TaskMismatch {
            op_id: rejected,
            entry,
            expected: first,
            actual,
        }) => {
            assert_eq!(rejected, OpId(op_id));
            assert_eq!(entry, ENTRY);
            assert_eq!(first, TaskId(expected));
            assert_ne!(actual, first);
        }
        other => panic!("expected a task mismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn accepts_more_ops_for_an_entry_on_its_task() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[entry_op(1, 7)]).await.unwrap();

    store.append(USER, &[entry_op(2, 7)]).await.unwrap();

    assert_eq!(seqs(&all(&store, USER).await), [1, 2]);
}

#[tokio::test]
async fn rejects_an_entry_moved_to_another_task() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[entry_op(1, 7)]).await.unwrap();

    assert_mismatch(store.append(USER, &[entry_op(2, 8)]).await, 2, 7);
    assert_eq!(ops(all(&store, USER).await), [entry_op(1, 7)]);
}

#[tokio::test]
async fn rejects_the_whole_batch_on_a_mismatch() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[entry_op(1, 7)]).await.unwrap();

    let result = store
        .append(USER, &[rename(2, "a"), entry_op(3, 8), rename(4, "b")])
        .await;

    assert_mismatch(result, 3, 7);
    assert_eq!(seqs(&all(&store, USER).await), [1]);
}

#[tokio::test]
async fn rejects_a_mismatch_within_one_batch() {
    let store = Store::in_memory().await.unwrap();

    let result = store.append(USER, &[entry_op(1, 7), entry_op(2, 8)]).await;

    assert_mismatch(result, 2, 7);
    assert!(all(&store, USER).await.is_empty());
}

#[tokio::test]
async fn does_not_recheck_retransmitted_ops() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[entry_op(1, 7)]).await.unwrap();

    let accepted = store.append(USER, &[entry_op(1, 8)]).await.unwrap();

    assert_eq!(accepted, [OpId(1)]);
    assert_eq!(ops(all(&store, USER).await), [entry_op(1, 7)]);
}

#[tokio::test]
async fn entries_of_different_users_are_independent() {
    let store = Store::in_memory().await.unwrap();
    add_user(&store, OTHER).await;
    store.append(USER, &[entry_op(1, 7)]).await.unwrap();

    store.append(OTHER, &[entry_op(1, 8)]).await.unwrap();

    assert_eq!(ops(all(&store, OTHER).await), [entry_op(1, 8)]);
}

/// A database file in a fresh directory, removed on drop.
struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("trackdo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> std::path::PathBuf {
        self.0.join("trackdo.db")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn open_creates_and_reopens_a_file() {
    let db = TempDb::new("reopen");

    Store::open(&db.path())
        .await
        .unwrap()
        .append(USER, &[rename(1, "a")])
        .await
        .unwrap();
    let reopened = Store::open(&db.path()).await.unwrap();

    assert_eq!(ops(all(&reopened, USER).await), [rename(1, "a")]);
}

#[tokio::test]
async fn concurrent_appends_get_distinct_seqs() {
    let db = TempDb::new("concurrent");
    let store = Store::open(&db.path()).await.unwrap();

    let appends = (1..=20).map(|id| {
        let store = store.clone();
        async move { store.append(USER, &[rename(id, "x")]).await }
    });
    for result in join_spawned(appends).await {
        result.unwrap();
    }

    assert_eq!(seqs(&all(&store, USER).await), (1..=20).collect::<Vec<_>>());
}

async fn join_spawned<F: std::future::Future + Send + 'static>(
    futures: impl IntoIterator<Item = F>,
) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futures.into_iter().map(tokio::spawn).collect();
    let mut outputs = Vec::new();
    for handle in handles {
        outputs.push(handle.await.unwrap());
    }
    outputs
}

#[tokio::test]
async fn reports_rows_no_valid_op_encodes_to() {
    let store = Store::in_memory().await.unwrap();
    store.append(USER, &[rename(1, "a")]).await.unwrap();
    sqlx::query("UPDATE ops SET value_text = ' padded '")
        .execute(&store.pool)
        .await
        .unwrap();

    let result = store.ops_after(USER, None, 10).await;

    assert!(matches!(result, Err(super::StoreError::Corrupt(1))));
}

fn any_hlc() -> impl Strategy<Value = Hlc> {
    (any::<u64>(), any::<u32>(), any::<u128>())
        .prop_map(|(wall, counter, device)| Hlc::new(wall, counter, DeviceId(device)))
}

fn any_task_change() -> impl Strategy<Value = TaskChange> {
    prop_oneof![
        "[a-zあ-ん]{1,8}( [a-z]{1,8})?".prop_map(|s| TaskChange::Title(Title::new(&s).unwrap())),
        prop::sample::select(vec![TaskStatus::Todo, TaskStatus::Done]).prop_map(TaskChange::Status),
        any::<bool>().prop_map(TaskChange::Deleted),
    ]
}

fn any_entry_change() -> impl Strategy<Value = TimeEntryChange> {
    let time = any::<u64>().prop_map(Timestamp);
    prop_oneof![
        time.clone().prop_map(TimeEntryChange::StartedAt),
        prop::option::of(time).prop_map(TimeEntryChange::StoppedAt),
        any::<bool>().prop_map(TimeEntryChange::Deleted),
    ]
}

fn any_op() -> impl Strategy<Value = Op> {
    let task = (any::<u128>(), any::<u128>(), any_hlc(), any_task_change())
        .prop_map(|(op, task, hlc, change)| Op::task(OpId(op), TaskId(task), hlc, change));
    let entry = (
        any::<u128>(),
        any::<u128>(),
        any::<u128>(),
        any_hlc(),
        any_entry_change(),
    )
        .prop_map(|(op, entry, task, hlc, change)| {
            Op::time_entry(OpId(op), TimeEntryId(entry), TaskId(task), hlc, change)
        });
    prop_oneof![task, entry]
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn stored_ops_read_back_unchanged(op in any_op()) {
        let stored = block_on(async {
            let store = Store::in_memory().await.unwrap();
            store.append(USER, std::slice::from_ref(&op)).await.unwrap();
            all(&store, USER).await
        });

        prop_assert_eq!(stored, vec![(Seq(1), op)]);
    }
}
