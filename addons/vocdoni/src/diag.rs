//! `claudio __remote-manifest`: the diag-only (`--features diag`) word the
//! remote tests (t9) use to drive `vocdoni/apply` on an ssh host.

use std::process::ExitCode;
use std::time::Duration;

/// `claudio __remote-manifest HOST CWD NAME SHA256 SECRET`; `None` for any
/// other word.
pub fn dispatch(argv: &[String]) -> Option<ExitCode> {
    if argv.get(1).map(String::as_str) != Some("__remote-manifest") {
        return None;
    }
    if let [_, _, host, cwd, name, sha256, secret] = argv {
        return Some(remote_manifest_cmd(host, cwd, name, sha256, secret));
    }
    eprintln!("usage: claudio __remote-manifest <host> <cwd> <name> <sha256> <secret>");
    Some(ExitCode::FAILURE)
}

fn make_runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
    tokio::runtime::Runtime::new().map_err(|e| {
        eprintln!("failed to create tokio runtime: {e}");
        ExitCode::FAILURE
    })
}

/// The `vocdoni/apply` op for `__remote-manifest`.
fn apply_request(name: &str, sha256: &str, secret: &str) -> claudio_core::proto::Msg {
    let body = serde_json::json!({"name": name, "url": "https://127.0.0.1:9/never.toml",
        "sha256": sha256, "secrets": [secret], "disabled": []});
    claudio_core::proto::Msg::AddonOp {
        addon: super::NAME.to_owned(),
        op: super::daemon::applied::OP_APPLY.to_owned(),
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
fn remote_manifest_cmd(host: &str, cwd: &str, name: &str, sha256: &str, secret: &str) -> ExitCode {
    use claudio_core::proto::{Msg, SessionExtensions, SpawnSpec};
    let rt = match make_runtime() {
        Ok(r) => r,
        Err(code) => return code,
    };
    rt.block_on(async move {
        // As the TUI does: the branch binary on the host, its daemon current.
        if let Err(e) = claudio_core::remote::bootstrap::ensure_remote(host).await {
            eprintln!("ensure_remote({host}) failed: {e}");
            return ExitCode::FAILURE;
        }
        let client = match claudio_core::client::connect_ssh(host).await {
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
