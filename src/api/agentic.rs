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

/// Build the prompt for an agentic turn.
///
/// We never forward the client's own system prompt verbatim: it describes a
/// different agent harness ("you are operating inside pi…") and makes Claude
/// treat the whole request as a prompt-injection attempt (the smart models
/// refuse it and revert to being Claude Code). Instead we *read* it for
/// environment facts (which CLI, its working directory, the date — see
/// [`ClientEnv`]) and present our *own* honest "tool-execution gateway" framing,
/// translating the client's `tools[]` into a clean, hand-written catalog (see
/// [`tool_help`]). The conversation (user/assistant/tool turns) is rendered
/// as-is.
pub fn build_prompt(
    messages: &[Message],
    tools: &[Tool],
    tool_choice: Option<&Value>,
) -> FlatPrompt {
    let (client_system, body) = render_transcript(messages);
    let env = ClientEnv::detect(&client_system);
    tracing::info!(
        client = env.client_id,
        workspace = env.workspace.as_deref().unwrap_or("<unknown>"),
        "agentic turn: client detected"
    );
    FlatPrompt {
        system: tool_preamble(tools, tool_choice, &env),
        user: body,
    }
}

/// Environment facts lifted from a client's system prompt. We forward *only*
/// these facts into claudio's own gateway prompt — never the client's prose.
#[derive(Debug, Default, PartialEq)]
pub struct ClientEnv {
    /// A recognized, officially-supported CLI ("pi", "opencode", …) or
    /// "generic" when no fingerprint matched.
    pub client_id: &'static str,
    /// The client's working directory, if its prompt stated one. This is what
    /// grounds the agent's path resolution in the *client's* workspace rather
    /// than claudio's own (possibly remote) cwd.
    pub workspace: Option<String>,
    /// The client's "current date", if stated.
    pub date: Option<String>,
}

/// Fingerprints that identify a supported client from its system-prompt text
/// (matched case-insensitively). Grow this list as new CLIs are supported; an
/// unmatched client still works via the "generic" profile.
const CLIENT_FINGERPRINTS: &[(&str, &[&str])] = &[
    ("pi", &["operating inside pi", "pi, a coding agent harness"]),
    ("opencode", &["you are opencode"]),
    (
        "hermes",
        &[
            "active hermes profile",
            "hermes agent persona",
            "you are a cli ai agent",
        ],
    ),
];

impl ClientEnv {
    /// Identify the client and pull environment facts out of its system prompt.
    pub fn detect(client_system: &str) -> Self {
        let lc = client_system.to_lowercase();
        let client_id = CLIENT_FINGERPRINTS
            .iter()
            .find(|(_, fps)| fps.iter().any(|fp| lc.contains(*fp)))
            .map(|(id, _)| *id)
            .unwrap_or("generic");
        ClientEnv {
            client_id,
            workspace: extract_after(client_system, &["working directory:", "cwd:"]),
            date: extract_after(client_system, &["current date:", "today's date:"]),
        }
    }
}

/// Find the first line containing one of `labels` (case-insensitive) and return
/// the trimmed remainder after the label. Used to lift facts like the working
/// directory out of a client's system prompt. Conservatively limited to ASCII
/// lines (the realistic case for "Current working directory: …" lines), where
/// lowercased byte offsets are valid in the original string.
fn extract_after(text: &str, labels: &[&str]) -> Option<String> {
    for line in text.lines() {
        if !line.is_ascii() {
            continue;
        }
        let hay = line.to_ascii_lowercase();
        for label in labels {
            if let Some(pos) = hay.find(label) {
                let rest = line[pos + label.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// claudio's own tool protocol: an honest gateway framing (Claude requests an
/// action, the *gateway* runs it and returns the result) plus a catalog built
/// from the client's `tools[]`. Honest, non-adversarial wording is the point —
/// it's what stops Opus/Sonnet from rejecting it as an injected foreign harness.
///
/// The "no direct access / may be remote / never use your own filesystem"
/// clause is the actual correctness fix: claudio's backend `claude` runs in its
/// own (possibly remote, default `/tmp`) directory, so any action it ran itself
/// would hit the wrong machine. Forcing every action through the gateway means
/// the *client* executes it in the *user's* workspace. The injected `Workspace`
/// line (from [`ClientEnv::workspace`]) tells the agent where that is.
pub fn tool_preamble(tools: &[Tool], tool_choice: Option<&Value>, env: &ClientEnv) -> String {
    let mut s = String::new();
    s.push_str(
        "You are the model powering a tool-execution gateway. You have no direct access to the \
         user's machine — which may be remote — so you cannot read files, run commands, or change \
         anything yourself. The ONLY way to act is to request an action through the gateway; it \
         runs the action in the user's workspace and returns the result to you in a following \
         message.\n\n\
         Never rely on your own environment, your own filesystem, or any directory you might assume \
         you are in. Do not use any built-in abilities — act exclusively through the actions listed \
         below, and treat the workspace and date given here as the single source of truth.\n\n",
    );

    // Environment facts lifted from the client (never its prose).
    match &env.workspace {
        Some(ws) => s.push_str(&format!("Workspace (current working directory): {ws}\n")),
        None => s.push_str(
            "Workspace: unknown — use paths exactly as the user gives them, and do not assume any \
             absolute root directory.\n",
        ),
    }
    if let Some(date) = &env.date {
        s.push_str(&format!("Today's date: {date}\n"));
    }
    s.push('\n');

    s.push_str(
        "To request one or more actions, reply with ONLY a fenced code block tagged `tool_calls` \
         containing a JSON array of objects of the form {\"name\": <action>, \"arguments\": { … }}. \
         Put several objects in the array to request several actions at once, and write nothing \
         outside the block. Paths may be written relative to the workspace above. Never say an \
         action ran or succeeded until its result is shown back to you. When the task is finished \
         and you need no further action, reply with a normal prose answer (no block).\n\n\
         Example — to list the workspace:\n\
         ```tool_calls\n\
         [{\"name\": \"bash\", \"arguments\": {\"command\": \"ls -la\"}}]\n\
         ```\n\n",
    );

    if let Some(instr) = choice_instruction(tool_choice) {
        s.push_str(&instr);
        s.push_str("\n\n");
    }

    s.push_str("Actions available to you:\n");
    let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for tool in tools {
        let f = &tool.function;
        if f.name.is_empty() {
            continue;
        }
        names.insert(f.name.as_str());
        s.push('\n');
        s.push_str(&tool_help(
            &f.name,
            f.description.as_deref(),
            f.parameters.as_ref(),
        ));
        s.push('\n');
    }

    // Cleaned, client-agnostic usage hints (rewritten in our voice), emitted
    // only for tools the client actually offers so they never reference an
    // action that isn't available.
    let mut hints: Vec<&str> = Vec::new();
    if names.contains("read") {
        hints.push("- Prefer `read` to view files instead of shelling out to `cat`/`sed`.");
    }
    if names.contains("bash") {
        hints.push("- Use `bash` (and any search/list actions such as grep/glob/find) to explore the workspace.");
    }
    if names.contains("edit") {
        hints.push("- For `edit`, target an exact, unique snippet of the current file, keep the replacement minimal, and don't overlap edits.");
    }
    if names.contains("write") {
        hints.push("- Use `write` only for new files or complete rewrites.");
    }
    hints.push("- Be concise, and show file paths clearly when you work with them.");
    s.push_str("\nWorking effectively:\n");
    for h in hints {
        s.push_str(h);
        s.push('\n');
    }
    s
}

/// Render one tool as a compact, accurate catalog entry built from *its own*
/// declared schema.
///
/// We deliberately derive the argument shape from the client's `parameters`
/// rather than hardcoding it per tool name: different clients name the same
/// conceptual tool's arguments differently (pi's `read` takes `path`, opencode's
/// takes `filePath`; pi's `edit` takes `edits[].oldText`, opencode's takes
/// `oldString`/`newString`), so only the client's own schema is authoritative.
/// We keep the client's human description (it's just tool docs, not foreign-agent
/// framing) and turn the JSON Schema into a readable one-liner instead of dumping
/// raw JSON.
fn tool_help(name: &str, description: Option<&str>, parameters: Option<&Value>) -> String {
    let mut out = match description.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("- `{name}` — {d}"),
        None => format!("- `{name}`"),
    };
    let args = parameters.map(arg_summary).unwrap_or_default();
    if args.is_empty() {
        out.push_str("\n  arguments: (none)");
    } else {
        out.push_str(&format!("\n  arguments: {args}"));
    }
    out
}

/// Summarize a JSON-Schema object as `name* (type), name (type), …`, where `*`
/// marks required fields. Authoritative for whichever client sent it.
fn arg_summary(parameters: &Value) -> String {
    let Some(obj) = parameters.as_object() else {
        return String::new();
    };
    let required = string_set(obj.get("required"));
    let Some(props) = obj.get("properties").and_then(|p| p.as_object()) else {
        return String::new();
    };
    props
        .iter()
        .map(|(name, spec)| {
            let marker = if required.contains(name.as_str()) {
                "*"
            } else {
                ""
            };
            format!("{name}{marker} ({})", type_str(spec))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A short type label for a schema node, expanding one level of nesting so
/// shapes like `array of {oldText*, newText*}` stay accurate.
fn type_str(spec: &Value) -> String {
    let t = spec.get("type").and_then(|v| v.as_str()).unwrap_or("any");
    match t {
        "array" => {
            let items = spec.get("items");
            let inner = items
                .and_then(|i| i.get("properties"))
                .and_then(|p| p.as_object());
            if let Some(inner) = inner {
                let req = string_set(items.and_then(|i| i.get("required")));
                let fields: Vec<String> = inner
                    .keys()
                    .map(|k| format!("{k}{}", if req.contains(k.as_str()) { "*" } else { "" }))
                    .collect();
                format!("array of {{{}}}", fields.join(", "))
            } else {
                let it = items
                    .and_then(|i| i.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("any");
                format!("array of {it}")
            }
        }
        "object" => match spec.get("properties").and_then(|p| p.as_object()) {
            Some(p) => format!("{{{}}}", p.keys().cloned().collect::<Vec<_>>().join(", ")),
            None => "object".to_string(),
        },
        other => other.to_string(),
    }
}

/// Collect a schema `required` array into a set of field names.
fn string_set(v: Option<&Value>) -> std::collections::HashSet<&str> {
    v.and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default()
}

/// Translate `tool_choice` into an extra instruction, if any.
fn choice_instruction(tool_choice: Option<&Value>) -> Option<String> {
    match tool_choice {
        Some(Value::String(s)) => match s.as_str() {
            "none" => {
                Some("For this turn you MUST NOT call any tool; respond only with prose.".into())
            }
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
    let mut call_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
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
                    let json =
                        serde_json::to_string(&rendered).unwrap_or_else(|_| "[]".to_string());
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
            id: format!(
                "call_{}",
                util::completion_id().trim_start_matches("chatcmpl-")
            ),
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
        let p = tool_preamble(
            &[tool("read_file", "Read a file")],
            None,
            &ClientEnv::default(),
        );
        assert!(p.contains("```tool_calls"));
        assert!(p.contains("read_file"));
        assert!(p.contains("Read a file"));
        // Arguments are rendered from the tool's own schema, not hardcoded.
        assert!(p.contains("arguments: path* (string)"));
        // The remote-safety clause must always be present.
        assert!(p.contains("no direct access"));
        assert!(p.contains("may be remote"));
    }

    #[test]
    fn detect_opencode_env_block() {
        let sys = "You are opencode, an interactive CLI tool that helps with software.\n\
                   <env>\n  Working directory: /home/p4u/repo\n  Is directory a git repo: no\n  \
                   Today's date: Mon Jun 01 2026\n</env>";
        let env = ClientEnv::detect(sys);
        assert_eq!(env.client_id, "opencode");
        assert_eq!(env.workspace.as_deref(), Some("/home/p4u/repo"));
        assert_eq!(env.date.as_deref(), Some("Mon Jun 01 2026"));
    }

    #[test]
    fn arg_summary_renders_required_types_and_nesting() {
        // opencode-style edit: flat fields with a required marker.
        let oc = serde_json::json!({
            "type": "object",
            "required": ["filePath", "oldString", "newString"],
            "properties": {
                "filePath": {"type": "string"},
                "oldString": {"type": "string"},
                "newString": {"type": "string"},
                "replaceAll": {"type": "boolean"}
            }
        });
        let s = arg_summary(&oc);
        assert!(s.contains("filePath* (string)"), "{s}");
        assert!(s.contains("replaceAll (boolean)"), "{s}");

        // pi-style edit: a nested array of objects, expanded one level.
        let pi = serde_json::json!({
            "type": "object",
            "required": ["path", "edits"],
            "properties": {
                "path": {"type": "string"},
                "edits": {"type": "array", "items": {
                    "type": "object", "required": ["oldText", "newText"],
                    "properties": {"oldText": {"type": "string"}, "newText": {"type": "string"}}
                }}
            }
        });
        let s = arg_summary(&pi);
        assert!(s.contains("path* (string)"), "{s}");
        assert!(s.contains("edits* (array of {"), "{s}");
        assert!(s.contains("oldText*"), "{s}");
    }

    #[test]
    fn choice_required_and_named() {
        let req = tool_preamble(
            &[tool("a", "")],
            Some(&Value::String("required".into())),
            &ClientEnv::default(),
        );
        assert!(req.contains("MUST call at least one tool"));
        let named = tool_preamble(
            &[tool("a", "")],
            Some(&serde_json::json!({"type": "function", "function": {"name": "a"}})),
            &ClientEnv::default(),
        );
        assert!(named.contains("MUST call the tool named `a`"));
    }

    #[test]
    fn detect_pi_and_workspace() {
        let sys =
            "You are an expert coding assistant operating inside pi, a coding agent harness.\n\
                   Current date: 2026-06-01\n\
                   Current working directory: /home/p4u/repo";
        let env = ClientEnv::detect(sys);
        assert_eq!(env.client_id, "pi");
        assert_eq!(env.workspace.as_deref(), Some("/home/p4u/repo"));
        assert_eq!(env.date.as_deref(), Some("2026-06-01"));
    }

    #[test]
    fn detect_generic_without_cwd() {
        let env = ClientEnv::detect("You are a helpful assistant.");
        assert_eq!(env.client_id, "generic");
        assert!(env.workspace.is_none());
    }

    #[test]
    fn detect_hermes_and_workspace() {
        let sys = "# Hermes Agent Persona\n\nYou are a CLI AI Agent.\n\
                   Active Hermes profile: default.\n\
                   User home directory: /home/p4u\n\
                   Current working directory: /home/p4u/repo";
        let env = ClientEnv::detect(sys);
        assert_eq!(env.client_id, "hermes");
        // The home-directory line must not be mistaken for the working dir.
        assert_eq!(env.workspace.as_deref(), Some("/home/p4u/repo"));
    }

    #[test]
    fn preamble_injects_workspace_then_falls_back() {
        let env = ClientEnv {
            client_id: "pi",
            workspace: Some("/home/p4u/repo".to_string()),
            date: None,
        };
        let with_ws = tool_preamble(&[tool("read", "")], None, &env);
        assert!(with_ws.contains("Workspace (current working directory): /home/p4u/repo"));

        let generic = tool_preamble(&[tool("read", "")], None, &ClientEnv::default());
        assert!(generic.contains("use paths exactly as the user gives them"));
    }

    #[test]
    fn usage_hints_gated_on_present_tools() {
        // A client offering only `bash` should not get edit/write hints.
        let only_bash = tool_preamble(&[tool("bash", "")], None, &ClientEnv::default());
        assert!(only_bash.contains("Use `bash`"));
        assert!(!only_bash.contains("`edit`"));
        assert!(!only_bash.contains("Use `write`"));
    }

    #[test]
    fn parse_single_tool_call() {
        let out = parse_output(
            "```tool_calls\n[{\"name\":\"read_file\",\"arguments\":{\"path\":\"x.py\"}}]\n```",
        );
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
