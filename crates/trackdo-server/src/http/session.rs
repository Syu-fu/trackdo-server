//! The caller of an authenticated endpoint.

use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use trackdo_protocol::PROTOCOL_VERSION;

use super::{ApiError, PROTOCOL_HEADER};
use crate::auth::Tokens;
use crate::store::UserId;

/// Extracting it checks the bearer token, then the protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub user: UserId,
}

impl<S> FromRequestParts<S> for Session
where
    Arc<Tokens>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let tokens = Arc::<Tokens>::from_ref(state);
        let user = bearer_token(&parts.headers)
            .and_then(|token| tokens.verify(token))
            .ok_or_else(ApiError::unauthorized)?;
        check_protocol_version(&parts.headers)?;
        Ok(Self { user })
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token)
}

fn check_protocol_version(headers: &HeaderMap) -> Result<(), ApiError> {
    let sent = headers
        .get(&PROTOCOL_HEADER)
        .map(|value| value.to_str().unwrap_or("<not text>"));
    if sent.and_then(|version| version.parse::<u32>().ok()) == Some(PROTOCOL_VERSION) {
        Ok(())
    } else {
        Err(ApiError::unsupported_protocol_version(sent))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::Session;
    use crate::auth::{TokenDigest, Tokens};
    use crate::http::{AppState, PROTOCOL_HEADER};

    const TOKEN: &str = "secret";

    fn probe() -> Router {
        let state = AppState {
            tokens: Arc::new(Tokens::new(vec![TokenDigest::of(TOKEN)])),
        };
        Router::new()
            .route(
                "/probe",
                get(|session: Session| async move { session.user.0.to_string() }),
            )
            .with_state(state)
    }

    async fn call(headers: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let request = headers
            .iter()
            .fold(Request::get("/probe"), |request, (name, value)| {
                request.header(*name, *value)
            })
            .body(Body::empty())
            .unwrap();
        let response = probe().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, body.to_vec())
    }

    fn error_code(body: &[u8]) -> Value {
        serde_json::from_slice::<Value>(body).unwrap()["error"]["code"].clone()
    }

    const BEARER: (&str, &str) = ("authorization", "Bearer secret");
    const VERSION: (&str, &str) = ("trackdo-protocol", "2");

    #[tokio::test]
    async fn admits_a_valid_token_and_version() {
        let (status, _, body) = call(&[BEARER, VERSION]).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"1");
    }

    #[tokio::test]
    async fn accepts_the_scheme_in_any_case() {
        let (status, _, _) = call(&[("authorization", "bEaReR secret"), VERSION]).await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_a_missing_token() {
        let (status, headers, body) = call(&[VERSION]).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
        assert_eq!(error_code(&body), json!("unauthorized"));
    }

    #[tokio::test]
    async fn rejects_an_unknown_token() {
        let (status, _, body) = call(&[("authorization", "Bearer guess"), VERSION]).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error_code(&body), json!("unauthorized"));
    }

    #[tokio::test]
    async fn rejects_another_scheme() {
        let (status, _, _) = call(&[("authorization", "Basic secret"), VERSION]).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_a_missing_version() {
        let (status, _, body) = call(&[BEARER]).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), json!("unsupported_protocol_version"));
    }

    #[tokio::test]
    async fn rejects_another_version() {
        let (status, _, body) = call(&[BEARER, ("trackdo-protocol", "1")]).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), json!("unsupported_protocol_version"));
    }

    #[tokio::test]
    async fn checks_the_token_before_the_version() {
        let (status, _, _) = call(&[("trackdo-protocol", "1")]).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn header_name_is_lowercase() {
        assert_eq!(PROTOCOL_HEADER.as_str(), VERSION.0);
    }
}
