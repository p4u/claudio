//! The bridge from the OpenAI API surface to the persistent-session pool.
//!
//! Every turn resolves through [`super::pool::SessionPool`] — a pool of
//! long-lived *interactive* `claude` sessions. **claudio never invokes
//! `claude -p`.** The system prompt rides in the typed user prompt (unscored by
//! Anthropic's third-party classifier), and each conversation reuses a session,
//! receiving only the delta. A turn is resolved whole (the TUI has no per-token
//! protocol), so a streaming request gets a small fixed SSE chunk set (see
//! `routes::chat`).

use crate::cli::WrapperEnv;

use super::config::{AppState, Config};
use super::error::{AppError, AppResult};
use super::prompt::FlatPrompt;
use super::types::{Message, Usage};

/// The raw outcome of a backend turn, before OpenAI shaping.
pub struct RawResult {
    pub text: String,
    pub usage: Usage,
    pub stop_reason: Option<String>,
}

/// Resolve one turn through the persistent session pool on a blocking thread.
/// `messages` is the raw request history (used by the pool for continuation
/// matching); `prompt` is the flattened system + full-conversation body.
pub async fn run_raw(
    state: &AppState,
    prompt: &FlatPrompt,
    messages: &[Message],
    model: &str,
    corr: &str,
) -> AppResult<RawResult> {
    let pool = state.pool.clone();
    let env = build_env(&state.config);
    let model = model.to_string();
    let messages = messages.to_vec();
    let system = prompt.system.clone();
    let body = prompt.user.clone();
    let corr = corr.to_string();
    tokio::task::spawn_blocking(move || {
        pool.resolve_turn(&env, &model, &messages, &system, &body, &corr)
    })
    .await
    .map_err(|e| AppError::Internal(format!("backend task panicked: {e}")))?
}

/// Build the driver environment for an API turn: start from `CLAUDIO_*` env, then
/// override the binary path and timeout from the server config.
pub(super) fn build_env(config: &Config) -> WrapperEnv {
    let mut env = WrapperEnv::from_env();
    env.claude_path = config.claude_bin.clone();
    env.timeout_sec = config.timeout_secs;
    env
}

/// Map an OpenAI `model` value to a `--model` argument. Claude aliases
/// (`opus`/`sonnet`/`haiku`) and full `claude-*` names pass through; anything
/// else (e.g. `gpt-4o`) falls back to the configured default.
pub fn map_model(requested: Option<&str>, default_model: &str) -> String {
    match requested {
        Some(m) if is_claude_model(m) => m.to_string(),
        _ => default_model.to_string(),
    }
}

fn is_claude_model(m: &str) -> bool {
    matches!(m, "opus" | "sonnet" | "haiku") || m.starts_with("claude")
}

/// Map a Claude `stop_reason` to an OpenAI `finish_reason`.
pub fn map_finish_reason(stop_reason: Option<&str>) -> String {
    match stop_reason {
        Some("end_turn") | Some("stop_sequence") => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_aliases_pass_through() {
        assert_eq!(map_model(Some("opus"), "sonnet"), "opus");
        assert_eq!(
            map_model(Some("claude-opus-4-8"), "sonnet"),
            "claude-opus-4-8"
        );
    }

    #[test]
    fn non_claude_falls_back_to_default() {
        assert_eq!(map_model(Some("gpt-4o"), "sonnet"), "sonnet");
        assert_eq!(map_model(None, "haiku"), "haiku");
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_finish_reason(Some("end_turn")), "stop");
        assert_eq!(map_finish_reason(Some("max_tokens")), "length");
        assert_eq!(map_finish_reason(Some("tool_use")), "tool_calls");
        assert_eq!(map_finish_reason(None), "stop");
    }
}
