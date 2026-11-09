//! The API addon: `-p` print emulation and the OpenAI-compatible `--api`
//! server, both on the interactive claude TUI under a PTY. It has
//! `Commands` only: the print hook relays, `-p`/`--print` and `--api`
//! (decided by `cli::parse`, so `--` keeps its meaning), and `--help`'s
//! appendix. Its PTY sessions stay outside launch composition.

// `addons::api::api`: the plan's layout (the server keeps its name until P4
// makes this addon a crate).
#[allow(clippy::module_inception)]
pub mod api;
pub mod cli;
pub mod msglog;
pub mod print;

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::sync::Arc;

use crate::addon::{Addon, Addons, Commands};
use cli::{OutputFormat, Parsed, WrapperEnv};
use print::emit::Outcome;

/// The hook events the concealed print relay is invoked with.
const RELAY_EVENTS: &[&str] = &[
    "SessionStart",
    "Stop",
    "PreToolUse",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "Notification",
];

/// The API addon.
pub struct ApiAddon;

impl Addon for ApiAddon {
    fn name(&self) -> &'static str {
        "api"
    }

    fn commands(&self) -> Option<&dyn Commands> {
        Some(self)
    }
}

/// `<neutral-binary> <EventName> <port>`: the concealed relay (§4.9.2). The
/// binary is a copy of claudio with a random hex name in /tmp; the hook
/// command is `<copy> Stop 49152`, no wrapper name, no env var.
fn port_relay(argv: &[String]) -> Option<(&str, u16)> {
    let [_, event, port] = argv else {
        return None;
    };
    let port = port.parse::<u16>().ok()?;
    RELAY_EVENTS
        .contains(&event.as_str())
        .then_some((event.as_str(), port))
}

/// `claudio __hook <Event>`, the legacy relay (the manager's own
/// `__hook <Event> <socket> <token>`, five words, is the core's).
fn legacy_relay(argv: &[String]) -> Option<&str> {
    (argv.len() >= 3 && argv.len() != 5 && argv[1] == "__hook").then(|| argv[2].as_str())
}

impl Commands for ApiAddon {
    fn is_relay(&self, argv: &[String]) -> bool {
        port_relay(argv).is_some() || legacy_relay(argv).is_some()
    }

    fn dispatch(&self, argv: &[String], _addons: &Arc<Addons>) -> Option<ExitCode> {
        if let Some((event, port)) = port_relay(argv) {
            print::hooks::relay_to_port(event, port);
            return Some(ExitCode::SUCCESS);
        }
        if let Some(event) = legacy_relay(argv) {
            print::hooks::run_relay(event);
            return Some(ExitCode::SUCCESS);
        }
        let parsed = cli::parse(argv.get(1..).unwrap_or_default());
        if !parsed.api_mode && !parsed.print_mode {
            return None;
        }
        init_message_log(&parsed);
        for w in &parsed.warnings {
            eprintln!("claudio: {w}");
        }
        // `--api` takes precedence over print mode.
        if parsed.api_mode {
            return Some(api::serve_blocking());
        }
        Some(run_print(&parsed))
    }

    fn help(&self) -> &'static str {
        cli::HELP_APPENDIX
    }
}

/// Optional message-flow logging (CLI⇄claudio⇄claude). Flag or env enables it;
/// a file path also enables the raw JSONL sink.
fn init_message_log(parsed: &Parsed) {
    let pretty = parsed.log_messages
        || std::env::var("CLAUDIO_LOG_MESSAGES")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);
    let file = parsed.log_messages_file.clone().or_else(|| {
        std::env::var("CLAUDIO_LOG_MESSAGES_FILE")
            .ok()
            .filter(|s| !s.is_empty())
    });
    if pretty || file.is_some() {
        msglog::init(pretty, file.as_deref());
    }
}

/// The prompt: explicit (positional/`--`) wins; otherwise stdin.
fn read_prompt(parsed: &Parsed) -> Result<String, ExitCode> {
    if let Some(p) = &parsed.prompt {
        return Ok(p.clone());
    }
    if std::io::stdin().is_terminal() {
        eprintln!("claudio: a prompt is required (positional arg, `-- <prompt>`, or stdin)");
        return Err(ExitCode::from(2));
    }
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
        eprintln!("claudio: empty prompt on stdin");
        return Err(ExitCode::from(2));
    }
    Ok(buf)
}

/// `-p`: one turn through the interactive TUI, emitted like `claude -p`.
fn run_print(parsed: &Parsed) -> ExitCode {
    let mut env = WrapperEnv::from_env();
    env.fast = env.fast || parsed.fast;
    let prompt = match read_prompt(parsed) {
        Ok(p) => p,
        Err(code) => return code,
    };
    match print::driver::run(parsed, &env, &prompt) {
        Ok(result) => {
            let outcome = Outcome {
                summary: &result.summary,
                duration_ms: result.duration_ms,
                failure: result.failure.as_deref(),
            };
            let mut stdout = std::io::stdout();
            if let Err(e) = print::emit::emit(&mut stdout, parsed.output_format, &outcome) {
                eprintln!("{e}");
                return ExitCode::from(2);
            }
            if outcome.failure.is_some() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("claudio: {e}");
            if matches!(
                parsed.output_format,
                OutputFormat::Json | OutputFormat::StreamJson
            ) {
                let obj = serde_json::json!({
                    "type": "result", "subtype": "error", "is_error": true,
                    "result": "", "terminal_reason": e.to_string(),
                });
                println!("{obj}");
            }
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        std::iter::once("claudio")
            .chain(words.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn relays_are_the_port_form_and_the_legacy_hook_only() {
        let api = ApiAddon;
        assert!(api.is_relay(&argv(&["Stop", "49152"])));
        assert!(api.is_relay(&argv(&["SessionStart", "1"])));
        assert!(!api.is_relay(&argv(&["Stop", "notaport"])));
        assert!(!api.is_relay(&argv(&["Other", "49152"])));
        assert!(api.is_relay(&argv(&["__hook", "Stop"])));
        // The manager's five-word relay is the core's.
        assert!(!api.is_relay(&argv(&["__hook", "Stop", "/sock", "tok"])));
        assert!(!api.is_relay(&argv(&["__hook"])));
    }

    #[test]
    fn dispatch_leaves_anything_but_print_and_api_alone() {
        let addons = Arc::new(Addons::empty());
        for words in [&["--help"][..], &["mcp", "list"], &["--", "-p"], &[]] {
            assert!(
                ApiAddon.dispatch(&argv(words), &addons).is_none(),
                "{words:?}"
            );
        }
        assert!(ApiAddon.help().contains("Claudio custom flags"));
    }
}
