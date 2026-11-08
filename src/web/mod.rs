//! `claudio web`: the session manager served to a browser. One more client
//! of the claudio daemons (local socket, remote ssh bridge), like the TUI.
//! See docs/specs/2026-11-08-web-ui-design.md and docs/web-protocol.md.

pub(crate) mod assets;
pub(crate) mod auth;
mod ws;

use std::io::{self, IsTerminal, Write};
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;

use axum::extract::FromRef;
use axum::routing::{get, post};
use axum::{middleware, Router};
use tracing_subscriber::EnvFilter;

pub(crate) const DEFAULT_PORT: u16 = 7337;

const USAGE: &str = "usage: claudio web [--listen 127.0.0.1:7337] [--allow-host NAME]... \
[--no-open] [--print-url] [--rotate-token]";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WebArgs {
    pub listen: SocketAddr,
    /// Lowercase host names a reverse proxy on this host may use.
    pub allow_hosts: Vec<String>,
    pub no_open: bool,
    pub print_url: bool,
    pub rotate_token: bool,
}

/// Shared by every handler.
#[derive(Clone)]
pub(crate) struct AppState {
    pub auth: Arc<auth::Auth>,
    /// Read by the asset route in debug disk mode only.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub assets: Arc<assets::Assets>,
    // Task 4 adds `hub: hub::HubHandle`; Task 5 adds `ctx: Arc<spawn::SpawnCtx>`.
}

impl FromRef<AppState> for Arc<auth::Auth> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.auth)
    }
}

pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
    let mut out = WebArgs {
        listen: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
        allow_hosts: Vec::new(),
        no_open: false,
        print_url: false,
        rotate_token: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--listen" => out.listen = listen_addr(&value(inline, &mut it, flag)?)?,
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
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    Ok(out)
}

fn value(
    inline: Option<String>,
    it: &mut std::slice::Iter<'_, String>,
    flag: &str,
) -> Result<String, String> {
    inline
        .or_else(|| it.next().cloned())
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// `[A-Za-z0-9.-]{1,253}`, not starting with `-`.
fn host_name_ok(name: &str) -> bool {
    (1..=253).contains(&name.len())
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
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
             Behind a reverse proxy on this host (e.g. tailscale serve), add --allow-host <its name>."
        )),
        None => Err(format!(
            "--listen {addr:?}: not an address (example: 127.0.0.1:7337)"
        )),
    }
}

/// `claudio web …` (`args` are the words after `web`).
pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("claudio web: {e}");
            return ExitCode::from(2);
        }
    };
    // Blocking I/O, before the runtime exists.
    let token = match auth::load_or_create_token(&auth::token_path(), args.rotate_token) {
        Ok(token) => token,
        Err(e) => {
            eprintln!("claudio web: cannot read or write the token file: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `SocketAddr`'s `Display` brackets an IPv6 address.
    let url = format!("http://{}/#t={token}", args.listen);
    if args.print_url {
        println!("{url}");
        return ExitCode::SUCCESS;
    }

    // A fixed filter, not RUST_LOG: a dependency's trace output could carry
    // request headers (the cookie).
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
    match runtime.block_on(serve(args, token, url)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("claudio web: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(args: WebArgs, token: String, url: String) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", args.listen))?;
    let assets = Arc::new(assets::Assets::load());
    let auth = auth::Auth::new(
        token,
        auth::Allowed::new(args.listen.port(), &args.allow_hosts),
        auth::csp(assets.importmap_sha256()),
    )
    .map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
    let state = AppState {
        auth: Arc::new(auth),
        assets,
    };

    // The one line on stdout (the tests read it).
    let mut out = io::stdout();
    let _ = writeln!(out, "claudio web: {url}");
    let _ = out.flush();
    let wanted = crate::proxy::login::browser_wanted(
        std::env::consts::OS,
        |k| std::env::var(k).ok(),
        io::stdout().is_terminal(),
    );
    if !args.no_open && wanted {
        crate::proxy::login::open_browser(&url);
    }
    tracing::info!(listen = %args.listen, "web: serving");

    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|e| format!("server error: {e}"))
}

/// Ctrl+C or SIGTERM.
async fn shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let term = async {
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term => {}
    }
    tracing::info!("web: shutting down");
}

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/login", post(auth::login))
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
        assert!(parse_args(&args(&["--allow-host"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
    }
}
