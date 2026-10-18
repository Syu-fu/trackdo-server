//! `POST /ops`: validate, order and store a batch of operations.

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use trackdo_core::op::{Op, OpId};
use trackdo_protocol::op::OpDto;
use uuid::Uuid;

use super::{ApiError, AppState, Session};
use crate::store::Store;

/// Most ops accepted in one request.
const MAX_OPS: usize = 500;
/// Largest accepted request body.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/ops", post(post_ops))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

#[derive(Serialize)]
struct Accepted {
    /// Every op of the request, retransmitted ones included.
    accepted: Vec<Uuid>,
}

/// Stores the batch atomically: one invalid op rejects all of them.
async fn post_ops(
    session: Session,
    State(store): State<Store>,
    body: Result<Json<Vec<OpDto>>, JsonRejection>,
) -> Result<Json<Accepted>, ApiError> {
    let Json(dtos) = body.map_err(rejected_body)?;
    if dtos.len() > MAX_OPS {
        return Err(ApiError::too_large(format!(
            "at most {MAX_OPS} ops are accepted per request, got {}",
            dtos.len()
        )));
    }
    let ops = dtos
        .into_iter()
        .map(decode)
        .collect::<Result<Vec<_>, _>>()?;
    let accepted = store.append(session.user, &ops).await?;
    Ok(Json(Accepted {
        accepted: accepted
            .into_iter()
            .map(|OpId(id)| Uuid::from_u128(id))
            .collect(),
    }))
}

fn decode(dto: OpDto) -> Result<Op, ApiError> {
    let op_id = dto.op_id;
    Op::try_from(dto).map_err(|error| ApiError::invalid_op(op_id, &error))
}

fn rejected_body(rejection: JsonRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::too_large(rejection.body_text())
    } else {
        ApiError::invalid_json(rejection.body_text())
    }
}
