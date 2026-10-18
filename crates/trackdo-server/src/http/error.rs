//! Error responses: `{"error": {"code": ..., "message": ...}}`.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use trackdo_protocol::PROTOCOL_VERSION;
use trackdo_protocol::error::ProtocolError;
use uuid::Uuid;

use crate::store::StoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidJson,
    InvalidLastEventId,
    UnsupportedProtocolVersion,
    Unauthorized,
    TaskMismatch,
    ResyncRequired,
    TooLarge,
    InvalidOp,
    Internal,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: ErrorCode,
    message: String,
    /// The operation the error is about, if any.
    op_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: ErrorCode,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    op_id: Option<Uuid>,
}

impl ApiError {
    fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            op_id: None,
        }
    }

    fn about(self, op_id: Uuid) -> Self {
        Self {
            op_id: Some(op_id),
            ..self
        }
    }

    pub fn invalid_json(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidJson, message)
    }

    pub fn invalid_last_event_id() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidLastEventId,
            "Last-Event-ID must be `<log_id>:<seq>`",
        )
    }

    pub fn resync_required(message: impl Into<String>) -> Self {
        Self::new(StatusCode::GONE, ErrorCode::ResyncRequired, message)
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, ErrorCode::TooLarge, message)
    }

    pub fn invalid_op(op_id: Uuid, error: &ProtocolError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::InvalidOp,
            error.to_string(),
        )
        .about(op_id)
    }

    /// Details stay in the server log.
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "internal error",
        )
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "a valid bearer token is required",
        )
    }

    pub fn unsupported_protocol_version(sent: Option<&str>) -> Self {
        let sent = sent.map_or_else(|| "none".to_owned(), |version| format!("{version:?}"));
        Self::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::UnsupportedProtocolVersion,
            format!("protocol version {PROTOCOL_VERSION} is required, got {sent}"),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: &self.message,
                op_id: self.op_id,
            },
        });
        let mut response = (self.status, body).into_response();
        if self.code == ErrorCode::Unauthorized {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::TaskMismatch {
                op_id,
                entry,
                expected,
                actual,
            } => Self::new(
                StatusCode::CONFLICT,
                ErrorCode::TaskMismatch,
                format!(
                    "time entry {} belongs to task {}, not {}",
                    Uuid::from_u128(entry.0),
                    Uuid::from_u128(expected.0),
                    Uuid::from_u128(actual.0),
                ),
            )
            .about(Uuid::from_u128(op_id.0)),
            other => {
                tracing::error!(error = %other, "store failed");
                Self::internal()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::{Value, json};

    use super::*;

    async fn render(error: ApiError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn store_failures_are_internal_without_details() {
        let error = StoreError::Corrupt(42);

        let (status, body) = render(ApiError::from(error)).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({ "error": { "code": "internal", "message": "internal error" } })
        );
    }
}
