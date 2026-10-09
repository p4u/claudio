//! OpenAI wire types: chat-completion request/response, streaming chunks, models.
//!
//! These are hand-rolled and deliberately *lenient*: requests use
//! `#[serde(default)]` and a catch-all `extra` map so unknown fields from any
//! OpenAI client never cause a deserialization failure. Responses are built to
//! match OpenAI's documented shapes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// Incoming `POST /v1/chat/completions` body. Lenient by design.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    /// Tool/function definitions the client wants the model to be able to call.
    /// Honored only when agentic mode is enabled (see config).
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    /// `tool_choice`: "auto" | "none" | "required" | {"type":"function",...}.
    #[serde(default)]
    pub tool_choice: Option<Value>,
    /// Any other OpenAI fields (temperature, top_p, …) are accepted and ignored
    /// rather than rejected. Captured only to keep deserialization lenient.
    #[serde(flatten)]
    #[allow(dead_code)]
    pub extra: Map<String, Value>,
}

impl ChatRequest {
    pub fn is_stream(&self) -> bool {
        self.stream.unwrap_or(false)
    }

    pub fn include_usage(&self) -> bool {
        self.stream_options
            .as_ref()
            .and_then(|o| o.include_usage)
            .unwrap_or(false)
    }

    /// True when the client supplied a non-empty `tools` array.
    pub fn has_tools(&self) -> bool {
        self.tools.as_ref().is_some_and(|t| !t.is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: Option<bool>,
}

/// A single chat message. `role` is kept as a free string for tolerance.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: String,
    /// Optional: assistant tool-call messages send `content: null`.
    #[serde(default)]
    pub content: Option<Content>,
    /// Tool calls made by a prior assistant turn (echoed back by the client).
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Set on `role: "tool"` messages, linking a result to its call.
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Message text, or empty string when content is absent/null.
    pub fn text(&self) -> String {
        self.content
            .as_ref()
            .map(Content::as_text)
            .unwrap_or_default()
    }
}

/// Message content is either a plain string or an array of typed parts.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl Default for Content {
    fn default() -> Self {
        Content::Text(String::new())
    }
}

impl Content {
    /// Flatten content to plain text, concatenating any text parts. Non-text
    /// parts (e.g. images) are ignored in v1.
    pub fn as_text(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// One element of a structured content array. We only read `text`; `type` is
/// captured for completeness but currently unused.
#[derive(Debug, Clone, Deserialize)]
pub struct ContentPart {
    #[serde(default)]
    #[allow(dead_code)]
    pub r#type: String,
    #[serde(default)]
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// Tools / function calling
// ---------------------------------------------------------------------------

/// A tool the client offers the model. Only `function` tools are supported.
#[derive(Debug, Clone, Deserialize)]
pub struct Tool {
    #[serde(default)]
    #[allow(dead_code)]
    pub r#type: String, // "function"
    pub function: FunctionDef,
}

/// The schema of a callable function.
#[derive(Debug, Clone, Deserialize)]
pub struct FunctionDef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema for the function arguments.
    #[serde(default)]
    pub parameters: Option<Value>,
}

/// A tool call — emitted by us in responses, and echoed back by the client in
/// subsequent assistant history messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_type")]
    pub r#type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments string (OpenAI convention, even for objects).
    pub arguments: String,
}

fn function_type() -> String {
    "function".to_string()
}

// ---------------------------------------------------------------------------
// Non-streaming response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletion {
    pub id: String,
    pub object: &'static str, // "chat.completion"
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ResponseMessage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseMessage {
    pub role: &'static str, // "assistant"
    /// Always present (null on tool-call turns), matching OpenAI.
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

// ---------------------------------------------------------------------------
// Streaming response chunks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str, // "chat.completion.chunk"
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// A tool call inside a streaming delta. Carries an `index` per OpenAI's
/// streaming protocol; we emit each call complete in a single delta.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallDelta {
    pub index: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: &'static str, // "function"
    pub function: FunctionCall,
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Model {
    pub id: String,
    pub object: &'static str, // "model"
    pub created: u64,
    pub owned_by: &'static str, // "anthropic"
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelList {
    pub object: &'static str, // "list"
    pub data: Vec<Model>,
}
