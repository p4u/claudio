//! Diagnostic / test-harness CLI subcommands — only compiled with `--features diag`.
//!
//! These are invoked with a `__` prefix so they are clearly internal and not
//! part of the user-facing interface. They exist primarily for the integration
//! tests in `tests/remote.rs` and for manual debugging.
//!
//! Kept behind the `diag` feature so release builds don't ship these
//! subcommands, which install binaries and spawn real sessions on remote
//! hosts.

use std::process::ExitCode;
use std::time::Duration;

/// Build a single-threaded Tokio runtime (factored out to avoid the same
/// boilerplate in every command).
fn make_runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
    tokio::runtime::Runtime::new().map_err(|e| {
        eprintln!("failed to create tokio runtime: {e}");
        ExitCode::FAILURE
    })
}

/// `claudio __bootstrap HOST`
///
/// Runs `ensure_remote(host)` and prints the result in a human-readable form.
/// Exits 0 on success, 1 on error.
pub fn bootstrap_cmd(host: &str) -> ExitCode {
    let rt = match make_runtime() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let host = host.to_owned();
    rt.block_on(async move {
        match super::bootstrap::ensure_remote(&host).await {
            Ok(info) => {
                if info.was_current {
                    println!(
                        "was_current=true probe={}@{}/{} (up-to-date)",
                        info.probe.version, info.probe.os, info.probe.arch
                    );
                } else {
                    println!(
                        "was_current=false probe={}@{}/{} (uploaded)",
                        info.probe.version, info.probe.os, info.probe.arch
                    );
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bootstrap failed: {e}");
                ExitCode::FAILURE
            }
        }
    })
}

/// `claudio __connect-check HOST`
///
/// Connects via SSH (which performs the handshake and reads the Welcome), then
/// prints the Welcome fields as JSON to stdout. Exits 0 on success, 1 on error.
pub fn connect_check_cmd(host: &str) -> ExitCode {
    let rt = match make_runtime() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let host = host.to_owned();
    rt.block_on(async move {
        let client = match crate::client::connect_ssh(&host).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("connect_ssh({host}) failed: {e}");
                return ExitCode::FAILURE;
            }
        };

        // Send a Ping-style request to cleanly exercise the channel (and give
        // the bridge's stderr, which may report a deferred update, time to
        // arrive).
        let _ = client.request(crate::proto::Msg::Ping).await;
        // The handshake already consumed the Welcome message.
        let w = client.welcome();
        let output = serde_json::json!({
            "host": w.host.hostname,
            "home": w.host.home,
            "os": w.host.os,
            "arch": w.host.arch,
            "claude_ok": w.host.claude.is_some(),
            "claudio_version": w.claudio_version,
            "proto": w.proto,
            "build": w.build,
            "daemon_outdated": client.daemon_outdated(),
        });
        println!("{}", serde_json::to_string(&output).unwrap_or_default());
        ExitCode::SUCCESS
    })
}

/// `claudio __remote-session HOST CWD`
///
/// Spawns a session in `CWD` on `HOST`, attaches, waits for a terminal data
/// frame (the session output), kills the session, and exits. Prints a summary
/// to stdout.
pub fn remote_session_cmd(host: &str, cwd: &str) -> ExitCode {
    let rt = match make_runtime() {
        Ok(r) => r,
        Err(code) => return code,
    };
    let host = host.to_owned();
    let cwd = cwd.to_owned();
    rt.block_on(async move {
        use crate::client::Incoming;
        use crate::proto::{Msg, SpawnSpec};
        use uuid::Uuid;

        let client = match crate::client::connect_ssh(&host).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("connect_ssh({host}) failed: {e}");
                return ExitCode::FAILURE;
            }
        };

        let session_id = Uuid::new_v4();
        let spec = SpawnSpec {
            id: session_id,
            cwd: cwd.clone(),
            name: None,
            rows: 24,
            cols: 80,
            ..Default::default()
        };

        // Spawn the session.
        let spawn_reply = match client.request(Msg::Spawn(spec)).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Spawn request failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        match &spawn_reply {
            Msg::Spawned { pid, .. } => println!("spawned: session={session_id} pid={pid:?}"),
            other => {
                eprintln!("unexpected Spawn reply: {other:?}");
                return ExitCode::FAILURE;
            }
        }

        // Attach to get terminal data.
        match client
            .request(Msg::Attach {
                id: session_id,
                rows: 24,
                cols: 80,
            })
            .await
        {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Attach request failed: {e}");
                let _ = client.request(Msg::Kill { id: session_id }).await;
                return ExitCode::FAILURE;
            }
        }

        // Read the incoming channel for terminal data (the "snapshot").
        let mut rx = match client.take_incoming() {
            Some(r) => r,
            None => {
                eprintln!("incoming channel already taken");
                let _ = client.request(Msg::Kill { id: session_id }).await;
                return ExitCode::FAILURE;
            }
        };

        let got_data = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match rx.recv().await {
                    Some(Incoming::Data { id, .. }) if id == session_id => return true,
                    Some(Incoming::Attached { id, .. }) if id == session_id => {
                        // Attached confirmation; data follows.
                    }
                    Some(Incoming::Disconnected) | None => return false,
                    Some(_) => continue,
                }
            }
        })
        .await
        .unwrap_or(false);

        if got_data {
            println!("snapshot received (ok)");
        } else {
            eprintln!("timed out or error waiting for terminal data");
            let _ = client.request(Msg::Kill { id: session_id }).await;
            return ExitCode::FAILURE;
        }

        // Clean up.
        let _ = client.request(Msg::Kill { id: session_id }).await;
        ExitCode::SUCCESS
    })
}

/// The Vocdoni addon's `apply` op for `__remote-manifest`, as plain JSON so
/// this diag-only path imports no addon (transitional; P3.17 moves it).
fn apply_request(name: &str, sha256: &str, secret: &str) -> crate::proto::Msg {
    let body = serde_json::json!({"name": name, "url": "https://127.0.0.1:9/never.toml",
        "sha256": sha256, "secrets": [secret], "disabled": []});
    crate::proto::Msg::AddonOp {
        addon: "vocdoni".to_owned(),
        op: "apply".to_owned(),
        body,
    }
}

/// `claudio __remote-manifest HOST CWD NAME SHA256 SECRET`
///
/// Asks `HOST`'s daemon to apply plugin `NAME`'s manifest with hash
/// `SHA256` (from a URL nothing listens on, so only a pre-seeded cache
/// answers), with `SECRET` the name of the one secret it is given; prints
/// the `vocdoni/apply` reply (an `Applied`) as JSON, then spawns claude in `CWD` with what it
/// applied (its flags, its env, the secret set to `t9-secret`, its
/// instructions), prints `pid=<pid>`, keeps the session 5 s (for the caller
/// to look at its argv) and kills it.
pub fn remote_manifest_cmd(
    host: &str,
    cwd: &str,
    name: &str,
    sha256: &str,
    secret: &str,
) -> ExitCode {
    use crate::proto::{Msg, SessionExtensions, SpawnSpec};
    let rt = match make_runtime() {
        Ok(r) => r,
        Err(code) => return code,
    };
    rt.block_on(async move {
        // As the TUI does: the branch binary on the host, its daemon current.
        if let Err(e) = super::bootstrap::ensure_remote(host).await {
            eprintln!("ensure_remote({host}) failed: {e}");
            return ExitCode::FAILURE;
        }
        let client = match crate::client::connect_ssh(host).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("connect_ssh({host}) failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        let request = apply_request(name, sha256, secret);
        let applied = match client.request(request).await {
            Ok(Msg::AddonReply { body }) => body,
            Ok(other) => {
                eprintln!("unexpected apply reply: {other:?}");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("apply request failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!("{applied}");
        let text = |key: &str| applied[key].as_str().map(str::to_owned);
        let mut args = Vec::new();
        if let Some(agents) = text("agents") {
            args.extend(["--agents".to_owned(), agents]);
        }
        if let Some(mcp) = text("mcp_config") {
            args.extend(["--mcp-config".to_owned(), mcp]);
        }
        let mut env: Vec<(String, String)> =
            serde_json::from_value(applied["env"].clone()).unwrap_or_default();
        env.push((secret.to_owned(), "t9-secret".to_owned()));
        let id = uuid::Uuid::new_v4();
        let spec = SpawnSpec {
            id,
            cwd: cwd.to_owned(),
            args,
            env,
            rows: 24,
            cols: 80,
            ephemeral: true,
            extensions: Some(SessionExtensions {
                instructions: text("instructions").unwrap_or_default(),
                ..Default::default()
            }),
            ..Default::default()
        };
        match client.request(Msg::Spawn(spec)).await {
            Ok(Msg::Spawned { pid, .. }) => {
                println!("pid={}", pid.unwrap_or(0));
            }
            other => {
                eprintln!("Spawn failed: {other:?}");
                return ExitCode::FAILURE;
            }
        }
        use std::io::Write;
        let _ = std::io::stdout().flush();
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = client.request(Msg::Kill { id }).await;
        ExitCode::SUCCESS
    })
}

/// `claudio __ssh-hosts` — print all known SSH host aliases, one per line.
pub fn ssh_hosts_cmd() -> ExitCode {
    let hosts = super::hosts::candidates();
    for h in &hosts {
        println!("{h}");
    }
    ExitCode::SUCCESS
}
