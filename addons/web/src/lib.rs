//! `claudio web`: the session manager served to a browser. One more client
//! of the claudio daemons (local socket, remote ssh bridge), like the TUI.
//! See docs/specs/2026-11-08-web-ui-design.md and docs/web-protocol.md.

mod addon;
mod addon_feed;
mod addons;
pub(crate) mod assets;
mod atomic;
pub(crate) mod auth;
mod cli;
mod dto;
mod events;
mod ext;
mod git_ops;
mod hub;
mod hub_io;
mod input;
mod lenses;
mod limit;
mod login;
mod password;
mod peer;
mod remote;
mod service;
mod session_events;
mod sessions;
mod spawn;
mod state;
mod tls;
mod ws;

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use axum::extract::FromRef;
use axum::routing::{get, post};
use axum::{middleware, Router};
use tracing_subscriber::EnvFilter;

pub use addon::WebAddon;

pub(crate) const DEFAULT_PORT: u16 = 7337;

/// This build's version as the page shows it: the release version, plus the
/// label of a local build (`make dev`: `0.8.0+dev.20f0b49`).
pub(crate) fn version() -> String {
    format!(
        "{}{}",
        env!("CARGO_PKG_VERSION"),
        env!("CLAUDIO_BUILD_LABEL")
    )
}

const USAGE: &str =
    "usage: claudio web [--foreground] [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
[--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
       claudio web stop | status
       claudio web --set-password
       claudio web --print-cert";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WebArgs {
    pub listen: SocketAddr,
    /// `--listen` was given (it does not go with `--remote`).
    pub listen_given: bool,
    /// `--remote [ADDR][:PORT]`: TLS on that address, password logins.
    pub remote: Option<SocketAddr>,
    /// `--rotate-cert`: a new `--remote` certificate.
    pub rotate_cert: bool,
    /// `--print-cert`: the kept certificate on stdout, then exit.
    pub print_cert: bool,
    /// Lowercase host names a reverse proxy on this host may use.
    pub allow_hosts: Vec<String>,
    pub no_open: bool,
    pub print_url: bool,
    pub rotate_token: bool,
    /// `--set-password`: store the `--remote` password, then exit.
    pub set_password: bool,
    /// `--foreground`: serve from this terminal, as `claudio web` always did.
    pub foreground: bool,
    /// `--service`: the child the daemon runs (docs/daemon-protocol.md,
    /// "Addon services"): `ready` on stdout, never the token, stops at stdin EOF.
    pub service: bool,
}

/// Shared by every handler.
#[derive(Clone)]
pub(crate) struct AppState {
    pub auth: Arc<auth::Auth>,
    /// Read by the asset route in debug disk mode only.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub assets: Arc<assets::Assets>,
    pub hub: hub::HubHandle,
    /// One search at a time per host (docs/web-protocol.md §5).
    pub search_slots: Arc<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>>,
    /// Open tabs, counted at the upgrade (at most `hub::MAX_TABS`).
    pub tabs: Arc<AtomicUsize>,
    /// Spawns, proxy, plugins and extensions.
    pub ctx: Arc<spawn::SpawnCtx>,
    /// The addon runtime, fed by the hub (`addon_feed`).
    pub addons: claudio_core::app::addons::AddonRuntime,
}

impl FromRef<AppState> for Arc<auth::Auth> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.auth)
    }
}

pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
    let mut out = WebArgs {
        listen: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
        listen_given: false,
        remote: None,
        rotate_cert: false,
        print_cert: false,
        allow_hosts: Vec::new(),
        no_open: false,
        print_url: false,
        rotate_token: false,
        set_password: false,
        foreground: false,
        service: false,
    };
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--listen" => {
                out.listen = listen_addr(&value(inline, &mut it, flag)?)?;
                out.listen_given = true;
            }
            "--remote" => {
                // A value only as `--remote=V` or a next word not starting with `-`.
                let v = inline.or_else(|| it.next_if(|next| !next.starts_with('-')).cloned());
                out.remote = Some(match v {
                    Some(v) => remote::remote_addr(&v)?,
                    None => SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
                });
            }
            "--rotate-cert" => out.rotate_cert = true,
            "--print-cert" => out.print_cert = true,
            "--allow-host" => {
                let name = value(inline, &mut it, flag)?;
                if !host_name_ok(&name) {
                    return Err(format!("--allow-host {name:?}: not a host name"));
                }
                out.allow_hosts.push(name.to_ascii_lowercase());
            }
            "--no-open" => out.no_open = true,
            "--print-url" => out.print_url = true,
            "--rotate-token" => out.rotate_token = true,
            "--set-password" => out.set_password = true,
            "--foreground" => out.foreground = true,
            "--service" => out.service = true,
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    combination_ok(&out, args.len())?;
    Ok(out)
}

/// The flags that do not go together.
fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
    let one_shot = a.no_open || a.print_url || a.rotate_token || a.rotate_cert;
    if a.service && (one_shot || a.set_password || a.print_cert || a.foreground) {
        return Err("--service takes only --listen=, --remote= and --allow-host=".to_owned());
    }
    if (a.set_password || a.print_cert) && n_args != 1 {
        return Err("--set-password and --print-cert take no other argument".to_owned());
    }
    if a.remote.is_some() && a.listen_given {
        return Err("--remote is the one listener: drop --listen".to_owned());
    }
    // Without --foreground the stored [web] may be a --remote one: `cli`
    // checks the settings it merges.
    if a.rotate_cert && a.remote.is_none() && a.foreground {
        return Err("--rotate-cert needs --remote".to_owned());
    }
    if let Some(remote) = a.remote {
        let port = remote.port().to_string();
        let foreign = a
            .allow_hosts
            .iter()
            .find(|h| h.split_once(':').is_some_and(|(_, p)| p != port));
        if let Some(h) = foreign {
            return Err(format!(
                "--allow-host {h}: with --remote the one port is {port}"
            ));
        }
    }
    Ok(())
}

fn value(
    inline: Option<String>,
    it: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
    flag: &str,
) -> Result<String, String> {
    inline
        .or_else(|| it.next().cloned())
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// `[A-Za-z0-9.-]{1,253}`, not starting with `-`, optionally `:<port>`
/// (1..=65535).
pub(crate) fn host_name_ok(name: &str) -> bool {
    let (host, port) = match name.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (name, None),
    };
    (1..=253).contains(&host.len())
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && port.is_none_or(|p| {
            p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n > 0)
        })
}

/// `addr` as a socket address, if it is a loopback one: `127.0.0.0/8`,
/// `::1`, or `localhost` (127.0.0.1). Without a port: 7337.
pub(crate) fn listen_addr(addr: &str) -> Result<SocketAddr, String> {
    let localhost = |port: u16| SocketAddr::from(([127, 0, 0, 1], port));
    let parsed = if addr == "localhost" {
        Some(localhost(DEFAULT_PORT))
    } else if let Some(port) = addr.strip_prefix("localhost:") {
        port.parse().ok().map(localhost)
    } else {
        addr.parse::<SocketAddr>().ok().or_else(|| {
            addr.parse::<IpAddr>()
                .ok()
                .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
        })
    };
    match parsed {
        Some(sock) if sock.ip().is_loopback() => Ok(sock),
        Some(_) => Err(format!(
            "refusing to listen on {addr}: claudio web listens on loopback only \
             (127.0.0.1, ::1, localhost).\n\
             From another machine, tunnel it: ssh -L 7337:127.0.0.1:7337 <this host>\n\
             Behind a reverse proxy on this host (e.g. tailscale serve), add --allow-host <its name>.\n\
             Or serve it over TLS with a password: claudio web --remote (docs/web.md)."
        )),
        None => Err(format!(
            "--listen {addr:?}: not an address (example: 127.0.0.1:7337)"
        )),
    }
}

/// `claudio web …` (`args` are the words after `web`).
pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match args {
        [word] if word == "stop" => return cli::stop(),
        [word] if word == "status" => return cli::status(),
        _ => {}
    }
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("claudio web: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(code) = one_shot(&args) {
        return code;
    }
    match args.foreground || args.service {
        true => serve_here(args, addons),
        // The daemon serves it (docs/web.md, "The web service").
        false => cli::start(args),
    }
}

/// Serve from this process: `claudio web --foreground`, or `--service`, the
/// daemon's child.
fn serve_here(args: WebArgs, addons: Arc<claudio_core::addon::Addons>) -> ExitCode {
    // First thing: a daemon that stops us (or dies) during the start is obeyed.
    let eof = args.service.then(watch_stdin);
    // Blocking I/O, before the runtime exists.
    let token = match token_for(&args) {
        Ok(token) => token,
        Err(e) => {
            eprintln!("claudio web: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `--service` never prints the token, so it never makes this URL.
    let url = (!args.service).then(|| start_url(&args, &token));
    if args.print_url {
        println!("{}", url.unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    let password = password::read(&password::path());
    let remote = match args
        .remote
        .map(|addr| remote::prepare(&args, addr, &token, &password))
    {
        None => None,
        Some(Ok(r)) => Some(r),
        Some(Err(e)) => {
            eprintln!("claudio web: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sessions = load_sessions(&args, &token, &password);
    init_tracing();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio web: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    if args.service {
        hub_io::connect_local_plainly();
    }
    match runtime.block_on(serve(args, token, url, remote, sessions, eof, addons)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("claudio web: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The token this server serves with. `--service` only loads it (`claudio
/// web` made it before it had the service started); a foreground start
/// makes one when there is none, or a new one with `--rotate-token`.
fn token_for(args: &WebArgs) -> Result<String, String> {
    let path = auth::token_path();
    if !args.service {
        return auth::load_or_create_token(&path, args.rotate_token)
            .map_err(|e| format!("cannot read or write the token file: {e}"));
    }
    match auth::load_token(&path) {
        Ok(Some(token)) => Ok(token),
        Ok(None) => Err(format!("no token in {}: run claudio web", path.display())),
        Err(e) => Err(format!("cannot read the token file: {e}")),
    }
}

/// The URL a start prints and opens: the token link, or with `--remote`
/// the one for this machine (else the public one).
pub(crate) fn start_url(args: &WebArgs, token: &str) -> String {
    match args.remote {
        // `SocketAddr`'s `Display` brackets an IPv6 address.
        None => format!("http://{}/#t={token}", args.listen),
        Some(addr) => remote::token_url(addr, token)
            .unwrap_or_else(|| remote::public_url(addr, &claudio_core::sys::hostname())),
    }
}

/// Logs on stderr. A fixed filter, not RUST_LOG: a dependency's trace
/// output could carry request headers (the cookie).
fn init_tracing() {
    let debug = std::env::var("CLAUDIO_DEBUG").is_ok_and(|v| v == "1");
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal())
        .with_env_filter(EnvFilter::new(if debug {
            "claudio=debug"
        } else {
            "claudio=info"
        }))
        .init();
}

/// How long `--service` may take to stop after stdin's EOF before it exits.
const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

/// `--service`: a thread (not a task: a blocked runtime must not stop it)
/// reads stdin, a pipe the daemon holds and never writes. EOF means the
/// daemon is gone or asks us to stop: the graceful shutdown starts, and 3 s
/// later the process exits if it still runs, so nothing stays on the port.
fn watch_stdin() -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        if test_hang() {
            return;
        }
        let _ = tx.send(());
        std::thread::sleep(STOP_DEADLINE);
        std::process::exit(0);
    });
    rx
}

/// `CLAUDIO_TEST_SERVICE_HANG=1`, debug builds only: a `--service` that
/// ignores its stop (stdin's EOF and SIGTERM), so the suites can check the
/// daemon's SIGKILL. A release build has no such body: the variable is not
/// read, and `make static`'s final check greps for its name.
#[cfg(debug_assertions)]
fn test_hang() -> bool {
    std::env::var("CLAUDIO_TEST_SERVICE_HANG").is_ok_and(|v| v == "1")
}

#[cfg(not(debug_assertions))]
fn test_hang() -> bool {
    false
}

/// The sign-ins of earlier runs, bound to this start's mode, token and
/// password bytes (a file of another epoch: none, silently). Blocking I/O,
/// before the runtime exists and before `serve` binds.
fn load_sessions(
    args: &WebArgs,
    token: &str,
    password: &Result<Vec<u8>, String>,
) -> sessions::Sessions {
    let mode = match args.remote {
        Some(_) => sessions::Mode::Remote,
        None => sessions::Mode::Local,
    };
    let epoch = sessions::epoch(mode, token, password.as_deref().unwrap_or_default());
    let path = sessions::path();
    let (store, why) = sessions::load(&path, &epoch, claudio_core::paths::unix_now());
    if let Some(why) = why {
        // The tracing subscriber is not installed yet.
        eprintln!(
            "claudio web: ignoring {}: {why}; browsers sign in again",
            path.display()
        );
    }
    sessions::Sessions::new(store, epoch, path)
}

/// `--set-password` or `--print-cert`: one thing, then exit.
fn one_shot(args: &WebArgs) -> Option<ExitCode> {
    if args.set_password {
        return Some(match password::set_password() {
            Ok(done) => {
                eprintln!("claudio web: {done}");
                cli::restart_if_enabled("the new password");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("claudio web: {e}");
                ExitCode::from(2)
            }
        });
    }
    args.print_cert.then(print_cert)
}

/// `--print-cert`: the kept certificate (never the key) on stdout.
fn print_cert() -> ExitCode {
    match tls::cert::read_cert_pem(&tls::cert::dir()) {
        Ok(pem) => {
            print!("{pem}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("claudio web: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Serve until Ctrl+C, SIGTERM or (`--service`) stdin's EOF. `url`: what a
/// foreground start prints and opens; `None` for `--service`.
async fn serve(
    args: WebArgs,
    token: String,
    url: Option<String>,
    remote: Option<remote::Remote>,
    sessions: sessions::Sessions,
    eof: Option<tokio::sync::oneshot::Receiver<()>>,
    addons: Arc<claudio_core::addon::Addons>,
) -> Result<(), String> {
    use tokio::signal::unix::{signal, SignalKind};
    // Before `ready`: from then on a SIGTERM is the graceful stop, never
    // the default action (axum polls `stop` only later).
    let term = signal(SignalKind::terminate()).ok();
    let listen = remote.as_ref().map_or(args.listen, |r| r.addr);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    let assets = Arc::new(assets::Assets::load());
    let allowed = match &remote {
        Some(r) => r.allowed.clone(),
        None => auth::Allowed::new(args.listen, &args.allow_hosts),
    };
    let csp = auth::csp(assets.importmap_sha256());
    let mut auth = auth::Auth::new(token, allowed, csp, sessions)
        .map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
    if let Some(r) = &remote {
        auth = auth.with_password(r.password.clone());
    }
    let state = build_state(auth, assets, addons).await?;
    let (hub, ext, addon_rt, sessions) = (
        state.hub.clone(),
        Arc::clone(&state.ctx.ext),
        state.addons.clone(),
        Arc::clone(&state.auth.sessions),
    );
    let writer = sessions.start_writer();
    started(&args, url.as_deref(), remote.as_ref());
    tracing::info!(listen = %listen, tls = remote.is_some(), "web: serving");

    // Tabs are closed (1001), web-state.json and the sessions written
    // before the server stops waiting for its connections.
    let stop = async move {
        shutdown(eof, term).await;
        hub.shutdown().await;
        sessions.flush().await;
        writer.inspect(|w| w.abort());
        ext.shutdown().await;
        addon_rt.shutdown();
    };
    let app = router(state).into_make_service_with_connect_info::<peer::Peer>();
    let served = match remote {
        None => {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop)
                .await
        }
        Some(r) => {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::clone(&r.cert.config));
            let limits = tls::listener::Limits::DEFAULT;
            let tls = tls::listener::TlsListener::new(listener, acceptor, limits)
                .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
            axum::serve(tls, app).with_graceful_shutdown(stop).await
        }
    };
    served.map_err(|e| format!("server error: {e}"))
}

/// Once the listener is bound and the hub runs: `--service` (no `url`)
/// writes `ready` on stdout, the line the daemon waits for, and nothing
/// else (never the token); a foreground start prints its URL (and banner)
/// and opens a browser.
fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
    if !env!("CLAUDIO_BUILD_LABEL").is_empty() {
        eprintln!("claudio web: local build {}", version());
    }
    let Some(url) = url else {
        let mut out = io::stdout();
        let _ = writeln!(out, "ready");
        let _ = out.flush();
        return;
    };
    announce(url, remote);
    let wanted = claudio_core::app::browser::browser_wanted(
        std::env::consts::OS,
        |k| std::env::var(k).ok(),
        io::stdout().is_terminal(),
    );
    if !args.no_open && wanted {
        claudio_core::app::browser::open_browser(url);
    }
}

/// The start lines: with `--remote` the banner's stderr lines first, then
/// the one stdout line the tests read.
pub(crate) fn announce(url: &str, remote: Option<&remote::Remote>) {
    let line = match remote {
        Some(r) => {
            for l in &r.banner.stderr {
                eprintln!("{l}");
            }
            r.banner.stdout.clone()
        }
        None => format!("claudio web: {url}"),
    };
    let mut out = io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The server's state: the hub (from config.toml and both state files,
/// read off the executor), the extensions and the spawn
/// context, and the addon runtime they feed. The runtime watches
/// config.toml (D15) and says when it re-read it.
async fn build_state(
    auth: auth::Auth,
    assets: Arc<assets::Assets>,
    addons: Arc<claudio_core::addon::Addons>,
) -> Result<AppState, String> {
    let opts = claudio_core::app::addons::RuntimeOptions {
        config_path: claudio_core::paths::config_file(),
        watch_config: true,
    };
    let (runtime, rt_rx) = claudio_core::app::addons::AddonRuntime::start(addons, opts);
    // Files, before the hub starts: off the executor.
    let state_path = state::path();
    let hub_cfg = {
        let (state_path, runtime) = (state_path.clone(), runtime.clone());
        tokio::task::spawn_blocking(move || hub::HubConfig {
            config: claudio_core::config::load(),
            tui_state: claudio_core::app::state::ClientState::load(
                &claudio_core::paths::client_state(),
            ),
            web: state::load(&state_path),
            state_path,
            runtime,
        })
        .await
        .map_err(|e| format!("cannot read the configuration: {e}"))?
    };
    let config = hub_cfg.config.clone();
    let hub = hub::start(hub_cfg);
    let ext = ext::Extensions::load(&config, &hub).await;
    runtime.set_core_extensions(ext.own_names());
    let feed = addon_feed::Feed::new(hub.clone(), Arc::clone(&ext), runtime.clone());
    addon_feed::follow(hub.clone(), runtime.clone(), rt_rx, feed);
    let ctx = Arc::new(spawn::SpawnCtx {
        hub: hub.clone(),
        ext: Arc::clone(&ext),
        runtime: runtime.clone(),
    });
    Ok(AppState {
        auth: Arc::new(auth),
        assets,
        hub: hub.clone(),
        search_slots: Arc::default(),
        tabs: Arc::default(),
        ctx,
        addons: runtime,
    })
}

/// Ctrl+C, SIGTERM (`term`, registered before the start's output), or
/// `eof` (`--service`: stdin closed).
async fn shutdown(
    eof: Option<tokio::sync::oneshot::Receiver<()>>,
    term: Option<tokio::signal::unix::Signal>,
) {
    if eof.is_some() && test_hang() {
        // SIGTERM is caught (`term` stays registered) and never acted on;
        // only SIGKILL ends us.
        std::future::pending::<()>().await;
    }
    let eof = async {
        match eof {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    let term = async {
        match term {
            Some(mut s) => {
                s.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term => {}
        _ = eof => {}
    }
    tracing::info!("web: shutting down");
}

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/login", post(login::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/ws", get(ws::upgrade))
        .fallback(assets::serve)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state.auth),
            auth::guard,
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn listen_must_be_loopback() {
        assert_eq!(
            listen_addr("127.0.0.1:7337").unwrap().to_string(),
            "127.0.0.1:7337"
        );
        assert_eq!(
            listen_addr("127.1.2.3:80").unwrap().to_string(),
            "127.1.2.3:80"
        );
        assert_eq!(listen_addr("[::1]:9000").unwrap().to_string(), "[::1]:9000");
        assert_eq!(
            listen_addr("localhost:8080").unwrap().to_string(),
            "127.0.0.1:8080"
        );
        assert_eq!(
            listen_addr("localhost").unwrap().to_string(),
            "127.0.0.1:7337"
        );
        assert_eq!(
            listen_addr("127.0.0.1").unwrap().to_string(),
            "127.0.0.1:7337"
        );
        for bad in ["0.0.0.0:7337", "192.168.1.2:7337", "[::]:7337", "10.0.0.1"] {
            let err = listen_addr(bad).unwrap_err();
            assert!(
                err.contains("ssh -L 7337:127.0.0.1:7337") && err.contains("--allow-host"),
                "{err}"
            );
        }
        assert!(listen_addr("example.org:7337").is_err());
    }

    #[test]
    fn args_parse_flags_and_defaults() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.listen.to_string(), "127.0.0.1:7337");
        assert!(a.allow_hosts.is_empty() && !a.no_open && !a.print_url && !a.rotate_token);
        let a = parse_args(&args(&[
            "--allow-host",
            "Claudio.Example",
            "--no-open",
            "--print-url",
            "--rotate-token",
            "--listen=127.0.0.1:9000",
            "--allow-host=b.example",
        ]))
        .unwrap();
        assert_eq!(a.listen.to_string(), "127.0.0.1:9000");
        assert_eq!(a.allow_hosts, vec!["claudio.example", "b.example"]);
        assert!(a.no_open && a.print_url && a.rotate_token);
        assert!(parse_args(&args(&["--allow-host", "bad/host"])).is_err());
        let a = parse_args(&args(&["--allow-host", "Proxy.Example:8443"])).unwrap();
        assert_eq!(a.allow_hosts, vec!["proxy.example:8443"]);
        for bad in [
            "proxy.example:",
            "proxy.example:0",
            "proxy.example:65536",
            "proxy.example:x",
            "proxy.example:+8443",
            ":8443",
            "a:b:8443",
            "-x:8443",
        ] {
            assert!(parse_args(&args(&["--allow-host", bad])).is_err(), "{bad}");
        }
        assert!(parse_args(&args(&["--allow-host"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
        assert!(!a.set_password);
        assert!(parse_args(&args(&["--set-password"])).unwrap().set_password);
        let err = parse_args(&args(&["--set-password", "--no-open"])).unwrap_err();
        assert!(err.contains("no other argument"), "{err}");
        assert!(a.remote.is_none() && !a.listen_given && !a.rotate_cert && !a.print_cert);
        let remote =
            |list: &[&str]| parse_args(&args(list)).map(|a| a.remote.map(|r| r.to_string()));
        assert_eq!(remote(&["--remote"]), Ok(Some("0.0.0.0:7337".into())));
        let a = parse_args(&args(&["--remote", "--no-open"])).unwrap();
        assert_eq!(a.remote.unwrap().to_string(), "0.0.0.0:7337");
        assert!(a.no_open);
        assert_eq!(remote(&["--remote=:8443"]), Ok(Some("0.0.0.0:8443".into())));
        assert_eq!(
            remote(&["--remote", "[::]:9000"]),
            Ok(Some("[::]:9000".into()))
        );
        let a = parse_args(&args(&["--remote", ":7999", "--rotate-cert"])).unwrap();
        assert!(a.rotate_cert);
        // Without --foreground, --rotate-cert is checked on the merged
        // settings (`service::run_args`), which may be a stored --remote.
        assert!(parse_args(&args(&["--rotate-cert"])).is_ok());
        for bad in [
            &["--remote", "--listen", "127.0.0.1:1"][..],
            &["--rotate-cert", "--foreground"][..],
            &["--remote", ":7999", "--allow-host", "a.example:8443"][..],
            &["--print-cert", "--no-open"][..],
            &["--remote", "fe80::1"][..],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
        assert!(parse_args(&args(&[
            "--remote",
            ":7999",
            "--allow-host",
            "a.example:7999"
        ]))
        .is_ok());
        assert!(parse_args(&args(&["--print-cert"])).unwrap().print_cert);
        assert!(!a.foreground);
        let a = parse_args(&args(&["--foreground", "--listen", "127.0.0.1:9000"])).unwrap();
        assert!(a.foreground && a.listen_given);
        let err = parse_args(&args(&["--set-password", "--foreground"])).unwrap_err();
        assert!(err.contains("no other argument"), "{err}");
        let a = parse_args(&args(&[
            "--service",
            "--remote=0.0.0.0:7999",
            "--allow-host=a.example",
        ]))
        .unwrap();
        assert!(a.service && a.remote.is_some() && a.allow_hosts == ["a.example"]);
        for one_shot in [
            "--no-open",
            "--print-url",
            "--rotate-token",
            "--rotate-cert",
            "--set-password",
            "--print-cert",
            "--foreground",
        ] {
            let err = parse_args(&args(&["--service", "--remote", one_shot])).unwrap_err();
            assert!(err.contains("--service takes only"), "{one_shot}: {err}");
        }
    }
}
