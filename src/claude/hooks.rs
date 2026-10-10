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
//! - [`new_token`] — generate a per-spawn secret token.

use std::io::{Read, Write};
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
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// Write timeout for the relay's socket. A local Unix-socket connect never
/// blocks for long, but a wedged daemon must not stall claude's hook.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum hook payload size. Hooks should be tiny; 1 MiB is a hard cap.
const MAX_PAYLOAD: usize = 1 << 20;

// ── Settings generation ───────────────────────────────────────────────────────

/// Build the `--settings` JSON value that registers all daemon hooks.
///
/// The hook command is `'<claudio_bin>' __hook <Event> '<socket>' <token>`,
/// with paths shell-quoted so that spaces and special characters are safe.
pub fn settings(claudio_bin: &Path, socket: &Path, token: &str) -> serde_json::Value {
    let bin = shell_quote(&claudio_bin.to_string_lossy());
    let sock = shell_quote(&socket.to_string_lossy());

    let mut hooks_map = serde_json::Map::new();
    for event in EVENTS {
        let command = format!("{bin} __hook {event} {sock} {token}");
        let entry = serde_json::json!([{
            "matcher": "*",
            "timeout": 5,
            "hooks": [{"type": "command", "command": command}]
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
        );
        let hooks = v["hooks"].as_object().unwrap();
        for event in EVENTS {
            assert!(hooks.contains_key(*event), "missing event: {event}");
        }
    }

    #[test]
    fn settings_command_contains_socket_and_token() {
        let v = settings(Path::new("/bin/cl"), Path::new("/run/s.sock"), "mytoken");
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
        );
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        // Both paths must be single-quoted.
        assert!(cmd.contains("'/home/user name/bin/claudio'"), "cmd: {cmd}");
        assert!(cmd.contains("'/run/user data/s.sock'"), "cmd: {cmd}");
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
