//! Flatten an OpenAI `messages[]` array into the inputs the Claude backend
//! takes: a single system prompt (forwarded via `--system-prompt`) and a single
//! rendered conversation string (typed into the TUI as the user prompt).
//!
//! We are deliberately stateless: OpenAI clients resend the full history on
//! every call, so we render that history into one prompt rather than relying on
//! session continuity.

use super::types::Message;

/// Default system prompt when the request carries none. Keeps the backend
/// behaving as a plain assistant instead of the Claude Code coding agent.
const DEFAULT_SYSTEM: &str = "You are a helpful assistant.";

pub struct FlatPrompt {
    /// Goes to `--system-prompt`.
    pub system: String,
    /// Typed into the TUI as the user prompt (the conversation transcript).
    pub user: String,
}

/// Render messages into a system prompt + a single conversation string.
pub fn flatten(messages: &[Message]) -> FlatPrompt {
    let mut system_parts: Vec<String> = Vec::new();
    let mut turns: Vec<String> = Vec::new();

    for msg in messages {
        let text = msg.text();
        match msg.role.as_str() {
            "system" | "developer" => {
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "assistant" => turns.push(format!("Assistant: {text}")),
            "tool" | "function" => turns.push(format!("Tool result: {text}")),
            // "user" and anything unrecognized are treated as user input.
            _ => turns.push(format!("User: {text}")),
        }
    }

    let system = if system_parts.is_empty() {
        DEFAULT_SYSTEM.to_string()
    } else {
        system_parts.join("\n\n")
    };

    FlatPrompt {
        system,
        user: turns.join("\n\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::Content;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: Some(Content::Text(text.to_string())),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn extracts_system_and_renders_turns() {
        let msgs = vec![
            msg("system", "Be terse."),
            msg("user", "Hello"),
            msg("assistant", "Hi"),
            msg("user", "Bye"),
        ];
        let f = flatten(&msgs);
        assert_eq!(f.system, "Be terse.");
        assert_eq!(f.user, "User: Hello\n\nAssistant: Hi\n\nUser: Bye");
    }

    #[test]
    fn default_system_when_absent() {
        let f = flatten(&[msg("user", "yo")]);
        assert_eq!(f.system, DEFAULT_SYSTEM);
        assert_eq!(f.user, "User: yo");
    }

    #[test]
    fn multiple_system_messages_joined() {
        let f = flatten(&[msg("system", "A"), msg("developer", "B"), msg("user", "x")]);
        assert_eq!(f.system, "A\n\nB");
    }
}
