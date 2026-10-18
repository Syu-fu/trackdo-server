//! Full state rebuilt from the op log (`GET /snapshot`).

use std::collections::BTreeMap;

use trackdo_core::op::{LogId, Op, OpTarget, Seq};
use trackdo_core::replica::Snapshot;
use trackdo_core::task::Task;
use trackdo_core::time_entry::TimeEntry;

/// Applies `log`, the ops of the log `log_id` in sequence order, to empty
/// state. Tasks and time entries come out ordered by id.
pub fn replay(log_id: LogId, log: &[(Seq, Op)]) -> Snapshot {
    let mut tasks = BTreeMap::new();
    let mut entries = BTreeMap::new();
    for (_, op) in log {
        match &op.target {
            OpTarget::Task { task_id, change } => {
                let task = tasks
                    .remove(task_id)
                    .unwrap_or_else(|| Task::empty(*task_id));
                tasks.insert(*task_id, task.apply(change, op.hlc));
            }
            // The first op of an entry fixes its task; the store rejects
            // later ops naming another one.
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
    Snapshot {
        log_id,
        tasks: tasks.into_values().collect(),
        time_entries: entries.into_values().collect(),
        seq: log.last().map(|(seq, _)| *seq),
    }
}

#[cfg(test)]
mod tests {
    use trackdo_core::hlc::{DeviceId, Hlc};
    use trackdo_core::op::{LogId, Op, OpId, Seq};
    use trackdo_core::replica::Snapshot;
    use trackdo_core::task::{Task, TaskChange, TaskId, TaskStatus};
    use trackdo_core::time_entry::{TimeEntry, TimeEntryChange, TimeEntryId, Timestamp};
    use trackdo_core::title::Title;

    use super::replay as replay_in;

    const LOG: LogId = LogId(0x10);

    fn replay(log: &[(Seq, Op)]) -> Snapshot {
        replay_in(LOG, log)
    }

    const TASK: TaskId = TaskId(7);
    const ENTRY: TimeEntryId = TimeEntryId(0xe);

    fn at(wall: u64) -> Hlc {
        Hlc::new(wall, 0, DeviceId(9))
    }

    fn title(s: &str) -> TaskChange {
        TaskChange::Title(Title::new(s).unwrap())
    }

    fn task_op(seq: u64, wall: u64, change: TaskChange) -> (Seq, Op) {
        (Seq(seq), Op::task(OpId(seq.into()), TASK, at(wall), change))
    }

    fn entry_op(seq: u64, wall: u64, change: TimeEntryChange) -> (Seq, Op) {
        let op = Op::time_entry(OpId(seq.into()), ENTRY, TASK, at(wall), change);
        (Seq(seq), op)
    }

    #[test]
    fn empty_log_gives_empty_snapshot() {
        assert_eq!(
            replay(&[]),
            Snapshot {
                log_id: LOG,
                tasks: Vec::new(),
                time_entries: Vec::new(),
                seq: None,
            }
        );
    }

    #[test]
    fn names_the_log() {
        let log = [task_op(1, 10, title("a"))];

        assert_eq!(replay_in(LogId(7), &log).log_id, LogId(7));
    }

    #[test]
    fn snapshot_is_as_of_the_last_seq() {
        let log = [
            task_op(3, 10, title("a")),
            task_op(8, 20, TaskChange::Deleted(false)),
        ];

        assert_eq!(replay(&log).seq, Some(Seq(8)));
    }

    #[test]
    fn applies_task_writes() {
        let log = [
            task_op(1, 10, title("a")),
            task_op(2, 20, TaskChange::Status(TaskStatus::Done)),
        ];

        let expected = Task::empty(TASK)
            .apply(&title("a"), at(10))
            .apply(&TaskChange::Status(TaskStatus::Done), at(20));
        assert_eq!(replay(&log).tasks, [expected]);
    }

    #[test]
    fn later_seq_with_older_clock_loses() {
        let log = [task_op(1, 20, title("new")), task_op(2, 10, title("old"))];

        let snapshot = replay(&log);

        assert_eq!(snapshot.tasks[0].title().unwrap().as_str(), "new");
    }

    #[test]
    fn applies_time_entry_writes() {
        let started = TimeEntryChange::StartedAt(Timestamp(5));
        let running = TimeEntryChange::StoppedAt(None);
        let log = [
            entry_op(1, 10, started.clone()),
            entry_op(2, 10, running.clone()),
        ];

        let expected = TimeEntry::empty(ENTRY, TASK)
            .apply(&started, at(10))
            .apply(&running, at(10));
        let snapshot = replay(&log);
        assert_eq!(snapshot.time_entries, [expected]);
        assert!(snapshot.tasks.is_empty());
    }

    #[test]
    fn orders_entities_by_id() {
        let log = [
            (Seq(1), Op::task(OpId(1), TaskId(2), at(1), title("b"))),
            (Seq(2), Op::task(OpId(2), TaskId(1), at(1), title("a"))),
        ];

        let ids: Vec<TaskId> = replay(&log).tasks.iter().map(Task::id).collect();

        assert_eq!(ids, [TaskId(1), TaskId(2)]);
    }
}
