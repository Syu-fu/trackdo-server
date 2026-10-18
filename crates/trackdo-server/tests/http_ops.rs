mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};

use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{Op, OpId, Seq};
use trackdo_core::task::{TaskChange, TaskId};
use trackdo_core::time_entry::{TimeEntryChange, TimeEntryId};
use trackdo_core::title::Title;
use trackdo_protocol::op::OpDto;
use trackdo_server::store::{Store, UserId};

use common::{Reply, app, authorized, send};

fn rename(id: u128, title: &str) -> Op {
    Op::task(
        OpId(id),
        TaskId(7),
        Hlc::new(1_000, 0, DeviceId(9)),
        TaskChange::Title(Title::new(title).unwrap()),
    )
}

fn entry_op(id: u128, task: u128) -> Op {
    Op::time_entry(
        OpId(id),
        TimeEntryId(0xe),
        TaskId(task),
        Hlc::new(1_000, 0, DeviceId(9)),
        TimeEntryChange::Deleted(false),
    )
}

fn wire(ops: &[Op]) -> Value {
    serde_json::to_value(ops.iter().map(OpDto::from).collect::<Vec<_>>()).unwrap()
}

fn uuid(id: u128) -> String {
    uuid::Uuid::from_u128(id).to_string()
}

async fn post(app: &axum::Router, body: impl Into<Body>) -> Reply {
    let request = authorized("POST", "/ops")
        .header("content-type", "application/json")
        .body(body.into())
        .unwrap();
    send(app, request).await
}

async fn post_json(app: &axum::Router, body: &Value) -> Reply {
    post(app, body.to_string()).await
}

async fn stored(store: &Store) -> Vec<Op> {
    store
        .ops_after(UserId::DEFAULT, None, u32::MAX)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, op)| op)
        .collect()
}

#[tokio::test]
async fn stores_ops_and_acknowledges_them() {
    let (app, store) = app().await;
    let ops = [rename(1, "a"), rename(2, "b")];

    let reply = post_json(&app, &wire(&ops)).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({ "accepted": [uuid(1), uuid(2)] }));
    assert_eq!(stored(&store).await, ops);
}

#[tokio::test]
async fn acknowledges_retransmissions_without_storing_them_again() {
    let (app, store) = app().await;
    post_json(&app, &wire(&[rename(1, "a")])).await;

    let reply = post_json(&app, &wire(&[rename(1, "a"), rename(2, "b")])).await;

    assert_eq!(reply.json(), json!({ "accepted": [uuid(1), uuid(2)] }));
    let seqs: Vec<Seq> = store
        .ops_after(UserId::DEFAULT, None, u32::MAX)
        .await
        .unwrap()
        .into_iter()
        .map(|(seq, _)| seq)
        .collect();
    assert_eq!(seqs, [Seq(1), Seq(2)]);
}

#[tokio::test]
async fn accepts_an_empty_batch() {
    let (app, _) = app().await;

    let reply = post_json(&app, &json!([])).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json(), json!({ "accepted": [] }));
}

#[tokio::test]
async fn requires_a_token_before_reading_the_body() {
    let (app, _) = app().await;
    let request = Request::post("/ops")
        .header("trackdo-protocol", "2")
        .body(Body::from("not json"))
        .unwrap();

    let reply = send(&app, request).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn requires_the_protocol_version() {
    let (app, _) = app().await;
    let request = Request::post("/ops")
        .header("authorization", "Bearer secret")
        .header("content-type", "application/json")
        .body(Body::from("[]"))
        .unwrap();

    let reply = send(&app, request).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), json!("unsupported_protocol_version"));
}

#[tokio::test]
async fn rejects_malformed_json() {
    let (app, _) = app().await;

    let reply = post(&app, "[{").await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), json!("invalid_json"));
}

#[tokio::test]
async fn rejects_json_that_is_not_ops() {
    let (app, _) = app().await;
    let mut body = wire(&[rename(1, "a")]);
    body[0]["target"]["change"] = json!({ "field": "status", "value": "doing" });

    let reply = post_json(&app, &body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), json!("invalid_json"));
}

#[tokio::test]
async fn rejects_a_missing_content_type() {
    let (app, _) = app().await;
    let request = authorized("POST", "/ops").body(Body::from("[]")).unwrap();

    let reply = send(&app, request).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), json!("invalid_json"));
}

#[tokio::test]
async fn rejects_the_batch_when_an_op_is_invalid() {
    let (app, store) = app().await;
    let mut body = wire(&[rename(1, "a"), rename(2, "b")]);
    body[1]["target"]["change"]["value"] = json!(" padded ");

    let reply = post_json(&app, &body).await;

    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.error_code(), json!("invalid_op"));
    assert_eq!(reply.json()["error"]["op_id"], json!(uuid(2)));
    assert!(stored(&store).await.is_empty());
}

#[tokio::test]
async fn rejects_the_batch_when_an_entry_changes_task() {
    let (app, store) = app().await;
    post_json(&app, &wire(&[entry_op(1, 7)])).await;

    let reply = post_json(&app, &wire(&[rename(2, "a"), entry_op(3, 8)])).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error_code(), json!("task_mismatch"));
    assert_eq!(reply.json()["error"]["op_id"], json!(uuid(3)));
    assert_eq!(stored(&store).await, [entry_op(1, 7)]);
}

#[tokio::test]
async fn rejects_more_than_500_ops() {
    let (app, store) = app().await;
    let ops: Vec<Op> = (1..=501).map(|id| rename(id, "x")).collect();

    let reply = post_json(&app, &wire(&ops)).await;

    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.error_code(), json!("too_large"));
    assert!(stored(&store).await.is_empty());
}

#[tokio::test]
async fn accepts_exactly_500_ops() {
    let (app, _) = app().await;
    let ops: Vec<Op> = (1..=500).map(|id| rename(id, "x")).collect();

    let reply = post_json(&app, &wire(&ops)).await;

    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn rejects_a_body_over_2_mib() {
    let (app, _) = app().await;
    let padding = " ".repeat(2 * 1024 * 1024);

    let reply = post(&app, format!("[]{padding}")).await;

    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(reply.error_code(), json!("too_large"));
}
