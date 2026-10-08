//! Diagnostic / test-harness CLI subcommands.
//!
//! These are invoked with a `__` prefix so they are clearly internal and not
//! part of the user-facing interface. They exist primarily for the integration
//! tests in `tests/remote.rs` and for manual debugging.

use std::process::ExitCode;
use std::time::Duration;

/// `claudio __bootstrap HOST`
///
/// Runs `ensure_remote(host)` and prints the result in a human-readable form.
/// Exits 0 on success, 1 on error.
pub fn bootstrap_cmd(host: &str) -> ExitCode {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to create tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
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
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to create tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
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
        });
        println!("{}", serde_json::to_string(&output).unwrap_or_default());
        // Send a Ping-style request to cleanly exercise the channel.
        let _ = client.request(crate::proto::Msg::Ping).await;
        ExitCode::SUCCESS
    })
}

/// `claudio __remote-session HOST CWD`
///
/// Spawns a session in `CWD` on `HOST`, attaches, waits for a terminal data
/// frame (the session output), kills the session, and exits. Prints a summary
/// to stdout.
pub fn remote_session_cmd(host: &str, cwd: &str) -> ExitCode {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to create tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
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
            args: vec![],
            env: vec![],
            rows: 24,
            cols: 80,
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
            .request(Msg::Attach { id: session_id, rows: 24, cols: 80 })
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
