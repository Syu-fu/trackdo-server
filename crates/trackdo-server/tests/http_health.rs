mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;

use common::{app, send};

#[tokio::test]
async fn healthz_needs_no_token_and_reports_the_protocol() {
    let reply = send(
        &app(),
        Request::get("/healthz").body(Body::empty()).unwrap(),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({ "status": "ok", "protocol_version": 2 })
    );
}

#[tokio::test]
async fn every_response_names_the_protocol_version() {
    let app = app();

    let found = send(&app, Request::get("/healthz").body(Body::empty()).unwrap()).await;
    let missing = send(&app, Request::get("/nowhere").body(Body::empty()).unwrap()).await;

    assert_eq!(found.headers["trackdo-protocol"], "2");
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.headers["trackdo-protocol"], "2");
}
