//! Prompt-based OpenAI tool-calling passthrough.
//!
//! The `claude` backend is an agent that executes its own tools, with no way to
//! do a single non-executing model turn. To honor OpenAI's *client-executes*
//! tool protocol we instead instruct Claude (via the system prompt) to emit a
//! parseable ```tool_calls``` block *instead of* executing, then translate that
//! block into OpenAI `tool_calls`. Each turn is stateless: the client resends
//! the full history, which we re-render into a transcript.
//!
//! All functions here are pure and unit-tested.

use serde::Deserialize;
use serde_json::Value;

use super::prompt::FlatPrompt;
use super::types::{FunctionCall, Message, Tool, ToolCall};
use super::util;

/// Fence tag the model must use to request tool calls.
const FENCE: &str = "tool_calls";

/// Build the full prompt for an agentic turn: a system prompt (the client's
/// own system text + our protocol preamble) and a rendered conversation body.
pub fn build_prompt(messages: &[Message], tools: &[Tool], tool_choice: Option<&Value>) -> FlatPrompt {
    let (client_system, body) = render_transcript(messages);
    let preamble = tool_preamble(tools, tool_choice);

    let system = if client_system.is_empty() {
        preamble
    } else {
        format!("{client_system}\n\n{preamble}")
    };

    FlatPrompt { system, user: body }
}

/// The protocol instructions + tool catalog, appended to the system prompt.
pub fn tool_preamble(tools: &[Tool], tool_choice: Option<&Value>) -> String {
    let mut s = String::new();
    s.push_str(
        "You are operating through a tool-calling protocol. You CANNOT execute tools yourself; \
         another system executes them on your behalf.\n\n\
         PROTOCOL (strict):\n\
         - To call one or more tools, respond with ONLY a fenced code block tagged `tool_calls` \
         containing a JSON array. Each element is an object {\"name\": <tool name>, \"arguments\": \
         <object of arguments>}. Put multiple objects in the array to call several tools at once.\n\
         - Output NOTHING else in a tool-calling turn: no prose before or after the block, and \
         never claim a task is done in the same turn you call tools (the tools have not run yet).\n\
         - When you have the final answer and need no tool, respond with normal prose and DO NOT \
         emit a tool_calls block.\n\n\
         Example tool-calling turn:\n\
         ```tool_calls\n\
         [{\"name\": \"read_file\", \"arguments\": {\"path\": \"main.py\"}}]\n\
         ```\n\n",
    );

    if let Some(instr) = choice_instruction(tool_choice) {
        s.push_str(&instr);
        s.push_str("\n\n");
    }

    s.push_str("Available tools:\n");
    for tool in tools {
        let f = &tool.function;
        if f.name.is_empty() {
            continue;
        }
        let params = f
            .parameters
            .as_ref()
            .map(|p| serde_json::to_string(p).unwrap_or_else(|_| "{}".to_string()))
            .unwrap_or_else(|| "{}".to_string());
        s.push_str("\n- ");
        s.push_str(&f.name);
        if let Some(desc) = &f.description {
            if !desc.is_empty() {
                s.push_str(": ");
                s.push_str(desc);
            }
        }
        s.push_str("\n  parameters (JSON Schema): ");
        s.push_str(&params);
    }
    s
}

/// Translate `tool_choice` into an extra instruction, if any.
fn choice_instruction(tool_choice: Option<&Value>) -> Option<String> {
    match tool_choice {
        Some(Value::String(s)) => match s.as_str() {
            "none" => Some("For this turn you MUST NOT call any tool; respond only with prose.".into()),
            "required" => Some("For this turn you MUST call at least one tool.".into()),
            _ => None, // "auto" or unknown → default behavior
        },
        Some(Value::Object(o)) => {
            // {"type":"function","function":{"name":"X"}}
            let name = o
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str());
            name.map(|n| format!("For this turn you MUST call the tool named `{n}`."))
        }
        _ => None,
    }
}

/// Render messages into (client_system_text, conversation_body). Assistant
/// tool-call turns and tool results are rendered so Claude can continue the loop.
pub fn render_transcript(messages: &[Message]) -> (String, String) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut turns: Vec<String> = Vec::new();
    // Map tool_call_id -> tool name, to label tool results readably.
    let mut call_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut last_was_tool_result = false;

    for msg in messages {
        let text = msg.text();
        match msg.role.as_str() {
            "system" | "developer" => {
                if !text.is_empty() {
                    system_parts.push(text);
                }
                last_was_tool_result = false;
            }
            "assistant" => {
                if let Some(calls) = &msg.tool_calls {
                    for c in calls {
                        call_names.insert(c.id.clone(), c.function.name.clone());
                    }
                    let rendered: Vec<Value> = calls
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "name": c.function.name,
                                "arguments": parse_args(&c.function.arguments),
                            })
                        })
                        .collect();
                    let json = serde_json::to_string(&rendered).unwrap_or_else(|_| "[]".to_string());
                    let mut block = String::new();
                    if !text.is_empty() {
                        block.push_str(&format!("Assistant: {text}\n"));
                    }
                    block.push_str(&format!("Assistant (tool calls):\n{json}"));
                    turns.push(block);
                } else {
                    turns.push(format!("Assistant: {text}"));
                }
                last_was_tool_result = false;
            }
            "tool" | "function" => {
                let label = msg
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| call_names.get(id))
                    .cloned()
                    .or_else(|| msg.tool_call_id.clone())
                    .unwrap_or_else(|| "tool".to_string());
                turns.push(format!("Tool result [{label}]: {text}"));
                last_was_tool_result = true;
            }
            _ => {
                turns.push(format!("User: {text}"));
                last_was_tool_result = false;
            }
        }
    }

    let mut body = turns.join("\n\n");
    // Nudge continuation when the conversation ends on a tool result.
    if last_was_tool_result {
        body.push_str("\n\nContinue.");
    }

    (system_parts.join("\n\n"), body)
}

/// Result of interpreting the model's output for an agentic turn.
pub enum ParsedOutput {
    /// The model requested tool calls.
    ToolCalls(Vec<ToolCall>),
    /// The model produced a final answer.
    Text(String),
}

/// Interpret the model's raw text: extract a ```tool_calls``` block into OpenAI
/// tool calls, otherwise treat the whole output as a final text answer.
pub fn parse_output(text: &str) -> ParsedOutput {
    let Some(inner) = extract_fenced(text, FENCE) else {
        return ParsedOutput::Text(text.trim().to_string());
    };

    let Ok(raw_calls) = serde_json::from_str::<Vec<RawCall>>(inner.trim()) else {
        // A block was present but unparseable — surface the text rather than
        // breaking the client's loop.
        return ParsedOutput::Text(text.trim().to_string());
    };

    let calls: Vec<ToolCall> = raw_calls
        .into_iter()
        .filter(|c| !c.name.is_empty())
        .map(|c| ToolCall {
            id: format!("call_{}", util::completion_id().trim_start_matches("chatcmpl-")),
            r#type: "function".to_string(),
            function: FunctionCall {
                // OpenAI wants `arguments` as a JSON-encoded string.
                arguments: serde_json::to_string(&c.arguments).unwrap_or_else(|_| "{}".to_string()),
                name: c.name,
            },
        })
        .collect();

    if calls.is_empty() {
        ParsedOutput::Text(text.trim().to_string())
    } else {
        ParsedOutput::ToolCalls(calls)
    }
}

#[derive(Deserialize)]
struct RawCall {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// If `arguments` came in as a JSON-encoded string, decode it; otherwise pass
/// the value through. Used when re-rendering assistant calls in the transcript.
fn parse_args(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| Value::String(arguments.to_string()))
}

/// Extract the contents of the first ```<tag> … ``` fenced block.
fn extract_fenced<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("```{tag}");
    let start = text.find(&open)? + open.len();
    // Skip to end of the opening fence line.
    let after = &text[start..];
    let body_start = after.find('\n').map(|i| start + i + 1)?;
    let rest = &text[body_start..];
    let end = rest.find("```")?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{Content, FunctionDef};

    fn tool(name: &str, desc: &str) -> Tool {
        Tool {
            r#type: "function".to_string(),
            function: FunctionDef {
                name: name.to_string(),
                description: Some(desc.to_string()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                })),
            },
        }
    }

    fn user(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: Some(Content::Text(text.to_string())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn preamble_lists_tools_and_protocol() {
        let p = tool_preamble(&[tool("read_file", "Read a file")], None);
        assert!(p.contains("```tool_calls"));
        assert!(p.contains("read_file"));
        assert!(p.contains("Read a file"));
        assert!(p.contains("JSON Schema"));
    }

    #[test]
    fn choice_required_and_named() {
        let req = tool_preamble(&[tool("a", "")], Some(&Value::String("required".into())));
        assert!(req.contains("MUST call at least one tool"));
        let named = tool_preamble(
            &[tool("a", "")],
            Some(&serde_json::json!({"type": "function", "function": {"name": "a"}})),
        );
        assert!(named.contains("MUST call the tool named `a`"));
    }

    #[test]
    fn parse_single_tool_call() {
        let out = parse_output("```tool_calls\n[{\"name\":\"read_file\",\"arguments\":{\"path\":\"x.py\"}}]\n```");
        match out {
            ParsedOutput::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].function.name, "read_file");
                assert_eq!(calls[0].function.arguments, "{\"path\":\"x.py\"}");
                assert!(calls[0].id.starts_with("call_"));
            }
            ParsedOutput::Text(_) => panic!("expected tool calls"),
        }
    }

    #[test]
    fn parse_parallel_calls_and_ignores_trailing_prose() {
        let text = "```tool_calls\n[{\"name\":\"a\",\"arguments\":{}},{\"name\":\"b\",\"arguments\":{\"k\":1}}]\n```\nDone!";
        match parse_output(text) {
            ParsedOutput::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[1].function.name, "b");
            }
            ParsedOutput::Text(_) => panic!("expected tool calls"),
        }
    }

    #[test]
    fn no_block_is_text() {
        match parse_output("The port is 8080.") {
            ParsedOutput::Text(t) => assert_eq!(t, "The port is 8080."),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn malformed_block_falls_back_to_text() {
        let text = "```tool_calls\nnot json\n```";
        assert!(matches!(parse_output(text), ParsedOutput::Text(_)));
    }

    #[test]
    fn transcript_renders_calls_and_results() {
        let messages = vec![
            user("Read x.py"),
            Message {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "read_file".to_string(),
                        arguments: "{\"path\":\"x.py\"}".to_string(),
                    },
                }]),
                tool_call_id: None,
            },
            Message {
                role: "tool".to_string(),
                content: Some(Content::Text("print('hi')".to_string())),
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
            },
        ];
        let (system, body) = render_transcript(&messages);
        assert!(system.is_empty());
        assert!(body.contains("User: Read x.py"));
        assert!(body.contains("Assistant (tool calls):"));
        assert!(body.contains("\"name\":\"read_file\""));
        // Tool result labeled by resolved tool name, and continuation nudge added.
        assert!(body.contains("Tool result [read_file]: print('hi')"));
        assert!(body.trim_end().ends_with("Continue."));
    }
}
