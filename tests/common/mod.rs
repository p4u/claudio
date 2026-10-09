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
use alacritty_terminal::term::cell::Flags;
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
pub const RECONNECT_WAIT: Duration = Duration::from_secs(90);

/// PTY dimensions used by every test.
pub const PTY_ROWS: u16 = 40;
pub const PTY_COLS: u16 = 120;

// ── Key sequences (VT100/xterm) ───────────────────────────────────────────────

pub const ALT_LEFT: &[u8] = b"\x1b[1;3D";
pub const ALT_RIGHT: &[u8] = b"\x1b[1;3C";
/// Kitty-protocol `Alt+Shift+<digit>`: `CSI <codepoint>;<1+shift(1)+alt(2)> u`.
pub const ALT_SHIFT_1: &[u8] = b"\x1b[49;4u";
pub const ALT_SHIFT_2: &[u8] = b"\x1b[50;4u";
pub const ALT_N: &[u8] = b"\x1bn";
pub const ALT_R: &[u8] = b"\x1br";
pub const ALT_X: &[u8] = b"\x1bx";
pub const ALT_Q: &[u8] = b"\x1bq";
pub const ALT_G: &[u8] = b"\x1bg";
pub const ALT_H: &[u8] = b"\x1bh";
pub const ALT_C: &[u8] = b"\x1bc";
pub const ALT_L: &[u8] = b"\x1bl";
pub const ALT_E: &[u8] = b"\x1be";
pub const ENTER: &[u8] = b"\r";
pub const CTRL_U: &[u8] = b"\x15";
pub const ESC: &[u8] = b"\x1b";
pub const UP_ARROW: &[u8] = b"\x1b[A";
pub const DOWN_ARROW: &[u8] = b"\x1b[B";
pub const RIGHT_ARROW: &[u8] = b"\x1b[C";
pub const LEFT_ARROW: &[u8] = b"\x1b[D";

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
            Region::StatusBar => {
                let start = rows.saturating_sub(2);
                (start..rows)
                    .map(|r| self.row_text(r))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Region::Pane => (1..rows.saturating_sub(2))
                .map(|r| self.row_text(r))
                .collect::<Vec<_>>()
                .join("\n"),
            Region::Screen => (0..rows)
                .map(|r| self.row_text(r))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// The foreground colour of the first cell of the first occurrence of
    /// `needle` on screen, as a named/indexed/RGB colour.
    pub fn fg_of(&self, needle: &str) -> Option<ansi::Color> {
        let grid = self.term.grid();
        (0..self.rows as usize).find_map(|row| {
            let at = self.row_text(row).find(needle)?;
            let col = self.row_text(row)[..at].chars().count();
            Some(grid[Line(row as i32)][Column(col)].fg)
        })
    }

    /// Whether `text` appears anywhere in `region`.
    pub fn contains(&self, text: &str, region: Region) -> bool {
        self.region_text(region).contains(text)
    }

    /// Export the current cell grid as an SVG string.
    ///
    /// Renders all cells with their colours (Catppuccin Mocha palette), bold,
    /// italic, and inverse attributes.  Wide-char spacer cells are skipped.
    /// The SVG is wrapped in a rounded window frame with a macOS-style title
    /// bar and three traffic-light dots.
    ///
    /// Convert to PNG with:
    ///   `inkscape --export-type=png --export-dpi=192 -o out.png in.svg`
    pub fn export_svg(&self) -> String {
        // ── Geometry ──────────────────────────────────────────────────────────
        const CHAR_W: f64 = 7.8;
        const LINE_H: f64 = 19.0;
        const TITLE_H: f64 = 44.0; // title-bar height (traffic lights)
        const PAD_L: f64 = 8.0;    // left padding inside window
        const PAD_R: f64 = 8.0;    // right padding inside window
        const PAD_B: f64 = 8.0;    // bottom padding inside window
        const SHADOW: f64 = 8.0;   // drop-shadow offset (right+down)

        const DEFAULT_BG: &str = "#1e1e2e";
        const DEFAULT_FG: &str = "#cdd6f4";
        const TITLE_BG: &str = "#181825";

        let rows = self.rows as usize;
        let cols = self.cols as usize;
        let term_w = cols as f64 * CHAR_W;
        let term_h = rows as f64 * LINE_H;
        let win_w = term_w + PAD_L + PAD_R;
        let win_h = term_h + TITLE_H + PAD_B;
        let svg_w = win_w + SHADOW;
        let svg_h = win_h + SHADOW;

        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;

        let mut bg_rects = String::new();
        let mut text_rows = String::new();

        for row in 0..rows {
            let line = Line(row as i32 - offset);

            // ── Background-colour runs ─────────────────────────────────────
            let mut col = 0usize;
            while col < cols {
                let cell = &grid[line][Column(col)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    col += 1;
                    continue;
                }
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                let span = if wide { 2 } else { 1 };

                let (mut fg, mut bg) = (cell.fg, cell.bg);
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let bg_hex = svg_color(bg, DEFAULT_FG, DEFAULT_BG, false, false);

                // Extend the run while BG colour is the same.
                let run_start = col;
                let mut run_end = col + span;
                while run_end < cols {
                    let c2 = &grid[line][Column(run_end)];
                    if c2.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        run_end += 1;
                        continue;
                    }
                    let (mut f2, mut b2) = (c2.fg, c2.bg);
                    if c2.flags.contains(Flags::INVERSE) {
                        std::mem::swap(&mut f2, &mut b2);
                    }
                    if svg_color(b2, DEFAULT_FG, DEFAULT_BG, false, false) != bg_hex {
                        break;
                    }
                    run_end += if c2.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                }

                if bg_hex != DEFAULT_BG {
                    let x = PAD_L + run_start as f64 * CHAR_W;
                    let y = TITLE_H + row as f64 * LINE_H;
                    let w = (run_end - run_start) as f64 * CHAR_W;
                    bg_rects.push_str(&format!(
                        "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{LINE_H:.1}\" fill=\"{bg_hex}\"/>\n"
                    ));
                }
                col = run_end;
            }

            // ── Text runs ─────────────────────────────────────────────────
            let ty = TITLE_H + row as f64 * LINE_H + LINE_H * 0.78;
            text_rows.push_str(&format!("<text y=\"{ty:.1}\" class=\"t\" xml:space=\"preserve\">"));

            col = 0;
            while col < cols {
                let cell = &grid[line][Column(col)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    col += 1;
                    continue;
                }
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                let span = if wide { 2 } else { 1 };

                let (mut fg, mut bg) = (cell.fg, cell.bg);
                let flags = cell.flags;
                if flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let fg_hex =
                    svg_color(fg, DEFAULT_FG, DEFAULT_BG, flags.contains(Flags::BOLD), flags.contains(Flags::DIM));
                let bold = flags.contains(Flags::BOLD);
                let italic = flags.contains(Flags::ITALIC);

                // Extend run while same fg + style.
                let run_start = col;
                let mut run_end = col + span;
                while run_end < cols {
                    let c2 = &grid[line][Column(run_end)];
                    if c2.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        run_end += 1;
                        continue;
                    }
                    let f2 = c2.flags;
                    let (mut fg2, mut bg2) = (c2.fg, c2.bg);
                    if f2.contains(Flags::INVERSE) {
                        std::mem::swap(&mut fg2, &mut bg2);
                    }
                    let fg2_hex = svg_color(
                        fg2,
                        DEFAULT_FG,
                        DEFAULT_BG,
                        f2.contains(Flags::BOLD),
                        f2.contains(Flags::DIM),
                    );
                    if fg2_hex != fg_hex
                        || f2.contains(Flags::BOLD) != bold
                        || f2.contains(Flags::ITALIC) != italic
                    {
                        break;
                    }
                    run_end += if c2.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                }

                // Collect text, escaping XML specials.
                let mut run_text = String::new();
                for c in run_start..run_end {
                    if c >= cols { break; }
                    let ch = grid[line][Column(c)].c;
                    if grid[line][Column(c)].flags.contains(Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    let ch = if ch == '\0' { ' ' } else { ch };
                    match ch {
                        '&' => run_text.push_str("&amp;"),
                        '<' => run_text.push_str("&lt;"),
                        '>' => run_text.push_str("&gt;"),
                        '"' => run_text.push_str("&quot;"),
                        c => run_text.push(c),
                    }
                }

                if !run_text.trim().is_empty() {
                    let x = PAD_L + run_start as f64 * CHAR_W;
                    let mut style = format!("fill:{fg_hex}");
                    if bold { style.push_str(";font-weight:bold"); }
                    if italic { style.push_str(";font-style:italic"); }
                    text_rows.push_str(&format!(
                        "<tspan x=\"{x:.1}\" style=\"{style}\">{run_text}</tspan>"
                    ));
                }

                col = run_end;
            }

            text_rows.push_str("</text>\n");
        }

        // Build SVG using explicit string concat to avoid raw-string "#color" delimiter issues.
        let mut svg = String::with_capacity(512 * 1024);
        svg.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        svg.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{svg_w:.0}\" height=\"{svg_h:.0}\" viewBox=\"0 0 {svg_w:.0} {svg_h:.0}\">\n"
        ));
        svg.push_str("  <defs>\n");
        svg.push_str("    <style>.t { font-family: 'JetBrains Mono', 'Cascadia Code', 'Fira Code', 'Source Code Pro', 'Consolas', 'Courier New', monospace; font-size: 13px; font-feature-settings: 'liga' 0; white-space: pre; }</style>\n");
        svg.push_str(&format!(
            "    <clipPath id=\"wc\"><rect width=\"{win_w:.0}\" height=\"{win_h:.0}\" rx=\"12\" ry=\"12\"/></clipPath>\n"
        ));
        svg.push_str("  </defs>\n");
        // Drop shadow
        svg.push_str(&format!(
            "  <rect x=\"{SHADOW:.0}\" y=\"{SHADOW:.0}\" width=\"{win_w:.0}\" height=\"{win_h:.0}\" rx=\"12\" ry=\"12\" fill=\"rgba(0,0,0,0.40)\"/>\n"
        ));
        // Window background (title bar colour)
        svg.push_str(&format!(
            "  <rect width=\"{win_w:.0}\" height=\"{win_h:.0}\" rx=\"12\" ry=\"12\" fill=\"{TITLE_BG}\"/>\n"
        ));
        // Traffic lights
        svg.push_str("  <circle cx=\"20\" cy=\"22\" r=\"6\" fill=\"#ff5f57\"/>\n");
        svg.push_str("  <circle cx=\"40\" cy=\"22\" r=\"6\" fill=\"#febc2e\"/>\n");
        svg.push_str("  <circle cx=\"60\" cy=\"22\" r=\"6\" fill=\"#28c840\"/>\n");
        // Terminal area (clipped to window frame)
        svg.push_str("  <g clip-path=\"url(#wc)\">\n");
        svg.push_str(&format!(
            "    <rect y=\"{TITLE_H:.0}\" width=\"{win_w:.0}\" height=\"{term_h:.0}\" fill=\"{DEFAULT_BG}\"/>\n"
        ));
        svg.push_str(&bg_rects);
        svg.push_str(&text_rows);
        svg.push_str("  </g>\n");
        svg.push_str("</svg>\n");
        svg
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
    /// Spawn `claudio` in a PTY at a custom size.
    ///
    /// Used by the screenshot test which needs a wider terminal (140 cols).
    /// Prefer [`Self::spawn`] for regular tests that use `PTY_ROWS`/`PTY_COLS`.
    pub fn spawn_sized(cmd: CommandBuilder, rows: u16, cols: u16) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pty");

        let child = pair.slave.spawn_command(cmd).expect("spawn claudio");
        let writer = pair.master.take_writer().expect("pty writer");

        let screen = Arc::new(Mutex::new(ScreenModel::new(rows, cols)));
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

    /// Export the current screen state as an SVG string.
    ///
    /// See [`ScreenModel::export_svg`] for details.
    pub fn screen_svg(&self) -> String {
        self.screen.lock().expect("screen lock").export_svg()
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

    /// Write a fake claude that records every real launch (not `--version`)
    /// by creating `marker`, then follows the same nonce+cat protocol. Lets a
    /// test prove claude was never started.
    pub fn write_marker_fake_claude(&self, marker: &std::path::Path) {
        let marker = marker.display();
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "claude 0.0.0-fake"
    exit 0
fi
touch "{marker}"
echo "FAKE_CLAUDE_BANNER pid=$$"
exec cat
"#
        );
        fs::write(&self.fake_claude, &script).expect("write marker fake claude");
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
        cmd.env("CLAUDIO_NO_UPDATE_CHECK", "1"); // never check for updates in tests
        cmd.env("TERM", "xterm-256color");
        // Terminal tabs run $SHELL: pin a plain one (prompt "$ ") so tests do
        // not depend on the developer's shell and rc files.
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("COLORTERM");
    }

    /// Start a `claudio` TUI process with the default env and wait until the
    /// status bar shows "Alt+h help" (the TUI is fully up).
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
        tui.wait_for("Alt+h help", Region::StatusBar, DAEMON_WAIT);
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

// ── SVG colour helpers ────────────────────────────────────────────────────────

/// Map an alacritty `Color` to an SVG hex string (`#rrggbb`).
///
/// `default_fg` / `default_bg` are the terminal's default colours (hex).
/// When `dim` is true the colour is blended 50% toward the default background.
fn svg_color(
    color: ansi::Color,
    default_fg: &str,
    default_bg: &str,
    bold: bool,
    dim: bool,
) -> String {
    use ansi::NamedColor as NC;
    let (r, g, b) = match color {
        ansi::Color::Named(nc) => match nc {
            NC::Black        => (0x45, 0x47, 0x5a),
            NC::Red          => (0xf3, 0x8b, 0xa8),
            NC::Green        => (0xa6, 0xe3, 0xa1),
            NC::Yellow       => (0xf9, 0xe2, 0xaf),
            NC::Blue         => (0x89, 0xb4, 0xfa),
            NC::Magenta      => (0xcb, 0xa6, 0xf7),
            NC::Cyan         => (0x89, 0xdc, 0xeb),
            NC::White        => (0xba, 0xc2, 0xde),
            NC::BrightBlack  => (0x58, 0x5b, 0x70),
            NC::BrightRed    => (0xf3, 0x8b, 0xa8),
            NC::BrightGreen  => (0xa6, 0xe3, 0xa1),
            NC::BrightYellow => (0xf9, 0xe2, 0xaf),
            NC::BrightBlue   => (0x89, 0xb4, 0xfa),
            NC::BrightMagenta => (0xcb, 0xa6, 0xf7),
            NC::BrightCyan   => (0x94, 0xe2, 0xd5),
            NC::BrightWhite  => (0xa6, 0xad, 0xc8),
            NC::Foreground | NC::BrightForeground => {
                if bold { (0xff, 0xff, 0xff) } else { hex_to_rgb(default_fg) }
            }
            NC::Background   => hex_to_rgb(default_bg),
            NC::Cursor       => hex_to_rgb(default_fg),
            // Dim variants — map to normal colour (dim applied below).
            NC::DimBlack     => (0x45, 0x47, 0x5a),
            NC::DimRed       => (0xf3, 0x8b, 0xa8),
            NC::DimGreen     => (0xa6, 0xe3, 0xa1),
            NC::DimYellow    => (0xf9, 0xe2, 0xaf),
            NC::DimBlue      => (0x89, 0xb4, 0xfa),
            NC::DimMagenta   => (0xcb, 0xa6, 0xf7),
            NC::DimCyan      => (0x89, 0xdc, 0xeb),
            NC::DimWhite     => (0xba, 0xc2, 0xde),
            NC::DimForeground => hex_to_rgb(default_fg),
        },
        ansi::Color::Spec(rgb) => (rgb.r, rgb.g, rgb.b),
        ansi::Color::Indexed(idx) => indexed_rgb(idx),
    };
    if dim {
        let (br, bg, bb) = hex_to_rgb(default_bg);
        let r = ((r as u16 + br as u16) / 2) as u8;
        let g = ((g as u16 + bg as u16) / 2) as u8;
        let b = ((b as u16 + bb as u16) / 2) as u8;
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// Convert an xterm-256 colour index to `(r, g, b)`.
fn indexed_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        0  => (0x45, 0x47, 0x5a),
        1  => (0xf3, 0x8b, 0xa8),
        2  => (0xa6, 0xe3, 0xa1),
        3  => (0xf9, 0xe2, 0xaf),
        4  => (0x89, 0xb4, 0xfa),
        5  => (0xcb, 0xa6, 0xf7),
        6  => (0x89, 0xdc, 0xeb),
        7  => (0xba, 0xc2, 0xde),
        8  => (0x58, 0x5b, 0x70),
        9  => (0xf3, 0x8b, 0xa8),
        10 => (0xa6, 0xe3, 0xa1),
        11 => (0xf9, 0xe2, 0xaf),
        12 => (0x89, 0xb4, 0xfa),
        13 => (0xcb, 0xa6, 0xf7),
        14 => (0x94, 0xe2, 0xd5),
        15 => (0xa6, 0xad, 0xc8),
        n @ 16..=231 => {
            let n = n - 16;
            let ri = n / 36;
            let gi = (n % 36) / 6;
            let bi = n % 6;
            let c = |x: u8| -> u8 { if x == 0 { 0 } else { 55u8.saturating_add(x.saturating_mul(40)) } };
            (c(ri), c(gi), c(bi))
        }
        n => {
            let v = 8u8.saturating_add((n - 232).saturating_mul(10));
            (v, v, v)
        }
    }
}

/// Parse a `#rrggbb` hex colour to `(r, g, b)`.
fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    let r = u8::from_str_radix(h.get(0..2).unwrap_or("cc"), 16).unwrap_or(0xcc);
    let g = u8::from_str_radix(h.get(2..4).unwrap_or("dd"), 16).unwrap_or(0xdd);
    let b = u8::from_str_radix(h.get(4..6).unwrap_or("f4"), 16).unwrap_or(0xf4);
    (r, g, b)
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
