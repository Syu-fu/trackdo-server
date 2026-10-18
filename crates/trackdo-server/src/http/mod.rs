//! HTTP API. See `docs/design.md` for the contract.

use std::sync::Arc;

use axum::extract::FromRef;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::map_response;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use trackdo_protocol::PROTOCOL_VERSION;

use crate::auth::Tokens;
use crate::notify::Notifier;
use crate::store::Store;

mod error;
mod events;
mod ops;
mod session;
mod snapshot;

pub use self::error::ApiError;
pub use self::session::Session;

/// Carries `PROTOCOL_VERSION` on requests and responses.
pub const PROTOCOL_HEADER: HeaderName = HeaderName::from_static("trackdo-protocol");

#[derive(Debug, Clone)]
pub struct AppState {
    pub tokens: Arc<Tokens>,
    pub store: Store,
    pub notifier: Notifier,
    /// Cancelled on shutdown, which ends open event streams.
    pub closing: CancellationToken,
}

impl FromRef<AppState> for Arc<Tokens> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.tokens)
    }
}

impl FromRef<AppState> for Store {
    fn from_ref(state: &AppState) -> Self {
        state.store.clone()
    }
}

impl FromRef<AppState> for CancellationToken {
    fn from_ref(state: &AppState) -> Self {
        state.closing.clone()
    }
}

impl FromRef<AppState> for Notifier {
    fn from_ref(state: &AppState) -> Self {
        state.notifier.clone()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .merge(events::routes())
        .merge(ops::routes())
        .merge(snapshot::routes())
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
