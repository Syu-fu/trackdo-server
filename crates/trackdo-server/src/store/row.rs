//! Mapping between [`Op`] and rows of the `ops` table.

use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{Op, OpId, OpTarget, Seq};
use trackdo_core::task::{TaskChange, TaskId, TaskStatus};
use trackdo_core::time_entry::{TimeEntryChange, TimeEntryId, Timestamp};
use trackdo_core::title::Title;

#[derive(Debug, sqlx::FromRow)]
pub(super) struct OpRow {
    pub seq: i64,
    pub op_id: Vec<u8>,
    pub hlc_wall_ms: i64,
    pub hlc_counter: i64,
    pub hlc_device: Vec<u8>,
    pub target: String,
    pub task_id: Vec<u8>,
    pub entry_id: Option<Vec<u8>>,
    pub field: String,
    pub value_text: Option<String>,
    pub value_int: Option<i64>,
}

/// A field change as `(field, value_text, value_int)`.
type Value = (&'static str, Option<String>, Option<i64>);

impl OpRow {
    pub fn encode(seq: i64, op: &Op) -> Self {
        let (target, task_id, entry_id, (field, value_text, value_int)) = match &op.target {
            OpTarget::Task { task_id, change } => ("task", task_id.0, None, task_value(change)),
            OpTarget::TimeEntry {
                entry_id,
                task_id,
                change,
            } => (
                "time_entry",
                task_id.0,
                Some(entry_id.0),
                entry_value(change),
            ),
        };
        Self {
            seq,
            op_id: id_bytes(op.op_id.0),
            hlc_wall_ms: to_i64(op.hlc.wall_ms()),
            hlc_counter: i64::from(op.hlc.counter()),
            hlc_device: id_bytes(op.hlc.device().0),
            target: target.to_owned(),
            task_id: id_bytes(task_id),
            entry_id: entry_id.map(id_bytes),
            field: field.to_owned(),
            value_text,
            value_int,
        }
    }

    /// `None` when the row holds something no valid op encodes to.
    pub fn decode(self) -> Option<(Seq, Op)> {
        let op_id = OpId(id(&self.op_id)?);
        let hlc = Hlc::new(
            to_u64(self.hlc_wall_ms),
            u32::try_from(self.hlc_counter).ok()?,
            DeviceId(id(&self.hlc_device)?),
        );
        let task_id = TaskId(id(&self.task_id)?);
        let field = self.field.as_str();
        let op = match (self.target.as_str(), &self.entry_id) {
            ("task", None) => Op::task(
                op_id,
                task_id,
                hlc,
                task_change(field, self.value_text, self.value_int)?,
            ),
            ("time_entry", Some(entry_id)) => Op::time_entry(
                op_id,
                TimeEntryId(id(entry_id)?),
                task_id,
                hlc,
                entry_change(field, self.value_text, self.value_int)?,
            ),
            _ => return None,
        };
        Some((Seq(u64::try_from(self.seq).ok()?), op))
    }
}

fn task_value(change: &TaskChange) -> Value {
    match change {
        TaskChange::Title(title) => ("title", Some(title.as_str().to_owned()), None),
        TaskChange::Status(TaskStatus::Todo) => ("status", Some("todo".to_owned()), None),
        TaskChange::Status(TaskStatus::Done) => ("status", Some("done".to_owned()), None),
        TaskChange::Deleted(deleted) => ("deleted", None, Some(i64::from(*deleted))),
    }
}

fn entry_value(change: &TimeEntryChange) -> Value {
    match change {
        TimeEntryChange::StartedAt(Timestamp(started)) => {
            ("started_at", None, Some(to_i64(*started)))
        }
        TimeEntryChange::StoppedAt(stopped) => (
            "stopped_at",
            None,
            stopped.map(|Timestamp(stopped)| to_i64(stopped)),
        ),
        TimeEntryChange::Deleted(deleted) => ("deleted", None, Some(i64::from(*deleted))),
    }
}

fn task_change(field: &str, text: Option<String>, int: Option<i64>) -> Option<TaskChange> {
    match (field, text, int) {
        ("title", Some(title), None) => {
            let decoded = Title::new(&title).ok()?;
            (decoded.as_str() == title).then_some(TaskChange::Title(decoded))
        }
        ("status", Some(status), None) => match status.as_str() {
            "todo" => Some(TaskChange::Status(TaskStatus::Todo)),
            "done" => Some(TaskChange::Status(TaskStatus::Done)),
            _ => None,
        },
        ("deleted", None, Some(deleted)) => flag(deleted).map(TaskChange::Deleted),
        _ => None,
    }
}

fn entry_change(field: &str, text: Option<String>, int: Option<i64>) -> Option<TimeEntryChange> {
    match (field, text, int) {
        ("started_at", None, Some(started)) => {
            Some(TimeEntryChange::StartedAt(Timestamp(to_u64(started))))
        }
        ("stopped_at", None, stopped) => Some(TimeEntryChange::StoppedAt(
            stopped.map(|stopped| Timestamp(to_u64(stopped))),
        )),
        ("deleted", None, Some(deleted)) => flag(deleted).map(TimeEntryChange::Deleted),
        _ => None,
    }
}

fn flag(value: i64) -> Option<bool> {
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// Big-endian, matching the UUID byte order on the wire.
pub(super) fn id_bytes(id: u128) -> Vec<u8> {
    id.to_be_bytes().to_vec()
}

pub(super) fn id(bytes: &[u8]) -> Option<u128> {
    Some(u128::from_be_bytes(bytes.try_into().ok()?))
}

/// Lossless bit-cast; SQLite integers are signed.
fn to_i64(value: u64) -> i64 {
    value as i64
}

fn to_u64(value: i64) -> u64 {
    value as u64
}
