//! End-to-end driver: spawn interactive `claude` under a PTY, answer terminal
//! probes, type the prompt the way a person does once the UI is ready
//! (`SessionStart`), wait for the turn to finish (`Stop`), and read the answer
//! from the canonical session JSONL.
//!
//! Robustness stance: this depends only on stable, documented contracts — a TTY
//! that accepts typed input and submits on Enter (the TUI's core contract), the
//! `SessionStart`/`Stop` lifecycle hooks, and the session JSONL. It does not
//! rely on version-specific behavior (e.g. auto-running a positional prompt),
//! and forwards every flag it doesn't own verbatim so new claude flags keep
//! working without code changes.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::cli::{Parsed, WrapperEnv};
use crate::print::hooks::{self, HookEvent, Listener};
use crate::print::session::{self, Summary};
use crate::term::probe::ProbeResponder;

#[derive(Debug)]
pub enum DriverError {
    Spawn(String),
    SessionStartTimeout,
    StopTimeout,
    TranscriptUnavailable,
    NoPrompt,
    Internal(String),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::Spawn(s) => write!(f, "failed to spawn claude: {s}"),
            DriverError::SessionStartTimeout => write!(f, "timed out waiting for the UI to start"),
            DriverError::StopTimeout => write!(f, "timed out waiting for the turn to finish"),
            DriverError::TranscriptUnavailable => {
                write!(f, "could not read the session transcript")
            }
            DriverError::NoPrompt => write!(f, "no prompt supplied"),
            DriverError::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}

pub struct RunResult {
    pub summary: Summary,
    pub duration_ms: u64,
    pub failure: Option<String>,
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

const RECENT_CAPACITY: usize = 16 * 1024;

/// Owns the child and guarantees it is killed/reaped on every exit path,
/// including early `?`/`return` errors and panics.
struct ChildGuard {
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl ChildGuard {
    fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Shared {
    last_output_ns: AtomicI64,
    exited: AtomicBool,
    recent: Mutex<Vec<u8>>,
    raw_log: Option<Mutex<Vec<u8>>>,
    /// Set once the TUI enables bracketed-paste mode (DECSET `?2004h`). When on,
    /// we frame the typed prompt in paste markers so embedded newlines don't
    /// submit a partial prompt — essential for large multi-line API prompts.
    bracketed_paste: AtomicBool,
}

macro_rules! trace {
    ($debug:expr, $start:expr, $($arg:tt)*) => {
        if $debug {
            let ms = $start.elapsed().as_millis();
            eprintln!("[claudio +{}ms] {}", ms, format!($($arg)*));
        }
    };
}

/// One-shot turn: start a session, run a single turn, tear it down. The CLI
/// `-p` path. Thin wrapper over [`PtySession`].
pub fn run(parsed: &Parsed, env: &WrapperEnv, prompt: &str) -> Result<RunResult, DriverError> {
    if prompt.trim().is_empty() {
        return Err(DriverError::NoPrompt);
    }
    let start = Instant::now();
    let session_id = parsed
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut sess = PtySession::start(
        env,
        &parsed.forward,
        &session_id,
        parsed.user_settings.as_deref(),
    )?;

    let corr = if crate::msglog::enabled() {
        let c = crate::msglog::new_corr();
        let sid: String = session_id.chars().take(8).collect();
        crate::msglog::record(
            crate::msglog::Dir::ClaudioToClaude,
            &c,
            &format!("-p turn · session {sid}"),
            prompt,
        );
        c
    } else {
        String::new()
    };

    let (summary, failure) = sess.turn(prompt)?;

    if crate::msglog::enabled() {
        crate::msglog::record(
            crate::msglog::Dir::ClaudeToClaudio,
            &corr,
            &format!(
                "reply · turns={} · error={}",
                summary.num_turns, summary.is_error
            ),
            &summary.final_text,
        );
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    sess.close();
    Ok(RunResult {
        summary,
        duration_ms,
        failure,
    })
}

/// A live, interactive `claude` driven under a PTY, reusable across turns.
///
/// `start` spawns claude and waits for the UI (`SessionStart`); `turn` types a
/// prompt, waits for that turn's `Stop`, and returns the *new* assistant message
/// from the session transcript. Keeping one process alive across turns is what
/// lets the API server feed only the conversation delta instead of re-sending
/// the whole history (and re-paying the cold-start) each request.
pub struct PtySession {
    guard: ChildGuard,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    shared: Arc<Shared>,
    listener: Listener,
    /// Held for the session's lifetime so the neutral relay binary isn't removed.
    _relay: Option<hooks::Relay>,
    transcript_path: Option<String>,
    session_id: String,
    fast: bool,
    debug: bool,
    timeout: Duration,
    raw_log_path: Option<String>,
    /// Identity of the last assistant message we returned, to detect the next one.
    last_msg_id: Option<String>,
    start: Instant,
}

impl PtySession {
    /// Spawn claude under a PTY and wait until the UI is ready to accept input.
    pub fn start(
        env: &WrapperEnv,
        forward: &[String],
        session_id: &str,
        user_settings: Option<&str>,
    ) -> Result<Self, DriverError> {
        let start = Instant::now();
        let debug = env.debug;
        trace!(debug, start, "session_id={session_id}");

        let listener =
            Listener::start(env.hook_transport).map_err(|e| DriverError::Spawn(e.to_string()))?;

        // §4.9.2 — neutral relay binary (TCP transport only); fall back silently.
        let relay = listener.port().and_then(|_p| hooks::Relay::setup().ok());

        let exe = std::env::current_exe()
            .map_err(|e| DriverError::Internal(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        let relay_arg = relay.as_ref().zip(listener.port()).map(|(r, p)| (r, p));
        let (settings, settings_warns) =
            hooks::build_settings_merged(&exe, relay_arg, user_settings);
        for w in &settings_warns {
            eprintln!("claudio: {w}");
        }

        // §4.9.5 — randomise PTY geometry per session within realistic ranges.
        let (cols, rows) = {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0xabcdef12);
            let cols = if env.cols == 120 {
                180 + (seed % 81) as u16
            } else {
                env.cols
            };
            let rows = if env.rows == 40 {
                40 + (seed >> 8 & 0x1f) as u16
            } else {
                env.rows
            };
            (cols, rows)
        };
        trace!(debug, start, "PTY size: {cols}×{rows}");

        // Assemble child argv. Add --session-id unless the caller already
        // forwarded one, or is resuming/continuing a session (where --session-id
        // would conflict with --resume/--continue and claude refuses to start).
        let resuming = forward
            .iter()
            .any(|a| matches!(a.as_str(), "--resume" | "-r" | "--continue" | "-c"));
        let mut cargs: Vec<String> = vec!["--settings".into(), settings];
        if !resuming && !forward.iter().any(|a| a == "--session-id") {
            cargs.push("--session-id".into());
            cargs.push(session_id.to_string());
        }
        cargs.extend(forward.iter().cloned());

        let mut cmd = CommandBuilder::new(&env.claude_path);
        for a in &cargs {
            cmd.arg(a);
        }
        // §4.9.2 — child env WITHOUT any CLAUDIO_* vars.
        cmd.env_clear();
        for (k, v) in std::env::vars() {
            if !k.starts_with("CLAUDIO_") {
                cmd.env(k, v);
            }
        }
        for (k, v) in listener.child_env() {
            cmd.env(k, v);
        }
        let term = std::env::var("TERM")
            .ok()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "xterm-256color".into());
        cmd.env("TERM", term);
        if std::env::var("COLORTERM").is_err() {
            cmd.env("COLORTERM", "truecolor");
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| DriverError::Spawn(e.to_string()))?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| DriverError::Spawn(e.to_string()))?;
        let guard = ChildGuard { child };
        drop(pair.slave);
        trace!(debug, start, "claude spawned under PTY ({}x{})", cols, rows);

        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|e| DriverError::Spawn(e.to_string()))?,
        ));
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| DriverError::Spawn(e.to_string()))?;

        let shared = Arc::new(Shared {
            last_output_ns: AtomicI64::new(0),
            exited: AtomicBool::new(false),
            recent: Mutex::new(Vec::with_capacity(RECENT_CAPACITY)),
            bracketed_paste: AtomicBool::new(false),
            raw_log: env.raw_log.as_ref().map(|_| Mutex::new(Vec::new())),
        });

        {
            let writer = Arc::clone(&writer);
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                let mut responder = ProbeResponder::new(rows, cols);
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = &buf[..n];
                            shared.last_output_ns.store(now_ns(), Ordering::SeqCst);
                            if !shared.bracketed_paste.load(Ordering::Relaxed)
                                && chunk.windows(8).any(|w| w == b"\x1b[?2004h")
                            {
                                shared.bracketed_paste.store(true, Ordering::SeqCst);
                            }
                            responder.feed(chunk);
                            let resp = responder.take_responses();
                            if !resp.is_empty() {
                                if let Ok(mut w) = writer.lock() {
                                    let _ = w.write_all(&resp);
                                    let _ = w.flush();
                                }
                            }
                            if let Ok(mut recent) = shared.recent.lock() {
                                recent.extend_from_slice(chunk);
                                if recent.len() > RECENT_CAPACITY {
                                    let drop = recent.len() - RECENT_CAPACITY;
                                    recent.drain(0..drop);
                                }
                            }
                            if let Some(raw) = &shared.raw_log {
                                if let Ok(mut r) = raw.lock() {
                                    r.extend_from_slice(chunk);
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                shared.exited.store(true, Ordering::SeqCst);
            });
        }

        let mut sess = PtySession {
            guard,
            writer,
            shared,
            listener,
            _relay: relay,
            transcript_path: None,
            session_id: session_id.to_string(),
            fast: env.fast,
            debug,
            timeout: Duration::from_secs(env.timeout_sec),
            raw_log_path: env.raw_log.clone(),
            last_msg_id: None,
            start,
        };
        sess.wait_session_start()?;
        Ok(sess)
    }

    /// Block until `SessionStart` (UI ready), dismissing the workspace-trust
    /// dialog if it appears. Does not type anything.
    fn wait_session_start(&mut self) -> Result<(), DriverError> {
        let t0 = Instant::now();
        let mut trust_dismissed = false;
        while t0.elapsed() < self.timeout {
            if !trust_dismissed {
                if let Ok(recent) = self.shared.recent.lock() {
                    let low = strip_escapes(&recent).to_lowercase();
                    if low.contains("trust") && low.contains("folder") {
                        drop(recent);
                        trace!(
                            self.debug,
                            self.start,
                            "workspace-trust dialog — sending Enter"
                        );
                        if let Ok(mut w) = self.writer.lock() {
                            let _ = w.write_all(b"\r");
                            let _ = w.flush();
                        }
                        trust_dismissed = true;
                    }
                }
            }
            if self.shared.exited.load(Ordering::SeqCst) {
                return Err(DriverError::Spawn(
                    "claude exited before the UI was ready".into(),
                ));
            }
            if let Some(hook) = self.listener.poll(Duration::from_millis(100)) {
                if self.transcript_path.is_none() {
                    self.transcript_path = session::payload_field(&hook.payload, "transcript_path");
                }
                if hook.event == HookEvent::SessionStart {
                    trace!(self.debug, self.start, "SessionStart — UI ready");
                    return Ok(());
                }
            }
        }
        Err(DriverError::SessionStartTimeout)
    }

    /// Type `prompt`, wait for this turn's `Stop`, and return the new assistant
    /// message (plus a classified failure reason if it produced no text).
    pub fn turn(&mut self, prompt: &str) -> Result<(Summary, Option<String>), DriverError> {
        if prompt.trim().is_empty() {
            return Err(DriverError::NoPrompt);
        }
        let turn_start = Instant::now();
        // Baseline: the message we must produce something newer than. When we've
        // already served a turn, that's `last_msg_id`. Otherwise (first turn of a
        // fresh *or* resumed session) baseline on the transcript's current latest
        // so a resumed session doesn't return its pre-existing last answer.
        let prev_id = match &self.last_msg_id {
            Some(id) => Some(id.clone()),
            None => self
                .transcript_path
                .as_deref()
                .and_then(session::latest_terminal_with_id)
                .map(|(_, id)| id),
        };

        wait_quiescent(&self.shared, 150, 2000);
        let bracketed = self.shared.bracketed_paste.load(Ordering::SeqCst);
        trace!(
            self.debug,
            self.start,
            "typing prompt ({} bytes, bracketed_paste={})",
            prompt.len(),
            bracketed
        );
        type_prompt(&self.writer, prompt, self.fast, bracketed)
            .map_err(|e| DriverError::Internal(e.to_string()))?;
        trace!(self.debug, self.start, "prompt submitted; awaiting Stop");

        let mut got_stop = false;
        let mut last_assistant_message: Option<String> = None;
        while turn_start.elapsed() < self.timeout {
            if self.shared.exited.load(Ordering::SeqCst) {
                break;
            }
            if let Some(hook) = self.listener.poll(Duration::from_millis(100)) {
                if self.transcript_path.is_none() {
                    self.transcript_path = session::payload_field(&hook.payload, "transcript_path");
                }
                if hook.event == HookEvent::Stop {
                    last_assistant_message =
                        session::payload_field(&hook.payload, "last_assistant_message");
                    got_stop = true;
                    trace!(self.debug, self.start, "Stop — turn finished");
                    break;
                }
            }
        }

        if let (Some(path), Some(raw)) = (&self.raw_log_path, &self.shared.raw_log) {
            if let Ok(bytes) = raw.lock() {
                let _ = std::fs::write(path, &*bytes);
            }
        }

        let summary = self.read_new_message(prev_id.as_deref(), last_assistant_message.as_deref());

        let failure = if summary
            .as_ref()
            .map(|s| s.final_text.is_empty())
            .unwrap_or(true)
        {
            let recent = self
                .shared
                .recent
                .lock()
                .ok()
                .map(|r| strip_escapes(&r))
                .unwrap_or_default();
            Some(classify_failure(&recent, got_stop))
        } else {
            None
        };

        match summary {
            Some(s) => Ok((s, failure)),
            None if !got_stop => Err(DriverError::StopTimeout),
            None => Err(DriverError::TranscriptUnavailable),
        }
    }

    /// Read the transcript until a terminal assistant message whose identity
    /// differs from the previous turn's appears (absorbing the flush race).
    fn read_new_message(
        &mut self,
        prev_id: Option<&str>,
        fallback_text: Option<&str>,
    ) -> Option<Summary> {
        if let Some(path) = self.transcript_path.clone() {
            for _ in 0..60 {
                if let Some((summary, id)) = session::latest_terminal_with_id(&path) {
                    if Some(id.as_str()) != prev_id {
                        self.last_msg_id = Some(id);
                        return Some(summary);
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            // Last resort: any message we can read (even if we can't prove it's new).
            if let Some(s) = session::read_with_retry(&path, 3, Duration::from_millis(50)) {
                return Some(s);
            }
        }
        fallback_text.map(|t| fallback_summary(t, &self.session_id))
    }

    /// Whether the underlying claude process has exited.
    pub fn is_alive(&self) -> bool {
        !self.shared.exited.load(Ordering::SeqCst)
    }

    /// Kill the child and reap it.
    pub fn close(&mut self) {
        self.guard.kill_and_wait();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// §4.9.1 — Typing cadence: per-character delays that mimic human input.
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal XOR-shift PRNG — no external dependency.
struct Rng(u64);
impl Rng {
    fn from_time() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x853c49e6748fea9b);
        let mut r = Self(seed | 1);
        for _ in 0..16 {
            r.next();
        } // Warm up.
        r
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform random in [lo, hi).
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }
}

/// Type `prompt` character-by-character with human-like cadence (§4.9.1),
/// then send Enter after a randomised dwell.
///
/// Enabled by default.  Set `CLAUDIO_CADENCE=0` to revert to single-burst
/// delivery (faster, useful when typing latency matters more than stealth).
/// `fast` (from `--fast`/`CLAUDIO_FAST`) forces the burst path with a minimal
/// pre-Enter pause, overriding cadence entirely.
///
/// When `bracketed`, the prompt is framed in bracketed-paste markers
/// (`ESC[200~` … `ESC[201~`). The TUI then treats it as one atomic paste, so
/// embedded newlines land as literal text instead of submitting a partial
/// prompt — without this, a large multi-line prompt (e.g. a flattened API
/// conversation) submits at its first `\n` and the turn never completes.
fn type_prompt(
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    prompt: &str,
    fast: bool,
    bracketed: bool,
) -> std::io::Result<()> {
    let cadence = !fast
        && std::env::var("CLAUDIO_CADENCE")
            .map(|v| v != "0")
            .unwrap_or(true);

    // Helper to lock and write a slice.
    let put = |bytes: &[u8]| -> std::io::Result<()> {
        let mut w = writer
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        w.write_all(bytes)?;
        w.flush()
    };

    if bracketed {
        put(b"\x1b[200~")?;
    }

    if !cadence {
        // Fast burst path — send everything at once.
        put(prompt.as_bytes())?;
    } else {
        // Human-cadence path — per-character with jitter (see below).
        type_with_cadence(&put, prompt)?;
    }

    if bracketed {
        put(b"\x1b[201~")?;
    }

    // Let the TUI register the input before submitting; `fast` trims this to a
    // small-but-safe value (Enter too early drops the just-typed text).
    let dwell_ms: u64 = if cadence {
        Rng::from_time().range(180, 521)
    } else if fast {
        50
    } else {
        150
    };
    thread::sleep(Duration::from_millis(dwell_ms));
    put(b"\r")?;
    Ok(())
}

/// Per-character typing with human-like cadence (§4.9.1).
fn type_with_cadence(
    put: &impl Fn(&[u8]) -> std::io::Result<()>,
    prompt: &str,
) -> std::io::Result<()> {
    let mut rng = Rng::from_time();

    // Sample a per-session WPM from 45–95.  This gives base delay:
    //   base_us = 60_000_000 µs/min  ÷  (5 chars/word × WPM)
    // e.g. 70 WPM → ~171 µs/char base.
    let wpm = rng.range(45, 96);
    let base_us = 60_000_000u64 / (5 * wpm);

    let bytes = prompt.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;

        // Multiplicative jitter in [0.4, 1.6] (100 steps of 1.2 / 100).
        let jitter = 40 + rng.next() % 121; // 40–160 → ÷100
        let base_delay = base_us * jitter / 100;

        // Contextual multiplier: word boundary or punctuation → slower.
        let delay_us = if ch == ' ' || ch == '\t' || ch == '\n' {
            base_delay * 3 / 2 // 1.5× after whitespace
        } else if ".!?,;:".contains(ch) {
            base_delay * 2 // 2× after punctuation
        } else {
            base_delay
        };

        // Rare "think" pause (~1 % of characters, 150–700 ms).
        let think_us = if rng.range(0, 100) == 0 {
            rng.range(150_000, 700_001)
        } else {
            0
        };

        thread::sleep(Duration::from_micros(delay_us + think_us));
        put(&bytes[i..i + 1])?;
        i += 1;
    }
    Ok(())
}

fn wait_quiescent(shared: &Shared, quiet_ms: i64, max_ms: i64) {
    let started = now_ns();
    loop {
        let now = now_ns();
        if (now - started) / 1_000_000 > max_ms {
            return;
        }
        let last = shared.last_output_ns.load(Ordering::SeqCst);
        if last != 0 && (now - last) / 1_000_000 > quiet_ms {
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
}

fn fallback_summary(text: &str, session_id: &str) -> Summary {
    Summary {
        final_text: text.to_string(),
        session_id: session_id.to_string(),
        model: None,
        usage: None,
        num_turns: 1,
        is_error: false,
    }
}

fn classify_failure(recent_stripped: &str, got_stop: bool) -> String {
    let low = recent_stripped.to_lowercase();
    let compact: String = low.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if low.contains("failed to authenticate")
        || low.contains("api error: 403")
        || compact.contains("pleaserunlogin")
    {
        return "auth_blocked".into();
    }
    if low.contains("hit your limit") || low.contains("usage limit") || low.contains("rate limit") {
        return "rate_limit".into();
    }
    if (low.contains("do you trust") && low.contains("folder"))
        || compact.contains("trustthisfolder")
    {
        return "workspace_trust_blocked".into();
    }
    if low.contains("permission") && (low.contains("allow") || low.contains("deny")) {
        return "tool_approval_blocked".into();
    }
    if !got_stop {
        return "assistant_output_timeout".into();
    }
    "assistant_output_not_found".into()
}

/// Strip CSI / OSC / DCS escape sequences so plain-text matching is robust
/// against cursor-positioning escapes that pad words.
fn strip_escapes(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        match bytes[i + 1] {
            b'[' => {
                i += 2;
                while i < bytes.len() && (0x30..=0x3f).contains(&bytes[i]) {
                    i += 1;
                }
                while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b']' => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b'P' | b'X' | b'^' | b'_' => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            _ => {
                i += 2;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_csi_basic() {
        assert_eq!(strip_escapes(b"\x1b[1mhello\x1b[0m"), "hello");
        assert_eq!(strip_escapes(b"do\x1b[1Cyou\x1b[1Ctrust"), "doyoutrust");
    }

    #[test]
    fn classify_examples() {
        assert_eq!(
            classify_failure("Failed to authenticate", true),
            "auth_blocked"
        );
        assert_eq!(
            classify_failure("You've hit your limit", true),
            "rate_limit"
        );
        assert_eq!(
            classify_failure("Do you trust this folder?", true),
            "workspace_trust_blocked"
        );
        assert_eq!(classify_failure("", false), "assistant_output_timeout");
    }

    #[test]
    fn classify_api_error_403() {
        assert_eq!(
            classify_failure("API Error: 403 forbidden", true),
            "auth_blocked"
        );
    }

    #[test]
    fn classify_usage_limit() {
        assert_eq!(
            classify_failure("You have exceeded your usage limit", true),
            "rate_limit"
        );
        assert_eq!(classify_failure("rate limit exceeded", true), "rate_limit");
    }

    #[test]
    fn classify_tool_approval() {
        assert_eq!(
            classify_failure("permission required — allow or deny?", true),
            "tool_approval_blocked"
        );
    }

    #[test]
    fn classify_trust_compact() {
        // After CSI stripping the dialog words may run together.
        assert_eq!(
            classify_failure("doyoutrustthisfolder", true),
            "workspace_trust_blocked"
        );
    }

    #[test]
    fn classify_got_stop_no_answer() {
        // stop received but transcript had no text
        assert_eq!(classify_failure("", true), "assistant_output_not_found");
    }

    #[test]
    fn strip_csi_bold_and_cursor_move() {
        assert_eq!(strip_escapes(b"\x1b[1mhello\x1b[0m"), "hello");
        assert_eq!(strip_escapes(b"do\x1b[1Cyou\x1b[1Ctrust"), "doyoutrust");
    }

    #[test]
    fn strip_osc_sequence() {
        // OSC 0 (set window title): ESC ] 0 ; title BEL
        let input = b"before\x1b]0;My Terminal\x07after";
        assert_eq!(strip_escapes(input), "beforeafter");
    }

    #[test]
    fn strip_osc_st_terminated() {
        // OSC terminated by ST (ESC \) instead of BEL
        let input = b"x\x1b]2;title\x1b\\y";
        assert_eq!(strip_escapes(input), "xy");
    }

    #[test]
    fn strip_dcs_sequence() {
        // DCS (ESC P ... ESC \)
        let input = b"a\x1bP>|xterm\x1b\\b";
        assert_eq!(strip_escapes(input), "ab");
    }

    #[test]
    fn strip_leaves_plain_text() {
        assert_eq!(strip_escapes(b"hello world"), "hello world");
    }

    #[test]
    fn strip_incomplete_escape_at_end() {
        // Incomplete escape at end of buffer should not panic.
        assert_eq!(strip_escapes(b"text\x1b"), "text");
    }

    #[test]
    fn strip_two_letter_escape() {
        // ESC M (reverse index) — 2-byte sequence, no bracket
        let input = b"a\x1bMb";
        assert_eq!(strip_escapes(input), "ab");
    }
}
