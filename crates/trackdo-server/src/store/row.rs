//! Mapping between [`Op`] and rows of the `ops` table.

use trackdo_core::date::Date;
use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::note::Note;
use trackdo_core::op::{Op, OpId, OpTarget, Seq};
use trackdo_core::tag::{TagChange, TagId, TagName};
use trackdo_core::task::{TaskChange, TaskId, TaskPriority, TaskStatus};
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
    pub task_id: Option<Vec<u8>>,
    pub entry_id: Option<Vec<u8>>,
    pub tag_id: Option<Vec<u8>>,
    pub field: String,
    pub value_text: Option<String>,
    pub value_int: Option<i64>,
    pub value_id: Option<Vec<u8>>,
}

/// The value columns of a row.
#[derive(Default)]
struct Value {
    text: Option<String>,
    int: Option<i64>,
    id: Option<u128>,
}

impl Value {
    fn text(text: &str) -> Self {
        Self {
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    fn int(int: Option<i64>) -> Self {
        Self {
            int,
            ..Self::default()
        }
    }

    fn id(id: Option<u128>) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    fn flag(flag: bool) -> Self {
        Self::int(Some(i64::from(flag)))
    }
}

/// The ids of the entity a row writes to.
struct Keys {
    task: Option<u128>,
    entry: Option<u128>,
    tag: Option<u128>,
}

impl OpRow {
    pub fn encode(seq: i64, op: &Op) -> Self {
        let (target, keys, (field, value)) = match &op.target {
            OpTarget::Task { task_id, change } => {
                let tag = match change {
                    TaskChange::Tag { tag, .. } => Some(tag.0),
                    _ => None,
                };
                let keys = Keys {
                    task: Some(task_id.0),
                    entry: None,
                    tag,
                };
                ("task", keys, task_value(change))
            }
            OpTarget::TimeEntry {
                entry_id,
                task_id,
                change,
            } => {
                let keys = Keys {
                    task: Some(task_id.0),
                    entry: Some(entry_id.0),
                    tag: None,
                };
                ("time_entry", keys, entry_value(change))
            }
            OpTarget::Tag { tag_id, change } => {
                let keys = Keys {
                    task: None,
                    entry: None,
                    tag: Some(tag_id.0),
                };
                ("tag", keys, tag_value(change))
            }
        };
        Self {
            seq,
            op_id: id_bytes(op.op_id.0),
            hlc_wall_ms: to_i64(op.hlc.wall_ms()),
            hlc_counter: i64::from(op.hlc.counter()),
            hlc_device: id_bytes(op.hlc.device().0),
            target: target.to_owned(),
            task_id: keys.task.map(id_bytes),
            entry_id: keys.entry.map(id_bytes),
            tag_id: keys.tag.map(id_bytes),
            field: field.to_owned(),
            value_text: value.text,
            value_int: value.int,
            value_id: value.id.map(id_bytes),
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
        let field = self.field.as_str();
        let value = Value {
            text: self.value_text,
            int: self.value_int,
            id: nullable_id(self.value_id.as_deref())?,
        };
        let tag = nullable_id(self.tag_id.as_deref())?;
        let op = match (self.target.as_str(), &self.task_id, &self.entry_id) {
            ("task", Some(task_id), None) => Op::task(
                op_id,
                TaskId(id(task_id)?),
                hlc,
                task_change(field, tag.map(TagId), value)?,
            ),
            ("time_entry", Some(task_id), Some(entry_id)) if tag.is_none() => Op::time_entry(
                op_id,
                TimeEntryId(id(entry_id)?),
                TaskId(id(task_id)?),
                hlc,
                entry_change(field, value)?,
            ),
            ("tag", None, None) => Op::tag(op_id, TagId(tag?), hlc, tag_change(field, value)?),
            _ => return None,
        };
        Some((Seq(u64::try_from(self.seq).ok()?), op))
    }
}

fn task_value(change: &TaskChange) -> (&'static str, Value) {
    match change {
        TaskChange::Title(title) => ("title", Value::text(title.as_str())),
        TaskChange::Status(status) => ("status", Value::text(status_text(*status))),
        TaskChange::Deleted(deleted) => ("deleted", Value::flag(*deleted)),
        TaskChange::Priority(priority) => ("priority", Value::text(priority_text(*priority))),
        TaskChange::Note(note) => ("note", Value::text(note.as_str())),
        TaskChange::Due(due) => (
            "due",
            due.map_or_else(Value::default, |due| Value::text(&date_text(due))),
        ),
        TaskChange::ScheduledAt(scheduled) => (
            "scheduled_at",
            Value::int(scheduled.map(|Timestamp(scheduled)| to_i64(scheduled))),
        ),
        TaskChange::Parent(parent) => ("parent", Value::id(parent.map(|TaskId(parent)| parent))),
        // The tag itself goes into the `tag_id` key column.
        TaskChange::Tag { tagged, .. } => ("tag", Value::flag(*tagged)),
    }
}

fn entry_value(change: &TimeEntryChange) -> (&'static str, Value) {
    match change {
        TimeEntryChange::StartedAt(Timestamp(started)) => {
            ("started_at", Value::int(Some(to_i64(*started))))
        }
        TimeEntryChange::StoppedAt(stopped) => (
            "stopped_at",
            Value::int(stopped.map(|Timestamp(stopped)| to_i64(stopped))),
        ),
        TimeEntryChange::Deleted(deleted) => ("deleted", Value::flag(*deleted)),
    }
}

fn tag_value(change: &TagChange) -> (&'static str, Value) {
    match change {
        TagChange::Name(name) => ("name", Value::text(name.as_str())),
        TagChange::Deleted(deleted) => ("deleted", Value::flag(*deleted)),
    }
}

/// `tag` is the row's `tag_id`, which only `tag` changes have.
fn task_change(field: &str, tag: Option<TagId>, value: Value) -> Option<TaskChange> {
    let Value { text, int, id } = value;
    match (field, tag, text, int, id) {
        ("title", None, Some(title), None, None) => {
            canonical(&title, Title::new, Title::as_str).map(TaskChange::Title)
        }
        ("status", None, Some(status), None, None) => status_from(&status).map(TaskChange::Status),
        ("deleted", None, None, Some(deleted), None) => flag(deleted).map(TaskChange::Deleted),
        ("priority", None, Some(priority), None, None) => {
            priority_from(&priority).map(TaskChange::Priority)
        }
        ("note", None, Some(note), None, None) => {
            canonical(&note, Note::new, Note::as_str).map(TaskChange::Note)
        }
        ("due", None, None, None, None) => Some(TaskChange::Due(None)),
        ("due", None, Some(due), None, None) => {
            date_from(&due).map(|due| TaskChange::Due(Some(due)))
        }
        ("scheduled_at", None, None, scheduled, None) => Some(TaskChange::ScheduledAt(
            scheduled.map(|scheduled| Timestamp(to_u64(scheduled))),
        )),
        ("parent", None, None, None, parent) => Some(TaskChange::Parent(parent.map(TaskId))),
        ("tag", Some(tag), None, Some(tagged), None) => {
            flag(tagged).map(|tagged| TaskChange::Tag { tag, tagged })
        }
        _ => None,
    }
}

fn entry_change(field: &str, value: Value) -> Option<TimeEntryChange> {
    let Value { text, int, id } = value;
    match (field, text, int, id) {
        ("started_at", None, Some(started), None) => {
            Some(TimeEntryChange::StartedAt(Timestamp(to_u64(started))))
        }
        ("stopped_at", None, stopped, None) => Some(TimeEntryChange::StoppedAt(
            stopped.map(|stopped| Timestamp(to_u64(stopped))),
        )),
        ("deleted", None, Some(deleted), None) => flag(deleted).map(TimeEntryChange::Deleted),
        _ => None,
    }
}

fn tag_change(field: &str, value: Value) -> Option<TagChange> {
    let Value { text, int, id } = value;
    match (field, text, int, id) {
        ("name", Some(name), None, None) => {
            canonical(&name, TagName::new, TagName::as_str).map(TagChange::Name)
        }
        ("deleted", None, Some(deleted), None) => flag(deleted).map(TagChange::Deleted),
        _ => None,
    }
}

/// Builds a value from stored text, only if building it keeps the text as
/// is, so stored values stay canonical.
fn canonical<T, E>(
    text: &str,
    new: impl Fn(&str) -> Result<T, E>,
    as_str: impl Fn(&T) -> &str,
) -> Option<T> {
    new(text).ok().filter(|value| as_str(value) == text)
}

const fn status_text(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Todo => "todo",
        TaskStatus::Done => "done",
    }
}

fn status_from(text: &str) -> Option<TaskStatus> {
    match text {
        "todo" => Some(TaskStatus::Todo),
        "done" => Some(TaskStatus::Done),
        _ => None,
    }
}

const fn priority_text(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::None => "none",
        TaskPriority::Low => "low",
        TaskPriority::Medium => "medium",
        TaskPriority::High => "high",
    }
}

fn priority_from(text: &str) -> Option<TaskPriority> {
    match text {
        "none" => Some(TaskPriority::None),
        "low" => Some(TaskPriority::Low),
        "medium" => Some(TaskPriority::Medium),
        "high" => Some(TaskPriority::High),
        _ => None,
    }
}

/// `YYYY-MM-DD`, as on the wire.
fn date_text(date: Date) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn date_from(text: &str) -> Option<Date> {
    let mut parts = text.splitn(3, '-').map(str::parse::<u16>);
    let (year, month, day) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    let date = Date::new(year, u8::try_from(month).ok()?, u8::try_from(day).ok()?).ok()?;
    (date_text(date) == text).then_some(date)
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

/// `Some(None)` for NULL, `None` for bytes that are no id.
fn nullable_id(bytes: Option<&[u8]>) -> Option<Option<u128>> {
    bytes.map_or(Some(None), |bytes| id(bytes).map(Some))
}

/// Lossless bit-cast; SQLite integers are signed.
fn to_i64(value: u64) -> i64 {
    value as i64
}

fn to_u64(value: i64) -> u64 {
    value as u64
}
