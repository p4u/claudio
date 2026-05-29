//! Transparent, *dynamic* `claude` argument handling.
//!
//! This binary is a drop-in for `claude`. It deliberately does **not** mirror
//! claude's flag grammar — that would drift every release. Instead it
//! understands only the handful of flags it must own, and forwards everything
//! else (including the prompt, and any flag claude may add in the future)
//! verbatim to the real `claude`.
//!
//! Two facts make this possible:
//!   1. Without `-p`/`--print`, `main` execs the real claude unchanged, so
//!      interactive use, subcommands, `--help`, and `--version` are 100% native.
//!   2. Interactive `claude` auto-runs a positional prompt to completion. So in
//!      print mode we forward the prompt as-is and let claude parse and run it —
//!      we never type it and never need to know any other flag's arity.
//!
//! Flags this wrapper owns:
//!   -p/--print            mode switch (consumed; not forwarded)
//!   --output-format       we format output ourselves (consumed)
//!   --input-format        only `text` supported by this backend (consumed)
//!   --settings            captured; merged with our hooks; re-injected
//!   --session-id          captured (to locate the transcript) and forwarded
//!
//! Flags that would break the backend, so they are dropped with a warning:
//!   --bare                     skips hooks (we need the Stop hook)
//!   --no-session-persistence   suppresses the session JSONL (our source of truth)
//!
//! Everything else is forwarded untouched. Wrapper-only knobs live in
//! `CLAUDE_POC_*` env vars so the CLI surface stays byte-for-byte claude's.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    StreamJson,
}

impl OutputFormat {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(OutputFormat::Text),
            "json" => Some(OutputFormat::Json),
            "stream-json" => Some(OutputFormat::StreamJson),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookTransport {
    Tcp,
    File,
}

#[derive(Debug)]
pub struct Parsed {
    pub print_mode: bool,
    pub output_format: OutputFormat,
    pub session_id: Option<String>,
    pub user_settings: Option<String>,
    /// Flags to forward verbatim to the interactive `claude` child (our owned
    /// flags removed; the prompt removed — we type it ourselves; every other
    /// flag preserved in order, including ones we've never heard of).
    pub forward: Vec<String>,
    /// The prompt to type into the TUI. Extracted here for the common form;
    /// `main` overrides it with stdin when input is piped.
    pub prompt: Option<String>,
    pub warnings: Vec<String>,
}

/// A *minimal* set of claude flags known to take a value. This is used for one
/// purpose only: locating the trailing positional prompt so we can type it
/// rather than mis-read a flag's value as the prompt. It is NOT a model of
/// claude's grammar — unknown flags are still forwarded verbatim, and `--`
/// always delimits the prompt unambiguously (recommended for scripts).
fn known_value_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--model" | "--agent" | "--agents" | "--append-system-prompt" | "--system-prompt"
            | "--append-system-prompt-file" | "--system-prompt-file" | "--debug-file" | "--effort"
            | "--fallback-model" | "--json-schema" | "--max-budget-usd" | "-n" | "--name"
            | "--permission-mode" | "--plugin-dir" | "--plugin-url" | "--setting-sources"
            | "--remote-control-session-name-prefix"
    )
}

fn split_eq(tok: &str) -> (&str, Option<&str>) {
    match tok.split_once('=') {
        Some((f, v)) => (f, Some(v)),
        None => (tok, None),
    }
}

/// Parse argv with program name already stripped.
pub fn parse(args: &[String]) -> Parsed {
    let mut p = Parsed {
        print_mode: false,
        output_format: OutputFormat::Text,
        session_id: None,
        user_settings: None,
        forward: Vec::new(),
        prompt: None,
        warnings: Vec::new(),
    };

    let mut i = 0;
    while i < args.len() {
        let tok = &args[i];

        // End of options: everything after "--" is the prompt (unambiguous
        // form, recommended for scripts). We type it; it is not forwarded.
        if tok == "--" {
            let rest: Vec<String> = args[i + 1..].to_vec();
            if !rest.is_empty() {
                p.prompt = Some(rest.join(" "));
            }
            break;
        }

        let (flag, inline) = split_eq(tok);

        // Helper: fetch this owned flag's value (inline `=v` or next token) and
        // return how many argv tokens it consumed.
        let take_value = |args: &[String], i: usize| -> (Option<String>, usize) {
            if let Some(v) = inline {
                (Some(v.to_string()), 1)
            } else if i + 1 < args.len() {
                (Some(args[i + 1].clone()), 2)
            } else {
                (None, 1)
            }
        };

        match flag {
            "-p" | "--print" => {
                p.print_mode = true;
                i += 1;
            }
            "--output-format" => {
                let (v, n) = take_value(args, i);
                match v.as_deref().and_then(OutputFormat::parse) {
                    Some(f) => p.output_format = f,
                    None => {
                        if let Some(v) = v {
                            p.warnings.push(format!("unknown --output-format '{v}', using text"));
                        }
                    }
                }
                i += n;
            }
            "--input-format" => {
                let (v, n) = take_value(args, i);
                if let Some(v) = v {
                    if v != "text" {
                        p.warnings.push(format!(
                            "--input-format {v} is not supported by the interactive backend; using text"
                        ));
                    }
                }
                i += n;
            }
            "--settings" => {
                let (v, n) = take_value(args, i);
                p.user_settings = v;
                i += n; // not forwarded; driver injects merged settings
            }
            "--session-id" => {
                let (v, n) = take_value(args, i);
                p.session_id = v.clone();
                // capture AND forward so claude uses the same id
                if inline.is_some() {
                    p.forward.push(tok.clone());
                } else {
                    p.forward.push("--session-id".to_string());
                    if let Some(v) = v {
                        p.forward.push(v);
                    }
                }
                i += n;
            }
            "--bare" | "--no-session-persistence" => {
                p.warnings.push(format!(
                    "{flag} is incompatible with the interactive backend (it disables the hooks/transcript this wrapper relies on); dropping it"
                ));
                i += 1;
            }
            _ => {
                // Unknown to us → forward verbatim. We do not consume following
                // tokens: claude's own parser handles their arity.
                p.forward.push(tok.clone());
                i += 1;
            }
        }
    }

    // If the prompt wasn't given via `--`, extract the trailing positional from
    // the forwarded tokens (so we type it rather than letting claude run it).
    if p.print_mode && p.prompt.is_none() {
        if let Some(idx) = last_positional_index(&p.forward) {
            p.prompt = Some(p.forward.remove(idx));
        }
    }

    p
}

/// Index of the last bare positional in a forwarded-args list, skipping the
/// values of the minimal known value-flags. Best-effort: `--` (handled above)
/// is the unambiguous escape hatch for anything this heuristic can't resolve.
fn last_positional_index(forward: &[String]) -> Option<usize> {
    let mut i = 0;
    let mut last = None;
    while i < forward.len() {
        let tok = &forward[i];
        let (flag, inline) = split_eq(tok);
        if tok.starts_with('-') && tok != "-" {
            if known_value_flag(flag) && inline.is_none() {
                i += 2; // skip the flag's value
            } else {
                i += 1;
            }
        } else {
            last = Some(i);
            i += 1;
        }
    }
    last
}

/// Wrapper-only knobs, sourced from the environment so the CLI surface stays
/// identical to claude's.
pub struct WrapperEnv {
    pub debug: bool,
    pub timeout_sec: u64,
    pub hook_transport: HookTransport,
    pub raw_log: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub claude_path: String,
}

impl WrapperEnv {
    pub fn from_env() -> Self {
        let g = |k: &str| std::env::var(k).ok();
        WrapperEnv {
            debug: g("CLAUDE_POC_DEBUG").map(|v| v == "1" || v == "true").unwrap_or(false),
            timeout_sec: g("CLAUDE_POC_TIMEOUT_SEC").and_then(|v| v.parse().ok()).unwrap_or(300),
            hook_transport: match g("CLAUDE_POC_HOOK_TRANSPORT").as_deref() {
                Some("file") => HookTransport::File,
                _ => HookTransport::Tcp,
            },
            raw_log: g("CLAUDE_POC_RAW_LOG"),
            cols: g("CLAUDE_POC_COLS").and_then(|v| v.parse().ok()).unwrap_or(120),
            rows: g("CLAUDE_POC_ROWS").and_then(|v| v.parse().ok()).unwrap_or(40),
            claude_path: g("CLAUDE_POC_CLAUDE_PATH").unwrap_or_else(|| "claude".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn print_prompt_only() {
        let p = parse(&s(&["-p", "hello world"]));
        assert!(p.print_mode);
        assert_eq!(p.prompt.as_deref(), Some("hello world"));
        assert!(p.forward.is_empty());
        assert_eq!(p.output_format, OutputFormat::Text);
    }

    #[test]
    fn output_format_separate_and_inline() {
        let p = parse(&s(&["-p", "--output-format", "json", "go"]));
        assert_eq!(p.output_format, OutputFormat::Json);
        assert_eq!(p.prompt.as_deref(), Some("go"));
        assert!(p.forward.is_empty());
        let p = parse(&s(&["-p", "--output-format=stream-json", "go"]));
        assert_eq!(p.output_format, OutputFormat::StreamJson);
        assert_eq!(p.prompt.as_deref(), Some("go"));
    }

    #[test]
    fn value_flag_not_mistaken_for_prompt() {
        // --model takes a value; "opus" must not be read as the prompt.
        let p = parse(&s(&["-p", "--model", "opus", "go"]));
        assert_eq!(p.prompt.as_deref(), Some("go"));
        assert_eq!(p.forward, s(&["--model", "opus"]));
    }

    #[test]
    fn variadic_then_prompt() {
        let p = parse(&s(&["-p", "--allowedTools", "Bash", "Edit", "go"]));
        assert_eq!(p.prompt.as_deref(), Some("go"));
        assert_eq!(p.forward, s(&["--allowedTools", "Bash", "Edit"]));
    }

    #[test]
    fn dangerously_skip_forwarded() {
        let p = parse(&s(&["-p", "--dangerously-skip-permissions", "go"]));
        assert!(p.forward.contains(&"--dangerously-skip-permissions".to_string()));
        assert_eq!(p.prompt.as_deref(), Some("go"));
    }

    #[test]
    fn session_id_captured_and_forwarded() {
        let p = parse(&s(&["-p", "--session-id", "abc", "go"]));
        assert_eq!(p.session_id.as_deref(), Some("abc"));
        assert_eq!(p.prompt.as_deref(), Some("go"));
        assert_eq!(p.forward, s(&["--session-id", "abc"]));
    }

    #[test]
    fn settings_captured_not_forwarded() {
        let p = parse(&s(&["-p", "--settings", "{\"x\":1}", "go"]));
        assert_eq!(p.user_settings.as_deref(), Some("{\"x\":1}"));
        assert!(!p.forward.iter().any(|a| a == "--settings"));
        assert_eq!(p.prompt.as_deref(), Some("go"));
    }

    #[test]
    fn breakers_dropped_with_warning() {
        let p = parse(&s(&["-p", "--bare", "--no-session-persistence", "go"]));
        assert_eq!(p.prompt.as_deref(), Some("go"));
        assert!(p.forward.is_empty());
        assert_eq!(p.warnings.len(), 2);
    }

    #[test]
    fn no_print_when_absent() {
        let p = parse(&s(&["--model", "opus", "hi"]));
        assert!(!p.print_mode);
        assert!(p.prompt.is_none());
    }

    #[test]
    fn double_dash_is_the_prompt() {
        let p = parse(&s(&["-p", "--model", "opus", "--", "--looks-like-flag"]));
        assert_eq!(p.prompt.as_deref(), Some("--looks-like-flag"));
        assert_eq!(p.forward, s(&["--model", "opus"]));
    }
}
