//! `GET /v1/models` and `GET /v1/models/{id}` — a static, curated list of the
//! Claude models reachable through the backend.

use axum::Json;
use axum::extract::Path;
use axum::response::IntoResponse;

use crate::api::error::AppError;
use crate::api::types::{Model, ModelList};
use crate::api::util::now_secs;

const MODEL_IDS: &[&str] = &[
    "opus",
    "sonnet",
    "haiku",
    "claude-opus-4-8",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
];

fn model(id: &str, created: u64) -> Model {
    Model {
        id: id.to_string(),
        object: "model",
        created,
        owned_by: "anthropic",
    }
}

pub async fn list() -> impl IntoResponse {
    let created = now_secs();
    let data = MODEL_IDS.iter().map(|id| model(id, created)).collect();
    Json(ModelList {
        object: "list",
        data,
    })
}

pub async fn retrieve(Path(id): Path<String>) -> Result<impl IntoResponse, AppError> {
    if MODEL_IDS.contains(&id.as_str()) || id.starts_with("claude") {
        Ok(Json(model(&id, now_secs())))
    } else {
        Err(AppError::BadRequest(format!("model '{id}' not found")))
    }
}
