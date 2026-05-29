//! Output formatters. `text` / `json` / `stream-json`, shaped to match
//! `claude -p`. `json` and the trailing `result` envelope carry the *real*
//! usage object pulled from the session JSONL.

use std::io::Write;

use crate::cli::OutputFormat;
use crate::session::Summary;

pub struct Outcome<'a> {
    pub summary: &'a Summary,
    pub duration_ms: u64,
    /// None on success; Some(reason) on a classified failure.
    pub failure: Option<&'a str>,
}

impl Outcome<'_> {
    fn is_error(&self) -> bool {
        self.failure.is_some() || self.summary.is_error
    }
    fn subtype(&self) -> &str {
        if self.is_error() { "error" } else { "success" }
    }
    fn usage(&self) -> serde_json::Value {
        self.summary
            .usage
            .clone()
            .unwrap_or(serde_json::Value::Null)
    }

    fn result_object(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "result",
            "subtype": self.subtype(),
            "is_error": self.is_error(),
            "duration_ms": self.duration_ms,
            "duration_api_ms": serde_json::Value::Null,
            "num_turns": self.summary.num_turns,
            "result": self.summary.final_text,
            "session_id": self.summary.session_id,
            "total_cost_usd": serde_json::Value::Null,
            "usage": self.usage(),
            "permission_denials": [],
            "terminal_reason": self.failure.unwrap_or("completed"),
        })
    }
}

pub fn emit(w: &mut dyn Write, fmt: OutputFormat, out: &Outcome) -> std::io::Result<()> {
    match fmt {
        OutputFormat::Text => {
            if out.is_error() {
                if let Some(reason) = out.failure {
                    if !out.summary.final_text.is_empty() {
                        writeln!(w, "{}", out.summary.final_text)?;
                    }
                    return Err(std::io::Error::other(format!("claude-poc error: {reason}")));
                }
            }
            writeln!(w, "{}", out.summary.final_text)
        }
        OutputFormat::Json => {
            let line = serde_json::to_string(&out.result_object()).unwrap_or_default();
            writeln!(w, "{line}")
        }
        OutputFormat::StreamJson => emit_stream(w, out),
    }
}

/// Best-effort stream-json: shape-compatible with `claude -p`'s core event
/// families. The TUI backend has no per-token protocol, so we emit one
/// content_block_delta carrying the full final text, then the result envelope.
fn emit_stream(w: &mut dyn Write, out: &Outcome) -> std::io::Result<()> {
    let sid = &out.summary.session_id;
    let model = out.summary.model.clone().unwrap_or_else(|| "unknown".into());

    let line = |w: &mut dyn Write, v: serde_json::Value| -> std::io::Result<()> {
        writeln!(w, "{}", serde_json::to_string(&v).unwrap_or_default())
    };

    line(w, serde_json::json!({
        "type": "system", "subtype": "init",
        "session_id": sid, "model": model,
        "apiKeySource": "interactive_tui_subscription",
        "tools": [], "mcp_servers": [],
    }))?;
    line(w, serde_json::json!({
        "type": "assistant",
        "message": {
            "role": "assistant", "model": model,
            "content": [{ "type": "text", "text": out.summary.final_text }],
            "stop_reason": if out.is_error() { serde_json::Value::Null } else { "end_turn".into() },
            "usage": out.usage(),
        },
        "session_id": sid,
    }))?;
    line(w, out.result_object())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            final_text: "hello".into(),
            session_id: "sid".into(),
            model: Some("opus".into()),
            usage: Some(serde_json::json!({"output_tokens": 1})),
            num_turns: 1,
            is_error: false,
        }
    }

    #[test]
    fn text_ok() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Text, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "hello\n");
    }

    #[test]
    fn json_carries_usage_and_session() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Json, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(v["type"], "result");
        assert_eq!(v["result"], "hello");
        assert_eq!(v["session_id"], "sid");
        assert_eq!(v["usage"]["output_tokens"], 1);
        assert_eq!(v["is_error"], false);
    }

    #[test]
    fn stream_emits_three_lines_ending_in_result() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::StreamJson, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        let last: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(last["type"], "result");
    }

    #[test]
    fn text_empty_text_ok() {
        let mut s = summary();
        s.final_text = String::new();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Text, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "\n");
    }

    #[test]
    fn text_error_returns_io_error() {
        let s = summary();
        let mut buf = Vec::new();
        let result = emit(&mut buf, OutputFormat::Text, &Outcome {
            summary: &s,
            duration_ms: 5,
            failure: Some("rate_limit"),
        });
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("rate_limit"), "error message: {msg}");
    }

    #[test]
    fn json_error_subtype_and_is_error_flag() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Json, &Outcome {
            summary: &s,
            duration_ms: 10,
            failure: Some("auth_blocked"),
        }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(v["subtype"], "error");
        assert_eq!(v["is_error"], true);
        assert_eq!(v["terminal_reason"], "auth_blocked");
    }

    #[test]
    fn json_success_subtype() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Json, &Outcome { summary: &s, duration_ms: 7, failure: None }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(v["subtype"], "success");
        assert_eq!(v["terminal_reason"], "completed");
    }

    #[test]
    fn json_duration_ms_present() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Json, &Outcome { summary: &s, duration_ms: 1234, failure: None }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(v["duration_ms"], 1234);
    }

    #[test]
    fn json_null_usage_passes_through() {
        let mut s = summary();
        s.usage = None;
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Json, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert!(v["usage"].is_null());
    }

    #[test]
    fn json_num_turns_present() {
        let mut s = summary();
        s.num_turns = 3;
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::Json, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(v["num_turns"], 3);
    }

    #[test]
    fn stream_first_line_is_system_init() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::StreamJson, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        let first: serde_json::Value = serde_json::from_str(
            String::from_utf8(buf).unwrap().lines().next().unwrap()
        ).unwrap();
        assert_eq!(first["type"], "system");
        assert_eq!(first["subtype"], "init");
        assert_eq!(first["session_id"], "sid");
    }

    #[test]
    fn stream_second_line_is_assistant_with_text() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::StreamJson, &Outcome { summary: &s, duration_ms: 5, failure: None }).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let second: serde_json::Value = serde_json::from_str(
            text.lines().nth(1).unwrap()
        ).unwrap();
        assert_eq!(second["type"], "assistant");
        assert_eq!(second["message"]["content"][0]["text"], "hello");
        assert_eq!(second["message"]["stop_reason"], "end_turn");
    }

    #[test]
    fn stream_result_line_carries_session_and_usage() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::StreamJson, &Outcome { summary: &s, duration_ms: 99, failure: None }).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let last: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["session_id"], "sid");
        assert_eq!(last["result"], "hello");
        assert_eq!(last["usage"]["output_tokens"], 1);
        assert_eq!(last["duration_ms"], 99);
    }

    #[test]
    fn stream_error_stop_reason_null() {
        let s = summary();
        let mut buf = Vec::new();
        emit(&mut buf, OutputFormat::StreamJson, &Outcome {
            summary: &s, duration_ms: 5, failure: Some("rate_limit"),
        }).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let second: serde_json::Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
        assert!(second["message"]["stop_reason"].is_null());
    }
}
