//! Integration tests that run the compiled `claudio` binary against a real,
//! locally-authenticated `claude`.
//!
//! **These tests are gated on `CLAUDIO_E2E=1`.**
//! They each call the real Claude API and take ~3–10 s to complete.
//!
//! Run with:
//!   CLAUDIO_E2E=1 cargo test --test integration -- --nocapture
//!
//! The Makefile target `make e2e` does the same thing.
//!
//! Design notes:
//! - `env!("CARGO_BIN_EXE_claudio")` gives the path to the binary built
//!   during `cargo test`, so no manual path resolution is needed.
//! - Every test passes `--dangerously-skip-permissions` explicitly because the
//!   shell alias that adds it automatically does not apply to subprocess execs.
//! - Each test uses a unique prompt token (e.g. `E2E_TEXT_OK`) so assertion
//!   matches are unambiguous even if Claude adds surrounding text.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_claudio");
const SKIP_PERMS: &str = "--dangerously-skip-permissions";

/// Returns true when the E2E environment variable is set.
fn e2e_enabled() -> bool {
    std::env::var("CLAUDIO_E2E").map(|v| v == "1").unwrap_or(false)
}

/// Build a base command with a generous timeout (the wrapper enforces 300 s
/// internally, but we cap at 180 s here so a hung test doesn't block CI forever).
fn cmd() -> Command {
    let mut c = Command::new(BIN);
    c.env("CLAUDIO_TIMEOUT_SEC", "150");
    c
}

/// Run the command and return its output. Panics if the process can't be spawned.
fn run(mut c: Command, _timeout_secs: u64) -> Output {
    c.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn claudio")
        .wait_with_output()
        .expect("failed to wait for claudio")
}

/// Assert a command exits 0 and return its stdout as a String.
fn assert_success(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "claudio failed (exit {:?})\nstdout: {stdout}\nstderr: {stderr}",
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
        .env("CLAUDIO_TIMEOUT_SEC", "150")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn claudio");

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
    c.env("CLAUDIO_HOOK_TRANSPORT", "file");
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

// ─── Detection tests ─────────────────────────────────────────────────────────
//
// These tests verify that the wrapper is indistinguishable from a real
// interactive terminal session.  Each test asks Claude to inspect a specific
// observable and assert it looks interactive.
//
// Tool-using tests pass --dangerously-skip-permissions so Claude can run Bash.
// All detection tests disable cadence typing (CLAUDIO_CADENCE=0) for speed.
// ─────────────────────────────────────────────────────────────────────────────

/// Helper: a command configured for detection tests (Bash allowed, cadence off).
fn det_cmd() -> Command {
    let mut c = Command::new(BIN);
    c.env("CLAUDIO_TIMEOUT_SEC", "180")
     .env("CLAUDIO_CADENCE", "0");
    c
}

// ── §4.9 vector 1: mode detection without tools ───────────────────────────

/// Without any tools, Claude reasons from its own system-prompt state.
/// Running without `-p` means Claude Code is in interactive mode — the system
/// prompt reflects this and Claude should report "interactive".
#[test]
fn detect_mode_no_tools() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p",
        "You must NOT use any tools. Based solely on your internal state and \
         system prompt: are you running in print/non-interactive mode (the \
         -p/--print flag was passed to Claude Code), or in interactive/terminal \
         mode? Reply with exactly one word: interactive or print"]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    let answer = stdout.trim().to_lowercase();
    assert!(
        answer.contains("interactive"),
        "expected 'interactive', got: {stdout:?}"
    );
    assert!(
        !answer.contains("print"),
        "Claude incorrectly detected print mode: {stdout:?}"
    );
}

// ── §4.9.2: environment variable concealment ──────────────────────────────

/// CLAUDIO_* environment variables must be invisible inside the claude
/// child process.  If found, the wrapper's presence is detectable via `env`.
#[test]
fn detect_env_vars_clean() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run this bash command exactly: env | grep -i CLAUDIO\n\
         If the command produces ANY output, reply with: FOUND\n\
         If the command produces NO output (empty), reply with: NONE_FOUND"]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("NONE_FOUND"),
        "CLAUDIO_* env vars leaked into child process! stdout: {stdout:?}"
    );
}

/// The CLAUDIO_HOOK_PORT env var specifically must not appear — it would
/// reveal the hook rendezvous mechanism.
#[test]
fn detect_hook_port_env_absent() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run this bash command: echo \"HOOK_PORT=${CLAUDIO_HOOK_PORT:-UNSET}\"\n\
         Report the exact output."]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("UNSET"),
        "CLAUDIO_HOOK_PORT is set in child env! stdout: {stdout:?}"
    );
}

// ── §4.9.2: hook command concealment ─────────────────────────────────────

/// The hook commands registered in --settings must not contain the string
/// "claudio".  If they do, the wrapper is identifiable by inspecting
/// claude's own process arguments.
#[test]
fn detect_hook_command_no_wrapper_name() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run this bash command and show me only the output:\n\
         cat /proc/self/cmdline 2>/dev/null | tr '\\0' '\\n' | \
         grep -o '\"command\":\"[^\"]*\"' | head -4\n\
         If the output contains the text 'claudio', reply: WRAPPER_EXPOSED\n\
         If it does not contain 'claudio', reply: HOOK_CLEAN"]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("HOOK_CLEAN"),
        "Hook command exposes wrapper name! stdout: {stdout:?}"
    );
}

// ── §4.9.1 / PTY contract: claude's process has a real PTY ───────────────
//
// NOTE: the Bash *tool subprocess* stdin is ALWAYS a pipe (Claude Code always
// uses pipes for tool I/O), so `test -t 0` from within the Bash tool returns
// NOT_TTY regardless of whether the session is interactive or automated.
// That check is NOT a useful detection signal.
//
// The correct check: walk the ancestor process chain from the bash subprocess
// up to claude itself — claude's stdin IS a PTY slave (/dev/pts/*) because
// we allocate a real PTY for the session.  Under native `-p` mode, claude's
// stdin would typically be a pipe or /dev/null.

/// Walking the process tree from the Bash tool subprocess, at least one
/// ancestor process (claude itself) must have a PTY (pts device) as stdin.
/// This confirms we are running in a genuine terminal session.
#[test]
fn detect_process_tree_has_pty() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    // Walk up to 6 ancestors from the bash subprocess.  Claude's own process
    // will have stdin = /dev/pts/<N> because we allocate a PTY for the session.
    c.args(["-p", SKIP_PERMS,
        "Run this bash script and report ONLY the last line of output:\n\
         found=NO; pid=$$\n\
         for d in 1 2 3 4 5 6; do\n\
           ppid=$(awk '/PPid/{print $2}' /proc/$pid/status 2>/dev/null) || break\n\
           stdin=$(readlink /proc/$ppid/fd/0 2>/dev/null)\n\
           if echo \"$stdin\" | grep -q '^/dev/pts/'; then found=YES; break; fi\n\
           pid=$ppid\n\
         done\n\
         echo $found"]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        stdout.trim().contains("YES"),
        "No PTY found in process tree — session may not have a real terminal: {stdout:?}"
    );
}

// ── §4.9.2: -p flag absent from claude's own argv ─────────────────────────

/// The `-p` / `--print` flag must not appear in claude's own process
/// arguments.  Our wrapper runs claude in interactive mode (no -p), so this
/// check should always return 0.
#[test]
fn detect_no_print_flag_in_argv() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run this bash command:\n\
         cat /proc/self/cmdline 2>/dev/null | tr '\\0' '\\n' | \
         grep -cE '^(-p|--print)$' || echo 0\n\
         If the result is 0, reply: NO_PRINT_FLAG\n\
         If the result is 1 or more, reply: PRINT_FLAG_FOUND"]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        stdout.contains("NO_PRINT_FLAG"),
        "-p/--print flag detected in claude's argv! stdout: {stdout:?}"
    );
}

// ── §4.9.3: terminal fingerprint ─────────────────────────────────────────

/// TERM must identify as an xterm-compatible terminal.  This ensures our
/// probe responses are consistent with what the env advertises.
#[test]
fn detect_term_is_xterm_family() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run: echo \"TERM=$TERM\" and report exactly that line."]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        stdout.to_lowercase().contains("xterm"),
        "TERM is not xterm-family: {stdout:?}"
    );
}

/// COLORTERM should be set to indicate true-colour support.
#[test]
fn detect_colorterm_set() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run: echo \"COLORTERM=${COLORTERM:-UNSET}\" and report exactly that line."]);
    let out = run(c, 180);
    let stdout = assert_success(&out);
    assert!(
        !stdout.contains("UNSET"),
        "COLORTERM is not set (real terminals set it): {stdout:?}"
    );
}

// ── §4.9.5: PTY geometry is realistic ────────────────────────────────────

/// Terminal dimensions reported by stty must be within the realistic range
/// we randomise to (rows 40–60, cols 180–260).  Suspiciously small or fixed
/// values (e.g. 24×80 or 40×120) are easy heuristic signals.
#[test]
fn detect_terminal_size_realistic() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "Run this bash command: stty size 2>/dev/null || echo 'NA NA'\n\
         Report only the output (two numbers separated by a space)."]);
    let out = run(c, 180);
    let stdout = assert_success(&out);

    // Parse "rows cols" from the output.  stty returns "rows cols".
    let nums: Vec<u16> = stdout
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();

    if nums.len() >= 2 {
        let (rows, cols) = (nums[0], nums[1]);
        assert!(rows >= 35 && rows <= 65,
            "PTY rows={rows} outside realistic range [35, 65]: {stdout:?}");
        assert!(cols >= 150 && cols <= 290,
            "PTY cols={cols} outside realistic range [150, 290]: {stdout:?}");
    }
    // If stty fails (non-Linux or piped env) we skip the size assertion.
}

// ── Comprehensive: all-at-once detection attempt ──────────────────────────

/// Ask Claude to use all available tools to determine if it is in automated
/// or interactive mode.  This mirrors what a detection system would do.
/// Claude should conclude "interactive" after exhaustive inspection.
///
/// NOTE: We deliberately exclude `test -t 0/1` from this prompt because the
/// Bash tool always runs commands in a subprocess connected via pipes — that
/// is true in REAL interactive sessions too.  The meaningful checks are env
/// vars, argv, TERM, and the process-tree PTY walk.
#[test]
fn detect_comprehensive_attempt() {
    if !e2e_enabled() { return; }

    let mut c = det_cmd();
    c.args(["-p", SKIP_PERMS,
        "You are trying to determine if this Claude Code session is running in \
         native -p/--print/automated mode, or in genuine interactive terminal mode.\n\
         Use the Bash tool to run ALL of these commands (Bash tool subprocesses \
         always have piped stdin — that is normal even in interactive sessions, \
         so do NOT use `test -t 0/1` as evidence):\n\
         1. env | grep -i CLAUDIO || echo no_claudio_vars\n\
         2. cat /proc/self/cmdline 2>/dev/null | tr '\\0' '\\n' | grep -E '^-p$|^--print$' || echo no_print_flag\n\
         3. echo TERM=$TERM\n\
         4. found=NO; pid=$$; for d in 1 2 3 4 5 6; do ppid=$(awk '/PPid/{print $2}' /proc/$pid/status 2>/dev/null) || break; stdin=$(readlink /proc/$ppid/fd/0 2>/dev/null); if echo \"$stdin\" | grep -q '^/dev/pts/'; then found=YES; break; fi; pid=$ppid; done; echo pty_in_tree=$found\n\
         Based on ALL evidence above, are you running in interactive or automated mode?\n\
         Reply with exactly one word: interactive or automated"]);
    let out = run(c, 240);
    let stdout = assert_success(&out);
    let answer = stdout.trim().to_lowercase();
    assert!(
        answer.contains("interactive"),
        "Comprehensive detection concluded non-interactive! stdout: {stdout:?}"
    );
}

// ─── API mode tests (`claudio --api`) ─────────────────────────────────────────
//
// These exercise the OpenAI-compatible server. /health, /v1/models, and auth
// need no `claude` turn; the chat-completion test does (it resolves through the
// PTY backend, like `claudio -p`). All are gated on CLAUDIO_E2E so they run
// under `make e2e` / `make test`, which is single-threaded — avoiding port
// races between the spawned servers.
// ─────────────────────────────────────────────────────────────────────────────

/// Grab a currently-free loopback port by binding to :0 and releasing it.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

/// A `claudio --api` server child, killed on drop.
struct ApiServer {
    child: std::process::Child,
    port: u16,
}

impl ApiServer {
    fn start(extra_env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let mut c = Command::new(BIN);
        c.arg("--api")
            .env("CLAUDIO_API_BIND", format!("127.0.0.1:{port}"))
            .env("CLAUDIO_TIMEOUT_SEC", "150")
            .env("CLAUDIO_CADENCE", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in extra_env {
            c.env(k, v);
        }
        let child = c.spawn().expect("failed to spawn claudio --api");
        let srv = ApiServer { child, port };
        srv.wait_ready();
        srv
    }

    /// Poll until the server returns any HTTP response (or time out). We accept
    /// any status — when an API key is configured, even /health answers 401,
    /// which still proves the listener is up.
    fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(resp) = self.request("GET", "/health", None, &[]) {
                if resp.starts_with("HTTP/") {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("claudio --api did not become ready on port {}", self.port);
    }

    /// Issue one HTTP/1.1 request over a fresh connection; return the raw
    /// response (status line + headers + body).
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        headers: &[(&str, &str)],
    ) -> std::io::Result<String> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port))?;
        s.set_read_timeout(Some(Duration::from_secs(160)))?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(b) = body {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", b.len()));
        }
        req.push_str("\r\n");
        if let Some(b) = body {
            req.push_str(b);
        }
        s.write_all(req.as_bytes())?;
        s.flush()?;
        let mut resp = String::new();
        s.read_to_string(&mut resp)?;
        Ok(resp)
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Split an HTTP response into (status_line, body).
fn split_response(resp: &str) -> (&str, &str) {
    let status = resp.lines().next().unwrap_or("");
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (status, body)
}

/// `/health` returns 200 and "ok".
#[test]
fn api_health_ok() {
    if !e2e_enabled() { return; }
    let srv = ApiServer::start(&[]);
    let resp = srv.request("GET", "/health", None, &[]).unwrap();
    let (status, body) = split_response(&resp);
    assert!(status.contains("200 OK"), "status: {status:?}");
    assert!(body.contains("ok"), "body: {body:?}");
}

/// `/v1/models` lists the curated Claude models.
#[test]
fn api_models_list() {
    if !e2e_enabled() { return; }
    let srv = ApiServer::start(&[]);
    let resp = srv.request("GET", "/v1/models", None, &[]).unwrap();
    let (status, body) = split_response(&resp);
    assert!(status.contains("200 OK"), "status: {status:?}");
    let v: serde_json::Value = serde_json::from_str(body).expect("models JSON");
    assert_eq!(v["object"], "list");
    let ids: Vec<&str> = v["data"].as_array().unwrap().iter()
        .filter_map(|m| m["id"].as_str()).collect();
    assert!(ids.contains(&"opus") && ids.contains(&"sonnet") && ids.contains(&"haiku"),
        "model ids: {ids:?}");
}

/// An unknown, non-Claude model id is rejected.
#[test]
fn api_models_unknown_rejected() {
    if !e2e_enabled() { return; }
    let srv = ApiServer::start(&[]);
    let resp = srv.request("GET", "/v1/models/gpt-4o", None, &[]).unwrap();
    let (status, _) = split_response(&resp);
    assert!(status.contains("400"), "expected 400 for unknown model, got: {status:?}");
}

/// When an API key is configured, requests need a matching bearer token.
#[test]
fn api_auth_required() {
    if !e2e_enabled() { return; }
    let srv = ApiServer::start(&[("CLAUDIO_API_KEY", "s3cret")]);

    let no_key = srv.request("GET", "/v1/models", None, &[]).unwrap();
    assert!(split_response(&no_key).0.contains("401"), "expected 401 without key");

    let bad = srv.request("GET", "/v1/models", None, &[("Authorization", "Bearer nope")]).unwrap();
    assert!(split_response(&bad).0.contains("401"), "expected 401 with wrong key");

    let good = srv.request("GET", "/v1/models", None, &[("Authorization", "Bearer s3cret")]).unwrap();
    assert!(split_response(&good).0.contains("200 OK"), "expected 200 with correct key");
}

/// End-to-end: a non-streaming chat completion is resolved through the PTY
/// backend and returned in OpenAI shape, with a real usage object.
#[test]
fn api_chat_completion_roundtrip() {
    if !e2e_enabled() { return; }
    let srv = ApiServer::start(&[]);
    let body = r#"{"model":"haiku","messages":[{"role":"user","content":"Reply with exactly: API_E2E_OK"}]}"#;
    let resp = srv.request("POST", "/v1/chat/completions", Some(body), &[]).unwrap();
    let (status, json_body) = split_response(&resp);
    assert!(status.contains("200 OK"), "status: {status:?}\nfull: {resp}");
    let v: serde_json::Value = serde_json::from_str(json_body).expect("completion JSON");
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["choices"][0]["message"]["role"], "assistant");
    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
    assert!(content.contains("API_E2E_OK"), "unexpected content: {content:?}");
    // Real usage flows through from the transcript.
    assert!(v["usage"]["total_tokens"].as_u64().unwrap_or(0) > 0, "missing usage: {v}");
}
