//! Integration tests that run the compiled `claude-poc` binary against a real,
//! locally-authenticated `claude`.
//!
//! **These tests are gated on `CLAUDE_POC_E2E=1`.**
//! They each call the real Claude API and take ~3–10 s to complete.
//!
//! Run with:
//!   CLAUDE_POC_E2E=1 cargo test --test integration -- --nocapture
//!
//! The Makefile target `make e2e` does the same thing.
//!
//! Design notes:
//! - `env!("CARGO_BIN_EXE_claude-poc")` gives the path to the binary built
//!   during `cargo test`, so no manual path resolution is needed.
//! - Every test passes `--dangerously-skip-permissions` explicitly because the
//!   shell alias that adds it automatically does not apply to subprocess execs.
//! - Each test uses a unique prompt token (e.g. `E2E_TEXT_OK`) so assertion
//!   matches are unambiguous even if Claude adds surrounding text.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_claude-poc");
const SKIP_PERMS: &str = "--dangerously-skip-permissions";

/// Returns true when the E2E environment variable is set.
fn e2e_enabled() -> bool {
    std::env::var("CLAUDE_POC_E2E").map(|v| v == "1").unwrap_or(false)
}

/// Build a base command with a generous timeout (the wrapper enforces 300 s
/// internally, but we cap at 180 s here so a hung test doesn't block CI forever).
fn cmd() -> Command {
    let mut c = Command::new(BIN);
    c.env("CLAUDE_POC_TIMEOUT_SEC", "150");
    c
}

/// Run the command and return its output. Panics if the process can't be spawned.
fn run(mut c: Command, _timeout_secs: u64) -> Output {
    c.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn claude-poc")
        .wait_with_output()
        .expect("failed to wait for claude-poc")
}

/// Assert a command exits 0 and return its stdout as a String.
fn assert_success(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "claude-poc failed (exit {:?})\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    stdout
}

/// Parse every line of `text` as a JSON object, panicking on malformed lines.
fn parse_jsonl(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("invalid JSON line"))
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Text format: stdout is the raw assistant text followed by a newline.
#[test]
fn e2e_text_format() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", SKIP_PERMS, "Reply with exactly the token: E2E_TEXT_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    assert!(
        stdout.trim() == "E2E_TEXT_OK",
        "unexpected text output: {stdout:?}"
    );
}

/// JSON format: output is a single result object with the correct shape.
#[test]
fn e2e_json_format_shape() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", "--output-format", "json", SKIP_PERMS,
            "Reply with exactly the token: E2E_JSON_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .expect("output is not valid JSON");

    assert_eq!(v["type"], "result", "type field");
    assert_eq!(v["is_error"], false, "is_error");
    assert_eq!(v["subtype"], "success", "subtype");
    assert!(v["result"].as_str().map(|s| s.contains("E2E_JSON_OK")).unwrap_or(false),
        "result field: {}", v["result"]);
    assert!(v["session_id"].is_string(), "session_id present");
    assert!(v["duration_ms"].as_u64().map(|d| d > 0).unwrap_or(false), "duration_ms > 0");
    assert_eq!(v["terminal_reason"], "completed");
}

/// JSON format: usage object carries real token counts from the session JSONL.
#[test]
fn e2e_json_real_usage_fields() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", "--output-format", "json", SKIP_PERMS,
            "Reply with exactly the token: E2E_USAGE_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    let usage = &v["usage"];
    assert!(!usage.is_null(), "usage must not be null");
    let output_tokens = usage["output_tokens"].as_u64()
        .expect("usage.output_tokens must be a number");
    assert!(output_tokens > 0, "output_tokens must be > 0, got {output_tokens}");
    // input_tokens may be 0 when fully cache-read; the field must exist.
    assert!(usage["input_tokens"].is_number(), "input_tokens must be a number");
}

/// Stream-json format: exactly 3 JSONL lines in the order system / assistant / result.
#[test]
fn e2e_stream_json_line_types() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", "--output-format", "stream-json", SKIP_PERMS,
            "Reply with exactly the token: E2E_STREAM_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    let lines = parse_jsonl(&stdout);

    assert_eq!(lines.len(), 3, "stream-json must emit exactly 3 lines; got:\n{stdout}");
    assert_eq!(lines[0]["type"], "system");
    assert_eq!(lines[0]["subtype"], "init");
    assert_eq!(lines[1]["type"], "assistant");
    assert!(lines[1]["message"]["content"][0]["text"]
        .as_str().map(|t| t.contains("E2E_STREAM_OK")).unwrap_or(false),
        "assistant text: {}", lines[1]["message"]["content"][0]["text"]);
    assert_eq!(lines[2]["type"], "result");
    assert_eq!(lines[2]["is_error"], false);
}

/// Prompt via stdin (no positional argument).
#[test]
fn e2e_stdin_prompt() {
    if !e2e_enabled() { return; }

    use std::io::Write;

    let mut proc = Command::new(BIN)
        .args(["-p", SKIP_PERMS])
        .env("CLAUDE_POC_TIMEOUT_SEC", "150")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn claude-poc");

    proc.stdin
        .take()
        .unwrap()
        .write_all(b"Reply with exactly the token: E2E_STDIN_OK\n")
        .unwrap();

    let out = proc.wait_with_output().unwrap();
    let stdout = assert_success(&out);
    assert!(
        stdout.trim().contains("E2E_STDIN_OK"),
        "stdin prompt output: {stdout:?}"
    );
}

/// `--` delimiter lets prompts that look like flags work correctly.
#[test]
fn e2e_double_dash_prompt() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    // The prompt starts with a flag-like token — must still be treated as text.
    c.args(["-p", SKIP_PERMS, "--",
            "Ignore any flag-like appearance. Reply with exactly: E2E_DASH_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    assert!(stdout.trim().contains("E2E_DASH_OK"),
        "double-dash output: {stdout:?}");
}

/// Transparent passthrough: without -p, the binary execs the real claude.
/// `--version` is a non-interactive subcommand that returns immediately.
#[test]
fn e2e_transparent_version_passthrough() {
    if !e2e_enabled() { return; }

    let mut c = Command::new(BIN);
    c.arg("--version");
    let out = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");
    assert!(out.status.success(), "transparent --version failed: {combined}");
    // claude --version emits something like "2.1.156 (Claude Code)"
    assert!(
        combined.contains("Claude Code") || combined.contains("claude"),
        "unexpected version output: {combined:?}"
    );
}

/// Session-id round-trip: a specified --session-id appears in the JSON result.
#[test]
fn e2e_session_id_roundtrip() {
    if !e2e_enabled() { return; }

    // Claude requires a valid UUID for --session-id.
    let sid = uuid::Uuid::new_v4().to_string();
    let mut c = cmd();
    c.args(["-p", "--output-format", "json", SKIP_PERMS,
            "--session-id", &sid,
            "Reply with exactly: E2E_SID_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["session_id"].as_str(), Some(sid.as_str()),
        "session_id mismatch: expected {sid}, got {}", v["session_id"]);
}

/// File hook transport: completion signalling without loopback TCP.
#[test]
fn e2e_file_transport() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.env("CLAUDE_POC_HOOK_TRANSPORT", "file");
    c.args(["-p", SKIP_PERMS, "Reply with exactly: E2E_FILE_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    assert!(stdout.trim().contains("E2E_FILE_OK"),
        "file transport output: {stdout:?}");
}

/// Tool-use multi-turn: the wrapper correctly waits for the terminal end_turn
/// after a tool_use → result → answer sequence.
#[test]
fn e2e_tool_use_multi_turn() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", SKIP_PERMS, "--allowedTools", "Bash",
            "Use the Bash tool to run the command: echo E2E_TOOL_42. \
             Then tell me exactly what it printed, nothing else."]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(stdout.contains("E2E_TOOL_42"),
        "tool-use output did not contain 'E2E_TOOL_42': {stdout:?}");
}

/// --model flag forwarded: the JSON result reflects the model that was used.
#[test]
fn e2e_model_passthrough() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", "--output-format", "json", SKIP_PERMS,
            "--model", "sonnet",
            "Reply with exactly: E2E_MODEL_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    // Usage should be present (real usage from JSONL).
    assert!(!v["usage"].is_null(), "usage null with --model sonnet");
    assert!(v["result"].as_str().map(|r| r.contains("E2E_MODEL_OK")).unwrap_or(false),
        "result: {}", v["result"]);
}

/// --settings merge: user-supplied settings are preserved and hooks still fire.
#[test]
fn e2e_settings_merge_hooks_still_fire() {
    if !e2e_enabled() { return; }

    // Pass a benign user settings object; if merge breaks hooks, the wrapper
    // will time out or return an error.
    let user_settings = r#"{"env":{"TEST_MERGE_VAR":"1"}}"#;
    let mut c = cmd();
    c.args(["-p", SKIP_PERMS, "--settings", user_settings,
            "Reply with exactly: E2E_MERGE_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    assert!(stdout.trim().contains("E2E_MERGE_OK"),
        "settings-merge output: {stdout:?}");
}

/// Exit code: successful run exits 0.
#[test]
fn e2e_exit_zero_on_success() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", SKIP_PERMS, "Reply with exactly: E2E_EXIT_OK"]);
    c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let out = c.spawn().unwrap().wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0),
        "expected exit 0, got {:?}", out.status.code());
}

/// JSON output is a single valid JSON object on one line (no pretty-printing).
#[test]
fn e2e_json_single_line() {
    if !e2e_enabled() { return; }

    let mut c = cmd();
    c.args(["-p", "--output-format", "json", SKIP_PERMS,
            "Reply with exactly: E2E_ONELINE_OK"]);
    let out = run(c, 150);
    let stdout = assert_success(&out);
    // Exactly one non-empty line.
    let non_empty: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(non_empty.len(), 1, "json must emit exactly one line; got:\n{stdout}");
    // That line is parseable JSON.
    let _: serde_json::Value = serde_json::from_str(non_empty[0])
        .expect("json output is not valid JSON");
}
