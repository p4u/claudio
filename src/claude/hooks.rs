//! Hook registration and relay for the daemon session manager.
//!
//! The daemon spawns claude with merged `--settings` hooks. Each hook fires
//! `<claudio> __hook <Event> <socket> <token>` which reads stdin and forwards
//! it to the daemon's Unix socket as a [`proto::Msg::Hook`] frame.
//!
//! This module provides:
//! - [`EVENTS`] — the hook events the daemon registers.
//! - [`settings`] — build the `--settings` JSON blob for a session.
//! - [`relay`] — the short-lived relay process that ships the payload.
//! - [`context`] — `claudio __context file <path>`, the SessionStart hook
//!   that adds the session's extension instructions (the daemon's context
//!   file) to claude's context; `__context latex` is kept for sessions an
//!   older daemon started.
//! - [`new_token`] — generate a per-spawn secret token.

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::proto::{Envelope, Frame, Msg};

/// The Claude Code hook events the daemon subscribes to.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// The tools whose `PostToolUse` is relayed: the ones that wait for the user,
/// so an ordinary tool call costs no extra hook process.
pub const ASKING_TOOLS: &str = "AskUserQuestion|ExitPlanMode";

/// Write timeout for the relay's socket. A local Unix-socket connect never
/// blocks for long, but a wedged daemon must not stall claude's hook.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum hook payload size. Hooks should be tiny; 1 MiB is a hard cap.
const MAX_PAYLOAD: usize = 1 << 20;

/// The most of a context file `__context file` reads.
pub(crate) const MAX_CONTEXT: u64 = 16 * 1024;

/// What `claudio __context latex` adds to every conversation's context when
/// `[claude] latex` is on: the manager renders `<latex>` tags in the pane.
/// Context, not the system prompt, so the session stays a plain interactive
/// one (see CLAUDE.md on system prompts).
pub const LATEX_CONTEXT: &str = "This terminal renders math written as LaTeX inside \
<latex>…</latex> tags. Whenever you write a mathematical formula or expression, wrap its \
LaTeX in <latex>…</latex> instead of $…$ or $$…$$, for example \
<latex>x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}</latex>. Put a display formula alone on \
its own line. Never put the tags inside code blocks or inline code, and do not wrap plain \
numbers or code in them. Write relations with spaces around them (a < b, not a<b).";

// ── Settings generation ───────────────────────────────────────────────────────

/// Build the `--settings` JSON value that registers all daemon hooks.
///
/// The hook command is `'<claudio_bin>' __hook <Event> '<socket>' <token>`,
/// with paths shell-quoted so that spaces and special characters are safe.
/// With a `context` file, `SessionStart` runs
/// `'<claudio_bin>' __context file '<context>'` too: the path, never the text.
pub fn settings(
    claudio_bin: &Path,
    socket: &Path,
    token: &str,
    context: Option<&Path>,
) -> serde_json::Value {
    let bin = shell_quote(&claudio_bin.to_string_lossy());
    let sock = shell_quote(&socket.to_string_lossy());

    let mut hooks_map = serde_json::Map::new();
    for event in EVENTS {
        let command = format!("{bin} __hook {event} {sock} {token}");
        let mut hooks = vec![serde_json::json!({"type": "command", "command": command})];
        if let Some(path) = context.filter(|_| *event == "SessionStart") {
            let path = shell_quote(&path.to_string_lossy());
            let context = format!("{bin} __context file {path}");
            hooks.push(serde_json::json!({"type": "command", "command": context}));
        }
        let entry = serde_json::json!([{
            "matcher": if *event == "PostToolUse" { ASKING_TOOLS } else { "*" },
            "timeout": 5,
            "hooks": hooks
        }]);
        hooks_map.insert(event.to_string(), entry);
    }

    serde_json::json!({ "hooks": hooks_map })
}

/// Shell-quote a string so it is safe as a single argument in a POSIX shell
/// command line. Wraps in single quotes, escaping embedded single quotes as
/// `'\''`.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', r"'\''");
    format!("'{escaped}'")
}

// ── Context ───────────────────────────────────────────────────────────────────

/// `claudio __context <kind> [<path>]`: a SessionStart hook that adds to
/// claude's context. Drains the hook payload (capped like the relay), then
/// prints the one JSON line that adds [`LATEX_CONTEXT`] for `latex`, or the
/// context file's text for `file <path>` ([`read_context`]), and nothing for
/// anything else. Always exits 0 and never touches the daemon's socket.
pub fn context(kind: Option<&str>, path: Option<&str>) -> std::process::ExitCode {
    match (kind, path) {
        (Some("file"), Some(path)) => {
            context_file_to(Path::new(path), std::io::stdin(), &mut std::io::stdout())
        }
        _ => context_to(kind, std::io::stdin(), &mut std::io::stdout()),
    }
    std::process::ExitCode::SUCCESS
}

fn context_to(kind: Option<&str>, input: impl Read, out: &mut impl Write) {
    let _ = std::io::copy(&mut input.take(MAX_PAYLOAD as u64), &mut std::io::sink());
    if kind != Some("latex") {
        return;
    }
    print_context(LATEX_CONTEXT, out);
}

/// `__context file <path>`: the file's text as context, if it is acceptable.
fn context_file_to(path: &Path, input: impl Read, out: &mut impl Write) {
    let _ = std::io::copy(&mut input.take(MAX_PAYLOAD as u64), &mut std::io::sink());
    if let Some(text) = read_context(path) {
        print_context(&text, out);
    }
}

/// A context file's text: opened without following a symlink (and without
/// blocking on a FIFO), then, by the open descriptor, only a regular file
/// owned by this uid; at most [`MAX_CONTEXT`] bytes, sanitized and trimmed.
/// `None` when missing, empty or not acceptable.
fn read_context(path: &Path) -> Option<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() || meta.uid() != crate::paths::uid() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONTEXT).read_to_end(&mut bytes).ok()?;
    let text = crate::claude::latex::sanitize(&String::from_utf8_lossy(&bytes));
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The one JSON line that adds `text` to claude's context.
fn print_context(text: &str, out: &mut impl Write) {
    let line = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": text,
        }
    });
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

// ── Token generation ─────────────────────────────────────────────────────────

/// Generate a 128-bit random token as a 32-character hex string.
///
/// Used as a per-spawn secret that the daemon checks to authenticate hook
/// connections.
pub fn new_token() -> String {
    let id = uuid::Uuid::new_v4();
    format!("{:032x}", id.as_u128())
}

// ── Relay ─────────────────────────────────────────────────────────────────────

/// Relay a hook payload to the daemon. Called as the body of `claudio __hook
/// <Event> <socket> <token>`.
///
/// Reads up to [`MAX_PAYLOAD`] bytes from stdin, connects to the Unix socket,
/// and sends a single `Hook` frame. **Always exits 0 and prints nothing to
/// stdout** — a hook must never block or alter claude's output.
pub fn relay(event: &str, socket: &Path, token: &str) -> std::process::ExitCode {
    relay_from(std::io::stdin(), event, socket, token);
    std::process::ExitCode::SUCCESS
}

/// Read the hook payload from `input` (capped) and ship it. Errors are
/// swallowed: a missing or wedged daemon must never affect claude.
fn relay_from(input: impl Read, event: &str, socket: &Path, token: &str) {
    let mut buf = Vec::with_capacity(4096);
    let _ = input.take(MAX_PAYLOAD as u64).read_to_end(&mut buf);
    let payload = serde_json::from_slice(&buf).unwrap_or(Value::Null);
    let _ = send_hook_frame(event, socket, token, payload);
}

/// Connect to the Unix socket, write the Hook frame, and stay connected until
/// the daemon has taken it and hung up. The daemon checks the peer's uid
/// first, and macOS can no longer tell it once the peer is gone: a relay that
/// wrote and left at once would lose hooks there.
fn send_hook_frame(event: &str, socket: &Path, token: &str, payload: Value) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_read_timeout(Some(WRITE_TIMEOUT))?;

    let frame = Frame::Control(Envelope::event(Msg::Hook {
        token: token.to_owned(),
        event: event.to_owned(),
        payload,
    }));
    stream.write_all(&frame.encode())?;
    // The daemon replies nothing: this returns at its hang-up (or the timeout).
    let _ = stream.read(&mut [0u8; 1]);
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── shell_quote ───────────────────────────────────────────────────────────

    #[test]
    fn shell_quote_plain() {
        assert_eq!(shell_quote("/usr/bin/claudio"), "'/usr/bin/claudio'");
    }

    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(
            shell_quote("/path with spaces/claudio"),
            "'/path with spaces/claudio'"
        );
    }

    #[test]
    fn shell_quote_with_single_quotes() {
        assert_eq!(shell_quote("/path/with'quote"), "'/path/with'\\''quote'");
    }

    // ── settings shape ────────────────────────────────────────────────────────

    #[test]
    fn settings_registers_all_events() {
        let v = settings(
            Path::new("/usr/bin/claudio"),
            Path::new("/run/claudio/d.sock"),
            "tok",
            None,
        );
        let hooks = v["hooks"].as_object().unwrap();
        for event in EVENTS {
            assert!(hooks.contains_key(*event), "missing event: {event}");
        }
    }

    #[test]
    fn post_tool_use_matches_only_the_asking_tools() {
        let v = settings(Path::new("/b"), Path::new("/s"), "tok", None);
        for event in EVENTS {
            let want = if *event == "PostToolUse" { "AskUserQuestion|ExitPlanMode" } else { "*" };
            assert_eq!(v["hooks"][event][0]["matcher"], want, "{event}");
        }
    }

    #[test]
    fn settings_command_contains_socket_and_token() {
        let v = settings(
            Path::new("/bin/cl"),
            Path::new("/run/s.sock"),
            "mytoken",
            None,
        );
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("__hook Stop"), "cmd: {cmd}");
        assert!(cmd.contains("mytoken"), "cmd: {cmd}");
        assert!(cmd.contains("/run/s.sock"), "cmd: {cmd}");
    }

    #[test]
    fn settings_path_with_spaces_is_safe() {
        let v = settings(
            Path::new("/home/user name/bin/claudio"),
            Path::new("/run/user data/s.sock"),
            "tok",
            None,
        );
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        // Both paths must be single-quoted.
        assert!(cmd.contains("'/home/user name/bin/claudio'"), "cmd: {cmd}");
        assert!(cmd.contains("'/run/user data/s.sock'"), "cmd: {cmd}");
    }

    #[test]
    fn a_context_file_adds_one_command_to_session_start_only() {
        let (bin, sock) = (Path::new("/usr/bin/claudio"), Path::new("/run/d.sock"));
        let off = settings(bin, sock, "tok", None);
        let on = settings(bin, sock, "tok", Some(Path::new("/run/context/s 1.txt")));
        for event in EVENTS.iter().filter(|e| **e != "SessionStart") {
            assert_eq!(on["hooks"][event], off["hooks"][event], "{event}");
        }
        let start = |v: &serde_json::Value| v["hooks"]["SessionStart"][0].clone();
        let (off_start, on_start) = (start(&off), start(&on));
        assert_eq!(off_start["hooks"].as_array().unwrap().len(), 1);
        let hooks = on_start["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0], off_start["hooks"][0], "the relay stays first");
        assert_eq!(hooks[1]["type"], "command");
        assert_eq!(
            hooks[1]["command"],
            "'/usr/bin/claudio' __context file '/run/context/s 1.txt'"
        );
        assert_eq!(on_start["matcher"], off_start["matcher"]);
        assert_eq!(on_start["timeout"], off_start["timeout"]);
    }

    #[test]
    fn context_latex_prints_one_session_start_line() {
        let mut out = Vec::new();
        context_to(Some("latex"), &b"{\"session_id\":\"s\"}"[..], &mut out);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
        let v: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let context = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(context, LATEX_CONTEXT);
        assert!(context.contains("<latex>") && context.contains("</latex>"));
    }

    #[test]
    fn context_prints_nothing_for_anything_else() {
        for kind in [None, Some(""), Some("other"), Some("LATEX")] {
            let mut out = Vec::new();
            context_to(kind, &b"{}"[..], &mut out);
            assert!(out.is_empty(), "{kind:?}");
        }
    }

    /// A fresh private scratch dir for context files.
    fn context_dir() -> std::path::PathBuf {
        let dir = crate::paths::socket_test_dir("cl-ctx");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn context_of(path: &Path) -> String {
        let mut out = Vec::new();
        context_file_to(path, &b"{}"[..], &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn context_file_prints_its_sanitized_text() {
        let dir = context_dir();
        let path = dir.join("s.txt");
        std::fs::write(&path, "Use <keys>.\u{1b}[31m\n\nDraw mermaid.\n").unwrap();
        let text = context_of(&path);
        assert_eq!(text.matches('\n').count(), 1, "one line: {text}");
        let v: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let context = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.starts_with("Use <keys>."), "{context:?}");
        assert!(context.ends_with("Draw mermaid."), "{context:?}");
        assert!(!context.contains('\u{1b}'), "{context:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn context_file_reads_at_most_16_kib() {
        let dir = context_dir();
        let path = dir.join("big.txt");
        std::fs::write(&path, "a".repeat(20_000)).unwrap();
        let v: Value = serde_json::from_str(context_of(&path).trim_end()).unwrap();
        let context = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(context.len(), 16 * 1024);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn context_file_prints_nothing_for_what_it_must_not_read() {
        let dir = context_dir();
        let good = dir.join("good.txt");
        std::fs::write(&good, "secret instructions").unwrap();
        // Missing, empty, a symlink to a good file, a directory, a FIFO.
        let empty = dir.join("empty.txt");
        std::fs::write(&empty, " \n").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        let fifo = dir.join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        for path in [dir.join("missing.txt"), empty, link, dir.clone(), fifo] {
            assert_eq!(context_of(&path), "", "{}", path.display());
        }
        // Owned by another user (root's files, when we are not root).
        if crate::paths::uid() != 0 {
            assert_eq!(context_of(Path::new("/etc/passwd")), "");
        }
        assert!(!context_of(&good).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn new_token_is_32_hex_chars() {
        let tok = new_token();
        assert_eq!(tok.len(), 32);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()), "not hex: {tok}");
    }

    #[test]
    fn new_token_is_unique() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
    }

    // ── relay integration ─────────────────────────────────────────────────────

    /// Bind a temp Unix socket, run `relay`, and decode the received frame.
    #[test]
    fn relay_sends_hook_frame() {
        use std::os::unix::net::UnixListener;

        let dir = crate::paths::socket_test_dir("cl-relay");
        std::fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("relay.sock");

        let listener = UnixListener::bind(&sock_path).unwrap();

        // Run the relay in a thread, pretending stdin delivered JSON.
        let sock_path_clone = sock_path.clone();
        let handle = std::thread::spawn(move || {
            // Simulate stdin by replacing stdin in-process isn't feasible here,
            // so we call send_hook_frame directly (the core logic under relay).
            let payload = serde_json::json!({"session_id": "test-123", "cwd": "/tmp"});
            send_hook_frame("Stop", &sock_path_clone, "secret", payload)
        });

        // Accept the connection and read the frame.
        listener.set_nonblocking(false).unwrap();
        let (mut stream, _) = listener.accept().unwrap();

        // Read length prefix (4 bytes).
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).unwrap();
        let len = u32::from_be_bytes(len_buf) as usize;

        // Read the frame body.
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).unwrap();

        let frame = Frame::decode(&body).unwrap();

        // Ensure the thread succeeded.
        handle.join().unwrap().unwrap();

        // Validate the frame.
        match frame {
            Frame::Control(env) => match env.msg {
                Msg::Hook {
                    token,
                    event,
                    payload,
                } => {
                    assert_eq!(token, "secret");
                    assert_eq!(event, "Stop");
                    assert_eq!(payload["session_id"], "test-123");
                }
                other => panic!("unexpected msg: {other:?}"),
            },
            other => panic!("expected Control frame, got: {other:?}"),
        }

        // Cleanup.
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn relay_ignores_missing_socket() {
        // A missing daemon socket is silently ignored.
        relay_from(&b"{}"[..], "Stop", Path::new("/nonexistent/sock"), "tok");
    }
}
