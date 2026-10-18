//! Lets the web client call the API from its own origin.

use axum::Router;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::{LAST_EVENT_ID, PROTOCOL_HEADER};

/// Adds CORS for `origins`; without any, browsers keep the same-origin rule.
///
/// Tokens travel in `Authorization`, not cookies, so credentials stay off.
pub fn with_cors(router: Router, origins: &[HeaderValue]) -> Router {
    if origins.is_empty() {
        return router;
    }
    router.layer(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins.iter().cloned()))
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([AUTHORIZATION, CONTENT_TYPE, PROTOCOL_HEADER, LAST_EVENT_ID])
            .expose_headers([PROTOCOL_HEADER]),
    )
}
