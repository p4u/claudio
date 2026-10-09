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

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::backend::{self, map_finish_reason, map_model};
use crate::api::config::AppState;
use crate::api::error::{AppError, AppResult};
use crate::api::types::{
    ChatCompletion, ChatCompletionChunk, ChatRequest, Choice, ChunkChoice, Delta, Message,
    ResponseMessage, Tool, ToolCallDelta, Usage,
};
use crate::api::{agentic, prompt, util};
use crate::msglog;

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

    // Correlation id ties this turn's four message-flow hops together (no-op
    // unless --log-messages / --log-messages-file is active).
    let corr = if msglog::enabled() {
        let c = msglog::new_corr();
        log_client_request(&c, &req, &model);
        c
    } else {
        String::new()
    };

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
        let raw = backend::run_raw(&state, &flat, &req.messages, &model, &corr).await;
        drop(permit);
        let raw = raw?;
        if msglog::enabled() {
            log_claude_reply(&corr, &raw);
        }
        let parsed = agentic::parse_output(&raw.text);
        if msglog::enabled() {
            log_agentic_response(&corr, &parsed);
        }

        if req.is_stream() {
            let events = agentic_sse_events(&model, parsed, raw.usage, req.include_usage());
            let body = futures::stream::iter(events);
            return Ok(Sse::new(body)
                .keep_alive(KeepAlive::default())
                .into_response());
        } else {
            return Ok(Json(agentic_completion(&model, parsed, raw.usage)).into_response());
        }
    }

    // Plain chat path (tools ignored).
    let flat = prompt::flatten(&req.messages);
    let raw = backend::run_raw(&state, &flat, &req.messages, &model, &corr).await;
    drop(permit);
    let raw = raw?;
    let finish_reason = map_finish_reason(raw.stop_reason.as_deref());
    if msglog::enabled() {
        log_claude_reply(&corr, &raw);
        msglog::record(
            msglog::Dir::ClaudioToCli,
            &corr,
            &format!("response · finish={finish_reason}"),
            &raw.text,
        );
    }

    if req.is_stream() {
        let events = text_sse_events(
            &model,
            raw.text,
            finish_reason,
            raw.usage,
            req.include_usage(),
        );
        let body = futures::stream::iter(events);
        Ok(Sse::new(body)
            .keep_alive(KeepAlive::default())
            .into_response())
    } else {
        Ok(Json(build_completion(model, raw.text, finish_reason, raw.usage)).into_response())
    }
}

/// Build a non-streaming `ChatCompletion` carrying plain assistant text.
fn build_completion(
    model: String,
    content: String,
    finish_reason: String,
    usage: Usage,
) -> ChatCompletion {
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

/// Log the incoming client request (`CLI → claudio`): a one-line summary plus
/// the full rendered conversation as the body (untruncated in the file sink).
fn log_client_request(corr: &str, req: &ChatRequest, model: &str) {
    let tools = req.tools.as_deref().unwrap_or(&[]);
    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t: &Tool| t.function.name.as_str())
        .collect();
    let head = format!(
        "request · model={model} · msgs={} · tools=[{}]{}",
        req.messages.len(),
        tool_names.join(", "),
        if req.is_stream() { " · stream" } else { "" },
    );
    msglog::record(
        msglog::Dir::CliToClaudio,
        corr,
        &head,
        &render_messages(&req.messages),
    );
}

/// Render an OpenAI message array into a readable transcript for the log body.
fn render_messages(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| {
            let text = m.text();
            match (m.role.as_str(), &m.tool_calls) {
                (_, Some(calls)) => {
                    let calls: Vec<String> = calls
                        .iter()
                        .map(|c| format!("{}({})", c.function.name, c.function.arguments))
                        .collect();
                    format!("[{}] {}{}", m.role, text, calls.join(" "))
                }
                ("tool", _) => {
                    format!("[tool:{}] {text}", m.tool_call_id.as_deref().unwrap_or("?"))
                }
                _ => format!("[{}] {text}", m.role),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Log claude's raw reply (`claude → claudio`) before any agentic parsing.
fn log_claude_reply(corr: &str, raw: &backend::RawResult) {
    let head = format!(
        "raw reply · stop={} · usage in={} out={}",
        raw.stop_reason.as_deref().unwrap_or("-"),
        raw.usage.prompt_tokens,
        raw.usage.completion_tokens,
    );
    msglog::record(msglog::Dir::ClaudeToClaudio, corr, &head, &raw.text);
}

/// Log the response claudio returns to the client (`claudio → CLI`) in the
/// agentic path — either the parsed tool calls or the final text.
fn log_agentic_response(corr: &str, parsed: &agentic::ParsedOutput) {
    match parsed {
        agentic::ParsedOutput::ToolCalls(calls) => {
            let body = calls
                .iter()
                .map(|c| format!("{}({})", c.function.name, c.function.arguments))
                .collect::<Vec<_>>()
                .join("\n");
            let head = format!("response · tool_calls ({})", calls.len());
            msglog::record(msglog::Dir::ClaudioToCli, corr, &head, &body);
        }
        agentic::ParsedOutput::Text(text) => {
            msglog::record(
                msglog::Dir::ClaudioToCli,
                corr,
                "response · final text",
                text,
            );
        }
    }
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
