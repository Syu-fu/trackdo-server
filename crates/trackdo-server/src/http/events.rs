//! `GET /events`: server-sent events of stored ops, in seq order.

use std::collections::VecDeque;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures_util::stream;
use futures_util::{Stream, StreamExt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use trackdo_core::op::{Op, Seq};
use trackdo_protocol::op::OpDto;

use super::{ApiError, AppState, LAST_EVENT_ID, Session};
use crate::notify::Notifier;
use crate::store::{Store, UserId};

/// Most ops read from the store at a time.
const PAGE: u32 = 500;
/// Keeps idle streams open through proxies that time out after 60–100 s.
const KEEPALIVE: Duration = Duration::from_secs(15);

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/events", get(get_events))
}

/// Streams every op after `Last-Event-ID` (from the start without it), then
/// each op stored afterwards.
///
/// A position the log cannot resume from is refused before streaming, so the
/// client resyncs from `GET /snapshot` instead of silently missing ops.
async fn get_events(
    session: Session,
    State(store): State<Store>,
    State(notifier): State<Notifier>,
    State(closing): State<CancellationToken>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, ApiError> {
    let after = last_event_id(&headers)?;
    check_resumable(&store, session.user, after).await?;
    let cursor = Cursor {
        store,
        user: session.user,
        after,
        page: VecDeque::new(),
        changes: notifier.subscribe(session.user),
    };
    let events = stream::unfold(cursor, Cursor::next).take_until(closing.cancelled_owned());
    Ok(Sse::new(events).keep_alive(KeepAlive::new().interval(KEEPALIVE).text("keepalive")))
}

fn last_event_id(headers: &HeaderMap) -> Result<Option<Seq>, ApiError> {
    headers
        .get(LAST_EVENT_ID)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|id| id.parse().ok())
                .map(Seq)
                .ok_or_else(ApiError::invalid_last_event_id)
        })
        .transpose()
}

/// Positions ahead of the log mean the server lost ops the client has seen.
async fn check_resumable(store: &Store, user: UserId, after: Option<Seq>) -> Result<(), ApiError> {
    let Some(Seq(after)) = after else {
        return Ok(());
    };
    let head = store.head(user).await?.map_or(0, |Seq(seq)| seq);
    if after <= head {
        Ok(())
    } else {
        Err(ApiError::resync_required(format!(
            "the log ends at seq {head}, before {after}; fetch GET /snapshot"
        )))
    }
}

/// Position in one user's log. The store is the only source of ops; the
/// notifier just says when to look again.
struct Cursor {
    store: Store,
    user: UserId,
    after: Option<Seq>,
    page: VecDeque<(Seq, Op)>,
    changes: watch::Receiver<()>,
}

impl Cursor {
    async fn next(mut self) -> Option<(Result<Event, axum::Error>, Self)> {
        loop {
            if let Some((seq, op)) = self.page.pop_front() {
                self.after = Some(seq);
                return Some((op_event(seq, &op), self));
            }
            // Marking the signal seen before reading means anything stored
            // after the read wakes the wait below.
            self.changes.borrow_and_update();
            match self.store.ops_after(self.user, self.after, PAGE).await {
                Ok(page) if page.is_empty() => self.changes.changed().await.ok()?,
                Ok(page) => self.page = page.into(),
                Err(error) => {
                    tracing::error!(%error, "reading events failed");
                    return None;
                }
            }
        }
    }
}

fn op_event(Seq(seq): Seq, op: &Op) -> Result<Event, axum::Error> {
    Event::default()
        .id(seq.to_string())
        .event("op")
        .json_data(OpDto::from(op))
}
