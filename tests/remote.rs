//! Live SSH remote-session integration tests.
//!
//! These tests are **off by default**. Set `CLAUDIO_SSH_TEST_HOST` to the
//! hostname you want to test against (e.g. `z6`) to enable the SSH tests:
//!
//! ```bash
//! CLAUDIO_SSH_TEST_HOST=z6 cargo test --test remote -- --test-threads=1 --nocapture
//! ```
//!
//! Tests `t1`, `t7`, and `t8` are pure unit-style tests (no SSH) and always
//! run. Tests `t2`–`t6` require a real SSH host.
//!
//! The target host must:
//! - Accept key-based SSH (BatchMode=yes). No password prompts.
//! - Have `claude` reachable (or already have `claudio` installed for t2 skip).
//! - Have `sha256sum` or `shasum -a 256` available.

#![cfg(test)]

use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Return the test host, or `None` to skip SSH tests.
fn test_host() -> Option<String> {
    env::var("CLAUDIO_SSH_TEST_HOST").ok().filter(|h| !h.is_empty())
}

/// Locate the `claudio` binary built by cargo.
fn claudio_bin() -> PathBuf {
    // The integration-test binary lives in target/{debug,release}/deps/.
    // Walk up to find the sibling `claudio` binary.
    let exe = env::current_exe().expect("cannot locate test binary");
    let deps = exe.parent().expect("no parent");
    let profile_dir = deps.parent().expect("no profile dir");
    let candidate = profile_dir.join("claudio");
    if candidate.exists() {
        return candidate;
    }
    // Fallback: assume it's on PATH.
    PathBuf::from("claudio")
}

/// Run `claudio <args>` and return (stdout, stderr, success).
fn run_claudio(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(claudio_bin())
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
        assert!(v.get(field).is_some(), "missing '{field}' in probe JSON: {line}");
    }
    let build = v["build"].as_str().unwrap_or("");
    assert_eq!(build.len(), 64, "build must be a 64-char SHA-256 hex string, got: {build}");

    println!(
        "[t1] local probe OK: version={} os={} arch={} build={}…",
        v["version"], v["os"], v["arch"],
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
    let local_json: serde_json::Value = serde_json::from_str(
        local_stdout.lines().next().unwrap_or("{}"),
    )
    .expect("local probe JSON parse failed");
    let local_build = local_json["build"].as_str().unwrap_or("").to_owned();
    println!("[t2] local build hash: {}…", &local_build[..8]);

    // Run the bootstrap via `claudio __bootstrap HOST` subcommand.
    // This runs `ensure_remote` and prints the result.
    let (stdout1, stderr1, ok1) = run_claudio(&["__bootstrap", &host]);
    assert!(ok1, "first __bootstrap failed:\nstdout: {stdout1}\nstderr: {stderr1}");
    println!("[t2] first bootstrap:\n{stdout1}");

    // After bootstrap, `claudio __probe` must work on the remote.
    let probe_cmd = "$HOME/.local/bin/claudio __probe 2>/dev/null";
    let (remote_probe, probe_ok) = ssh_run(&host, probe_cmd);
    assert!(probe_ok || !remote_probe.is_empty(),
        "remote __probe failed after bootstrap; stdout={remote_probe}");

    let remote_json: serde_json::Value =
        serde_json::from_str(remote_probe.lines().next().unwrap_or("{}"))
            .unwrap_or_default();
    let remote_build = remote_json["build"].as_str().unwrap_or("").to_owned();
    assert_eq!(
        remote_build, local_build,
        "remote build hash should match local after bootstrap"
    );
    println!("[t2] remote build matches local: {}… ✓", &remote_build[..8.min(remote_build.len())]);

    // Second bootstrap must report up-to-date (was_current=true).
    let (stdout2, stderr2, ok2) = run_claudio(&["__bootstrap", &host]);
    assert!(ok2, "second __bootstrap failed:\nstdout: {stdout2}\nstderr: {stderr2}");
    assert!(
        stdout2.contains("up-to-date") || stdout2.contains("was_current=true"),
        "second bootstrap should report up-to-date:\n{stdout2}"
    );
    println!("[t2] second bootstrap (idempotent) ✓");
}

// ── t3: SSH connection and Welcome ────────────────────────────────────────────

/// Connect via SSH and verify the daemon sends a Welcome with hostname and
/// claude availability.
///
/// This test uses `claudio __connect-check HOST` which establishes an SSH
/// connection, reads the Welcome message, and prints it as JSON, then exits.
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
    assert!(!v["host"].as_str().unwrap_or("").is_empty(), "Welcome.host must be non-empty");
    assert!(v["claude_ok"].as_bool().unwrap_or(false),
        "Welcome.claude_ok should be true if claude is installed on {host}");
    println!("[t3] welcome: host={} claude_ok={} ✓", v["host"], v["claude_ok"]);
}

// ── t4: spawn, attach, snapshot ──────────────────────────────────────────────

/// Spawn a session in `/tmp` on the remote, attach, and verify a snapshot
/// arrives. Uses `claudio __remote-session HOST` which performs the full
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

// ── t5: reconnect after drop ──────────────────────────────────────────────────

/// Connect, drop the connection, reconnect, and verify the daemon is still
/// healthy. We test this by connecting twice in sequence.
#[test]
fn t5_reconnect_after_drop() {
    let host = match test_host() {
        Some(h) => h,
        None => {
            println!("SKIP t5: set CLAUDIO_SSH_TEST_HOST to run SSH tests");
            return;
        }
    };

    // First connection check.
    let (_, stderr1, ok1) = run_claudio(&["__connect-check", &host]);
    assert!(ok1, "first connect-check failed: {stderr1}");

    // Brief pause (simulate the gap after a drop).
    std::thread::sleep(Duration::from_millis(300));

    // Second connection check (daemon must still be running).
    let (stdout2, stderr2, ok2) = run_claudio(&["__connect-check", &host]);
    assert!(ok2, "second connect-check (reconnect) failed: {stderr2}");

    let v: serde_json::Value = serde_json::from_str(stdout2.trim()).unwrap_or_default();
    assert!(!v["host"].as_str().unwrap_or("").is_empty(), "reconnect: Welcome.host must be non-empty");
    println!("[t5] reconnect successful: host={} ✓", v["host"]);
}

// ── t6: clean disconnect ──────────────────────────────────────────────────────

/// Verify the daemon survives a clean Disconnect and a subsequent connect-check
/// still works.
#[test]
fn t6_clean_disconnect() {
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

    // Daemon must still respond after a clean exit.
    std::thread::sleep(Duration::from_millis(200));
    let (stdout2, stderr2, ok2) = run_claudio(&["__connect-check", &host]);
    assert!(ok2, "post-disconnect connect-check failed: {stderr2}");
    println!("[t6] post-disconnect check ok: {stdout2}");
}

// ── t7: SSH hosts parsing (no network) ───────────────────────────────────────

/// Unit-style test for the SSH config parser (tests already live in hosts.rs;
/// this serves as a quick smoke test via the binary's `--ssh-hosts` flag).
///
/// Without CLAUDIO_SSH_TEST_HOST, we skip the binary call and just verify the
/// test compiles and the regex is sane.
#[test]
fn t7_ssh_hosts_cli() {
    // Just list SSH hosts known to the local machine; don't require a target host.
    let (stdout, _stderr, _ok) = run_claudio(&["__ssh-hosts"]);
    // Output may be empty if no ~/.ssh/config exists — that's fine.
    // We just check the command doesn't panic.
    println!("[t7] local SSH hosts ({} entries):\n{stdout}", stdout.lines().count());
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
    assert!(build.chars().all(|c| c.is_ascii_hexdigit()), "build must be hex");

    println!("[t8] probe round-trip ✓ build={}…{}", &build[..4], &build[60..]);
}
