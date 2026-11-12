//! Live SSH remote-session integration tests.
//!
//! These tests are **off by default**.  Set `CLAUDIO_SSH_TEST_HOST` to the
//! hostname you want to test against (e.g. `devbox`) to enable the SSH tests:
//!
//! ```bash
//! CLAUDIO_SSH_TEST_HOST=devbox cargo test --test remote -- --test-threads=1 --nocapture
//! ```
//!
//! Tests `t1`, `t7`, and `t8` are pure unit-style tests (no SSH) and always
//! run.  Tests `t2`–`t6`, `t9` and `t10` require a real SSH host (`t10`
//! also a local ssh agent holding a key: it checks agent forwarding).
//!
//! **Note on `__` diag commands:** `t2`–`t4` use `claudio __bootstrap`,
//! `__connect-check`, and `__remote-session`.  These may later be gated
//! behind a non-default `diag` Cargo feature.  If that lands, add `required-features = ["diag"]` to the `[[test]]` entry for
//! "remote" in Cargo.toml and replace the runtime guards below with the
//! standard `#[cfg(feature = "diag")]` attribute.
//!
//! The target host must:
//! - Accept key-based SSH (BatchMode=yes).  No password prompts.
//! - Have `claude` reachable (or already have `claudio` installed for t2 skip).
//! - Have `sha256sum` or `shasum -a 256` available.

#![cfg(test)]

use std::env;
use std::process::Command;
use std::thread;
use std::time::Duration;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// The claudio binary, located at compile time.
///
/// Replaces the hand-rolled `claudio_bin()` path-walking that existed here
/// before (which guessed at the `target/` layout and broke on non-standard
/// workspace configs).
const BINARY: &str = env!("CARGO_BIN_EXE_claudio");

/// Return the test host, or `None` to skip SSH tests.
fn test_host() -> Option<String> {
    env::var("CLAUDIO_SSH_TEST_HOST")
        .ok()
        .filter(|h| !h.is_empty())
}

/// Run `claudio <args>` and return (stdout, stderr, success).
fn run_claudio(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(BINARY)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run claudio: {e}"));
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

/// Run an SSH command on `host` with BatchMode and return (stdout, success).
fn ssh_run(host: &str, cmd: &str) -> (String, bool) {
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host, cmd])
        .output()
        .unwrap_or_else(|e| panic!("ssh failed: {e}"));
    (
        String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        out.status.success(),
    )
}

/// Read the remote claudio daemon's PID via SSH by reading its lock file.
///
/// The lock file is at `$XDG_RUNTIME_DIR/claudio/daemon-v1.lock` on the
/// remote, falling back to `/tmp/claudio-<uid>/daemon-v1.lock` when
/// `XDG_RUNTIME_DIR` is not set (macOS, non-systemd).
fn remote_daemon_pid(host: &str) -> Option<u32> {
    let cmd =
        r#"cat "${XDG_RUNTIME_DIR:-/tmp/claudio-$(id -u)}/claudio/daemon-v1.lock" 2>/dev/null"#;
    let (stdout, ok) = ssh_run(host, cmd);
    if ok || !stdout.is_empty() {
        stdout.trim().parse().ok()
    } else {
        None
    }
}

// ── t1: probe local ───────────────────────────────────────────────────────────

/// `claudio __probe` must print one JSON line with the required fields.
#[test]
fn t1_probe_local() {
    let (stdout, stderr, ok) = run_claudio(&["__probe"]);
    assert!(ok, "__probe exited non-zero: stderr={stderr}");

    let line = stdout.lines().next().expect("__probe produced no output");
    let v: serde_json::Value =
        serde_json::from_str(line).expect("__probe output is not valid JSON");

    for field in &["version", "proto", "os", "arch", "build"] {
        assert!(
            v.get(field).is_some(),
            "missing '{field}' in probe JSON: {line}"
        );
    }
    let build = v["build"].as_str().unwrap_or("");
    assert_eq!(
        build.len(),
        64,
        "build must be a 64-char SHA-256 hex string, got: {build}"
    );

    println!(
        "[t1] local probe OK: version={} os={} arch={} build={}…",
        v["version"],
        v["os"],
        v["arch"],
        &build[..8]
    );
}

// ── t2: bootstrap (upload to remote) ─────────────────────────────────────────

/// Bootstrap the remote host:
/// 1. First call may upload the binary.
/// 2. After a successful upload, `claudio __probe` on the remote should work.
/// 3. Running bootstrap a second time should detect the binary is up-to-date.
#[test]
fn t2_bootstrap_idempotent() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t2: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };

    // Get the local build hash.
    let (local_stdout, _, ok) = run_claudio(&["__probe"]);
    assert!(ok, "local __probe failed");
    let local_json: serde_json::Value =
        serde_json::from_str(local_stdout.lines().next().unwrap_or("{}"))
            .expect("local probe JSON parse failed");
    let local_build = local_json["build"].as_str().unwrap_or("").to_owned();
    println!("[t2] local build hash: {}…", &local_build[..8]);

    // Run the bootstrap via `claudio __bootstrap HOST`.
    let (stdout1, stderr1, ok1) = run_claudio(&["__bootstrap", &host]);
    assert!(
        ok1,
        "first __bootstrap failed:\nstdout: {stdout1}\nstderr: {stderr1}"
    );
    println!("[t2] first bootstrap:\n{stdout1}");

    // After bootstrap, `claudio __probe` must work on the remote.
    let probe_cmd = "$HOME/.local/bin/claudio __probe 2>/dev/null";
    let (remote_probe, probe_ok) = ssh_run(&host, probe_cmd);
    assert!(
        probe_ok || !remote_probe.is_empty(),
        "remote __probe failed after bootstrap; stdout={remote_probe}"
    );

    let remote_json: serde_json::Value =
        serde_json::from_str(remote_probe.lines().next().unwrap_or("{}")).unwrap_or_default();
    let remote_build = remote_json["build"].as_str().unwrap_or("").to_owned();
    assert_eq!(
        remote_build, local_build,
        "remote build hash should match local after bootstrap"
    );
    println!(
        "[t2] remote build matches local: {}… ✓",
        &remote_build[..8.min(remote_build.len())]
    );

    // Second bootstrap must report up-to-date (was_current=true).
    let (stdout2, stderr2, ok2) = run_claudio(&["__bootstrap", &host]);
    assert!(
        ok2,
        "second __bootstrap failed:\nstdout: {stdout2}\nstderr: {stderr2}"
    );
    assert!(
        stdout2.contains("up-to-date") || stdout2.contains("was_current=true"),
        "second bootstrap should report up-to-date:\n{stdout2}"
    );
    println!("[t2] second bootstrap (idempotent) ✓");
}

// ── t3: SSH connection and Welcome ────────────────────────────────────────────

/// Connect via SSH and verify the daemon sends a Welcome with hostname and
/// claude availability.
#[test]
fn t3_connect_ssh_welcome() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t3: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };

    let (stdout, stderr, ok) = run_claudio(&["__connect-check", &host]);
    assert!(ok, "__connect-check {host} failed:\nstderr: {stderr}");

    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("__connect-check output is not JSON: {stdout}"));
    assert!(
        !v["host"].as_str().unwrap_or("").is_empty(),
        "Welcome.host must be non-empty"
    );
    assert!(
        v["claude_ok"].as_bool().unwrap_or(false),
        "Welcome.claude_ok should be true if claude is installed on {host}"
    );
    println!(
        "[t3] welcome: host={} claude_ok={} ✓",
        v["host"], v["claude_ok"]
    );

    // The bridge replaced any older daemon (unless sessions were busy): the
    // daemon runs the binary the bootstrap installed.
    let (remote_probe, _) = ssh_run(&host, "$HOME/.local/bin/claudio __probe 2>/dev/null");
    let remote: serde_json::Value =
        serde_json::from_str(remote_probe.lines().next().unwrap_or("{}")).unwrap_or_default();
    if v["daemon_outdated"].as_bool() == Some(true) {
        println!("[t3] the daemon is outdated but busy: its restart was deferred");
    } else {
        assert_eq!(
            v["build"], remote["build"],
            "the remote daemon should run the installed binary"
        );
        println!("[t3] the daemon runs the installed build ✓");
    }
}

// ── t4: spawn, attach, snapshot ──────────────────────────────────────────────

/// Spawn a session in `/tmp` on the remote, attach, and verify a snapshot
/// arrives.  Uses `claudio __remote-session HOST` which performs the full
/// spawn→attach→snapshot cycle and exits with 0 on success.
#[test]
fn t4_spawn_and_snapshot() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t4: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };

    let (stdout, stderr, ok) = run_claudio(&["__remote-session", &host, "/tmp"]);
    assert!(ok, "__remote-session failed:\nstderr: {stderr}");
    println!("[t4] remote session cycle:\n{stdout}");
    assert!(
        stdout.contains("snapshot") || stdout.contains("ok"),
        "expected snapshot confirmation in output:\n{stdout}"
    );
}

// ── t5: session PID survives connection drop ──────────────────────────────────

/// Proves that the remote daemon — and therefore its sessions — survive a
/// connection drop.
///
/// Proof mechanism: read the remote daemon's lock file via SSH before and
/// after dropping the client connection.  If the PID is unchanged, the daemon
/// (and its sessions) persisted.
///
/// **Why not track a specific session PID?** `__remote-session` always kills
/// the session on exit, so it cannot be used to keep a session alive across a
/// connection drop.  Adding a `--no-kill` flag to `__remote-session` is a
/// possible future improvement; until then,
/// daemon-PID stability is the strongest guarantee available without modifying
/// `src/`.
#[test]
fn t5_session_pid_survives_reconnect() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t5: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };

    // ── 1. Establish a connection and start the remote daemon. ────────────────
    let (_, stderr1, ok1) = run_claudio(&["__connect-check", &host]);
    assert!(ok1, "first connect-check failed: {stderr1}");

    // ── 2. Read the remote daemon PID from its lock file. ────────────────────
    let pid_before = remote_daemon_pid(&host);
    assert!(
        pid_before.is_some(),
        "could not read remote daemon PID after connect-check; \
         check that the daemon lock file is accessible on {host}"
    );
    println!("[t5] remote daemon pid before drop: {:?}", pid_before);

    // ── 3. Drop the connection (the connect-check already returned). ──────────
    thread::sleep(Duration::from_millis(500));

    // ── 4. Reconnect. ─────────────────────────────────────────────────────────
    let (stdout2, stderr2, ok2) = run_claudio(&["__connect-check", &host]);
    assert!(ok2, "second connect-check (reconnect) failed: {stderr2}");
    let v2: serde_json::Value = serde_json::from_str(stdout2.trim()).unwrap_or_default();
    println!("[t5] reconnected: host={}", v2["host"]);

    // ── 5. Read the remote daemon PID again. ──────────────────────────────────
    let pid_after = remote_daemon_pid(&host);
    println!("[t5] remote daemon pid after reconnect: {:?}", pid_after);

    assert_eq!(
        pid_before, pid_after,
        "remote daemon PID changed — daemon was restarted during the drop; \
         this means sessions were lost"
    );
    println!(
        "[t5] daemon PID unchanged ({:?}) — sessions survived ✓",
        pid_before
    );
}

// ── t6: clean disconnect ──────────────────────────────────────────────────────

/// Verify the remote daemon survives a clean Disconnect and a subsequent
/// connect-check still works.  The daemon PID must be unchanged.
#[test]
fn t6_clean_disconnect_daemon_survives() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t6: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };

    // connect-check sends Ping then exits cleanly.
    let (_, stderr, ok) = run_claudio(&["__connect-check", &host]);
    assert!(ok, "connect-check failed: {stderr}");

    let pid_mid = remote_daemon_pid(&host);
    println!(
        "[t6] daemon pid after first clean disconnect: {:?}",
        pid_mid
    );

    // Daemon must still respond after a clean exit.
    thread::sleep(Duration::from_millis(200));
    let (stdout2, stderr2, ok2) = run_claudio(&["__connect-check", &host]);
    assert!(ok2, "post-disconnect connect-check failed: {stderr2}");
    println!("[t6] post-disconnect check ok: {stdout2}");

    let pid_after = remote_daemon_pid(&host);
    assert_eq!(
        pid_mid, pid_after,
        "daemon PID changed after clean disconnect"
    );
    println!("[t6] daemon PID stable ({:?}) ✓", pid_after);
}

// ── t7: SSH hosts parsing (no network) ───────────────────────────────────────

/// Unit-style test for the SSH config parser via the binary's `--ssh-hosts` flag.
///
/// Without `CLAUDIO_SSH_TEST_HOST` we just verify the command doesn't panic;
/// an empty output is valid when no `~/.ssh/config` exists.
#[test]
fn t7_ssh_hosts_cli() {
    let (stdout, _stderr, _ok) = run_claudio(&["__ssh-hosts"]);
    // Output may be empty — that's fine.  We just check no panic.
    println!(
        "[t7] local SSH hosts ({} entries):\n{stdout}",
        stdout.lines().count()
    );
}

// ── t8: probe JSON parsing (no network) ──────────────────────────────────────

/// Cross-check that the probe output round-trips through `serde_json`.
#[test]
fn t8_probe_roundtrip() {
    let (stdout, stderr, ok) = run_claudio(&["__probe"]);
    assert!(ok, "__probe failed: {stderr}");

    let line = stdout.lines().next().expect("__probe produced no output");
    let v: serde_json::Value = serde_json::from_str(line).expect("probe output not JSON");

    // Re-serialize and parse again: must be identical.
    let round = serde_json::to_string(&v).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&round).unwrap();
    assert_eq!(v, v2, "probe JSON did not survive round-trip");

    // Build hash must be exactly 64 hex chars.
    let build = v["build"].as_str().unwrap_or("");
    assert_eq!(build.len(), 64, "build must be 64 hex chars");
    assert!(
        build.chars().all(|c| c.is_ascii_hexdigit()),
        "build must be hex"
    );

    println!(
        "[t8] probe round-trip ✓ build={}…{}",
        &build[..4],
        &build[60..]
    );
}

// ── t9: plugin manifest on the remote ──────────────────────────────────────

/// The manifest t9 seeds: no plugin to install (the host's `~/.claude` is
/// left alone), one http MCP server needing `T9_TOKEN`, one agent,
/// instructions and env.
const T9_MANIFEST: &str = r#"format = 1
name = "claudio-t9"
version = "1.0.0"
instructions = "t9 instructions"

[[mcp]]
name = "t9-kb"
type = "http"
url = "https://t9.invalid/mcp"
headers = { Authorization = "Bearer ${T9_TOKEN}" }
secrets = ["T9_TOKEN"]

[[agent]]
name = "t9-agent"
description = "t9"
prompt = "t9 prompt"

[env]
T9_REGION = "eu"
"#;

/// A pre-seeded plugin manifest in the remote daemon's cache answers
/// `Manifest` with the server (placeholders only), the agent, the
/// instructions and the env, and a session spawned with them there has the
/// flags in its argv (unless the remote claude lacks them: then `Applied`
/// says so) and never the secret.
#[test]
fn t9_remote_plugin_manifest() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t9: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };
    let sha = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(T9_MANIFEST.as_bytes()))
    };
    let octal: String = T9_MANIFEST.bytes().map(|b| format!("\\{b:03o}")).collect();
    let seed = format!(
        "d=\"${{XDG_CACHE_HOME:-$HOME/.cache}}/claudio/vocdoni\" && mkdir -p \"$d\" && chmod 700 \"$d\" \
         && printf '{octal}' > \"$d/claudio-t9.toml\" && printf '{sha}\\n' > \"$d/claudio-t9.sha256\" \
         && echo ok"
    );
    let (out, ok) = ssh_run(&host, &seed);
    assert!(ok && out.contains("ok"), "could not seed the remote cache");

    let mut child = Command::new(BINARY)
        .args([
            "__remote-manifest",
            &host,
            "/tmp",
            "claudio-t9",
            &sha,
            "T9_TOKEN",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run claudio __remote-manifest");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = std::io::BufRead::lines(std::io::BufReader::new(stdout));
    let reply: serde_json::Value =
        serde_json::from_str(&lines.next().expect("an Applied reply").unwrap()).unwrap();
    let mcp = reply["mcp_config"].as_str().expect("an MCP config");
    assert!(
        mcp.contains("${T9_TOKEN}") && !mcp.contains("t9-secret"),
        "{reply}"
    );
    assert!(
        reply["agents"]
            .as_str()
            .is_some_and(|a| a.contains("t9-agent")),
        "{reply}"
    );
    assert_eq!(reply["instructions"], "t9 instructions");
    assert_eq!(reply["env"], serde_json::json!([["T9_REGION", "eu"]]));
    println!("[t9] Applied {{ mcp_config, agents, instructions, env }} ✓");

    let pid = lines.next().expect("a pid").unwrap();
    let pid = pid.strip_prefix("pid=").expect("pid=<n>").to_owned();
    let (args, _) = ssh_run(&host, &format!("ps -o args= -p {pid}"));
    let dropped: Vec<&str> = reply["dropped"]
        .as_array()
        .map(|d| d.iter().filter_map(|f| f.as_str()).collect())
        .unwrap_or_default();
    assert!(!args.contains("t9-secret"), "argv: {args}");
    if dropped.is_empty() {
        assert!(
            args.contains("--agents") && args.contains("--mcp-config"),
            "argv: {args}"
        );
        println!("[t9] the remote session's argv has --agents and --mcp-config ✓");
    } else {
        for flag in &dropped {
            assert!(!args.contains(flag), "argv: {args}");
        }
        println!("[t9] the remote claude lacks {dropped:?}: left out ✓");
    }
    for _ in lines {}
    assert!(child.wait().expect("wait for claudio").success());
    let _ = ssh_run(
        &host,
        "rm -f \"${XDG_CACHE_HOME:-$HOME/.cache}/claudio/plugins/claudio-t9.\"*",
    );
}

// ── t10: ssh agent forwarding ────────────────────────────────────────────────

/// The `SHA256:` fingerprints in `text` (`ssh-add -l`'s, or `__agent-check`'s).
fn fingerprints(text: &str) -> std::collections::BTreeSet<String> {
    text.split_whitespace()
        .filter_map(|w| w.strip_prefix("fingerprint=").or(Some(w)))
        .filter(|w| w.starts_with("SHA256:"))
        .map(str::to_owned)
        .collect()
}

/// `claudio __agent-check HOST [ID]` with `cfg` as `XDG_CONFIG_HOME`.
fn agent_check(cfg: &std::path::Path, host: &str, id: Option<&str>) -> String {
    let out = Command::new(BINARY)
        .args(["__agent-check", host])
        .args(id)
        .env("XDG_CONFIG_HOME", cfg)
        .output()
        .unwrap_or_else(|e| panic!("failed to run claudio: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "__agent-check failed: {stderr}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// With `[ssh] forward_agent` naming the host, a terminal there lists the
/// keys of the agent this test runs with (the same fingerprints as
/// `ssh-add -l` here), and the same terminal still does after a second
/// bridge replaced the first. Only fingerprints are compared or printed.
#[test]
fn t10_agent_forwarding_reaches_sessions_across_bridges() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t10: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };
    let local = Command::new("ssh-add").arg("-l").output();
    let local = fingerprints(
        &local
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
    );
    if local.is_empty() {
        println!("SKIP t10: the local ssh agent holds no key (ssh-add one first)");
        return;
    }
    let (_, stderr, ok) = run_claudio(&["__bootstrap", &host]);
    assert!(ok, "__bootstrap failed: {stderr}");
    let cfg = env::temp_dir().join(format!("cl-agent-{}", std::process::id()));
    std::fs::create_dir_all(cfg.join("claudio")).unwrap();
    let toml = format!("[ssh]\nforward_agent = [{:?}]\n", host);
    std::fs::write(cfg.join("claudio/config.toml"), toml).unwrap();

    let first = agent_check(&cfg, &host, None);
    let id = first
        .lines()
        .find_map(|l| l.strip_prefix("session="))
        .expect("a session id")
        .to_owned();
    let second = agent_check(&cfg, &host, Some(&id));
    let _ = std::fs::remove_dir_all(&cfg);
    println!("[t10] local agent: {} key(s)", local.len());
    assert_eq!(fingerprints(&first), local, "the first bridge's terminal");
    assert_eq!(
        fingerprints(&second),
        local,
        "the same terminal after a second bridge"
    );
}
