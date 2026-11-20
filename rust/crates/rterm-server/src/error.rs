use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    #[error("revision conflict")]
    Conflict,
    #[error("payload too large")]
    TooLarge,
    #[error("upstream auth error: {0}")]
    Upstream(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Conflict => (StatusCode::CONFLICT, "conflict"),
            ApiError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            ApiError::Upstream(_) => (StatusCode::BAD_GATEWAY, "upstream_error"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        // Internal and upstream errors can contain SQL, filesystem paths or
        // upstream response bodies. Log them in full, but never send details
        // to the renderer (F-41).
        let message = match &self {
            ApiError::Internal(_) => {
                tracing::error!(error = %self, "internal server error");
                "internal server error".to_string()
            }
            ApiError::Upstream(_) => {
                tracing::error!(error = %self, "upstream auth error");
                "upstream authentication is unavailable".to_string()
            }
            _ => self.to_string(),
        };
        let body = ErrorBody {
            error: code,
            message,
        };
        (status, Json(body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        ApiError::Internal(value.into())
    }
}
