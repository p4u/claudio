//! Optional bearer-token authentication.
//!
//! If `config.api_key` is set, every request must carry a matching
//! `Authorization: Bearer <key>` header. If it is unset, all requests pass.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use super::config::AppState;
use super::error::AppError;

pub async fn require_api_key(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let Some(expected) = state.config.api_key.as_deref() else {
        return Ok(next.run(request).await);
    };

    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    match provided {
        Some(key) if key == expected => Ok(next.run(request).await),
        _ => Err(AppError::Unauthorized(
            "missing or invalid API key".to_string(),
        )),
    }
}
