mod common;

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::time::timeout;
use tower::ServiceExt;

use trackdo_core::hlc::{DeviceId, Hlc};
use trackdo_core::op::{Op, OpId};
use trackdo_core::task::{TaskChange, TaskId};
use trackdo_core::title::Title;
use trackdo_protocol::op::OpDto;
use trackdo_server::store::UserId;

use common::{app, authorized, send};

/// Fails a test whose event never comes instead of hanging it.
const DEADLINE: Duration = Duration::from_secs(5);
/// How long to listen when nothing is expected to arrive.
const QUIET: Duration = Duration::from_millis(100);

fn rename(id: u128) -> Op {
    Op::task(
        OpId(id),
        TaskId(7),
        Hlc::new(1_000, 0, DeviceId(9)),
        TaskChange::Title(Title::new("x").unwrap()),
    )
}

fn wire(op: &Op) -> Value {
    serde_json::to_value(OpDto::from(op)).unwrap()
}

#[derive(Debug, PartialEq)]
struct Event {
    id: String,
    event: String,
    data: Value,
}

/// Reads server-sent events from a streaming response body.
struct Events {
    body: Body,
    buffer: String,
}

impl Events {
    async fn open(app: &Router, last_event_id: Option<&str>) -> Self {
        let request = last_event_id
            .into_iter()
            .fold(authorized("GET", "/events"), |request, id| {
                request.header("last-event-id", id)
            })
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        Self {
            body: response.into_body(),
            buffer: String::new(),
        }
    }

    async fn next(&mut self) -> Event {
        timeout(DEADLINE, self.read_event())
            .await
            .expect("no event in time")
    }

    async fn assert_quiet(&mut self) {
        if let Ok(event) = timeout(QUIET, self.read_event()).await {
            panic!("unexpected event {event:?}");
        }
    }

    async fn read_event(&mut self) -> Event {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                if let Some(event) = parse(&block) {
                    return event;
                }
                continue;
            }
            let frame = self.body.frame().await.expect("stream ended").unwrap();
            if let Ok(data) = frame.into_data() {
                self.buffer.push_str(std::str::from_utf8(&data).unwrap());
            }
        }
    }
}

/// `None` for comment-only blocks such as keepalives.
fn parse(block: &str) -> Option<Event> {
    let field = |name: &str| {
        block
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .map(|value| value.trim_start().to_owned())
    };
    Some(Event {
        id: field("id")?,
        event: field("event")?,
        data: serde_json::from_str(&field("data")?).unwrap(),
    })
}

fn event(seq: u64, op: &Op) -> Event {
    Event {
        id: seq.to_string(),
        event: "op".to_owned(),
        data: wire(op),
    }
}

async fn post(app: &Router, ops: &[Op]) {
    let body: Vec<Value> = ops.iter().map(wire).collect();
    let request = authorized("POST", "/ops")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    assert_eq!(send(app, request).await.status, StatusCode::OK);
}

#[tokio::test]
async fn requires_a_token() {
    let (app, _) = app().await;

    let reply = send(&app, Request::get("/events").body(Body::empty()).unwrap()).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn streams_the_log_in_seq_order() {
    let (app, store) = app().await;
    let ops = [rename(1), rename(2), rename(3)];
    store.append(UserId::DEFAULT, &ops).await.unwrap();

    let mut events = Events::open(&app, None).await;

    for (seq, op) in (1..).zip(&ops) {
        assert_eq!(events.next().await, event(seq, op));
    }
    events.assert_quiet().await;
}

#[tokio::test]
async fn resumes_after_the_last_event_id() {
    let (app, store) = app().await;
    let ops = [rename(1), rename(2), rename(3)];
    store.append(UserId::DEFAULT, &ops).await.unwrap();

    let mut events = Events::open(&app, Some("2")).await;

    assert_eq!(events.next().await, event(3, &ops[2]));
    events.assert_quiet().await;
}

#[tokio::test]
async fn delivers_ops_posted_while_connected() {
    let (app, _) = app().await;
    let mut events = Events::open(&app, None).await;
    events.assert_quiet().await;

    post(&app, &[rename(1)]).await;

    assert_eq!(events.next().await, event(1, &rename(1)));
}

#[tokio::test]
async fn moves_from_the_backlog_to_live_ops() {
    let (app, store) = app().await;
    store.append(UserId::DEFAULT, &[rename(1)]).await.unwrap();
    let mut events = Events::open(&app, None).await;
    assert_eq!(events.next().await, event(1, &rename(1)));

    post(&app, &[rename(2), rename(3)]).await;

    assert_eq!(events.next().await, event(2, &rename(2)));
    assert_eq!(events.next().await, event(3, &rename(3)));
}

#[tokio::test]
async fn retransmissions_cause_no_events() {
    let (app, _) = app().await;
    post(&app, &[rename(1)]).await;
    let mut events = Events::open(&app, None).await;
    assert_eq!(events.next().await, event(1, &rename(1)));

    post(&app, &[rename(1)]).await;

    events.assert_quiet().await;
}

#[tokio::test]
async fn pages_through_a_long_backlog() {
    let (app, store) = app().await;
    let ops: Vec<Op> = (1..=1_201).map(rename).collect();
    for batch in ops.chunks(400) {
        store.append(UserId::DEFAULT, batch).await.unwrap();
    }

    let mut events = Events::open(&app, None).await;

    for (seq, op) in (1..).zip(&ops) {
        assert_eq!(events.next().await, event(seq, op));
    }
    events.assert_quiet().await;
}

#[tokio::test]
async fn rejects_a_malformed_last_event_id() {
    let (app, _) = app().await;
    let request = authorized("GET", "/events")
        .header("last-event-id", "seven")
        .body(Body::empty())
        .unwrap();

    let reply = send(&app, request).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.error_code(), json!("invalid_last_event_id"));
}
