//! However ops arrive — shuffled, split into batches, retransmitted — the
//! snapshot replayed from the stored log equals applying each distinct op
//! once, in any order.

use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::sample::Index;

use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{LogId, Op, OpId, OpTarget, Seq};
use trackdo_core::task::{Task, TaskChange, TaskId, TaskStatus};
use trackdo_core::time_entry::{TimeEntry, TimeEntryChange, TimeEntryId, Timestamp};
use trackdo_core::title::Title;
use trackdo_server::snapshot::replay;
use trackdo_server::store::{Store, UserId};

/// Few ids and clock values, so writes collide on fields and timestamps tie.
const IDS: u128 = 3;

fn any_hlc() -> impl Strategy<Value = Hlc> {
    (0u64..4, 0u32..2, 0u128..2)
        .prop_map(|(wall, counter, device)| Hlc::new(wall, counter, DeviceId(device)))
}

fn any_task_change() -> impl Strategy<Value = TaskChange> {
    prop_oneof![
        prop::sample::select(vec!["a", "b"])
            .prop_map(|title| TaskChange::Title(Title::new(title).unwrap())),
        prop::sample::select(vec![TaskStatus::Todo, TaskStatus::Done]).prop_map(TaskChange::Status),
        any::<bool>().prop_map(TaskChange::Deleted),
    ]
}

fn any_entry_change() -> impl Strategy<Value = TimeEntryChange> {
    let time = (0u64..3).prop_map(Timestamp);
    prop_oneof![
        time.clone().prop_map(TimeEntryChange::StartedAt),
        prop::option::of(time).prop_map(TimeEntryChange::StoppedAt),
        any::<bool>().prop_map(TimeEntryChange::Deleted),
    ]
}

/// Every entry belongs to a fixed task, as with well-behaved clients.
fn any_target() -> impl Strategy<Value = OpTarget> {
    prop_oneof![
        (0..IDS, any_task_change()).prop_map(|(task, change)| OpTarget::Task {
            task_id: TaskId(task),
            change,
        }),
        (0..IDS, any_entry_change()).prop_map(|(entry, change)| OpTarget::TimeEntry {
            entry_id: TimeEntryId(entry),
            task_id: TaskId(entry % IDS),
            change,
        }),
    ]
}

/// Distinct ops, in the order they were generated.
fn any_ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec((any_hlc(), any_target()), 0..30).prop_map(|writes| {
        writes
            .into_iter()
            .zip(1..)
            .map(|((hlc, target), id)| Op {
                op_id: OpId(id),
                hlc,
                target,
            })
            .collect()
    })
}

/// The ops as batches reaching the server: reordered, with some sent again.
fn arrivals(ops: Vec<Op>) -> impl Strategy<Value = Vec<Vec<Op>>> {
    let resent = prop::collection::vec(any::<Index>(), 0..10);
    let sizes = prop::collection::vec(1usize..6, 1..40);
    (resent, sizes)
        .prop_flat_map(move |(resent, sizes)| {
            let mut sent = ops.clone();
            if !ops.is_empty() {
                sent.extend(resent.iter().map(|index| index.get(&ops).clone()));
            }
            (Just(sent).prop_shuffle(), Just(sizes))
        })
        .prop_map(|(sent, sizes)| batches(&sent, &sizes))
}

fn batches(ops: &[Op], sizes: &[usize]) -> Vec<Vec<Op>> {
    let mut rest = ops;
    let mut batches = Vec::new();
    for size in sizes.iter().cycle() {
        if rest.is_empty() {
            break;
        }
        let (batch, tail) = rest.split_at((*size).min(rest.len()));
        batches.push(batch.to_vec());
        rest = tail;
    }
    batches
}

/// Applies each op once, in generation order, independently of the server.
fn expected_state(ops: &[Op]) -> (Vec<Task>, Vec<TimeEntry>) {
    let mut tasks = BTreeMap::new();
    let mut entries = BTreeMap::new();
    for op in ops {
        match &op.target {
            OpTarget::Task { task_id, change } => {
                let task = tasks
                    .remove(task_id)
                    .unwrap_or_else(|| Task::empty(*task_id));
                tasks.insert(*task_id, task.apply(change, op.hlc));
            }
            OpTarget::TimeEntry {
                entry_id,
                task_id,
                change,
            } => {
                let entry = entries
                    .remove(entry_id)
                    .unwrap_or_else(|| TimeEntry::empty(*entry_id, *task_id));
                entries.insert(*entry_id, entry.apply(change, op.hlc));
            }
        }
    }
    (
        tasks.into_values().collect(),
        entries.into_values().collect(),
    )
}

fn store_all(batches: &[Vec<Op>]) -> Vec<(Seq, Op)> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = Store::in_memory().await.unwrap();
            for batch in batches {
                store.append(UserId::DEFAULT, batch).await.unwrap();
            }
            store
                .ops_after(UserId::DEFAULT, None, u32::MAX)
                .await
                .unwrap()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn snapshot_does_not_depend_on_arrival_order(
        (ops, batches) in any_ops().prop_flat_map(|ops| (Just(ops.clone()), arrivals(ops)))
    ) {
        let snapshot = replay(LogId(1), &store_all(&batches));

        let (tasks, time_entries) = expected_state(&ops);
        prop_assert_eq!(snapshot.tasks, tasks);
        prop_assert_eq!(snapshot.time_entries, time_entries);
        let distinct = u64::try_from(ops.len()).unwrap();
        prop_assert_eq!(snapshot.seq, (distinct > 0).then_some(Seq(distinct)));
    }
}
