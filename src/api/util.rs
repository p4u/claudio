//! Small shared helpers for building responses.

use std::time::{SystemTime, UNIX_EPOCH};

/// Generate an OpenAI-style completion id: `chatcmpl-<uuid-no-dashes>`.
pub fn completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
}

/// Current Unix time in seconds (for the `created` field).
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
