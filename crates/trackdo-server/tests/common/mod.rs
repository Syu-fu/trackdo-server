//! Drives the full router in-process.

// Each test binary uses only some of these helpers.
#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use trackdo_server::auth::{TokenDigest, Tokens};
use trackdo_server::http::{AppState, router};
use trackdo_server::notify::Notifier;
use trackdo_server::store::Store;

pub const TOKEN: &str = "secret";

/// The router over a fresh in-memory store, which tests may inspect.
pub async fn app() -> (Router, Store) {
    app_closed_by(CancellationToken::new()).await
}

/// Like [`app`], with event streams ending when `closing` is cancelled.
pub async fn app_closed_by(closing: CancellationToken) -> (Router, Store) {
    let store = Store::in_memory().await.unwrap();
    let app = router(AppState {
        tokens: Arc::new(Tokens::new(vec![TokenDigest::of(TOKEN)])),
        store: store.clone(),
        notifier: Notifier::default(),
        closing,
    });
    (app, store)
}

/// A request to an authenticated endpoint, with token and protocol version.
pub fn authorized(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("trackdo-protocol", "2")
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    pub fn error_code(&self) -> Value {
        self.json()["error"]["code"].clone()
    }
}

pub async fn send(app: &Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Reply {
        status,
        headers,
        body: body.to_vec(),
    }
}
