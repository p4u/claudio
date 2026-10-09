//! Shared test harness for claudio integration tests.
//!
//! Provides:
//!  - [`ManagerHarness`]: isolated dirs, fake claude, daemon lifecycle.
//!  - [`TuiProcess`]: claudio TUI in a PTY backed by a live VT screen model.
//!  - [`Region`]: named screen regions for assertions.
//!
//! Key design rules (per review findings T1–T3):
//!  - All assertions check the CURRENT rendered screen (alacritty_terminal::Term),
//!    never accumulated raw bytes.  A restart test therefore sees the NEW pane,
//!    not the pre-restart one.
//!  - All wait calls panic on timeout; nothing logs-and-continues.
//!  - No fixed sleeps as readiness barriers.
//!  - Default fixture clears every CLAUDIO_* and CLAUDIO_PROXY_URL env var so
//!    the offline suite never touches the network.
//!  - Tmp roots are short (/tmp/cl-<8hex>) to stay under the macOS Unix-socket
//!    path-length limit.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs, os::unix::fs::PermissionsExt, thread};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config as AlacConfig, Term};
use alacritty_terminal::vte::ansi;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

// ── Public constants ──────────────────────────────────────────────────────────

pub const BINARY: &str = env!("CARGO_BIN_EXE_claudio");

/// Default assertion timeout.
pub const WAIT: Duration = Duration::from_secs(10);
/// Longer timeout when the daemon must be (re-)started.
pub const DAEMON_WAIT: Duration = Duration::from_secs(20);
/// Timeout for reconnect after a daemon kill.
pub const RECONNECT_WAIT: Duration = Duration::from_secs(30);

/// PTY dimensions used by every test.
pub const PTY_ROWS: u16 = 40;
pub const PTY_COLS: u16 = 120;

// ── Key sequences (VT100/xterm) ───────────────────────────────────────────────

pub const ALT_LEFT: &[u8] = b"\x1b[1;3D";
pub const ALT_RIGHT: &[u8] = b"\x1b[1;3C";
pub const ALT_N: &[u8] = b"\x1bn";
pub const ALT_R: &[u8] = b"\x1br";
pub const ALT_X: &[u8] = b"\x1bx";
pub const ALT_Q: &[u8] = b"\x1bq";
pub const ALT_G: &[u8] = b"\x1bg";
pub const ALT_H: &[u8] = b"\x1bh";
pub const ENTER: &[u8] = b"\r";
pub const CTRL_U: &[u8] = b"\x15";
pub const ESC: &[u8] = b"\x1b";
pub const UP_ARROW: &[u8] = b"\x1b[A";
pub const DOWN_ARROW: &[u8] = b"\x1b[B";

// ── Region ───────────────────────────────────────────────────────────────────

/// Named regions of the claudio TUI screen.
///
/// - `TabBar`:   row 0  (session tabs).
/// - `Pane`:     rows 1 .. rows-2  (session terminal output).
/// - `StatusBar`:last row  (key hints and mode indicators).
/// - `Screen`:   the entire terminal including all rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    TabBar,
    Pane,
    StatusBar,
    Screen,
}

// ── No-op EventListener for alacritty_terminal ───────────────────────────────

struct NoopListener;

impl EventListener for NoopListener {
    fn send_event(&self, _event: alacritty_terminal::event::Event) {}
}

// ── TermDims ─────────────────────────────────────────────────────────────────

struct TermDims {
    rows: usize,
    cols: usize,
}

impl Dimensions for TermDims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

// ── ScreenModel ───────────────────────────────────────────────────────────────

/// A live VT screen model.  The reader thread feeds PTY bytes here; assertions
/// read the current rendered state.
pub struct ScreenModel {
    term: Term<NoopListener>,
    processor: ansi::Processor,
    rows: u16,
    cols: u16,
}

impl ScreenModel {
    pub fn new(rows: u16, cols: u16) -> Self {
        let size = TermDims {
            rows: rows as usize,
            cols: cols as usize,
        };
        let term = Term::new(AlacConfig::default(), &size, NoopListener);
        Self {
            term,
            processor: ansi::Processor::new(),
            rows,
            cols,
        }
    }

    /// Feed raw PTY bytes into the VT state machine.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Render a single row to a string (trailing spaces stripped).
    pub fn row_text(&self, row: usize) -> String {
        if row >= self.rows as usize {
            return String::new();
        }
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        let line_idx = Line(row as i32 - offset);
        let cols = self.cols as usize;
        let mut s = String::with_capacity(cols);
        for c in 0..cols {
            let cell = &grid[line_idx][Column(c)];
            let ch = if cell.c == '\0' { ' ' } else { cell.c };
            s.push(ch);
        }
        // Trim trailing spaces for readability, but keep at least one char.
        s.trim_end().to_owned()
    }

    /// Text for a named region.  Rows are joined with newlines.
    pub fn region_text(&self, region: Region) -> String {
        let rows = self.rows as usize;
        match region {
            Region::TabBar => self.row_text(0),
            Region::StatusBar => self.row_text(rows.saturating_sub(1)),
            Region::Pane => (1..rows.saturating_sub(1))
                .map(|r| self.row_text(r))
                .collect::<Vec<_>>()
                .join("\n"),
            Region::Screen => (0..rows)
                .map(|r| self.row_text(r))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Whether `text` appears anywhere in `region`.
    pub fn contains(&self, text: &str, region: Region) -> bool {
        self.region_text(region).contains(text)
    }
}

// ── TuiProcess ───────────────────────────────────────────────────────────────

/// A running `claudio` TUI in a PTY.
///
/// PTY output is fed into a live `ScreenModel`; all assertions operate on the
/// current rendered screen, not accumulated bytes.
pub struct TuiProcess {
    writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    screen: Arc<Mutex<ScreenModel>>,
    _reader: thread::JoinHandle<()>,
}

impl TuiProcess {
    /// Spawn `claudio` in a PTY with the given environment.
    pub fn spawn(cmd: CommandBuilder) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: PTY_ROWS,
                cols: PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");

        let child = pair.slave.spawn_command(cmd).expect("spawn claudio");
        let writer = pair.master.take_writer().expect("pty writer");

        let screen = Arc::new(Mutex::new(ScreenModel::new(PTY_ROWS, PTY_COLS)));
        let screen_clone = Arc::clone(&screen);
        let mut reader = pair.master.try_clone_reader().expect("pty reader");

        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut s) = screen_clone.lock() {
                            s.feed(&buf[..n]);
                        }
                    }
                }
            }
        });

        TuiProcess {
            writer,
            child,
            screen,
            _reader: reader_thread,
        }
    }

    /// Send raw bytes to the PTY (keystrokes or escape sequences).
    pub fn send_keys(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
    }

    /// Send `text` via the bracketed-paste protocol so it arrives as a single
    /// `Event::Paste`, avoiding character-at-a-time key processing.
    pub fn send_paste(&mut self, text: &str) {
        self.send_keys(b"\x1b[200~");
        self.send_keys(text.as_bytes());
        self.send_keys(b"\x1b[201~");
    }

    /// Current rendered text for a region.
    pub fn screen_text(&self, region: Region) -> String {
        self.screen.lock().expect("screen lock").region_text(region)
    }

    /// Wait until `pattern` appears in `region`, panicking on timeout.
    ///
    /// Uses the CURRENT rendered screen at each poll — not accumulated bytes.
    pub fn wait_for(&self, pattern: &str, region: Region, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .screen
                .lock()
                .expect("screen lock")
                .contains(pattern, region)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "wait_for timeout: {:?} not found in {:?}\nscreen:\n{}",
                pattern,
                region,
                self.screen.lock().unwrap().region_text(Region::Screen),
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Wait until `pred` returns true for the current screen, panicking on timeout.
    pub fn wait_until<F>(&self, pred: F, timeout: Duration)
    where
        F: Fn(&ScreenModel) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            if pred(&self.screen.lock().expect("screen lock")) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "wait_until timeout\nscreen:\n{}",
                self.screen.lock().unwrap().region_text(Region::Screen),
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Send Alt+q and wait for the process to exit, panicking on timeout.
    pub fn quit(&mut self, timeout: Duration) {
        self.send_keys(ALT_Q);
        self.wait_exit(timeout);
    }

    /// Wait for the TUI process to exit, panicking on timeout.
    pub fn wait_exit(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "wait_exit timeout: TUI did not exit; screen:\n{}",
                self.screen_text(Region::Screen)
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for TuiProcess {
    fn drop(&mut self) {
        // Best-effort kill so the PTY fds don't leak.
        let _ = self.child.kill();
    }
}

// ── ManagerHarness ────────────────────────────────────────────────────────────

/// Isolated on-disk environment for one test.
///
/// Owns:
///  - Private `XDG_RUNTIME_DIR` and `XDG_CONFIG_HOME` under a short `/tmp/cl-*` root.
///  - A fake `claude` script that prints a per-spawn nonce then execs `cat`.
///  - Two session directories used in multi-session tests.
///
/// Kills the daemon and removes the root in `Drop`.
pub struct ManagerHarness {
    pub root: PathBuf,
    pub runtime_dir: PathBuf,
    pub config_home: PathBuf,
    pub home: PathBuf,
    pub fake_claude: PathBuf,
    /// Two session directories for multi-session tests.
    pub dirs: [PathBuf; 2],
}

impl ManagerHarness {
    /// Build a harness with the default fake claude (nonce banner, then exec cat).
    ///
    /// Clears all `CLAUDIO_*` and `CLAUDIO_PROXY_URL` env vars so the offline
    /// test suite never touches the network.
    pub fn new() -> Self {
        let root = short_tmp_root();
        let runtime_dir = root.join("run");
        let config_home = root.join("cfg");
        let home = root.join("home");
        let fake_claude = root.join("fake-claude");
        let dirs = [root.join("sa"), root.join("sb")];

        for p in [&runtime_dir, &config_home, &home, &dirs[0], &dirs[1]] {
            fs::create_dir_all(p).expect("create test dir");
        }

        let h = ManagerHarness {
            root,
            runtime_dir,
            config_home,
            home,
            fake_claude,
            dirs,
        };
        h.write_default_fake_claude();
        h
    }

    /// Write (or overwrite) the default fake claude script.
    ///
    /// The script:
    ///  - Returns a clean `--version` string (no env dump, no nonce).
    ///  - On normal invocation prints a unique per-spawn nonce banner then
    ///    execs `cat` (which echoes stdin, useful for input-echo tests).
    pub fn write_default_fake_claude(&self) {
        let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "claude 0.0.0-fake"
    exit 0
fi
NONCE="${$}__${RANDOM}${RANDOM}"
echo "FAKE_CLAUDE_BANNER pid=$$ nonce=$NONCE args=$*"
exec cat
"#;
        fs::write(&self.fake_claude, script).expect("write fake claude");
        fs::set_permissions(&self.fake_claude, fs::Permissions::from_mode(0o755))
            .expect("chmod fake claude");
    }

    /// Write an env-dumping fake claude (for proxy tests).
    ///
    /// Dumps env to `env_file`, then follows the same nonce+cat protocol.
    /// Still returns a clean `--version` (no env dump).
    pub fn write_env_dumping_fake_claude(&self, env_file: &std::path::Path) {
        let env_path = env_file.display().to_string();
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "claude 0.0.0-fake"
    exit 0
fi
env > "{env_path}"
NONCE="${{$}}__${{RANDOM}}${{RANDOM}}"
echo "FAKE_CLAUDE_BANNER pid=$$ nonce=$NONCE args=$*"
exec cat
"#
        );
        fs::write(&self.fake_claude, &script).expect("write env-dumping fake claude");
        fs::set_permissions(&self.fake_claude, fs::Permissions::from_mode(0o755))
            .expect("chmod fake claude");
    }

    /// Path to the daemon lock file (holds the daemon PID after startup).
    pub fn lock_file(&self) -> PathBuf {
        self.runtime_dir.join("claudio").join("daemon-v1.lock")
    }

    /// Path to the TUI client's state.json.
    pub fn state_json(&self) -> PathBuf {
        self.config_home.join("claudio").join("state.json")
    }

    /// Read the daemon PID from the lock file.  Returns `None` if not yet written.
    pub fn daemon_pid(&self) -> Option<u32> {
        fs::read_to_string(self.lock_file())
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }

    /// SIGTERM the daemon and reap it.  Blocks up to ~400 ms for it to die.
    pub fn kill_daemon(&self) {
        if let Some(pid) = self.daemon_pid() {
            // SAFETY: kill(2) has no preconditions.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            // Wait for it to actually die.
            let deadline = Instant::now() + Duration::from_millis(400);
            loop {
                // SAFETY: waitpid with WNOHANG.
                let r = unsafe {
                    libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG)
                };
                if r != 0 {
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Read and parse state.json, returning an empty object if it doesn't exist.
    pub fn read_state(&self) -> serde_json::Value {
        fs::read_to_string(self.state_json())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Object(Default::default()))
    }

    /// Wait until `pred` is true for the state.json value.  Panics on timeout.
    pub fn wait_state<F>(&self, pred: F, timeout: Duration)
    where
        F: Fn(&serde_json::Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let v = self.read_state();
            if pred(&v) {
                return;
            }
            assert!(Instant::now() < deadline, "wait_state timeout");
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Apply the isolated env vars to a `CommandBuilder`.
    ///
    /// Clears every `CLAUDIO_*` inherited env var, then sets:
    ///  - `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`, `HOME`
    ///  - `CLAUDIO_CLAUDE_PATH` → fake claude
    ///  - `ANTHROPIC_API_KEY` → dummy (prevents real API calls)
    ///  - `TERM` → xterm-256color
    ///  - Removes `COLORTERM` to avoid fancy-mode probes
    pub fn apply(&self, cmd: &mut CommandBuilder) {
        // Clear all CLAUDIO_* inherited vars (proxy URL, model overrides, etc.)
        // This ensures the offline suite never touches the network.
        for (k, _) in std::env::vars() {
            if k.starts_with("CLAUDIO_") || k == "CLAUDIO_PROXY_URL" {
                cmd.env_remove(&k);
            }
        }
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("HOME", &self.home);
        cmd.env("CLAUDIO_CLAUDE_PATH", &self.fake_claude);
        cmd.env("ANTHROPIC_API_KEY", "test-key-not-real");
        cmd.env("TERM", "xterm-256color");
        cmd.env_remove("COLORTERM");
    }

    /// Start a `claudio` TUI process with the default env and wait until the
    /// status bar shows "Alt+q" (the TUI is fully up).
    pub fn start_tui(&self) -> TuiProcess {
        self.start_tui_with(|_| {})
    }

    /// Start a `claudio` TUI with a custom setup closure applied AFTER the
    /// default env vars.
    pub fn start_tui_with<F: FnOnce(&mut CommandBuilder)>(&self, setup: F) -> TuiProcess {
        let mut cmd = CommandBuilder::new(BINARY);
        self.apply(&mut cmd);
        setup(&mut cmd);
        let tui = TuiProcess::spawn(cmd);
        tui.wait_for("Alt+q", Region::StatusBar, DAEMON_WAIT);
        tui
    }
}

impl Drop for ManagerHarness {
    fn drop(&mut self) {
        self.kill_daemon();
        let _ = fs::remove_dir_all(&self.root);
    }
}

// ── Wizard helper ─────────────────────────────────────────────────────────────

/// Navigate the new-session wizard: confirm "local" host (Enter), then paste
/// the directory path and confirm (Enter).
///
/// Waits for the wizard to close (the fake claude banner to appear in the pane)
/// before returning, so callers don't need a sleep.
pub fn wizard_pick_dir(tui: &mut TuiProcess, dir: &PathBuf) {
    let path = dir.to_str().expect("non-UTF-8 dir");
    // Host step: confirm "local" (pre-selected).
    tui.send_keys(ENTER);
    // Directory step: paste the path and confirm.
    // Wait briefly for the host step to process before pasting.
    // We detect the directory prompt by waiting for any state change.
    // Short sleep here is unavoidable because the wizard renders asynchronously
    // after Enter. We keep it minimal (50 ms is well below any real latency).
    thread::sleep(Duration::from_millis(80));
    tui.send_paste(path);
    thread::sleep(Duration::from_millis(50));
    tui.send_keys(ENTER);
    // Wait for the session to start (banner in pane).
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);
}

// ── Nonce helpers ─────────────────────────────────────────────────────────────

/// Extract the `nonce=...` value from pane text.
///
/// The fake claude banner looks like:
///   `FAKE_CLAUDE_BANNER pid=NNN nonce=NNN__NNNN args=...`
pub fn extract_nonce(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        if let Some(n) = word.strip_prefix("nonce=") {
            // Trim any trailing whitespace/control chars.
            let n = n.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
            if !n.is_empty() {
                return Some(n.to_owned());
            }
        }
    }
    None
}

/// Read the current nonce from the pane.  Panics if no banner is visible.
pub fn current_nonce(tui: &TuiProcess) -> String {
    let text = tui.screen_text(Region::Pane);
    extract_nonce(&text).expect("no nonce in pane — is the banner visible?")
}

// ── Claude project-dir cleanup guard (Bug 3) ─────────────────────────────────

/// Encode a cwd path the same way `claude::projects::project_dir` does:
/// replace every non-alphanumeric character with `-`.
///
/// Replicated here because integration tests cannot import the binary crate.
/// Keep in sync with `src/claude/projects.rs::encode_cwd`.
pub fn encode_cwd(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// On drop, removes `~/.claude/projects/<encoded-cwd>/` for the test's temp
/// session directory.
///
/// This prevents real-claude E2E tests from permanently polluting the user's
/// `~/.claude/projects/` with stale test transcripts.  Only the exact
/// directory corresponding to `cwd` is removed — nothing else.
pub struct ClaudeProjectGuard {
    pub project_dir: PathBuf,
}

impl ClaudeProjectGuard {
    /// Create a guard for a test whose session was run in `session_cwd`.
    pub fn new(session_cwd: &std::path::Path) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let encoded = encode_cwd(session_cwd);
        let project_dir = home.join(".claude").join("projects").join(encoded);
        ClaudeProjectGuard { project_dir }
    }
}

impl Drop for ClaudeProjectGuard {
    fn drop(&mut self) {
        if self.project_dir.exists() {
            let _ = fs::remove_dir_all(&self.project_dir);
        }
    }
}

// ── Short tmp root ────────────────────────────────────────────────────────────

/// Create a short temp path `/tmp/cl-<8hex>` to stay well under the macOS
/// Unix-socket path length limit (104 chars).
fn short_tmp_root() -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        .hash(&mut h);
    std::thread::current().id().hash(&mut h);
    // Extra entropy from the process ID.
    std::process::id().hash(&mut h);
    let id = format!("{:08x}", h.finish() as u32);
    std::env::temp_dir().join(format!("cl-{id}"))
}
