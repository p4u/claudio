//! Error handling. Every error rendered to a client uses OpenAI's error
//! envelope shape: `{"error": {"message", "type", "param", "code"}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// An error that can be returned to a client as an OpenAI-style error response.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Caller is missing/using a wrong API key.
    #[error("{0}")]
    Unauthorized(String),

    /// The request body was malformed or unusable.
    #[error("{0}")]
    BadRequest(String),

    /// The Claude backend returned an error or unparseable output.
    #[error("{0}")]
    Upstream(String),

    /// The backend invocation exceeded the configured timeout.
    #[error("{0}")]
    Timeout(String),

    /// Something went wrong on our side (spawn failure, IO, etc.).
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The `type` field in OpenAI's error envelope.
    fn error_type(&self) -> &'static str {
        match self {
            AppError::Unauthorized(_) => "invalid_request_error",
            AppError::BadRequest(_) => "invalid_request_error",
            AppError::Upstream(_) => "api_error",
            AppError::Timeout(_) => "api_error",
            AppError::Internal(_) => "api_error",
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    message: String,
    r#type: &'static str,
    param: Option<String>,
    code: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = ErrorEnvelope {
            error: ErrorBody {
                message: self.to_string(),
                r#type: self.error_type(),
                param: None,
                code: None,
            },
        };
        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
