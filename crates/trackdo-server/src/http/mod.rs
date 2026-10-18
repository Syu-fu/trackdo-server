//! HTTP API. See `docs/design.md` for the contract.

use std::sync::Arc;

use axum::extract::FromRef;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::map_response;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use trackdo_protocol::PROTOCOL_VERSION;

use crate::auth::Tokens;

mod error;
mod session;

pub use self::error::ApiError;
pub use self::session::Session;

/// Carries `PROTOCOL_VERSION` on requests and responses.
pub const PROTOCOL_HEADER: HeaderName = HeaderName::from_static("trackdo-protocol");

#[derive(Debug, Clone)]
pub struct AppState {
    pub tokens: Arc<Tokens>,
}

impl FromRef<AppState> for Arc<Tokens> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.tokens)
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .layer(map_response(name_protocol_version))
        .with_state(state)
}

async fn name_protocol_version(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(PROTOCOL_HEADER, HeaderValue::from(PROTOCOL_VERSION));
    response
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    protocol_version: u32,
}

/// Lets apps check the URL and compatibility before asking for a token.
async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        protocol_version: PROTOCOL_VERSION,
    })
}
