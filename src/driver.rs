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
use crate::hooks::{self, HookEvent, Listener};
use crate::session::{self, Summary};
use crate::vt::ProbeResponder;

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
            DriverError::TranscriptUnavailable => write!(f, "could not read the session transcript"),
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
}

macro_rules! trace {
    ($debug:expr, $start:expr, $($arg:tt)*) => {
        if $debug {
            let ms = $start.elapsed().as_millis();
            eprintln!("[claude-poc +{}ms] {}", ms, format!($($arg)*));
        }
    };
}

pub fn run(parsed: &Parsed, env: &WrapperEnv, prompt: &str) -> Result<RunResult, DriverError> {
    if prompt.trim().is_empty() {
        return Err(DriverError::NoPrompt);
    }
    let start = Instant::now();
    let debug = env.debug;
    let session_id = parsed
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    trace!(debug, start, "session_id={session_id}");

    let listener = Listener::start(env.hook_transport).map_err(|e| DriverError::Spawn(e.to_string()))?;

    let exe = std::env::current_exe()
        .map_err(|e| DriverError::Internal(e.to_string()))?
        .to_string_lossy()
        .into_owned();
    let (settings, settings_warns) = hooks::build_settings_merged(&exe, parsed.user_settings.as_deref());
    for w in &settings_warns {
        eprintln!("claude-poc: {w}");
    }

    // Assemble the child argv: our injected flags first (so they parse as
    // options regardless of any later `--`), then everything the user passed
    // that we don't own. The prompt is NOT here — we type it.
    let mut cargs: Vec<String> = Vec::new();
    cargs.push("--settings".into());
    cargs.push(settings);
    if parsed.session_id.is_none() {
        cargs.push("--session-id".into());
        cargs.push(session_id.clone());
    }
    cargs.extend(parsed.forward.iter().cloned());

    let mut cmd = CommandBuilder::new(&env.claude_path);
    for a in &cargs {
        cmd.arg(a);
    }
    // The child inherits our full environment (portable-pty seeds it from
    // std::env), so every variable claude honors — ANTHROPIC_*, CLAUDE_CODE_*,
    // proxies, etc. — passes through unchanged. We add only our hook-relay vars
    // and ensure a sane TERM (preferring the user's, defaulting to a value our
    // probe responder satisfies) so Ink renders.
    for (k, v) in listener.child_env() {
        cmd.env(k, v);
    }
    let term = std::env::var("TERM").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "xterm-256color".into());
    cmd.env("TERM", term);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows: env.rows, cols: env.cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| DriverError::Spawn(e.to_string()))?;
    let child = pair.slave.spawn_command(cmd).map_err(|e| DriverError::Spawn(e.to_string()))?;
    let mut guard = ChildGuard { child };
    drop(pair.slave);
    trace!(debug, start, "claude spawned under PTY ({}x{})", env.cols, env.rows);

    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().map_err(|e| DriverError::Spawn(e.to_string()))?,
    ));
    let mut reader = pair.master.try_clone_reader().map_err(|e| DriverError::Spawn(e.to_string()))?;

    let shared = Arc::new(Shared {
        last_output_ns: AtomicI64::new(0),
        exited: AtomicBool::new(false),
        recent: Mutex::new(Vec::with_capacity(RECENT_CAPACITY)),
        raw_log: env.raw_log.as_ref().map(|_| Mutex::new(Vec::new())),
    });

    {
        let writer = Arc::clone(&writer);
        let shared = Arc::clone(&shared);
        let rows = env.rows;
        let cols = env.cols;
        thread::spawn(move || {
            let mut responder = ProbeResponder::new(rows, cols);
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        shared.last_output_ns.store(now_ns(), Ordering::SeqCst);
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

    let timeout = Duration::from_secs(env.timeout_sec);
    let mut typed = false;
    let mut trust_dismissed = false;
    let mut transcript_path: Option<String> = None;
    let mut last_assistant_message: Option<String> = None;
    let mut got_stop = false;

    while start.elapsed() < timeout {
        // Workspace-trust dialog can block startup before SessionStart and is
        // not bypassed by --dangerously-skip-permissions. Default is "trust";
        // Enter accepts.
        if !typed && !trust_dismissed {
            if let Ok(recent) = shared.recent.lock() {
                let stripped = strip_escapes(&recent);
                let low = stripped.to_lowercase();
                if low.contains("trust") && low.contains("folder") {
                    drop(recent);
                    trace!(debug, start, "workspace-trust dialog detected — sending Enter");
                    if let Ok(mut w) = writer.lock() {
                        let _ = w.write_all(b"\r");
                        let _ = w.flush();
                    }
                    trust_dismissed = true;
                }
            }
        }

        if !typed && shared.exited.load(Ordering::SeqCst) {
            return Err(DriverError::Spawn("claude exited before the UI was ready".into()));
        }

        if let Some(hook) = listener.poll(Duration::from_millis(100)) {
            // Either hook carries transcript_path; grab it as soon as we see it.
            if transcript_path.is_none() {
                transcript_path = session::payload_field(&hook.payload, "transcript_path");
            }
            match hook.event {
                HookEvent::SessionStart => {
                    if !typed {
                        trace!(debug, start, "SessionStart — waiting for Ink quiescence");
                        wait_quiescent(&shared, 150, 2000);
                        trace!(debug, start, "typing prompt ({} bytes)", prompt.len());
                        type_prompt(&writer, prompt).map_err(|e| DriverError::Internal(e.to_string()))?;
                        typed = true;
                        trace!(debug, start, "prompt submitted; awaiting Stop");
                    }
                }
                HookEvent::Stop => {
                    last_assistant_message = session::payload_field(&hook.payload, "last_assistant_message");
                    got_stop = true;
                    trace!(debug, start, "Stop — turn finished");
                    break;
                }
                HookEvent::Unknown => {}
            }
        }
    }

    if !typed {
        return Err(DriverError::SessionStartTimeout);
    }
    if !got_stop {
        trace!(debug, start, "no Stop within timeout; attempting transcript read anyway");
    }

    if let (Some(path), Some(raw)) = (&env.raw_log, &shared.raw_log) {
        if let Ok(bytes) = raw.lock() {
            let _ = std::fs::write(path, &*bytes);
        }
    }

    let summary = if let Some(path) = &transcript_path {
        match session::read_with_retry(path, 40, Duration::from_millis(50)) {
            Some(s) => Some(s),
            None => last_assistant_message.as_ref().map(|t| fallback_summary(t, &session_id)),
        }
    } else {
        last_assistant_message.as_ref().map(|t| fallback_summary(t, &session_id))
    };

    let failure = if summary.as_ref().map(|s| s.final_text.is_empty()).unwrap_or(true) {
        let recent = shared.recent.lock().ok().map(|r| strip_escapes(&r)).unwrap_or_default();
        Some(classify_failure(&recent, got_stop))
    } else {
        None
    };

    guard.kill_and_wait();
    let duration_ms = start.elapsed().as_millis() as u64;

    let summary = match summary {
        Some(s) => s,
        None if !got_stop => return Err(DriverError::StopTimeout),
        None => return Err(DriverError::TranscriptUnavailable),
    };

    Ok(RunResult { summary, duration_ms, failure })
}

fn type_prompt(writer: &Arc<Mutex<Box<dyn Write + Send>>>, prompt: &str) -> std::io::Result<()> {
    // Ink merges back-to-back writes via its bracketed-paste/burst heuristic;
    // a gap between the body and Enter makes it register two events so the
    // Enter submits rather than landing in the input buffer.
    {
        let mut w = writer.lock().map_err(|e| std::io::Error::other(e.to_string()))?;
        w.write_all(prompt.as_bytes())?;
        w.flush()?;
    }
    thread::sleep(Duration::from_millis(150));
    {
        let mut w = writer.lock().map_err(|e| std::io::Error::other(e.to_string()))?;
        w.write_all(b"\r")?;
        w.flush()?;
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
    if low.contains("failed to authenticate") || low.contains("api error: 403") || compact.contains("pleaserunlogin") {
        return "auth_blocked".into();
    }
    if low.contains("hit your limit") || low.contains("usage limit") || low.contains("rate limit") {
        return "rate_limit".into();
    }
    if (low.contains("do you trust") && low.contains("folder")) || compact.contains("trustthisfolder") {
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
        assert_eq!(classify_failure("Failed to authenticate", true), "auth_blocked");
        assert_eq!(classify_failure("You've hit your limit", true), "rate_limit");
        assert_eq!(classify_failure("Do you trust this folder?", true), "workspace_trust_blocked");
        assert_eq!(classify_failure("", false), "assistant_output_timeout");
    }
}
