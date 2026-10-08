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

mod api;
mod claude;
mod cli;
mod client;
mod daemon;
mod msglog;
mod paths;
mod print;
mod proto;
mod remote;
mod term;
mod tui;

use std::io::{IsTerminal, Read, Write};

use cli::{OutputFormat, WrapperEnv};
use print::emit::Outcome;

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();

    // Remote probe: `claudio __probe` prints one JSON line describing this
    // binary (version, proto, os, arch, sha256 of the exe). Must run before
    // cli::parse so it never accidentally enters passthrough mode.
    if argv.get(1).map(String::as_str) == Some("__probe") {
        return remote::probe::run();
    }

    // Slave bridge: `claudio --slave` is the remote end of an SSH transport.
    // Started by the local client as `ssh HOST '$HOME/.local/bin/claudio --slave'`.
    if argv.get(1).map(String::as_str) == Some("--slave") {
        return remote::bridge::run();
    }

    // Test-harness / diagnostic subcommands (undocumented `__` prefix).
    //
    // `claudio __bootstrap HOST` – run ensure_remote and print the result.
    if argv.get(1).map(String::as_str) == Some("__bootstrap") {
        if let Some(host) = argv.get(2) {
            return remote::diag::bootstrap_cmd(host);
        }
        eprintln!("usage: claudio __bootstrap <host>");
        return std::process::ExitCode::FAILURE;
    }

    // `claudio __connect-check HOST` – connect via SSH, print the Welcome JSON.
    if argv.get(1).map(String::as_str) == Some("__connect-check") {
        if let Some(host) = argv.get(2) {
            return remote::diag::connect_check_cmd(host);
        }
        eprintln!("usage: claudio __connect-check <host>");
        return std::process::ExitCode::FAILURE;
    }

    // `claudio __remote-session HOST CWD` – spawn a session on HOST in CWD,
    // attach, wait for a snapshot, kill and exit.
    if argv.get(1).map(String::as_str) == Some("__remote-session") {
        if let (Some(host), Some(cwd)) = (argv.get(2), argv.get(3)) {
            return remote::diag::remote_session_cmd(host, cwd);
        }
        eprintln!("usage: claudio __remote-session <host> <cwd>");
        return std::process::ExitCode::FAILURE;
    }

    // `claudio __ssh-hosts` – print all known SSH host aliases, one per line.
    if argv.get(1).map(String::as_str) == Some("__ssh-hosts") {
        let hosts = remote::hosts::candidates();
        for h in &hosts {
            println!("{h}");
        }
        return std::process::ExitCode::SUCCESS;
    }

    // §4.9.2 concealed relay: invoked as `<neutral-binary> <EventName> <port>`.
    // The binary is a copy of ourselves with a random hex name in /tmp; the hook
    // command is `<copy> Stop 49152` — no wrapper name, no env var.
    if argv.len() == 3 {
        if let Ok(port) = argv[2].parse::<u16>() {
            if matches!(argv[1].as_str(),
                "SessionStart" | "Stop" | "PreToolUse" | "PostToolUse"
                | "PreCompact" | "PostCompact" | "Notification"
            ) {
                print::hooks::relay_to_port(&argv[1], port);
                return std::process::ExitCode::SUCCESS;
            }
        }
    }

    // Manager hook relay: `claudio __hook <Event> <socket> <token>` ships the
    // hook payload to the daemon's Unix socket.
    if argv.len() == 5 && argv[1] == "__hook" {
        return claude::hooks::relay(&argv[2], std::path::Path::new(&argv[3]), &argv[4]);
    }

    // Legacy relay mode, invoked as `claudio __hook <Event>`.
    if argv.len() >= 3 && argv[1] == "__hook" {
        print::hooks::run_relay(&argv[2]);
        return std::process::ExitCode::SUCCESS;
    }

    // Session daemon: `claudio --daemon` owns the PTYs of the manager sessions.
    if argv.get(1).map(String::as_str) == Some("--daemon") {
        return daemon::run();
    }

    // A bare `claudio` opens the session manager.
    if argv.len() == 1 {
        return tui::run();
    }

    let args = &argv[1..];
    let parsed = cli::parse(args);

    // `claudio --help`/-h (when not driving a turn or the server): show the real
    // claude help, then append our wrapper-specific flag/env reference.
    if !parsed.api_mode && !parsed.print_mode && wants_help(args) {
        return print_help_with_appendix(args);
    }

    // Optional message-flow logging (CLI⇄claudio⇄claude). Flag or env enables it;
    // a file path also enables the raw JSONL sink. Init before any turn runs.
    {
        let pretty = parsed.log_messages
            || std::env::var("CLAUDIO_LOG_MESSAGES").map(|v| v == "1" || v == "true").unwrap_or(false);
        let file = parsed
            .log_messages_file
            .clone()
            .or_else(|| std::env::var("CLAUDIO_LOG_MESSAGES_FILE").ok().filter(|s| !s.is_empty()));
        if pretty || file.is_some() {
            msglog::init(pretty, file.as_deref());
        }
    }

    // §API — `--api` starts the OpenAI-compatible server, served by the same
    // PTY backend that powers `-p`. It takes precedence over print mode.
    if parsed.api_mode {
        for w in &parsed.warnings {
            eprintln!("claudio: {w}");
        }
        return api::serve_blocking();
    }

    // Transparent passthrough: without -p we are just `claude`.
    if !parsed.print_mode {
        return exec_claude_transparently(args);
    }

    for w in &parsed.warnings {
        eprintln!("claudio: {w}");
    }

    let mut env = WrapperEnv::from_env();
    env.fast = env.fast || parsed.fast;

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

    match print::driver::run(&parsed, &env, &prompt) {
        Ok(result) => {
            let outcome = Outcome {
                summary: &result.summary,
                duration_ms: result.duration_ms,
                failure: result.failure.as_deref(),
            };
            let mut stdout = std::io::stdout();
            if let Err(e) = print::emit::emit(&mut stdout, parsed.output_format, &outcome) {
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

/// Whether argv is asking for help (`-h`/`--help` anywhere on the line).
fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "-h" || a == "--help")
}

/// Show the real `claude` help, then append claudio's own flag/env reference.
/// Falls back to printing just our appendix if `claude` can't be run.
fn print_help_with_appendix(args: &[String]) -> std::process::ExitCode {
    let claude = std::env::var("CLAUDIO_CLAUDE_PATH").unwrap_or_else(|_| "claude".into());
    match std::process::Command::new(&claude).args(args).output() {
        Ok(o) => {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&o.stdout);
            let _ = std::io::stderr().write_all(&o.stderr);
            let _ = stdout.write_all(cli::HELP_APPENDIX.as_bytes());
            std::process::ExitCode::from(o.status.code().unwrap_or(0) as u8)
        }
        Err(e) => {
            eprintln!("claudio: could not run '{claude} --help': {e}");
            print!("{}", cli::HELP_APPENDIX);
            std::process::ExitCode::SUCCESS
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
