//! `GET /snapshot`: the full state, replayed from the op log.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use trackdo_protocol::snapshot::SnapshotDto;

use super::{ApiError, AppState, Session};
use crate::snapshot::replay;
use crate::store::Store;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/snapshot", get(get_snapshot))
}

/// Rebuilt on every request; read in one query so it is consistent.
async fn get_snapshot(
    session: Session,
    State(store): State<Store>,
) -> Result<Json<SnapshotDto>, ApiError> {
    let log = store.ops_after(session.user, None, u32::MAX).await?;
    Ok(Json(SnapshotDto::from(&replay(&log))))
}
