//! CLI subcommands: `claudio proxy login/status/logout/use`.
//!
//! These commands run synchronously (blocking tokio runtime) before the rest
//! of the TUI starts. They are dispatched in `main.rs` before `cli::parse`.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use super::api;
use super::profile::{self, Profile};

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// Handle `claudio proxy <args>`. Returns `Some(code)` when the subcommand
/// was handled, `None` when `args` is not a proxy subcommand.
pub fn dispatch(args: &[String]) -> Option<ExitCode> {
    // args[0] must be "proxy".
    if args.first().map(String::as_str) != Some("proxy") {
        return None;
    }
    let sub = args.get(1).map(String::as_str).unwrap_or("--help");
    let rest = if args.len() > 2 { &args[2..] } else { &[] };

    let code = match sub {
        "login" => cmd_login(rest),
        "status" => cmd_status(),
        "logout" => cmd_logout(rest),
        "use" => cmd_use(rest),
        "-h" | "--help" | "help" => {
            print!("{}", PROXY_HELP);
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("claudio proxy: unknown subcommand '{other}'");
            eprintln!("{PROXY_HELP}");
            ExitCode::FAILURE
        }
    };
    Some(code)
}

const PROXY_HELP: &str = "\
claudio proxy — manage claude-proxy profiles

USAGE
  claudio proxy login [URL]         Add or update a proxy profile
  claudio proxy status              Show profiles and live stats
  claudio proxy logout [NAME]       Remove a proxy profile
  claudio proxy use NAME|none       Set the default profile

OPTIONS (login)
  --name NAME   Profile name (default: first DNS label of the host)
";

// ── login ─────────────────────────────────────────────────────────────────────

fn cmd_login(args: &[String]) -> ExitCode {
    // Parse flags.
    let mut name_override: Option<String> = None;
    let mut url_arg: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                i += 1;
                name_override = args.get(i).cloned();
            }
            a if !a.starts_with('-') => {
                url_arg = Some(a.to_owned());
            }
            other => {
                eprintln!("claudio proxy login: unknown flag '{other}'");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    // Get URL.
    let url_raw = match url_arg {
        Some(u) => u,
        None => {
            eprint!("Proxy URL (e.g. https://claude.example.net): ");
            let _ = io::stderr().flush();
            let mut line = String::new();
            if io::stdin().lock().read_line(&mut line).is_err() || line.trim().is_empty() {
                eprintln!("claudio proxy login: no URL provided");
                return ExitCode::FAILURE;
            }
            line.trim().to_owned()
        }
    };

    // Normalize: if the URL contains '@', treat it as CLAUDIO_PROXY_URL format.
    let (url, token) = if url_raw.contains('@') {
        match profile::parse_proxy_url(&url_raw) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("claudio proxy login: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        // Normalize host string to a proper URL.
        let url = match profile::normalize_url(&url_raw) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("claudio proxy login: {e}");
                return ExitCode::FAILURE;
            }
        };
        let token = read_token_from_tty_or_stdin();
        if token.is_empty() {
            eprintln!("claudio proxy login: no token provided");
            return ExitCode::FAILURE;
        }
        (url, token)
    };

    let name = name_override.unwrap_or_else(|| profile::name_from_url(&url));

    // Validate with the proxy.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let ok = rt.block_on(async {
        match api::check_root(&url, &token).await {
            Ok(true) => {
                println!("✓  Proxy responded at {url}");
                true
            }
            Ok(false) => {
                // Endpoint doesn't exist — try /v1/models to at least check auth.
                eprintln!("notice: /v1/claudio not found; proxy may not support the Claudio API");
                eprintln!("        Checking auth with /v1/models …");
                match api::check_models_fallback(&url, &token).await {
                    Ok(()) => {
                        println!("✓  Token accepted at {url} (no Claudio API, stats unavailable)");
                        true
                    }
                    Err(e) => {
                        eprintln!("claudio proxy login: {e}");
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("claudio proxy login: could not reach {url}: {e}");
                false
            }
        }
    });

    if !ok {
        return ExitCode::FAILURE;
    }

    // Save.
    let mut sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio proxy login: could not read config.toml: {e}");
            return ExitCode::FAILURE;
        }
    };
    let is_first = sec.profiles.is_empty();
    sec.profiles.insert(name.clone(), Profile { url: url.clone(), token });
    if is_first || sec.default.is_none() {
        sec.default = Some(name.clone());
        println!("  Set '{name}' as default proxy.");
    }
    if let Err(e) = profile::save(&sec) {
        eprintln!("claudio proxy login: could not save config.toml: {e}");
        return ExitCode::FAILURE;
    }
    println!("  Saved profile '{name}'.");
    ExitCode::SUCCESS
}

/// Read a token with no-echo (raw TTY or plain stdin).
fn read_token_from_tty_or_stdin() -> String {
    use std::io::IsTerminal;

    eprint!("Token: ");
    let _ = io::stderr().flush();

    if io::stdin().is_terminal() {
        // Use libc termios to suppress echo.
        read_token_no_echo()
    } else {
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        eprintln!(); // mimic the newline the terminal would print.
        line.trim().to_owned()
    }
}

/// Read a line from stdin with echo disabled using libc termios.
fn read_token_no_echo() -> String {
    use std::os::unix::io::AsRawFd;

    let stdin_fd = io::stdin().as_raw_fd();

    // Save current termios.
    let mut old = libc::termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_line: 0,
        c_cc: [0u8; 32],
        c_ispeed: 0,
        c_ospeed: 0,
    };
    // SAFETY: tcgetattr has well-defined semantics on a valid fd.
    if unsafe { libc::tcgetattr(stdin_fd, &mut old) } != 0 {
        // Not a real terminal — fall back to echo-on read.
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        eprintln!();
        return line.trim().to_owned();
    }

    let mut raw = old;
    raw.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
    // SAFETY: tcsetattr restores the saved state after the read.
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) };

    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);

    // Restore.
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &old) };
    eprintln!();
    line.trim().to_owned()
}

// ── status ────────────────────────────────────────────────────────────────────

fn cmd_status() -> ExitCode {
    let sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio proxy: could not read config.toml: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Also consider the env-var ephemeral profile.
    let env_profile = profile::from_env();

    let default_name = env_profile
        .as_ref()
        .map(|(n, _)| n.to_string())
        .or_else(|| sec.default.clone());

    if sec.profiles.is_empty() && env_profile.is_none() {
        println!("No proxy profiles configured.");
        println!("Run `claudio proxy login` or set CLAUDIO_PROXY_URL.");
        return ExitCode::SUCCESS;
    }

    println!("Proxy profiles:");
    // Show env profile first, then saved ones.
    if let Some((n, p)) = &env_profile {
        let marker = if default_name.as_deref() == Some(n) { " (default, env)" } else { " (env)" };
        println!("  {n}{marker}  {}  {}", p.url, p.masked_token());
    }
    for (n, p) in &sec.profiles {
        let marker = if default_name.as_deref() == Some(n.as_str()) { " (default)" } else { "" };
        println!("  {n}{marker}  {}  {}", p.url, p.masked_token());
    }

    // Live stats for the default profile.
    let active_profile = env_profile
        .as_ref()
        .map(|(_, p)| p)
        .or_else(|| sec.default.as_ref().and_then(|n| sec.profiles.get(n)));

    if let Some(p) = active_profile {
        println!();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(print_live_status(p));
    }

    ExitCode::SUCCESS
}

async fn print_live_status(p: &Profile) {
    // Stats.
    match api::fetch_stats(&p.url, &p.token, "24h").await {
        Ok(s) => {
            let total_tok = s.totals.input_tokens + s.totals.output_tokens;
            println!("Stats ({}): {} req  {}  {} tok total",
                s.period,
                s.totals.requests,
                s.user_name,
                fmt_tokens(total_tok),
            );
            if !s.by_model.is_empty() {
                println!("  By model:");
                for m in &s.by_model {
                    println!("    {:50}  {} req  {} out tok", m.model, m.requests, fmt_tokens(m.output_tokens));
                }
            }
            if let Some(lim) = &s.limit {
                println!("  Limit: {:.0}% of {} out-tok/{}s",
                    lim.used_pct * 100.0,
                    fmt_tokens(lim.output_tokens),
                    lim.window_seconds,
                );
                if lim.blocked {
                    println!("  BLOCKED until {:?}", lim.blocked_until);
                }
            }
        }
        Err(e) => eprintln!("  Stats unavailable: {e}"),
    }

    // Pool health.
    match api::fetch_pool_health(&p.url, &p.token).await {
        Ok(h) => {
            println!("Pool: {}", h.overall().label());
            for prov in &h.providers {
                println!("  {:20} {}", prov.name, prov.status);
            }
        }
        Err(e) => eprintln!("  Pool health unavailable: {e}"),
    }
}

fn fmt_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ── logout ────────────────────────────────────────────────────────────────────

fn cmd_logout(args: &[String]) -> ExitCode {
    let mut sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio proxy: could not read config.toml: {e}");
            return ExitCode::FAILURE;
        }
    };

    let name = match args.first() {
        Some(n) => n.clone(),
        None => match &sec.default {
            Some(d) => d.clone(),
            None => {
                eprintln!("claudio proxy logout: no profile name given and no default set");
                return ExitCode::FAILURE;
            }
        },
    };

    if sec.profiles.remove(&name).is_none() {
        eprintln!("claudio proxy logout: profile '{name}' not found");
        return ExitCode::FAILURE;
    }

    // If we removed the default, clear it (or pick another one).
    if sec.default.as_deref() == Some(&name) {
        sec.default = sec.profiles.keys().next().cloned();
    }

    if let Err(e) = profile::save(&sec) {
        eprintln!("claudio proxy logout: could not save config.toml: {e}");
        return ExitCode::FAILURE;
    }
    println!("Removed profile '{name}'.");
    ExitCode::SUCCESS
}

// ── use ───────────────────────────────────────────────────────────────────────

fn cmd_use(args: &[String]) -> ExitCode {
    let name = match args.first() {
        Some(n) => n.clone(),
        None => {
            eprintln!("claudio proxy use: missing NAME or 'none'");
            return ExitCode::FAILURE;
        }
    };

    let mut sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio proxy: could not read config.toml: {e}");
            return ExitCode::FAILURE;
        }
    };

    if name == "none" {
        sec.default = None;
        println!("Default proxy cleared (sessions start without a proxy).");
    } else {
        if !sec.profiles.contains_key(&name) {
            eprintln!("claudio proxy use: profile '{name}' not found");
            return ExitCode::FAILURE;
        }
        sec.default = Some(name.clone());
        println!("Default proxy set to '{name}'.");
    }

    if let Err(e) = profile::save(&sec) {
        eprintln!("claudio proxy use: could not save config.toml: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
