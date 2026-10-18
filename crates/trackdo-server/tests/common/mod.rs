//! Drives the full router in-process.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use trackdo_server::auth::{TokenDigest, Tokens};
use trackdo_server::http::{AppState, router};

pub const TOKEN: &str = "secret";

pub fn app() -> Router {
    router(AppState {
        tokens: Arc::new(Tokens::new(vec![TokenDigest::of(TOKEN)])),
    })
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
