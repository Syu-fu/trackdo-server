//! Error responses: `{"error": {"code": ..., "message": ...}}`.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use trackdo_protocol::PROTOCOL_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    UnsupportedProtocolVersion,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: ErrorCode,
    message: String,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: ErrorCode,
    message: &'a str,
}

impl ApiError {
    fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
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
