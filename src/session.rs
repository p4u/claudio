//! Reading the canonical Claude Code session JSONL — the lossless source of
//! truth for the final assistant message and real token usage. The terminal
//! surface is a lossy rendering target (cursor redraws, spinners, wide glyphs);
//! the JSONL is exact.

use std::time::Duration;

const NON_TERMINAL_STOP_REASONS: &[&str] = &["tool_use", "pause_turn"];

#[derive(Debug, Clone)]
pub struct Summary {
    /// Final assistant message text (concatenated text blocks).
    pub final_text: String,
    pub session_id: String,
    pub model: Option<String>,
    /// Real usage object from the assistant message (passed through verbatim).
    pub usage: Option<serde_json::Value>,
    /// Number of assistant turns observed in the transcript.
    pub num_turns: u32,
    pub is_error: bool,
}

fn extract_text(content: &serde_json::Value) -> String {
    let mut text = String::new();
    if let Some(blocks) = content.as_array() {
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
        }
    } else if let Some(s) = content.as_str() {
        text.push_str(s);
    }
    text
}

fn is_terminal(message: &serde_json::Value) -> bool {
    match message.get("stop_reason").and_then(|s| s.as_str()) {
        None => false,
        Some(reason) => !NON_TERMINAL_STOP_REASONS.contains(&reason),
    }
}

/// Parse a transcript file. Returns the latest assistant message that has
/// non-empty text, preferring one with a terminal stop_reason. `require_terminal`
/// rejects tool-use/pause turns entirely (used while polling for completion).
pub fn parse_transcript(path: &str, require_terminal: bool) -> Option<Summary> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut session_id = String::new();
    let mut num_turns: u32 = 0;
    let mut best: Option<Summary> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if session_id.is_empty() {
            if let Some(s) = v.get("sessionId").and_then(|x| x.as_str()) {
                session_id = s.to_string();
            } else if let Some(s) = v.get("session_id").and_then(|x| x.as_str()) {
                session_id = s.to_string();
            }
        }
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(message) = v.get("message") else { continue };
        num_turns += 1;
        let terminal = is_terminal(message);
        if require_terminal && !terminal {
            continue;
        }
        let Some(content) = message.get("content") else { continue };
        let text = extract_text(content);
        if text.trim().is_empty() {
            continue;
        }
        best = Some(Summary {
            final_text: text.trim().to_string(),
            session_id: session_id.clone(),
            model: message.get("model").and_then(|m| m.as_str()).map(String::from),
            usage: message.get("usage").cloned(),
            num_turns,
            is_error: false,
        });
    }

    best.map(|mut s| {
        s.num_turns = num_turns;
        if s.session_id.is_empty() {
            s.session_id = session_id;
        }
        s
    })
}

/// Read the transcript with retry to absorb the flush race: the Stop hook can
/// fire a few ms before claude writes the final assistant line.
pub fn read_with_retry(path: &str, attempts: u32, backoff: Duration) -> Option<Summary> {
    for _ in 0..attempts {
        if let Some(s) = parse_transcript(path, true) {
            return Some(s);
        }
        std::thread::sleep(backoff);
    }
    // Last resort: accept a non-terminal message if that's all there is.
    parse_transcript(path, false)
}

/// Pull a string field out of a hook payload JSON.
pub fn payload_field(payload: &str, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    v.get(field).and_then(|x| x.as_str()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, content: &str) -> String {
        let path = std::env::temp_dir().join(format!("claudio-test-{name}.jsonl"));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn picks_last_terminal_text() {
        let jsonl = concat!(
            r#"{"type":"user","sessionId":"sid-1"}"#, "\n",
            r#"{"type":"assistant","message":{"model":"opus","content":[{"type":"tool_use"}],"stop_reason":"tool_use"}}"#, "\n",
            r#"{"type":"assistant","message":{"model":"opus","content":[{"type":"text","text":"final answer"}],"stop_reason":"end_turn","usage":{"output_tokens":3}}}"#, "\n",
        );
        let path = write_tmp("terminal", jsonl);
        let s = parse_transcript(&path, true).unwrap();
        assert_eq!(s.final_text, "final answer");
        assert_eq!(s.session_id, "sid-1");
        assert_eq!(s.model.as_deref(), Some("opus"));
        assert_eq!(s.usage.unwrap()["output_tokens"], 3);
    }

    #[test]
    fn require_terminal_skips_tool_use() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"thinking out loud"}],"stop_reason":"tool_use"}}"#, "\n",
        );
        let path = write_tmp("toolonly", jsonl);
        assert!(parse_transcript(&path, true).is_none());
        assert_eq!(parse_transcript(&path, false).unwrap().final_text, "thinking out loud");
    }

    #[test]
    fn payload_field_extracts() {
        let p = r#"{"transcript_path":"/a/b.jsonl","last_assistant_message":"OK"}"#;
        assert_eq!(payload_field(p, "transcript_path").as_deref(), Some("/a/b.jsonl"));
        assert_eq!(payload_field(p, "last_assistant_message").as_deref(), Some("OK"));
        assert_eq!(payload_field(p, "missing"), None);
    }

    #[test]
    fn empty_file_returns_none() {
        let path = write_tmp("empty", "");
        assert!(parse_transcript(&path, true).is_none());
        assert!(parse_transcript(&path, false).is_none());
    }

    #[test]
    fn malformed_json_lines_are_skipped() {
        let jsonl = concat!(
            "not json at all\n",
            r#"{"type":"user","sessionId":"s1"}"#, "\n",
            "{ broken json }\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("malformed", jsonl);
        let s = parse_transcript(&path, true).unwrap();
        assert_eq!(s.final_text, "ok");
    }

    #[test]
    fn session_id_camelcase_extracted() {
        let jsonl = concat!(
            r#"{"type":"user","sessionId":"camel-sid"}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("camelcase", jsonl);
        assert_eq!(parse_transcript(&path, true).unwrap().session_id, "camel-sid");
    }

    #[test]
    fn session_id_snake_case_fallback() {
        let jsonl = concat!(
            r#"{"type":"user","session_id":"snake-sid"}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("snakecase", jsonl);
        assert_eq!(parse_transcript(&path, true).unwrap().session_id, "snake-sid");
    }

    #[test]
    fn num_turns_counts_all_assistant_lines() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"t1"}],"stop_reason":"tool_use"}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"t2"}],"stop_reason":"tool_use"}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"final"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("numturns", jsonl);
        let s = parse_transcript(&path, true).unwrap();
        assert_eq!(s.num_turns, 3);
        assert_eq!(s.final_text, "final");
    }

    #[test]
    fn content_as_plain_string_extracted() {
        // Some older Claude transcript versions use a plain string for content.
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":"plain string answer","stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("contentstr", jsonl);
        let s = parse_transcript(&path, true).unwrap();
        assert_eq!(s.final_text, "plain string answer");
    }

    #[test]
    fn multi_content_blocks_concatenated() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello "},{"type":"text","text":"world"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("multiblocks", jsonl);
        assert_eq!(parse_transcript(&path, true).unwrap().final_text, "hello world");
    }

    #[test]
    fn non_text_blocks_skipped() {
        // tool_use blocks have no "text" field — they must not crash or appear.
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x"},{"type":"text","text":"answer"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("nontext", jsonl);
        assert_eq!(parse_transcript(&path, true).unwrap().final_text, "answer");
    }

    #[test]
    fn payload_field_invalid_json_returns_none() {
        assert_eq!(payload_field("{bad json}", "field"), None);
        assert_eq!(payload_field("", "field"), None);
    }

    #[test]
    fn payload_field_non_object_returns_none() {
        assert_eq!(payload_field("[1,2,3]", "field"), None);
    }

    #[test]
    fn read_with_retry_finds_content_immediately() {
        let jsonl = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"retry-ok"}],"stop_reason":"end_turn"}}"#, "\n",
        );
        let path = write_tmp("retry", jsonl);
        let s = read_with_retry(&path, 3, std::time::Duration::from_millis(1)).unwrap();
        assert_eq!(s.final_text, "retry-ok");
    }
}
