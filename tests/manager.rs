//! Integration tests for the claudio session manager.
//!
//! Drives the real binary inside a PTY (via `portable-pty`), against an
//! isolated daemon and a fake `claude` script, so tests run offline with no
//! real API keys. Fake claude prints a unique banner then `exec cat`, which
//! lets the test see session output and verify input echo.
//!
//! Run with:
//!   cargo test --test manager -- --test-threads=1
//!   make test-manager
//!
//! For the gated real-claude scenario:
//!   CLAUDIO_E2E=1 cargo test --test manager -- --test-threads=1

#![allow(clippy::zombie_processes)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs, thread};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use uuid::Uuid;

// ── Constants ──────────────────────────────────────────────────────────────────

const BINARY: &str = env!("CARGO_BIN_EXE_claudio");
/// Default assertion timeout.
const WAIT: Duration = Duration::from_secs(10);
/// Longer timeout used when the daemon must be (re-)started.
const DAEMON_WAIT: Duration = Duration::from_secs(15);
/// How long to wait for reconnect after a daemon kill.
const RECONNECT_WAIT: Duration = Duration::from_secs(20);

// Alt key raw byte sequences (VT100/xterm; no keyboard-enhancement protocol).
const ALT_LEFT: &[u8] = b"\x1b[1;3D";
const ALT_RIGHT: &[u8] = b"\x1b[1;3C";
const ALT_N: &[u8] = b"\x1bn";
const ALT_R: &[u8] = b"\x1br";
const ALT_X: &[u8] = b"\x1bx";
const ALT_Q: &[u8] = b"\x1bq";
const ENTER: &[u8] = b"\r";
const CTRL_U: &[u8] = b"\x15";
const ESC: &[u8] = b"\x1b";

// ── Env ────────────────────────────────────────────────────────────────────────

/// Isolated on-disk environment for one test: private XDG dirs, a fake claude
/// script and two session directories. Killed and cleaned up on drop.
struct Env {
    root: PathBuf,
    runtime_dir: PathBuf,
    config_home: PathBuf,
    home: PathBuf,
    fake_claude: PathBuf,
    /// The two session directories used in tests.
    pub dirs: [PathBuf; 2],
}

impl Env {
    /// Build an environment with a fake claude (`exec cat`).
    fn new() -> Self {
        let id = Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("claudio-mgr-test-{id}"));
        let runtime_dir = root.join("run");
        let config_home = root.join("config");
        let home = root.join("home");
        let fake_claude = root.join("fake-claude");
        let dirs = [root.join("sess-a"), root.join("sess-b")];

        for p in [&runtime_dir, &config_home, &home, &dirs[0], &dirs[1]] {
            fs::create_dir_all(p).expect("create test dir");
        }

        // Fake claude: print a unique banner then exec cat (echoes input).
        let script = "#!/bin/sh\necho \"FAKE_CLAUDE_BANNER cwd=$PWD args=$*\"\nexec cat\n";
        fs::write(&fake_claude, script).expect("write fake claude");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755))
            .expect("chmod fake claude");

        Env { root, runtime_dir, config_home, home, fake_claude, dirs }
    }

    /// Path to the daemon lock file (contains daemon PID after it starts).
    fn lock_file(&self) -> PathBuf {
        // paths::daemon_lock() = $XDG_RUNTIME_DIR/claudio/daemon-v1.lock
        self.runtime_dir.join("claudio").join("daemon-v1.lock")
    }

    /// Path to the TUI client's state.json.
    fn state_json(&self) -> PathBuf {
        // paths::client_state() = $XDG_CONFIG_HOME/claudio/state.json
        self.config_home.join("claudio").join("state.json")
    }

    /// Read the daemon PID from the lock file (returns None if not yet written).
    fn daemon_pid(&self) -> Option<u32> {
        fs::read_to_string(self.lock_file())
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }

    /// Send SIGTERM to the daemon. Blocks ~200 ms to let it clean up.
    fn kill_daemon(&self) {
        if let Some(pid) = self.daemon_pid() {
            // SAFETY: kill(2) is always safe to call.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            thread::sleep(Duration::from_millis(300));
        }
    }

    /// Apply per-test env vars to a CommandBuilder that will run `claudio`.
    fn apply(&self, cmd: &mut CommandBuilder) {
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("HOME", &self.home);
        cmd.env("CLAUDIO_CLAUDE_PATH", &self.fake_claude);
        // Prevent the TUI from trying a live Anthropic API.
        cmd.env("ANTHROPIC_API_KEY", "test-key-not-real");
        // Keep terminal simple so crossterm doesn't block on probes.
        cmd.env("TERM", "xterm-256color");
        // Suppress colorterm fancy modes.
        cmd.env_remove("COLORTERM");
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.kill_daemon();
        let _ = fs::remove_dir_all(&self.root);
    }
}

// ── TuiSession ─────────────────────────────────────────────────────────────────

/// A running `claudio` TUI in a PTY, with helpers for assertions and input.
struct TuiSession {
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Accumulated raw PTY output.
    raw: Arc<Mutex<Vec<u8>>>,
    _reader: thread::JoinHandle<()>,
}

impl TuiSession {
    /// Spawn `claudio` in a PTY with the given environment.
    fn start(env: &Env) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
            .expect("open pty");

        let mut cmd = CommandBuilder::new(BINARY);
        env.apply(&mut cmd);

        let child = pair.slave.spawn_command(cmd).expect("spawn claudio");
        let writer = pair.master.take_writer().expect("pty writer");
        let raw: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let raw_clone = Arc::clone(&raw);
        let mut reader = pair.master.try_clone_reader().expect("pty reader");

        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => raw_clone.lock().unwrap().extend_from_slice(&buf[..n]),
                }
            }
        });

        TuiSession { writer, child, raw, _reader: reader_thread }
    }

    /// Write raw bytes to the PTY (keystroke or paste sequence).
    fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
    }

    /// Write bytes then pause 60 ms for the event loop to process them.
    fn key(&mut self, bytes: &[u8]) {
        self.write(bytes);
        thread::sleep(Duration::from_millis(60));
    }

    /// Send `text` via the bracketed-paste protocol so it arrives as a single
    /// `Event::Paste` (faster than typing char-by-char).
    fn paste(&mut self, text: &str) {
        self.write(b"\x1b[200~");
        self.write(text.as_bytes());
        self.write(b"\x1b[201~");
        thread::sleep(Duration::from_millis(100));
    }

    /// Poll until `text` appears in the ANSI-stripped PTY output, or timeout.
    fn wait_for(&self, text: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.stripped().contains(text) {
                return true;
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "[wait_for] TIMEOUT looking for {:?}\nstripped buffer:\n{}",
                    text,
                    self.stripped()
                );
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Poll until the TUI process exits, or timeout. Returns the exit status.
    fn wait_exit(&mut self, timeout: Duration) -> Option<portable_pty::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// All PTY output so far, stripped of ANSI escape sequences.
    fn stripped(&self) -> String {
        strip_ansi(&self.raw.lock().unwrap())
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        // Best-effort: kill the child if still running so tests can't hang.
        let _ = self.child.kill();
    }
}

// ── ANSI strip ─────────────────────────────────────────────────────────────────

/// Remove ANSI/VT100 escape sequences, leaving only printable text.
fn strip_ansi(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != 0x1b {
            let b = input[i];
            if b == b'\r' {
                out.push(b'\n');
            } else if b == b'\n' || b == b'\t' || b >= 0x20 {
                out.push(b);
            }
            i += 1;
            continue;
        }
        // ESC
        i += 1;
        if i >= input.len() {
            break;
        }
        match input[i] {
            b'[' => {
                // CSI — skip to final byte (0x40–0x7e)
                i += 1;
                while i < input.len() && !(0x40..=0x7eu8).contains(&input[i]) {
                    i += 1;
                }
                if i < input.len() {
                    i += 1;
                }
            }
            b']' => {
                // OSC — skip to BEL or ESC backslash
                i += 1;
                while i < input.len() {
                    if input[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if input[i] == 0x1b && i + 1 < input.len() && input[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b'(' | b')' | b'#' | b'O' | b'>' | b'=' | b'<' | b'7' | b'8' | b'M' => {
                // Two-character sequences
                i += 1;
            }
            _ => {
                // Any other two-char sequence
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── Helpers ─────────────────────────────────────────────────────────────────────

/// Convenience: start a TUI, wait for it to be fully running (hints visible).
fn start_tui(env: &Env) -> TuiSession {
    let tui = TuiSession::start(env);
    // The key-hint string in the status bar is always present once TUI is up.
    assert!(
        tui.wait_for("Alt+q", DAEMON_WAIT),
        "TUI never started (status bar hints not seen)"
    );
    tui
}

/// Send a directory path through the wizard input (bracketed paste) and
/// confirm with Enter. Waits for `ListClaudeSessions` to resolve (the empty
/// temp dir causes an immediate spawn).
///
/// The wizard now has a host-selection step (step 0). We first press Enter to
/// confirm "local" (always pre-selected), then paste the path and press Enter
/// again to confirm the directory.
fn wizard_pick_dir(tui: &mut TuiSession, dir: &PathBuf) {
    let path = dir.to_str().unwrap();
    // Step 0: confirm "local" host (pre-selected) and advance to directory step.
    tui.key(ENTER);
    thread::sleep(Duration::from_millis(150));
    // Step 1: type directory path and confirm.
    tui.paste(path);
    thread::sleep(Duration::from_millis(200));
    tui.key(ENTER);
}

/// Wait for the fake-claude banner (confirms the session PTY is running and
/// its output has been reflected through the daemon's screen mirror and then
/// rendered by ratatui into the test PTY).
fn assert_banner(tui: &TuiSession, timeout: Duration) {
    assert!(
        tui.wait_for("FAKE_CLAUDE_BANNER", timeout),
        "session banner never appeared in the pane"
    );
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// 1. Fresh start: no sessions → wizard → session spawns → banner visible.
#[test]
fn test_1_fresh_start_wizard_and_session() {
    let env = Env::new();
    let mut tui = start_tui(&env);

    // Wizard should be open (no sessions). Type the first session directory.
    wizard_pick_dir(&mut tui, &env.dirs[0]);

    // Banner from fake claude must appear in the rendered pane.
    assert_banner(&tui, WAIT);

    // The directory basename should appear in the tab bar.
    let dir_name = env.dirs[0].file_name().unwrap().to_str().unwrap();
    assert!(
        tui.wait_for(dir_name, WAIT),
        "tab bar should show the session directory name"
    );

    // Quit cleanly.
    tui.key(ALT_Q);
    let status = tui.wait_exit(WAIT).expect("TUI did not exit after Alt+q");
    assert!(status.success(), "TUI should exit 0 after Alt+q");
}

/// 2. Two sessions; Alt+←/→ switches between them.
#[test]
fn test_2_two_sessions_switching() {
    let env = Env::new();
    let mut tui = start_tui(&env);

    // First session.
    wizard_pick_dir(&mut tui, &env.dirs[0]);
    assert_banner(&tui, WAIT);
    thread::sleep(Duration::from_millis(400));

    // Second session via Alt+n.
    tui.key(ALT_N);
    thread::sleep(Duration::from_millis(300));
    wizard_pick_dir(&mut tui, &env.dirs[1]);
    // Wait for the second banner (the second session's pane becomes active).
    assert_banner(&tui, WAIT);
    thread::sleep(Duration::from_millis(400));

    // Both session dir names should appear in the tab bar.
    let n0 = env.dirs[0].file_name().unwrap().to_str().unwrap();
    let n1 = env.dirs[1].file_name().unwrap().to_str().unwrap();
    assert!(tui.wait_for(n0, WAIT), "first session tab not visible");
    assert!(tui.wait_for(n1, WAIT), "second session tab not visible");

    // Switch left (to first session).
    tui.key(ALT_LEFT);
    thread::sleep(Duration::from_millis(400));

    // Switch right (back to second session).
    tui.key(ALT_RIGHT);
    thread::sleep(Duration::from_millis(400));

    tui.key(ALT_Q);
    tui.wait_exit(WAIT);
}

/// 3. Rename: Alt+r, type a name, Enter → tab shows the name; state.json contains it.
#[test]
fn test_3_rename() {
    let env = Env::new();
    let mut tui = start_tui(&env);

    wizard_pick_dir(&mut tui, &env.dirs[0]);
    assert_banner(&tui, WAIT);
    thread::sleep(Duration::from_millis(400));

    // Alt+r opens the rename modal; the default input is the current label.
    tui.key(ALT_R);
    thread::sleep(Duration::from_millis(200));
    // Ctrl+U clears the input.
    tui.key(CTRL_U);
    thread::sleep(Duration::from_millis(100));
    // Type the new name.
    tui.write(b"my-renamed-session");
    thread::sleep(Duration::from_millis(100));
    tui.key(ENTER);
    thread::sleep(Duration::from_millis(400));

    // Tab bar should now show the new name.
    assert!(
        tui.wait_for("my-renamed-session", WAIT),
        "renamed session not visible in tab bar"
    );

    // state.json should persist the name.
    let state = fs::read_to_string(env.state_json()).unwrap_or_default();
    assert!(
        state.contains("my-renamed-session"),
        "state.json should contain the renamed session name"
    );

    tui.key(ALT_Q);
    tui.wait_exit(WAIT);
}

/// 4. Quit and reattach: Alt+q leaves the daemon running; restarting the TUI
///    restores sessions (banner visible from the snapshot).
#[test]
fn test_4_quit_and_reattach() {
    let env = Env::new();

    // First launch: create a session.
    {
        let mut tui = start_tui(&env);
        wizard_pick_dir(&mut tui, &env.dirs[0]);
        assert_banner(&tui, WAIT);
        thread::sleep(Duration::from_millis(500));

        tui.key(ALT_Q);
        let status = tui.wait_exit(Duration::from_secs(5)).expect("TUI did not exit");
        assert!(status.success(), "TUI should exit 0");
    }

    // Daemon must still be alive.
    thread::sleep(Duration::from_millis(300));
    assert!(env.daemon_pid().is_some(), "daemon should still be running after quit");

    // Second launch: reattach; banner must come back from the screen snapshot.
    let mut tui2 = start_tui(&env);
    assert_banner(&tui2, DAEMON_WAIT);

    let dir_name = env.dirs[0].file_name().unwrap().to_str().unwrap();
    assert!(tui2.wait_for(dir_name, WAIT), "session tab not restored");

    tui2.key(ALT_Q);
    tui2.wait_exit(WAIT);
}

/// 5. Daemon restart: SIGTERM the daemon; the TUI reconnects and re-spawns
///    dormant sessions (banner reappears).
#[test]
fn test_5_daemon_restart() {
    let env = Env::new();
    let mut tui = start_tui(&env);

    wizard_pick_dir(&mut tui, &env.dirs[0]);
    assert_banner(&tui, WAIT);
    thread::sleep(Duration::from_millis(500));

    // Kill the daemon.
    env.kill_daemon();

    // The TUI should reconnect (after ~2 s) and re-spawn the dormant session.
    // The fake claude re-runs and prints the banner again.
    assert!(
        tui.wait_for("FAKE_CLAUDE_BANNER", RECONNECT_WAIT),
        "session not re-spawned after daemon restart"
    );

    tui.key(ALT_Q);
    tui.wait_exit(WAIT);
}

/// 6. Close: Alt+x + y removes the session from the tab bar and journal.
#[test]
fn test_6_close_session() {
    let env = Env::new();
    let mut tui = start_tui(&env);

    wizard_pick_dir(&mut tui, &env.dirs[0]);
    assert_banner(&tui, WAIT);
    thread::sleep(Duration::from_millis(500));

    // Alt+x opens the close confirm modal; 'y' confirms.
    tui.key(ALT_X);
    thread::sleep(Duration::from_millis(200));
    tui.write(b"y");
    thread::sleep(Duration::from_millis(600));

    // After closing the only session the wizard opens again; dismiss it.
    tui.key(ESC);
    thread::sleep(Duration::from_millis(200));

    // state.json should no longer contain the closed session's cwd.
    let state = fs::read_to_string(env.state_json()).unwrap_or_default();
    let dir_path = env.dirs[0].to_str().unwrap();
    // The state may be an empty sessions array; it must not contain the dir.
    let has_closed_session = state.contains(dir_path)
        && serde_json::from_str::<serde_json::Value>(&state)
            .ok()
            .and_then(|v| v.get("sessions")?.as_array().cloned())
            .map(|sessions| sessions.iter().any(|s| {
                s.get("cwd").and_then(|c| c.as_str()) == Some(dir_path)
            }))
            .unwrap_or(false);
    assert!(!has_closed_session, "closed session should be removed from state.json");

    tui.key(ALT_Q);
    tui.wait_exit(WAIT);
}

/// 7. Input: keystrokes reach the session PTY and are echoed back.
#[test]
fn test_7_input_echo() {
    let env = Env::new();
    let mut tui = start_tui(&env);

    wizard_pick_dir(&mut tui, &env.dirs[0]);
    assert_banner(&tui, WAIT);
    thread::sleep(Duration::from_millis(500));

    // Type a unique phrase. The session is `exec cat`, which echoes lines.
    let phrase = "hello-from-claudio-test";
    tui.write(phrase.as_bytes());
    tui.key(ENTER);

    // The echo should appear in the rendered pane.
    assert!(
        tui.wait_for(phrase, WAIT),
        "typed input was not echoed by the session"
    );

    tui.key(ALT_Q);
    tui.wait_exit(WAIT);
}

// ── Real-claude gate ───────────────────────────────────────────────────────────

/// Gated by `CLAUDIO_E2E=1`. Spawns a real claude session (haiku), sends a
/// prompt, asserts the expected reply and idle glyph (✓), then quits.
#[test]
fn test_e2e_real_claude() {
    if std::env::var("CLAUDIO_E2E").as_deref() != Ok("1") {
        // Skip silently when not gated.
        return;
    }

    // Fresh isolated claudio state; keep the real HOME so claude can read
    // its credentials from ~/.claude.
    let id = Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("claudio-e2e-{id}"));
    let runtime_dir = root.join("run");
    let config_home = root.join("config");
    let session_dir = root.join("session");
    for p in [&runtime_dir, &config_home, &session_dir] {
        fs::create_dir_all(p).unwrap();
    }

    // PTY setup.
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
        .unwrap();

    let mut cmd = CommandBuilder::new(BINARY);
    // Isolate claudio via XDG dirs only; HOME stays real so claude finds ~/.claude.
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("XDG_CONFIG_HOME", &config_home);
    // ANTHROPIC_MODEL is inherited by the daemon → passed to every claude subprocess.
    cmd.env("ANTHROPIC_MODEL", "claude-haiku-4-5");
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("COLORTERM");

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    // Grab the PID now so the drop guard can kill the TUI process on panic.
    let child_pid = child.process_id();

    // Drop guard: kill claudio TUI + daemon and clean up temp tree.
    let runtime_dir_guard = runtime_dir.clone();
    let cleanup_root = root.clone();
    let _guard = scopeguard(move || {
        if let Some(pid) = child_pid {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let lock = runtime_dir_guard.join("claudio").join("daemon-v1.lock");
        if let Some(pid) = fs::read_to_string(&lock)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let _ = fs::remove_dir_all(&cleanup_root);
    });

    let mut writer = pair.master.take_writer().unwrap();
    let raw: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let raw2 = Arc::clone(&raw);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let _rd = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw2.lock().unwrap().extend_from_slice(&buf[..n]),
            }
        }
    });

    // Reuse the same poll-loop pattern as TuiSession::wait_for.
    let wait_for = |text: &str, timeout: Duration| -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let stripped = strip_ansi(&raw.lock().unwrap());
            if stripped.contains(text) {
                return true;
            }
            if Instant::now() >= deadline {
                let buf = strip_ansi(&raw.lock().unwrap());
                eprintln!(
                    "[e2e] timeout waiting for {:?}\nlast output (tail):\n{}",
                    text,
                    &buf[buf.len().saturating_sub(3000)..]
                );
                return false;
            }
            thread::sleep(Duration::from_millis(200));
        }
    };
    let write_bytes = |w: &mut Box<dyn Write + Send>, bytes: &[u8]| {
        let _ = w.write_all(bytes);
    };

    // ── 1. Wait for TUI to start (status bar shows "Alt+q"). ─────────────────
    assert!(wait_for("Alt+q", Duration::from_secs(30)), "TUI never started");

    // ── 2. Pick "local" in the host step, then the session directory. ────────
    write_bytes(&mut writer, ENTER);
    thread::sleep(Duration::from_millis(150));
    let path = session_dir.to_str().unwrap();
    write_bytes(&mut writer, b"\x1b[200~");
    write_bytes(&mut writer, path.as_bytes());
    write_bytes(&mut writer, b"\x1b[201~");
    thread::sleep(Duration::from_millis(200));
    write_bytes(&mut writer, ENTER);

    // ── 3. Wait for claude to be ready for input. ────────────────────────────
    //
    // At startup claude transitions to NeedsInput (not Working), so the tab
    // shows "?" not the working spinner.  A brand-new directory also triggers
    // claude's trust dialog ("Do you trust the files in this folder?"); we
    // detect that text and accept it with Enter before continuing to wait.
    //
    // We detect readiness via claude's own banner text ("Claude Code") that
    // appears in the mirrored pane content.  Watching the tab-bar glyph is
    // unreliable: ratatui diff-renders only write the changed glyph cell, so
    // "? session" never appears as a contiguous substring in the accumulated
    // raw bytes after the first full frame.
    let ready_deadline = Instant::now() + Duration::from_secs(90);
    let mut trust_pressed = false;
    loop {
        let stripped = strip_ansi(&raw.lock().unwrap());
        // Accept the trust dialog if it appears (Enter selects the default Yes).
        if !trust_pressed && stripped.contains("Do you trust") {
            write_bytes(&mut writer, ENTER);
            trust_pressed = true;
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        // Claude's banner header ("Claude Code") appears in the pane once the
        // session is fully started and ready for input.
        if stripped.contains("Claude Code") {
            break;
        }
        if Instant::now() >= ready_deadline {
            eprintln!("[e2e] timeout waiting for claude's banner; proceeding anyway");
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    // Short settle so claude's event loop has processed any preceding output.
    thread::sleep(Duration::from_millis(500));

    // ── 4. Send the prompt via bracketed paste, then Enter separately. ────────
    write_bytes(&mut writer, b"\x1b[200~");
    write_bytes(&mut writer, b"Reply with exactly: TUI_E2E_OK");
    write_bytes(&mut writer, b"\x1b[201~");
    thread::sleep(Duration::from_millis(200));
    write_bytes(&mut writer, ENTER);

    // ── 5. Wait for the expected answer (generous timeout for haiku). ─────────
    assert!(
        wait_for("TUI_E2E_OK", Duration::from_secs(90)),
        "claude never replied with TUI_E2E_OK"
    );

    // ── 6. After the response the session should return to Idle (✓ glyph). ────
    assert!(
        wait_for("✓", Duration::from_secs(30)),
        "session didn't return to idle after reply"
    );

    // ── 7. Quit cleanly. ──────────────────────────────────────────────────────
    write_bytes(&mut writer, ALT_Q);
    let quit_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        assert!(Instant::now() < quit_deadline, "TUI did not exit after Alt+q");
        thread::sleep(Duration::from_millis(100));
    }
}

// ── SSH TUI test ───────────────────────────────────────────────────────────────

/// Gated by `CLAUDIO_SSH_TEST_HOST`. Drives the full TUI wizard against a real
/// SSH host: host step → directory step → spawn claude. Then kills the session
/// and verifies that `hosts.json` records the host as MRU.
///
/// The remote host must have `claude` installed and SSH key auth (BatchMode).
#[test]
fn test_ssh_remote_session() {
    let host = match std::env::var("CLAUDIO_SSH_TEST_HOST") {
        Ok(h) if !h.is_empty() => h,
        _ => {
            println!("SKIP test_ssh_remote_session: set CLAUDIO_SSH_TEST_HOST to run");
            return;
        }
    };

    // Isolate claudio state but keep real HOME so SSH credentials are available.
    let id = Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("claudio-ssh-tui-test-{id}"));
    let runtime_dir = root.join("run");
    let config_home = root.join("config");
    for p in [&runtime_dir, &config_home] {
        fs::create_dir_all(p).expect("create test dir");
    }
    let hosts_json = config_home.join("claudio").join("hosts.json");

    // PTY setup.
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
        .expect("open pty");

    let mut cmd = CommandBuilder::new(BINARY);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("XDG_CONFIG_HOME", &config_home);
    // Keep real HOME for SSH credentials and known_hosts.
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("COLORTERM");

    let mut child = pair.slave.spawn_command(cmd).expect("spawn claudio");
    let child_pid = child.process_id();

    // Drop guards: kill the TUI and the local daemon on exit.
    let runtime_dir_g = runtime_dir.clone();
    let cleanup_root = root.clone();
    let _guard = scopeguard(move || {
        if let Some(pid) = child_pid {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let lock = runtime_dir_g.join("claudio").join("daemon-v1.lock");
        if let Some(pid) =
            fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<u32>().ok())
        {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let _ = fs::remove_dir_all(&cleanup_root);
    });

    let mut writer = pair.master.take_writer().expect("pty writer");
    let raw: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let raw2 = Arc::clone(&raw);
    let mut reader = pair.master.try_clone_reader().expect("pty reader");
    let _rd = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw2.lock().unwrap().extend_from_slice(&buf[..n]),
            }
        }
    });

    let wait_for = |text: &str, timeout: Duration| -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let stripped = strip_ansi(&raw.lock().unwrap());
            if stripped.contains(text) {
                return true;
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "[ssh_tui] TIMEOUT waiting for {:?}\nlast output:\n{}",
                    text,
                    &stripped[stripped.len().saturating_sub(2000)..]
                );
                return false;
            }
            thread::sleep(Duration::from_millis(200));
        }
    };

    // ── 1. Wait for TUI to start. ────────────────────────────────────────────
    assert!(wait_for("Alt+q", Duration::from_secs(30)), "TUI never started");

    // ── 2. Host step: type the host name and press Enter. ────────────────────
    // The wizard opens with the host step. Type the remote hostname to filter
    // the list; it will be pre-selected if it is in ~/.ssh/config, or accepted
    // as a free-form target if not.
    let _ = writer.write_all(host.as_bytes());
    thread::sleep(Duration::from_millis(200));
    let _ = writer.write_all(ENTER);

    // Bootstrap may take several seconds on first run (binary upload).
    // Wait for the directory step: the popup title includes "on {host}".
    // (The full title is "New session on {host} (Tab complete…)" but ANSI
    // stripping can split "New session" from "on {host}" at cell boundaries.)
    let dir_step_text = format!("on {host}");
    assert!(
        wait_for(&dir_step_text, Duration::from_secs(90)),
        "directory step never appeared (bootstrap or connect failed)"
    );

    // ── 3. Directory step: type /tmp and press Enter. ────────────────────────
    let _ = writer.write_all(b"\x1b[200~");
    let _ = writer.write_all(b"/tmp");
    let _ = writer.write_all(b"\x1b[201~");
    thread::sleep(Duration::from_millis(300));
    let _ = writer.write_all(ENTER);

    // ── 4. Resume step: pick "+ New session" (index 0, just press Enter). ───
    // If /tmp has no claude sessions it auto-spawns; if it does the picker appears.
    // Either way, pressing Enter is correct.
    thread::sleep(Duration::from_millis(1500));
    let _ = writer.write_all(ENTER);

    // ── 5. Wait for claude's UI (or trust dialog). ───────────────────────────
    let ready_deadline = Instant::now() + Duration::from_secs(90);
    let mut trust_pressed = false;
    loop {
        let stripped = strip_ansi(&raw.lock().unwrap());
        if !trust_pressed && stripped.contains("Do you trust") {
            let _ = writer.write_all(ENTER);
            trust_pressed = true;
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        if stripped.contains("Claude Code") {
            break;
        }
        if Instant::now() >= ready_deadline {
            eprintln!("[ssh_tui] timeout waiting for claude banner, continuing");
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(500));

    // ── 6. Assert tab label contains @host. ──────────────────────────────────
    let stripped = strip_ansi(&raw.lock().unwrap());
    assert!(
        stripped.contains(&format!("@{host}")),
        "tab bar should show @{host}\nstripped:\n{}",
        &stripped[stripped.len().saturating_sub(2000)..]
    );

    // ── 7. Assert hosts.json lists the host first. ────────────────────────────
    // Allow up to 2 s for the async write to complete.
    let hosts_ok = {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut ok = false;
        while Instant::now() < deadline {
            if let Ok(content) = fs::read_to_string(&hosts_json) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    let first = v["hosts"].as_array().and_then(|a| a.first()).and_then(|h| h.as_str());
                    if first == Some(host.as_str()) {
                        ok = true;
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        ok
    };
    assert!(hosts_ok, "hosts.json should list {host} first after connecting");

    // ── 8. Kill the session (Alt+x, y) so nothing is left running remotely. ──
    let _ = writer.write_all(ALT_X);
    thread::sleep(Duration::from_millis(300));
    let _ = writer.write_all(b"y");
    thread::sleep(Duration::from_millis(600));

    // Dismiss the wizard that opens after the last session is closed.
    let _ = writer.write_all(ESC);
    thread::sleep(Duration::from_millis(200));

    // ── 9. Quit cleanly. ──────────────────────────────────────────────────────
    let _ = writer.write_all(ALT_Q);
    let quit_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        assert!(Instant::now() < quit_deadline, "TUI did not exit after Alt+q");
        thread::sleep(Duration::from_millis(100));
    }
}

// ── Scope guard ───────────────────────────────────────────────────────────────

/// Minimal scope guard: runs `f` when dropped (used in the E2E test).
struct ScopeGuard<F: FnOnce()>(Option<F>);
impl<F: FnOnce()> Drop for ScopeGuard<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}
fn scopeguard<F: FnOnce()>(f: F) -> ScopeGuard<F> {
    ScopeGuard(Some(f))
}
