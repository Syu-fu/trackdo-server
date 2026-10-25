mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;

use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{Op, OpId, Seq};
use trackdo_core::replica::Snapshot;
use trackdo_core::tag::{Tag, TagChange, TagId, TagName};
use trackdo_core::task::{Task, TaskChange, TaskId, TaskStatus};
use trackdo_core::time_entry::{TimeEntry, TimeEntryChange, TimeEntryId, Timestamp};
use trackdo_core::title::Title;
use trackdo_protocol::snapshot::SnapshotDto;
use trackdo_server::store::UserId;

use common::{LOG, LOG_UUID, Reply, app, authorized, send};

const TASK: TaskId = TaskId(7);
const ENTRY: TimeEntryId = TimeEntryId(0xe);
const TAG: TagId = TagId(0x7a);

fn at(wall: u64) -> Hlc {
    Hlc::new(wall, 0, DeviceId(9))
}

fn title(s: &str) -> TaskChange {
    TaskChange::Title(Title::new(s).unwrap())
}

async fn get_snapshot(app: &axum::Router) -> Reply {
    send(
        app,
        authorized("GET", "/snapshot").body(Body::empty()).unwrap(),
    )
    .await
}

fn decode(reply: &Reply) -> Snapshot {
    let dto: SnapshotDto = serde_json::from_slice(&reply.body).unwrap();
    Snapshot::try_from(dto).unwrap()
}

#[tokio::test]
async fn empty_log_has_no_seq() {
    let (app, _) = app().await;

    let reply = get_snapshot(&app).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({ "log_id": LOG_UUID, "seq": null, "tasks": [], "time_entries": [], "tags": [] })
    );
}

#[tokio::test]
async fn replays_the_log_as_of_its_last_seq() {
    let (app, store) = app().await;
    let started = TimeEntryChange::StartedAt(Timestamp(5));
    let tagged = TaskChange::Tag {
        tag: TAG,
        tagged: true,
    };
    let named = TagChange::Name(TagName::new("work").unwrap());
    store
        .append(
            UserId::DEFAULT,
            &[
                Op::task(OpId(1), TASK, at(20), title("new")),
                Op::task(OpId(2), TASK, at(10), title("old")),
                Op::task(OpId(3), TASK, at(30), TaskChange::Status(TaskStatus::Done)),
                Op::time_entry(OpId(4), ENTRY, TASK, at(40), started.clone()),
                Op::task(OpId(5), TASK, at(50), tagged.clone()),
                Op::tag(OpId(6), TAG, at(60), named.clone()),
            ],
        )
        .await
        .unwrap();

    let snapshot = decode(&get_snapshot(&app).await);

    let task = Task::empty(TASK)
        .apply(&title("new"), at(20))
        .apply(&TaskChange::Status(TaskStatus::Done), at(30))
        .apply(&tagged, at(50));
    let entry = TimeEntry::empty(ENTRY, TASK).apply(&started, at(40));
    assert_eq!(
        snapshot,
        Snapshot {
            log_id: LOG,
            tasks: vec![task],
            time_entries: vec![entry],
            tags: vec![Tag::empty(TAG).apply(&named, at(60))],
            seq: Some(Seq(6)),
        }
    );
}

#[tokio::test]
async fn sends_registers_with_their_clocks() {
    let (app, store) = app().await;
    store
        .append(
            UserId::DEFAULT,
            &[Op::task(OpId(1), TASK, at(20), title("a"))],
        )
        .await
        .unwrap();

    let reply = get_snapshot(&app).await;

    assert_eq!(
        reply.json()["tasks"][0]["title"],
        json!({
            "value": "a",
            "hlc": { "wall_ms": 20, "counter": 0, "device": uuid::Uuid::from_u128(9) },
        })
    );
}

#[tokio::test]
async fn requires_a_token() {
    let (app, _) = app().await;

    let reply = send(&app, Request::get("/snapshot").body(Body::empty()).unwrap()).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}
