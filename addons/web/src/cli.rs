//! `claudio web` without `--foreground` (docs/web.md, "The web service"):
//! the settings go to `[web]` in config.toml, the local daemon serves them
//! (`apply`), and this command prints what a foreground start prints, then
//! exits. Also `claudio web stop` and `status`, and the service restart
//! `--set-password` and `--print-url --rotate-token` ask for. Whether the
//! daemon has the service is decided on `Welcome.features` (`addon:web`)
//! only, never on an error's text. With `--app` the start opens a window
//! (`app_window.rs`) and every failure also reaches the desktop.

use std::future::Future;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use claudio_core::client;
use claudio_core::paths;
use claudio_core::proto::Msg;
use serde_json::{json, Value};

use crate::service::{self, Reported, Unit};
use crate::{app_window, auth, parse_args, password, remote, start_url, ticket, tls, WebArgs};

/// `claudio web` against a daemon without `addon:web`.
const NO_SERVICE: &str = "the daemon has no web service (older, or built without the web \
addon): claudio daemon restart when idle, or claudio web --foreground";

/// What the local daemon said to a web op.
enum Answer {
    /// No daemon listens (only when not asked to start one).
    NotRunning,
    /// Its `Welcome.features` lack `addon:web`: no op was sent.
    NoService,
    Replied(Reported),
}

/// `op` to the local daemon's web addon. `start`: start the daemon (or
/// replace an outdated idle one) first; only `claudio web` does.
async fn ask(op: &str, body: Value, start: bool) -> Result<Answer, String> {
    if start {
        client::ensure_local_daemon()
            .await
            .map_err(|e| format!("cannot start the daemon: {e}"))?;
    }
    let daemon = match client::connect(&paths::daemon_socket()).await {
        Ok(daemon) => daemon,
        Err(_) if !start => return Ok(Answer::NotRunning),
        Err(e) => return Err(format!("cannot reach the daemon: {e}")),
    };
    let feature = claudio_core::addon::feature("web");
    if !daemon.welcome().features.contains(&feature) {
        return Ok(Answer::NoService);
    }
    let msg = Msg::AddonOp {
        addon: "web".into(),
        op: op.into(),
        body,
    };
    match daemon.request(msg).await {
        Ok(Msg::AddonReply { body }) => Ok(Answer::Replied(service::read_reply(&body))),
        Ok(_) => Err("the daemon answered something else".to_owned()),
        Err(e) => Err(format!("the daemon refused the web op: {e}")),
    }
}

/// `f` on a runtime of its own (the CLI has none).
fn block_on<T>(f: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?
        .block_on(f)
}

/// The `[web]` table of the config file at `path` (empty when there is
/// none). Blocking I/O.
fn stored(path: &Path) -> Result<toml::Table, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let table: toml::Table = text
        .parse()
        .map_err(|_| format!("{} is not valid TOML", path.display()))?;
    match table.get("web") {
        None => Ok(toml::Table::new()),
        Some(toml::Value::Table(web)) => Ok(web.clone()),
        Some(_) => Err("[web] is not a table".to_owned()),
    }
}

/// Write `[web] enabled`, and `unit` when given (replacing the stored one),
/// through `config_edit` (minimal, atomic, serialised).
fn save(path: &Path, enabled: bool, unit: Option<&Unit>) -> Result<(), String> {
    use toml::Value as V;
    let edited = claudio_core::app::config_edit::edit_section(path, "web", |t| {
        t.insert("enabled".into(), V::Boolean(enabled));
        let Some(u) = unit else {
            return;
        };
        let (set, unset) = match u.remote {
            true => ("remote", "listen"),
            false => ("listen", "remote"),
        };
        t.insert(set.into(), V::String(u.addr.to_string()));
        t.remove(unset);
        let hosts: Vec<V> = u.allow_hosts.iter().cloned().map(V::String).collect();
        match hosts.is_empty() {
            true => t.remove("allow_hosts"),
            false => t.insert("allow_hosts".into(), V::Array(hosts)),
        };
    });
    edited
        .map(|_| ())
        .map_err(|e| format!("cannot save [web] in {}: {e}", path.display()))
}

/// `<runtime>/web.log`: the service's stderr, kept by the daemon.
fn log_path() -> std::path::PathBuf {
    paths::runtime_dir().join("web.log")
}

/// `claudio web [flags]`: check, save `[web]`, have the daemon serve it,
/// print what a foreground start prints.
pub(crate) fn start(args: WebArgs) -> ExitCode {
    let app = args.app.is_some();
    let config = paths::config_file();
    let run = match stored(&config) {
        Err(e) => return failed(app, 1, &e),
        Ok(web) => service::merge(&web, &args).and_then(|unit| service::run_args(&unit, &args)),
    };
    let run = match run {
        Ok(run) => run,
        Err(e) => return failed(app, 2, &e),
    };
    // The token first: the service only ever loads it.
    let token = match auth::load_or_create_token(&auth::token_path(), run.rotate_token) {
        Ok(token) => token,
        Err(e) => return failed(app, 1, &format!("cannot read or write the token file: {e}")),
    };
    if run.print_url {
        println!("{}", start_url(&run, &token));
        if run.rotate_token {
            restart_if_enabled("a new token");
        }
        return ExitCode::SUCCESS;
    }
    // The password and the certificate are checked (or made) here, so
    // their errors show in this terminal and nothing is saved.
    if let Some(addr) = run.remote {
        let password = password::read(&password::path());
        if let Err(e) = remote::prepare(&run, addr, &token, &password) {
            return fail_after_token(&run, &e);
        }
    }
    if let Err(e) = save(&config, true, Some(&Unit::of(&run))) {
        return fail_after_token(&run, &e);
    }
    let force = run.rotate_token || run.rotate_cert;
    match block_on(ask("apply", json!({ "force": force }), true)) {
        Ok(Answer::Replied(Reported::Running { argv, since, .. })) => {
            match current(argv, since, &run) {
                Ok(argv) => announce(&argv, &token, &run),
                Err(code) => code,
            }
        }
        Ok(Answer::Replied(Reported::Failed { tail, .. })) => failed(
            app,
            1,
            &format!(
                "the web service did not start: {tail}\nsee {}; it is tried again (claudio web status)",
                log_path().display()
            ),
        ),
        Ok(Answer::Replied(other)) => failed(app, 1, &format!("the web service is {}", what(&other))),
        Ok(Answer::NoService | Answer::NotRunning) => failed(app, 1, NO_SERVICE),
        Err(e) => failed(app, 1, &e),
    }
}

/// The argv of the service this start's browser reaches. One that started
/// before this binary's file changed runs older code (a busy daemon kept
/// it), whose page ignores `#k=`: when this start opens a browser on a
/// ticket, it is restarted (`apply { force: true }`; the daemon starts the
/// binary at `Config::claudio`, a path, so the one on disk, this one).
fn current(argv: Vec<String>, since: u64, run: &WebArgs) -> Result<Vec<String>, ExitCode> {
    if !older_than_me(since) || !app_window::opens(run) {
        return Ok(argv);
    }
    match block_on(ask("apply", json!({ "force": true }), false)) {
        Ok(Answer::Replied(Reported::Running { argv, .. })) => Ok(argv),
        _ => {
            let old = argv.get(2..).and_then(|a| parse_args(a).ok());
            if let Some(base) = old.as_ref().and_then(ticket::base_url) {
                println!("claudio web: {base}");
            }
            let why = "the web service is older than this claudio: run `claudio daemon restart`";
            Err(failed(run.app.is_some(), 1, why))
        }
    }
}

/// Whether a service that started at `since` (Unix seconds) is older than
/// this binary's file.
fn older_than_me(since: u64) -> bool {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|mtime| since < mtime.as_secs())
}

fn fail(why: &str) -> ExitCode {
    failed(false, 1, why)
}

/// `why` on stderr as its line (`app_window::line`), with `--app` (`app`)
/// also where a desktop user sees it (`app_window::tell_desktop`); then
/// exit `code`.
fn failed(app: bool, code: u8, why: &str) -> ExitCode {
    let line = app_window::line(why);
    eprintln!("{line}");
    if app {
        app_window::tell_desktop(&line);
    }
    ExitCode::from(code)
}

/// [`fail`] once the token is loaded. `--rotate-token` has then already
/// written a new one, so the service that runs (the stored `[web]`) is
/// restarted first: the old token stops working at once, and the token
/// file is again the one the service serves with.
fn fail_after_token(run: &WebArgs, why: &str) -> ExitCode {
    if run.rotate_token {
        restart_if_enabled("a new token");
    }
    failed(run.app.is_some(), 1, why)
}

/// What a foreground start prints, for what runs (the reply's argv: two
/// `claudio web` at once each print the one that won), then the browser
/// tab or (`--app`) the window, signed in with a ticket.
fn announce(argv: &[String], token: &str, asked: &WebArgs) -> ExitCode {
    let app = asked.app.is_some();
    let Some(Ok(running)) = argv.get(2..).map(parse_args) else {
        return failed(
            app,
            1,
            "the daemon runs a web service this claudio cannot read",
        );
    };
    let url = start_url(&running, token);
    let remote = match running.remote {
        None => None,
        Some(addr) => {
            let password = password::read(&password::path());
            match remote::prepare(&running, addr, token, &password) {
                Ok(r) => Some(r),
                Err(e) => return failed(app, 1, &e),
            }
        }
    };
    app_window::announce(&running, asked, &url, remote.as_ref());
    match app_window::open(&running, asked) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => failed(app, 1, &why),
    }
}

/// A state in words.
fn what(state: &Reported) -> String {
    match state {
        Reported::Off => "off".to_owned(),
        Reported::Starting => "starting".to_owned(),
        Reported::Running { pid, since, .. } => {
            let up = paths::unix_now().saturating_sub(*since);
            format!("running (pid {pid}, for {up} s)")
        }
        Reported::Failed {
            exit,
            signal,
            retry_in,
            ..
        } => {
            let how = match (exit, signal) {
                (Some(code), _) => format!("exit {code}"),
                (None, Some(sig)) => format!("signal {sig}"),
                (None, None) => "not started".to_owned(),
            };
            let next = match retry_in {
                Some(s) => format!(", next try in {s} s"),
                None => ", not tried again until claudio web or a daemon start".to_owned(),
            };
            format!("failed ({how}{next})")
        }
        Reported::Newer => "unknown to this claudio (the daemon is newer)".to_owned(),
    }
}

/// `claudio web stop`: `[web] enabled = false`, then the daemon stops it.
pub(crate) fn stop() -> ExitCode {
    if let Err(e) = save(&paths::config_file(), false, None) {
        return fail(&e);
    }
    match block_on(ask("apply", json!({ "force": false }), false)) {
        Ok(Answer::NotRunning | Answer::NoService) => ExitCode::SUCCESS,
        Ok(Answer::Replied(Reported::Off)) => {
            eprintln!("claudio web: stopped");
            ExitCode::SUCCESS
        }
        Ok(Answer::Replied(other)) => fail(&format!("the web service is {}", what(&other))),
        Err(e) => fail(&e),
    }
}

/// `claudio web status`: enabled or not, the daemon's state, the URL
/// (without the token) and, with `--remote`, the certificate's
/// fingerprint. Exit 0 while it runs, 3 otherwise.
pub(crate) fn status() -> ExitCode {
    let web = stored(&paths::config_file());
    match web
        .as_ref()
        .map_err(Clone::clone)
        .and_then(service::enabled)
    {
        Ok(true) => println!("web: enabled"),
        Ok(false) => println!("web: disabled"),
        Err(e) => println!("web: {e}"),
    }
    // The stored unit; an invalid one is said, and no URL is made up for it.
    let kept = match web.map(|w| service::unit(&w)) {
        Ok(Ok(unit)) => Some(unit.unwrap_or_default()),
        Ok(Err(e)) => {
            println!("web: {e}");
            None
        }
        Err(_) => None,
    };
    let (state, code) = match block_on(ask("status", json!({}), false)) {
        Ok(Answer::NotRunning) => {
            println!("daemon: not running");
            (None, 3)
        }
        Ok(Answer::NoService) => {
            println!("daemon: no web service (older, or built without the web addon)");
            (None, 3)
        }
        Ok(Answer::Replied(state)) => {
            println!("service: {}", what(&state));
            let code = if matches!(state, Reported::Running { .. }) {
                0
            } else {
                3
            };
            (Some(state), code)
        }
        Err(e) => return fail(&e),
    };
    if let Some(Reported::Failed { tail, .. }) = &state {
        tail.lines().for_each(|l| println!("  {l}"));
        println!("log: {}", log_path().display());
    }
    // What runs (the reply's argv), else what is stored.
    let running = match &state {
        Some(Reported::Running { argv, .. }) => argv.get(2..).and_then(|a| parse_args(a).ok()),
        _ => None,
    };
    let shown = running.or_else(|| kept.and_then(|unit| parse_args(&unit.flags()).ok()));
    if let Some(shown) = shown {
        listener(&shown);
    }
    ExitCode::from(code)
}

/// `args`' URL without the token and, with `--remote`, the kept
/// certificate's fingerprint.
fn listener(args: &WebArgs) {
    let Some(addr) = args.remote else {
        println!("url: http://{}/", args.listen);
        return;
    };
    let url = remote::public_url(addr, &claudio_core::sys::hostname());
    println!("url: {url}");
    match tls::cert::kept_fingerprint(&tls::cert::dir()) {
        Ok(fp) => println!("certificate SHA-256 {fp}"),
        Err(e) => println!("certificate: {e}"),
    }
}

/// After `--set-password` or `--print-url --rotate-token`: restart the
/// service (`apply { force: true }`) so `what` takes effect now, when
/// `[web]` is enabled and the daemon has the service; else nothing.
pub(crate) fn restart_if_enabled(what_changed: &str) {
    let enabled = stored(&paths::config_file())
        .ok()
        .and_then(|web| service::enabled(&web).ok());
    if enabled != Some(true) {
        return;
    }
    match block_on(ask("apply", json!({ "force": true }), false)) {
        Ok(Answer::Replied(Reported::Running { .. })) => eprintln!(
            "claudio web: the web service restarted with {what_changed}; browsers sign in again"
        ),
        Ok(Answer::Replied(other)) => {
            eprintln!("claudio web: the web service is {}", what(&other))
        }
        Ok(Answer::NotRunning | Answer::NoService) => {}
        Err(e) => eprintln!("claudio web: {e}"),
    }
}
