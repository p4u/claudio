//! Optional message-flow logging (`--log-messages` / `--log-messages-file`).
//!
//! When enabled, claudio records every hop of a turn so the full
//! CLI ⇄ claudio ⇄ upstream-`claude` exchange is easy to follow:
//!
//! 1. `CLI ──▶ claudio`   — the client's request (model, history, tools)
//! 2. `claudio ──▶ claude`— the prompt typed to the backend (full ctx or delta)
//! 3. `claude ──▶ claudio`— claude's raw reply
//! 4. `claudio ──▶ CLI`   — the response returned to the client
//!
//! `--log-messages` writes a compact, colorized block per hop to stderr;
//! `--log-messages-file <path>` appends the same events *untruncated* as JSON
//! Lines (one object per hop) for raw inspection. Hops of one turn share a short
//! correlation id so they can be matched up. All four legs are also produced for
//! the single-turn `-p` path (where the "CLI" is your shell).

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Which leg of the exchange a record represents.
#[derive(Clone, Copy)]
pub enum Dir {
    /// Incoming request from the client CLI (pi/opencode/hermes/…) to claudio.
    CliToClaudio,
    /// Prompt claudio types into the upstream `claude` (full context or delta).
    ClaudioToClaude,
    /// Raw reply read back from the upstream `claude`.
    ClaudeToClaudio,
    /// Response claudio returns to the client CLI.
    ClaudioToCli,
}

impl Dir {
    /// Human label with a direction arrow (pretty stderr sink).
    fn label(self) -> &'static str {
        match self {
            Dir::CliToClaudio => "CLI ──▶ claudio",
            Dir::ClaudioToClaude => "claudio ──▶ claude",
            Dir::ClaudeToClaudio => "claude ──▶ claudio",
            Dir::ClaudioToCli => "claudio ──▶ CLI",
        }
    }
    /// Stable machine tag (raw JSONL sink).
    fn tag(self) -> &'static str {
        match self {
            Dir::CliToClaudio => "cli->claudio",
            Dir::ClaudioToClaude => "claudio->claude",
            Dir::ClaudeToClaudio => "claude->claudio",
            Dir::ClaudioToCli => "claudio->cli",
        }
    }
    /// ANSI color code for the header (used only when stderr is a TTY).
    fn color(self) -> &'static str {
        match self {
            Dir::CliToClaudio => "36",    // cyan — entering
            Dir::ClaudioToClaude => "33", // yellow — to backend
            Dir::ClaudeToClaudio => "32", // green — from backend
            Dir::ClaudioToCli => "35",    // magenta — leaving
        }
    }
}

struct Sink {
    pretty: bool,
    color: bool,
    file: Option<Mutex<std::fs::File>>,
    seq: AtomicU64,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Initialize logging once. `pretty` enables the stderr blocks; `file` (if set
/// and openable) enables the raw JSONL sink. Safe to call when both are off.
pub fn init(pretty: bool, file: Option<&str>) {
    let file = file.and_then(|p| {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
        {
            Ok(f) => Some(Mutex::new(f)),
            Err(e) => {
                eprintln!("claudio: --log-messages-file: cannot open {p}: {e}");
                None
            }
        }
    });
    let color = pretty && std::io::stderr().is_terminal();
    let _ = SINK.set(Sink {
        pretty,
        color,
        file,
        seq: AtomicU64::new(1),
    });
}

/// True if any sink is active. Callers can gate body construction on this.
pub fn enabled() -> bool {
    SINK.get()
        .map(|s| s.pretty || s.file.is_some())
        .unwrap_or(false)
}

/// Generate a short correlation id tying one turn's hops together.
pub fn new_corr() -> String {
    let s = uuid::Uuid::new_v4().simple().to_string();
    s[..6].to_string()
}

/// Record one hop. `head` is a one-line summary; `body` is the full payload —
/// written untruncated to the file, truncated in the pretty stderr view.
pub fn record(dir: Dir, corr: &str, head: &str, body: &str) {
    let Some(s) = SINK.get() else { return };
    let seq = s.seq.fetch_add(1, Ordering::Relaxed);

    if let Some(file) = &s.file {
        let obj = serde_json::json!({
            "seq": seq, "corr": corr, "dir": dir.tag(), "head": head, "body": body,
        });
        if let Ok(mut f) = file.lock() {
            let _ = writeln!(f, "{obj}");
        }
    }

    if s.pretty {
        let (bold, dim, reset) = if s.color {
            (
                format!("\x1b[1;{}m", dir.color()),
                format!("\x1b[{}m", dir.color()),
                "\x1b[0m",
            )
        } else {
            (String::new(), String::new(), "")
        };
        let mut out = String::new();
        out.push_str(&format!(
            "{bold}┌─ #{seq} [{corr}] {}{reset}  {head}\n",
            dir.label()
        ));
        for line in truncate(body, 1200).lines() {
            out.push_str(&format!("{dim}│{reset} {line}\n"));
        }
        out.push_str(&format!("{dim}└─{reset}\n"));
        // Whole block under one lock so it doesn't interleave with tracing.
        let stderr = std::io::stderr();
        let mut lk = stderr.lock();
        let _ = lk.write_all(out.as_bytes());
    }
}

/// Truncate to `max` bytes on a char boundary, noting how much was elided.
fn truncate(s: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if s.len() <= max {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!("{}\n… [+{} bytes]", &s[..end], s.len() - end))
}
