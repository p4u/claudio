//! The addon's CLI: `claudio vocdoni login|status|use|logout`, and
//! `claudio proxy …` as an alias with the same subcommands. They run
//! synchronously (a blocking tokio runtime), dispatched by the addon
//! registry after the core's own words.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use super::proxy::api;
use super::proxy::login::{self, Granted};
use super::proxy::plugins;
use super::proxy::profile::{self, Profile, ProxySection};
use crate::text::fmt_tokens;

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// The words this addon claims: `vocdoni`, and `proxy` (its alias).
pub const WORDS: &[&str] = &["vocdoni", "proxy"];

/// Handle `claudio vocdoni <args>` or `claudio proxy <args>` (`args[0]` is
/// the word). `None` when `args` is neither.
pub fn dispatch(args: &[String]) -> Option<ExitCode> {
    args.first().filter(|w| WORDS.contains(&w.as_str()))?;
    let sub = args.get(1).map(String::as_str).unwrap_or("--help");
    let rest = args.get(2..).unwrap_or_default();
    let code = match (subcommand(sub), sub) {
        (Some(run), _) => run(rest),
        (None, "-h" | "--help" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        (None, other) => {
            eprintln!("claudio vocdoni: unknown subcommand '{other}'");
            eprintln!("{HELP}");
            ExitCode::FAILURE
        }
    };
    Some(code)
}

/// What runs subcommand `sub`, under either word.
fn subcommand(sub: &str) -> Option<fn(&[String]) -> ExitCode> {
    match sub {
        "login" => Some(cmd_login),
        "status" => Some(|_| cmd_status()),
        "logout" => Some(cmd_logout),
        "use" => Some(cmd_use),
        _ => None,
    }
}

/// `claudio vocdoni --help`.
pub const HELP: &str = "\
claudio vocdoni — the Vocdoni proxy (claude-proxy) profiles and team plugins
(`claudio proxy …` is an alias with the same subcommands)

USAGE
  claudio vocdoni login [URL]       Sign in through the browser (or paste a token)
  claudio vocdoni status            Show profiles and live stats
  claudio vocdoni logout [NAME]     Remove a proxy profile and its cached plugins
  claudio vocdoni use NAME|none     Set the default profile

OPTIONS (login)
  URL            The proxy (default: the default profile's, else
                 https://claude.vocdoni.net)
  --name NAME    Profile name (default: the default profile's when URL is not
                 given, else the first DNS label of the host)
  --token        Paste a token instead of signing in through the browser
                 (read from stdin when it is not a terminal)
  --unverified   Save the profile even if token verification fails

The sign-in page opens in a browser when there is a display; set
CLAUDIO_NO_BROWSER=1 to only print its URL.
";

/// The addon's lines in `claudio --help`.
pub const APPENDIX: &str = "\n\
\x20Vocdoni addon\n\
\x20─────────────\n\
\x20  claudio vocdoni login [URL] [--token]   Sign in to the Vocdoni proxy (default https://claude.vocdoni.net)\n\
\x20  claudio vocdoni status|use NAME|logout  Profiles, the default one, stats (`claudio vocdoni --help`)\n\
\x20  claudio proxy …                         The same subcommands (alias)\n\
\x20  CLAUDIO_PROXY_URL=<token>@<host>        An ephemeral proxy profile for this run (overrides the\n\
\x20                                          default); http:// only for 127.0.0.1/localhost.\n";

// ── login ─────────────────────────────────────────────────────────────────────

/// `login`'s arguments.
#[derive(Debug, Default, PartialEq)]
struct LoginArgs {
    name: Option<String>,
    url: Option<String>,
    unverified: bool,
    paste: bool,
}

fn parse_login(args: &[String]) -> Result<LoginArgs, String> {
    let mut out = LoginArgs::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--name" => out.name = args.next().cloned(),
            "--unverified" => out.unverified = true,
            "--token" => out.paste = true,
            a if !a.starts_with('-') => out.url = Some(a.to_owned()),
            other => return Err(format!("unknown flag '{other}'")),
        }
    }
    Ok(out)
}

/// The proxy to sign in to, normalized: `url` as given, else the default
/// profile's (and its name), else [`super::DEFAULT_URL`]. Never a token.
fn login_target(
    url: Option<String>,
    sec: &ProxySection,
) -> Result<(String, Option<String>), String> {
    let default = sec
        .default
        .as_ref()
        .and_then(|n| sec.profiles.get(n).map(|p| (p.url.clone(), n.clone())));
    let (raw, name) = match (url, default) {
        (Some(u), _) => (u, None),
        (None, Some((u, n))) => (u, Some(n)),
        (None, None) => (super::DEFAULT_URL.to_owned(), None),
    };
    // TOKEN@HOST: tokens come from the prompt, stdin or CLAUDIO_PROXY_URL.
    if raw.contains('@') {
        return Err("tokens are never accepted as arguments;\n  \
             use the interactive prompt, pipe it on stdin, or set CLAUDIO_PROXY_URL"
            .to_owned());
    }
    Ok((profile::normalize_url(&raw)?, name))
}

/// The token for `url` and the e-mail it was granted for: pasted when
/// asked (or when the proxy has no browser sign-in), else signed in.
fn obtain_token(
    rt: &tokio::runtime::Runtime,
    url: &str,
    paste: bool,
) -> Result<(String, Option<String>), String> {
    let (token, email) = if paste {
        (read_token_from_tty_or_stdin(), None)
    } else {
        match rt.block_on(browser_login(url)) {
            Ok(Some(granted)) => {
                let email = Some(granted.email).filter(|e| !e.is_empty());
                (granted.token, email)
            }
            Ok(None) => {
                eprintln!("This proxy has no browser login; paste a token.");
                (read_token_from_tty_or_stdin(), None)
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    if token.is_empty() {
        return Err("no token provided".to_owned());
    }
    Ok((token, email))
}

/// Whether the proxy at `url` accepts `token`, saying what it found.
fn verify_token(rt: &tokio::runtime::Runtime, url: &str, token: &str) -> bool {
    rt.block_on(async {
        match api::check_root(url, token).await {
            Ok(true) => {
                println!("✓  Proxy responded at {url}");
                true
            }
            Ok(false) => {
                // Endpoint doesn't exist — try /v1/models to at least check auth.
                eprintln!("notice: /v1/claudio not found; proxy may not support the Claudio API");
                eprintln!("        Checking auth with /v1/models …");
                match api::check_models_fallback(url, token).await {
                    Ok(()) => {
                        println!("✓  Token accepted at {url} (no Claudio API, stats unavailable)");
                        true
                    }
                    Err(e) => {
                        eprintln!("claudio vocdoni login: {e}");
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("claudio vocdoni login: could not reach {url}: {e}");
                false
            }
        }
    })
}

fn cmd_login(args: &[String]) -> ExitCode {
    let fail = |why: String| {
        eprintln!("claudio vocdoni login: {why}");
        ExitCode::FAILURE
    };
    let args = match parse_login(args) {
        Ok(args) => args,
        Err(why) => return fail(why),
    };
    let sec = match profile::load() {
        Ok(s) => s,
        Err(e) => return fail(format!("could not read config.toml: {e}")),
    };
    let (url, default_name) = match login_target(args.url, &sec) {
        Ok(target) => target,
        Err(why) => return fail(why),
    };
    let name = args
        .name
        .or(default_name)
        .unwrap_or_else(|| profile::name_from_url(&url));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (token, email) = match obtain_token(&rt, &url, args.paste) {
        Ok(got) => got,
        Err(why) => return fail(why),
    };
    if !verify_token(&rt, &url, &token) {
        if !args.unverified {
            return ExitCode::FAILURE;
        }
        eprintln!("warning: token NOT verified — saving profile as unverified");
    }
    let (as_default, sec) = match save_login(&name, url, token, email.clone()) {
        Ok(saved) => saved,
        Err(e) => return fail(format!("could not save config.toml: {e}")),
    };
    let saved = if as_default {
        format!("Profile \"{name}\" saved as default.")
    } else {
        format!("Profile \"{name}\" saved.")
    };
    match &email {
        Some(email) => println!("Signed in as {}. {saved}", printable(email)),
        None => println!("{saved}"),
    }
    // The new token's plugins, so the next session has them.
    if let Some(p) = sec.profiles.get(&name) {
        rt.block_on(refresh_plugins(&name, p));
    }
    ExitCode::SUCCESS
}

/// The browser sign-in at `url`: `Ok(None)` when the proxy has none.
async fn browser_login(url: &str) -> Result<Option<Granted>, login::LoginError> {
    use std::io::IsTerminal;
    let Some(start) = login::start(url).await? else {
        return Ok(None);
    };
    let page = printable(&start.verify_url);
    println!("Open this page, sign in, and type your code:");
    println!("  {page}");
    println!("Your code: {}", printable(&start.user_code));
    let web = page.starts_with("https://") || page.starts_with("http://");
    let env = |k: &str| std::env::var(k).ok();
    if web
        && crate::app::browser::browser_wanted(
            std::env::consts::OS,
            env,
            io::stdout().is_terminal(),
        )
    {
        crate::app::browser::open_browser(&page);
    }
    println!("Waiting… (Ctrl+C to cancel)");
    let _ = io::stdout().flush();
    login::poll(url, &start, std::time::Duration::from_secs(1))
        .await
        .map(Some)
}

/// Fetch profile `name`'s plugins and cache them; say which there are. A
/// proxy without plugins (404) says nothing.
async fn refresh_plugins(name: &str, p: &Profile) {
    match plugins::fetch(name, p).await {
        Ok(list) => {
            if let Err(e) = plugins::save_cache(&list) {
                eprintln!("  notice: could not save the plugins: {e}");
            } else if !list.plugins.is_empty() {
                println!("  Plugins: {}", printable(&list.summary()));
            }
        }
        Err(api::ApiError::Http(s)) if s.as_u16() == 404 => {}
        Err(e) => eprintln!("  notice: could not fetch the plugins: {e}"),
    }
}

/// `text` from a proxy, safe to print: no control characters.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Store a logged-in profile, replacing any profile of that name. A re-login
/// keeps the profile's `first_party` mode: only the URL, token and e-mail
/// change (a pasted token has no e-mail).
/// Save the login onto `[proxy]` as it is now (another process may have
/// edited it meanwhile), as the default when it is the first profile or
/// none is set; whether it became the default, and the saved section.
fn save_login(
    name: &str,
    url: String,
    token: String,
    email: Option<String>,
) -> io::Result<(bool, profile::ProxySection)> {
    profile::update(|sec| {
        let is_first = sec.profiles.is_empty();
        store_login(sec, name, url, token, email);
        let as_default = is_first || sec.default.is_none();
        if as_default {
            sec.default = Some(name.to_owned());
        }
        as_default
    })
}

fn store_login(
    sec: &mut profile::ProxySection,
    name: &str,
    url: String,
    token: String,
    email: Option<String>,
) {
    let first_party = sec.profiles.get(name).is_some_and(|p| p.first_party);
    sec.profiles.insert(
        name.to_owned(),
        Profile {
            url,
            token,
            first_party,
            email,
        },
    );
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

// ── No-echo token reader ───────────────────────────────────────────────────────
//
// Uses libc termios to disable echo. Portable across Linux and macOS via
// MaybeUninit + tcgetattr (no struct literal with platform-specific fields).
// An RAII guard restores the terminal on every exit path. A temporary SIGINT
// handler restores the terminal before the process is terminated by Ctrl-C.

use std::sync::atomic::{AtomicI32, Ordering};

/// File descriptor saved for the SIGINT handler. -1 = handler not installed.
static SIGINT_FD: AtomicI32 = AtomicI32::new(-1);

/// Saved termios bytes for the SIGINT handler.
struct TermCell(std::cell::UnsafeCell<std::mem::MaybeUninit<libc::termios>>);
// SAFETY: written by the main thread before the handler is installed (Release
// store on SIGINT_FD), read only by the handler (Acquire load). No concurrent
// writes after initialization.
unsafe impl Sync for TermCell {}
static SIGINT_SAVED: TermCell =
    TermCell(std::cell::UnsafeCell::new(std::mem::MaybeUninit::uninit()));

/// SIGINT handler: restore terminal and re-raise with the default handler.
extern "C" fn on_sigint(_: libc::c_int) {
    let fd = SIGINT_FD.load(Ordering::Acquire);
    if fd >= 0 {
        // SAFETY: initialized before SIGINT_FD was set to a non-negative value.
        unsafe {
            let saved = (*SIGINT_SAVED.0.get()).assume_init_ref();
            libc::tcsetattr(fd, libc::TCSANOW, saved);
        }
    }
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::raise(libc::SIGINT);
    }
}

/// RAII guard: restores termios and the previous SIGINT handler on drop.
struct NoEchoGuard {
    fd: i32,
    saved: libc::termios,
    old_sigint: libc::sighandler_t,
}

impl Drop for NoEchoGuard {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
            libc::signal(libc::SIGINT, self.old_sigint);
        }
        SIGINT_FD.store(-1, Ordering::Release);
    }
}

/// Read a line from stdin with echo disabled using libc termios.
///
/// Aborts the process (via `eprintln!` + `process::exit`) if echo cannot be
/// disabled — we never fall back to echoing a secret token.
fn read_token_no_echo() -> String {
    use std::mem::MaybeUninit;
    use std::os::unix::io::AsRawFd;

    let stdin_fd = io::stdin().as_raw_fd();

    // Fetch the current termios using MaybeUninit — avoids naming any
    // platform-specific struct fields (c_line exists on Linux but not macOS).
    let mut saved = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr writes into the pointer on success.
    if unsafe { libc::tcgetattr(stdin_fd, saved.as_mut_ptr()) } != 0 {
        eprintln!("\nclaudio vocdoni login: cannot query terminal state; aborting");
        std::process::exit(1);
    }
    // SAFETY: tcgetattr succeeded, so saved is fully initialized.
    let saved = unsafe { saved.assume_init() };

    // Store for the signal handler before installing it.
    unsafe {
        (*SIGINT_SAVED.0.get()).write(saved);
    }
    SIGINT_FD.store(stdin_fd, Ordering::Release);

    // Install temporary SIGINT handler.
    // SAFETY: on_sigint is a valid extern "C" fn.
    let old_sigint =
        unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t) };

    // Disable echo (keep ICANON so read_line still works line-by-line).
    let mut raw = saved;
    raw.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
    if unsafe { libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw) } != 0 {
        // Restore signal handler — the RAII guard hasn't been created yet.
        unsafe {
            libc::signal(libc::SIGINT, old_sigint);
        }
        SIGINT_FD.store(-1, Ordering::Release);
        eprintln!("\nclaudio vocdoni login: cannot disable echo; aborting");
        std::process::exit(1);
    }

    // RAII guard: restores termios and signal handler on every exit path
    // (normal return, panic, early return, etc.).
    let _guard = NoEchoGuard {
        fd: stdin_fd,
        saved,
        old_sigint,
    };

    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    eprintln!(); // newline after the hidden input

    line.trim().to_owned()
}

// ── status ────────────────────────────────────────────────────────────────────

fn cmd_status() -> ExitCode {
    let sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio vocdoni: could not read config.toml: {e}");
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
        println!("Run `claudio vocdoni login` or set CLAUDIO_PROXY_URL.");
        return ExitCode::SUCCESS;
    }

    println!("Proxy profiles:");
    // Show env profile first, then saved ones.
    if let Some((n, p)) = &env_profile {
        let marker = if default_name.as_deref() == Some(n) {
            " (default, env)"
        } else {
            " (env)"
        };
        println!("  {n}{marker}  {}  {}", p.url, p.masked_token());
        print_plugins(n);
    }
    for (n, p) in &sec.profiles {
        let marker = if default_name.as_deref() == Some(n.as_str()) {
            " (default)"
        } else {
            ""
        };
        let email = p.email.as_deref().map(printable).unwrap_or_default();
        let line = format!("  {n}{marker}  {}  {}  {email}", p.url, p.masked_token());
        println!("{}", line.trim_end());
        print_plugins(n);
    }

    // Live stats for the default profile.
    let active_profile = env_profile
        .as_ref()
        .map(|(_, p)| p)
        .or_else(|| sec.default.as_ref().and_then(|n| sec.profiles.get(n)));

    if let Some(p) = active_profile {
        println!();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(print_live_status(p));
    }

    ExitCode::SUCCESS
}

/// Profile `name`'s cached plugins (the admitted ones), if any.
fn print_plugins(name: &str) {
    let Some(list) = plugins::load_cache(name) else {
        return;
    };
    let names: Vec<String> = list
        .admitted()
        .iter()
        .map(|p| printable(&format!("{} {}", p.name, p.version)))
        .collect();
    if !names.is_empty() {
        println!("    plugins: {}", names.join(", "));
    }
}

async fn print_live_status(p: &Profile) {
    // Stats.
    match api::fetch_stats(&p.url, &p.token, "24h").await {
        Ok(s) => {
            let total_tok = s.totals.input_tokens + s.totals.output_tokens;
            println!(
                "Stats ({}): {} req  {}  {} tok total",
                s.period,
                s.totals.requests,
                s.user_name,
                fmt_tokens(total_tok),
            );
            if !s.by_model.is_empty() {
                println!("  By model:");
                for m in &s.by_model {
                    println!(
                        "    {:50}  {} req  {} out tok",
                        m.model,
                        m.requests,
                        fmt_tokens(m.output_tokens)
                    );
                }
            }
            if let Some(lim) = &s.limit {
                println!(
                    "  Limit: {:.0}% of {} out-tok/{}s",
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

// ── logout ────────────────────────────────────────────────────────────────────

fn cmd_logout(args: &[String]) -> ExitCode {
    let sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio vocdoni: could not read config.toml: {e}");
            return ExitCode::FAILURE;
        }
    };

    let name = match args.first() {
        Some(n) => n.clone(),
        None => match &sec.default {
            Some(d) => d.clone(),
            None => {
                eprintln!("claudio vocdoni logout: no profile name given and no default set");
                return ExitCode::FAILURE;
            }
        },
    };

    if !sec.profiles.contains_key(&name) {
        eprintln!("claudio vocdoni logout: profile '{name}' not found");
        return ExitCode::FAILURE;
    }
    // Its plugins' secrets go with it.
    if let Err(e) = plugins::remove_cache(&name) {
        eprintln!("claudio vocdoni logout: could not update plugins.json: {e}");
        return ExitCode::FAILURE;
    }

    let saved = profile::update(|sec| {
        sec.profiles.remove(&name);
        // If we removed the default, clear it (or pick another one).
        if sec.default.as_deref() == Some(&name) {
            sec.default = sec.profiles.keys().next().cloned();
        }
    });
    if let Err(e) = saved {
        eprintln!("claudio vocdoni logout: could not save config.toml: {e}");
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
            eprintln!("claudio vocdoni use: missing NAME or 'none'");
            return ExitCode::FAILURE;
        }
    };

    let sec = match profile::load() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("claudio vocdoni: could not read config.toml: {e}");
            return ExitCode::FAILURE;
        }
    };

    if name != "none" && !sec.profiles.contains_key(&name) {
        eprintln!("claudio vocdoni use: profile '{name}' not found");
        return ExitCode::FAILURE;
    }
    let default = (name != "none").then(|| name.clone());
    if let Err(e) = profile::update(|sec| sec.default = default) {
        eprintln!("claudio vocdoni use: could not save config.toml: {e}");
        return ExitCode::FAILURE;
    }
    if name == "none" {
        println!("Default proxy cleared (sessions start without a proxy).");
    } else {
        println!("Default proxy set to '{name}'.");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn proxy_alias_runs_the_same_subcommands() {
        for sub in ["login", "status", "logout", "use"] {
            assert!(subcommand(sub).is_some(), "{sub}");
        }
        assert!(subcommand("nope").is_none());
        for word in WORDS {
            assert_eq!(dispatch(&words(&[word, "--help"])), Some(ExitCode::SUCCESS));
            assert_eq!(dispatch(&words(&[word, "help"])), Some(ExitCode::SUCCESS));
            assert_eq!(dispatch(&words(&[word, "nope"])), Some(ExitCode::FAILURE));
            // A bad `login` flag fails before anything is read or asked.
            let bad = words(&[word, "login", "--nope"]);
            assert_eq!(dispatch(&bad), Some(ExitCode::FAILURE));
        }
        assert_eq!(dispatch(&words(&["proxies"])), None);
        assert_eq!(dispatch(&[]), None);
        assert!(HELP.contains("claudio vocdoni login") && HELP.contains("claudio proxy"));
        assert!(APPENDIX.starts_with('\n') && APPENDIX.contains("claudio proxy"));
    }

    #[test]
    fn login_takes_the_url_the_default_profile_or_the_vocdoni_proxy() {
        assert_eq!(
            parse_login(&words(&["https://p.example.net", "--name", "w", "--token"])),
            Ok(LoginArgs {
                name: Some("w".into()),
                url: Some("https://p.example.net".into()),
                unverified: false,
                paste: true,
            })
        );
        assert!(parse_login(&words(&["--what"])).is_err());
        let mut sec = ProxySection::default();
        let (url, name) = login_target(None, &sec).unwrap();
        assert_eq!((url.as_str(), name), (super::super::DEFAULT_URL, None));
        sec.profiles.insert(
            "self".into(),
            Profile {
                url: "https://self.example.org".into(),
                token: "t".into(),
                first_party: false,
                email: None,
            },
        );
        sec.default = Some("self".into());
        let (url, name) = login_target(None, &sec).unwrap();
        assert_eq!(
            (url.as_str(), name.as_deref()),
            ("https://self.example.org", Some("self"))
        );
        let given = login_target(Some("https://other.example.org".into()), &sec).unwrap();
        assert_eq!(given, ("https://other.example.org".to_owned(), None));
        assert!(login_target(Some("tok@p.example.org".into()), &sec).is_err());
    }

    #[test]
    fn relogin_keeps_first_party_mode() {
        let mut sec = profile::ProxySection::default();
        sec.profiles.insert(
            "work".into(),
            Profile {
                url: "https://old.example.net".into(),
                token: "old".into(),
                first_party: true,
                email: None,
            },
        );
        store_login(
            &mut sec,
            "work",
            "https://new.example.net".into(),
            "new".into(),
            Some("pau@example.org".into()),
        );
        let p = &sec.profiles["work"];
        assert_eq!(p.url, "https://new.example.net");
        assert_eq!(p.token, "new");
        assert!(p.first_party);
        assert_eq!(p.email.as_deref(), Some("pau@example.org"));
    }

    #[test]
    fn new_profile_is_gateway() {
        let mut sec = profile::ProxySection::default();
        store_login(
            &mut sec,
            "work",
            "https://a.example.net".into(),
            "t".into(),
            None,
        );
        assert!(!sec.profiles["work"].first_party);
        assert_eq!(sec.profiles["work"].email, None);
    }

    #[test]
    fn proxy_text_is_printed_without_control_characters() {
        assert_eq!(
            printable("https://p.example/\u{1b}]52;c;x\u{7}login\n"),
            "https://p.example/]52;c;xlogin"
        );
    }
}
