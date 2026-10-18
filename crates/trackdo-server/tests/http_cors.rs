mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};

use trackdo_server::http::with_cors;

use common::{Reply, app, send};

const APP: &str = "https://app.example";

async fn cors_app(origins: &[&str]) -> Router {
    let origins: Vec<HeaderValue> = origins
        .iter()
        .map(|origin| HeaderValue::from_str(origin).unwrap())
        .collect();
    with_cors(app().await.0, &origins)
}

async fn preflight(app: &Router, origin: &str, path: &str, method: &str, headers: &str) -> Reply {
    let request = Request::options(path)
        .header("origin", origin)
        .header("access-control-request-method", method)
        .header("access-control-request-headers", headers)
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply.headers.get(name).map(|value| value.to_str().unwrap())
}

#[tokio::test]
async fn allows_posting_ops_from_a_configured_origin() {
    let app = cors_app(&[APP]).await;

    let reply = preflight(
        &app,
        APP,
        "/ops",
        "POST",
        "authorization,content-type,trackdo-protocol",
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(header(&reply, "access-control-allow-origin"), Some(APP));
    let allowed = header(&reply, "access-control-allow-headers").unwrap();
    for name in ["authorization", "content-type", "trackdo-protocol"] {
        assert!(allowed.contains(name), "{allowed}");
    }
    assert!(
        header(&reply, "access-control-allow-methods")
            .unwrap()
            .contains("POST")
    );
}

#[tokio::test]
async fn allows_resuming_events_from_a_configured_origin() {
    let app = cors_app(&[APP]).await;

    let reply = preflight(&app, APP, "/events", "GET", "authorization,last-event-id").await;

    assert_eq!(header(&reply, "access-control-allow-origin"), Some(APP));
    assert!(
        header(&reply, "access-control-allow-headers")
            .unwrap()
            .contains("last-event-id")
    );
}

#[tokio::test]
async fn exposes_the_protocol_version_to_scripts() {
    let app = cors_app(&[APP]).await;
    let request = Request::get("/healthz")
        .header("origin", APP)
        .body(Body::empty())
        .unwrap();

    let reply = send(&app, request).await;

    assert_eq!(header(&reply, "access-control-allow-origin"), Some(APP));
    assert_eq!(
        header(&reply, "access-control-expose-headers"),
        Some("trackdo-protocol")
    );
}

#[tokio::test]
async fn ignores_other_origins() {
    let app = cors_app(&[APP]).await;

    let reply = preflight(
        &app,
        "https://evil.example",
        "/ops",
        "POST",
        "authorization",
    )
    .await;

    assert_eq!(header(&reply, "access-control-allow-origin"), None);
}

#[tokio::test]
async fn adds_nothing_without_configured_origins() {
    let app = cors_app(&[]).await;
    let request = Request::get("/healthz")
        .header("origin", APP)
        .body(Body::empty())
        .unwrap();

    let reply = send(&app, request).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(header(&reply, "access-control-allow-origin"), None);
}
