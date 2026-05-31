//! `POST /v1/chat/completions` — the core endpoint.
//!
//! Every turn is resolved through claudio's PTY backend ([`backend::run_raw`]),
//! which has no per-token protocol. So both the plain and agentic paths resolve
//! the full turn first, then shape it:
//! - **chat** (default): flatten messages → resolve → return text (buffered or
//!   as a small SSE chunk set).
//! - **agentic** (when `config.agentic` and the request has `tools`): build the
//!   tool-protocol prompt → resolve → detect a `tool_calls` block → return
//!   either a `tool_calls` response or a final text response.
//!
//! Streaming responses are therefore "resolve then chunk": a role+payload chunk,
//! a finish_reason chunk (with usage if requested), then `[DONE]`.

use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};

use crate::api::backend::{self, map_finish_reason, map_model};
use crate::api::config::AppState;
use crate::api::error::{AppError, AppResult};
use crate::api::types::{
    ChatCompletion, ChatCompletionChunk, ChatRequest, Choice, ChunkChoice, Delta, ResponseMessage,
    ToolCallDelta, Usage,
};
use crate::api::{agentic, prompt, util};

/// An SSE payload item (matches what `axum`'s `Sse` body yields).
type SseItem = Result<Event, std::convert::Infallible>;

pub async fn completions(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> AppResult<Response> {
    if req.messages.is_empty() {
        return Err(AppError::BadRequest(
            "'messages' must not be empty".to_string(),
        ));
    }

    let config = state.config.clone();
    let model = map_model(req.model.as_deref(), &config.default_model);

    // Acquire a concurrency permit; held until this turn resolves. The backend
    // resolves the whole turn up front (no token streaming), so we can drop the
    // permit before emitting the response/SSE body.
    let permit = state
        .permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal("server is shutting down".to_string()))?;

    // Agentic path: honor client tools via prompt-based tool-calling passthrough.
    if config.agentic && req.has_tools() {
        let tools = req.tools.clone().unwrap_or_default();
        let flat = agentic::build_prompt(&req.messages, &tools, req.tool_choice.as_ref());
        let raw = backend::run_raw(&state, &flat, &req.messages, &model).await;
        drop(permit);
        let raw = raw?;
        let parsed = agentic::parse_output(&raw.text);

        if req.is_stream() {
            let events = agentic_sse_events(&model, parsed, raw.usage, req.include_usage());
            let body = futures::stream::iter(events);
            return Ok(Sse::new(body).keep_alive(KeepAlive::default()).into_response());
        } else {
            return Ok(Json(agentic_completion(&model, parsed, raw.usage)).into_response());
        }
    }

    // Plain chat path (tools ignored).
    let flat = prompt::flatten(&req.messages);
    let raw = backend::run_raw(&state, &flat, &req.messages, &model).await;
    drop(permit);
    let raw = raw?;
    let finish_reason = map_finish_reason(raw.stop_reason.as_deref());

    if req.is_stream() {
        let events = text_sse_events(&model, raw.text, finish_reason, raw.usage, req.include_usage());
        let body = futures::stream::iter(events);
        Ok(Sse::new(body).keep_alive(KeepAlive::default()).into_response())
    } else {
        Ok(Json(build_completion(model, raw.text, finish_reason, raw.usage)).into_response())
    }
}

/// Build a non-streaming `ChatCompletion` carrying plain assistant text.
fn build_completion(model: String, content: String, finish_reason: String, usage: Usage) -> ChatCompletion {
    ChatCompletion {
        id: util::completion_id(),
        object: "chat.completion",
        created: util::now_secs(),
        model,
        choices: vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant",
                content: Some(content),
                tool_calls: None,
            },
            finish_reason,
        }],
        usage,
    }
}

/// Build a non-streaming `ChatCompletion` from a parsed agentic turn.
fn agentic_completion(model: &str, parsed: agentic::ParsedOutput, usage: Usage) -> ChatCompletion {
    let (message, finish_reason) = match parsed {
        agentic::ParsedOutput::ToolCalls(calls) => (
            ResponseMessage {
                role: "assistant",
                content: None,
                tool_calls: Some(calls),
            },
            "tool_calls".to_string(),
        ),
        agentic::ParsedOutput::Text(text) => (
            ResponseMessage {
                role: "assistant",
                content: Some(text),
                tool_calls: None,
            },
            "stop".to_string(),
        ),
    };

    ChatCompletion {
        id: util::completion_id(),
        object: "chat.completion",
        created: util::now_secs(),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message,
            finish_reason,
        }],
        usage,
    }
}

/// SSE event sequence for a streamed plain-text turn: one role+content chunk, a
/// finish_reason chunk (with usage if requested), then `[DONE]`.
fn text_sse_events(
    model: &str,
    text: String,
    finish_reason: String,
    usage: Usage,
    include_usage: bool,
) -> Vec<SseItem> {
    let mk = chunk_builder(model);
    let final_usage = if include_usage { Some(usage) } else { None };
    vec![
        mk(
            Delta {
                role: Some("assistant"),
                content: Some(text),
                tool_calls: None,
            },
            None,
            None,
        ),
        mk(Delta::default(), Some(finish_reason), final_usage),
        Ok(Event::default().data("[DONE]")),
    ]
}

/// Build the SSE event sequence for a streamed agentic turn. Because the turn is
/// already fully resolved, we emit it as a small fixed set of chunks rather than
/// token-by-token: a role+payload chunk, a final finish_reason chunk (with usage
/// if requested), then `[DONE]`.
fn agentic_sse_events(
    model: &str,
    parsed: agentic::ParsedOutput,
    usage: Usage,
    include_usage: bool,
) -> Vec<SseItem> {
    let mk = chunk_builder(model);

    let (first_delta, finish_reason) = match parsed {
        agentic::ParsedOutput::ToolCalls(calls) => {
            let tool_calls = calls
                .into_iter()
                .enumerate()
                .map(|(i, c)| ToolCallDelta {
                    index: i as u32,
                    id: c.id,
                    r#type: "function",
                    function: c.function,
                })
                .collect();
            (
                Delta {
                    role: Some("assistant"),
                    content: None,
                    tool_calls: Some(tool_calls),
                },
                "tool_calls".to_string(),
            )
        }
        agentic::ParsedOutput::Text(text) => (
            Delta {
                role: Some("assistant"),
                content: Some(text),
                tool_calls: None,
            },
            "stop".to_string(),
        ),
    };

    let final_usage = if include_usage { Some(usage) } else { None };
    vec![
        mk(first_delta, None, None),
        mk(Delta::default(), Some(finish_reason), final_usage),
        Ok(Event::default().data("[DONE]")),
    ]
}

/// Returns a closure that serializes a chunk into an SSE event, with stable
/// identity fields (`id`, `created`, `model`) shared across the whole stream.
fn chunk_builder(model: &str) -> impl Fn(Delta, Option<String>, Option<Usage>) -> SseItem {
    let id = util::completion_id();
    let created = util::now_secs();
    let model = model.to_string();
    move |delta, finish_reason, usage| {
        let chunk = ChatCompletionChunk {
            id: id.clone(),
            object: "chat.completion.chunk",
            created,
            model: model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage,
        };
        let json = serde_json::to_string(&chunk).unwrap_or_else(|_| "{}".to_string());
        Ok(Event::default().data(json))
    }
}
