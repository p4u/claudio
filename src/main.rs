//! claudio — a drop-in for `claude` that emulates `-p` (print mode) by
//! driving the interactive TUI under a PTY. Responsible-disclosure research
//! PoC: every mechanism is a documented, supported Claude Code feature, and
//! nothing is hidden from the vendor. See ../REPORT.md.
//!
//! Behavior:
//!   * No `-p`/`--print`  → exec the real `claude` unchanged (interactive,
//!     subcommands, --help, --version are 100% native).
//!   * `-p`/`--print`     → emulate print mode: forward all other flags to
//!     interactive claude, type the prompt, read the answer from the session
//!     JSONL, and emit text/json/stream-json.

mod cli;
mod driver;
mod emit;
mod hooks;
mod session;
mod vt;

use std::io::{IsTerminal, Read};

use cli::{OutputFormat, WrapperEnv};
use emit::Outcome;

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();

    // §4.9.2 concealed relay: invoked as `<neutral-binary> <EventName> <port>`.
    // The binary is a copy of ourselves with a random hex name in /tmp; the hook
    // command is `<copy> Stop 49152` — no wrapper name, no env var.
    if argv.len() == 3 {
        if let Ok(port) = argv[2].parse::<u16>() {
            if matches!(argv[1].as_str(),
                "SessionStart" | "Stop" | "PreToolUse" | "PostToolUse"
                | "PreCompact" | "PostCompact" | "Notification"
            ) {
                hooks::relay_to_port(&argv[1], port);
                return std::process::ExitCode::SUCCESS;
            }
        }
    }

    // Legacy relay mode, invoked as `claudio __hook <Event>`.
    if argv.len() >= 3 && argv[1] == "__hook" {
        hooks::run_relay(&argv[2]);
        return std::process::ExitCode::SUCCESS;
    }

    let args = &argv[1..];
    let parsed = cli::parse(args);

    // Transparent passthrough: without -p we are just `claude`.
    if !parsed.print_mode {
        return exec_claude_transparently(args);
    }

    for w in &parsed.warnings {
        eprintln!("claudio: {w}");
    }

    let env = WrapperEnv::from_env();

    // Resolve the prompt: explicit (positional/`--`) wins; otherwise stdin.
    let prompt = match &parsed.prompt {
        Some(p) => p.clone(),
        None => {
            if std::io::stdin().is_terminal() {
                eprintln!("claudio: a prompt is required (positional arg, `-- <prompt>`, or stdin)");
                return std::process::ExitCode::from(2);
            }
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
                eprintln!("claudio: empty prompt on stdin");
                return std::process::ExitCode::from(2);
            }
            buf
        }
    };

    match driver::run(&parsed, &env, &prompt) {
        Ok(result) => {
            let outcome = Outcome {
                summary: &result.summary,
                duration_ms: result.duration_ms,
                failure: result.failure.as_deref(),
            };
            let mut stdout = std::io::stdout();
            if let Err(e) = emit::emit(&mut stdout, parsed.output_format, &outcome) {
                eprintln!("{e}");
                return std::process::ExitCode::from(2);
            }
            if outcome.failure.is_some() {
                std::process::ExitCode::from(2)
            } else {
                std::process::ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("claudio: {e}");
            if matches!(parsed.output_format, OutputFormat::Json | OutputFormat::StreamJson) {
                let obj = serde_json::json!({
                    "type": "result", "subtype": "error", "is_error": true,
                    "result": "", "terminal_reason": e.to_string(),
                });
                println!("{obj}");
            }
            std::process::ExitCode::from(2)
        }
    }
}

/// Replace this process with the real `claude`, forwarding argv verbatim. On
/// Unix this is a true `execvp` (transparent signals, exit code, TTY). On other
/// platforms we spawn, wait, and propagate the exit code.
fn exec_claude_transparently(args: &[String]) -> std::process::ExitCode {
    let claude = std::env::var("CLAUDIO_CLAUDE_PATH").unwrap_or_else(|_| "claude".into());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&claude).args(args).exec();
        // exec only returns on failure.
        eprintln!("claudio: could not exec '{claude}': {err}");
        std::process::ExitCode::from(127)
    }

    #[cfg(not(unix))]
    {
        match std::process::Command::new(&claude).args(args).status() {
            Ok(status) => std::process::ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(e) => {
                eprintln!("claudio: could not run '{claude}': {e}");
                std::process::ExitCode::from(127)
            }
        }
    }
}
