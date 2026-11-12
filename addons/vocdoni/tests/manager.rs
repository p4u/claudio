//! The session manager with the Vocdoni addon (features: vocdoni): proxy
//! profiles and their env, `--proxy`/`--no-proxy`/`--first-party`, the fake
//! claude-proxy, team plugin manifests and their setup, `claudio vocdoni`.
//! It shares the manager suite's PTY harness (`common`).
//!
//! Run with:
//!   cargo test --test vocdoni_manager -- --test-threads=1
//!
//! For the gated real-claude scenario through a proxy:
//!   CLAUDIO_E2E=1 CLAUDIO_PROXY_URL=… cargo test --test vocdoni_manager -- --test-threads=1

#[path = "../../../crates/tui/tests/common/mod.rs"]
mod common;

use std::time::{Duration, Instant};
use std::{fs, thread};

use common::{
    current_nonce, extract_nonce, record_env_sh, scopeguard, wizard_pick_dir, ClaudeProjectGuard,
    FakeSsh, ManagerHarness, Region, TuiProcess, ALT_E, ALT_H, ALT_I, ALT_N, ALT_Q, ALT_X, BINARY,
    DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, RECONNECT_WAIT, WAIT,
};
use portable_pty::CommandBuilder;

// ── Proxy test ────────────────────────────────────────────────────────────────

/// Gated by `CLAUDIO_PROXY_TEST=1`. Verifies proxy env injection, badge, token
/// absence from state.json/journal, and recovery respawn also carries proxy env.
#[test]
fn test_proxy_env_injection() {
    if std::env::var("CLAUDIO_PROXY_TEST").as_deref() != Ok("1") {
        return;
    }

    // ── 1. Build the Go proxy ─────────────────────────────────────────────────
    let proxy_tmp = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        std::time::SystemTime::now()
            .elapsed()
            .unwrap_or_default()
            .subsec_nanos()
            .hash(&mut h);
        std::env::temp_dir().join(format!("cl-proxy-{:08x}", h.finish() as u32))
    };
    fs::create_dir_all(&proxy_tmp).expect("create proxy tmp dir");
    let proxy_bin = proxy_tmp.join("cp");
    let db_path = proxy_tmp.join("proxy.db");

    // The claude-proxy source: $CLAUDIO_PROXY_SRC, else a sibling checkout.
    let proxy_src = std::env::var_os("CLAUDIO_PROXY_SRC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../claude-proxy")
        });
    let build_out = std::process::Command::new("go")
        .args([
            "build",
            "-o",
            proxy_bin.to_str().unwrap(),
            "./cmd/claude-proxy",
        ])
        .current_dir(&proxy_src)
        .output()
        .expect("go build (is go installed?)");
    assert!(
        build_out.status.success(),
        "proxy build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr),
    );

    // ── 2. Find a free port and start the proxy ───────────────────────────────
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    let mut proxy_proc = std::process::Command::new(&proxy_bin)
        .args([
            "serve",
            "--addr",
            &format!("127.0.0.1:{port}"),
            "--db",
            db_path.to_str().unwrap(),
            "--log-format",
            "json",
            "--log-level",
            "error",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start proxy");

    let proxy_tmp_guard = proxy_tmp.clone();
    let _proxy_guard = scopeguard(move || {
        proxy_proc.kill().ok();
        proxy_proc.wait().ok();
        let _ = fs::remove_dir_all(&proxy_tmp_guard);
    });

    // Wait for the proxy to listen.
    let proxy_addr = format!("127.0.0.1:{port}");
    let dl = Instant::now() + Duration::from_secs(15);
    loop {
        if std::net::TcpStream::connect(&proxy_addr).is_ok() {
            break;
        }
        assert!(Instant::now() < dl, "proxy never started on :{port}");
        thread::sleep(Duration::from_millis(100));
    }

    // ── 3. Create a user token ────────────────────────────────────────────────
    let create_out = std::process::Command::new(&proxy_bin)
        .args([
            "users",
            "create",
            "--name",
            "testuser",
            "--db",
            db_path.to_str().unwrap(),
        ])
        .output()
        .expect("users create");
    assert!(
        create_out.status.success(),
        "users create failed: {}",
        String::from_utf8_lossy(&create_out.stderr)
    );
    let create_str = String::from_utf8_lossy(&create_out.stdout);
    let token = create_str
        .split_whitespace()
        .find_map(|w| w.strip_prefix("token="))
        .expect("token= not found in 'users create' output")
        .to_owned();
    assert!(!token.is_empty(), "token from proxy is empty");

    let proxy_url = format!("{token}@127.0.0.1:{port}");
    let expected_base_url = format!("http://127.0.0.1:{port}");

    // ── 4. Isolated env with an env-recording fake claude ─────────────────────
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("claude-env.txt");
    // Overwrite fake claude: record the asserted env vars (but not on
    // --version), then banner+cat. Values only for what the test sets.
    harness.write_env_recording_fake_claude(
        &env_file,
        &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_USE_GATEWAY",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        ],
        &[
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_API_KEY",
        ],
    );

    // ── 5. Start claudio TUI with CLAUDIO_PROXY_URL ────────────────────────────
    let mut tui = harness.start_tui_with(|cmd| {
        cmd.env("CLAUDIO_PROXY_URL", &proxy_url);
        // Never inherit the developer's own gateway settings.
        cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
        cmd.env_remove("ANTHROPIC_BASE_URL");
    });

    // ── 6. Wizard: pick a fresh dir ───────────────────────────────────────────
    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // ── 7. Read env.txt written by fake claude ────────────────────────────────
    let env_contents = {
        let dl = Instant::now() + WAIT;
        loop {
            if let Ok(c) = fs::read_to_string(&env_file) {
                if !c.is_empty() {
                    break c;
                }
            }
            assert!(Instant::now() < dl, "env.txt never written by fake claude");
            thread::sleep(Duration::from_millis(100));
        }
    };

    // ── 8. Assert proxy env vars ──────────────────────────────────────────────
    let env_get = |key: &str| -> Option<String> {
        env_contents.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k == key).then(|| v.to_owned())
        })
    };

    assert_eq!(
        env_get("ANTHROPIC_BASE_URL").as_deref(),
        Some(expected_base_url.as_str()),
        "ANTHROPIC_BASE_URL wrong; env:\n{env_contents}"
    );
    assert_eq!(
        env_get("ANTHROPIC_AUTH_TOKEN").as_deref(),
        Some(token.as_str()),
        "ANTHROPIC_AUTH_TOKEN wrong"
    );
    // Default profiles are gateway mode.
    assert_eq!(
        env_get("CLAUDE_CODE_USE_GATEWAY").as_deref(),
        Some("1"),
        "CLAUDE_CODE_USE_GATEWAY not set"
    );
    assert_eq!(
        env_get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC").as_deref(),
        Some("1"),
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC not set"
    );
    assert_eq!(
        env_get("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY").as_deref(),
        Some("1"),
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY not set"
    );
    assert!(
        env_get("ANTHROPIC_DEFAULT_FABLE_MODEL").is_some(),
        "ANTHROPIC_DEFAULT_FABLE_MODEL missing"
    );
    assert!(
        env_get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_some(),
        "ANTHROPIC_DEFAULT_OPUS_MODEL missing"
    );
    assert!(
        env_get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_some(),
        "ANTHROPIC_DEFAULT_SONNET_MODEL missing"
    );
    assert!(
        env_get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_some(),
        "ANTHROPIC_DEFAULT_HAIKU_MODEL missing"
    );
    // ANTHROPIC_API_KEY must NOT appear (daemon scrubs it when AUTH_TOKEN is set).
    assert!(
        !env_contents
            .lines()
            .any(|l| l.starts_with("ANTHROPIC_API_KEY=")),
        "ANTHROPIC_API_KEY leaked into session env"
    );

    // ── 9. The status bar names the session's proxy profile ──────────────────
    tui.wait_for("proxy:env", Region::StatusBar, WAIT);

    // ── 10. Assert state.json: profile name present, token absent ─────────────
    let state_json = fs::read_to_string(harness.state_json()).unwrap_or_default();
    assert!(
        state_json.contains("\"env\""),
        "state.json should contain proxy profile name \"env\"\nstate.json:\n{state_json}"
    );
    assert!(
        !state_json.contains(&token),
        "state.json must not contain the proxy token"
    );

    // ── 11. Assert daemon journal: token absent ───────────────────────────────
    let journal_path = harness
        .config_home
        .join("claudio")
        .join("daemon-sessions.json");
    if let Ok(journal) = fs::read_to_string(&journal_path) {
        assert!(
            !journal.contains(&token),
            "daemon journal must not contain the proxy token"
        );
    }

    // ── 12. Recovery respawn: quit → kill daemon → restart → assert proxy env ─
    // The restart test requires the NEW nonce.
    let old_nonce = current_nonce(&tui);
    let _ = fs::remove_file(&env_file); // so we detect the re-write

    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
    thread::sleep(Duration::from_millis(300));

    harness.kill_daemon();

    // Restart claudio with the proxy URL; daemon should respawn the dormant session.
    let mut tui2 = harness.start_tui_with(|cmd| {
        cmd.env("CLAUDIO_PROXY_URL", &proxy_url);
        cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
        cmd.env_remove("ANTHROPIC_BASE_URL");
    });

    // Wait for env.txt to be re-written by the respawned session.
    let env_contents2 = {
        let dl = Instant::now() + RECONNECT_WAIT;
        loop {
            if let Ok(c) = fs::read_to_string(&env_file) {
                if !c.is_empty() {
                    break c;
                }
            }
            assert!(Instant::now() < dl, "env.txt not re-written after respawn");
            thread::sleep(Duration::from_millis(200));
        }
    };

    // The respawned session must have a NEW nonce.
    tui2.wait_until(
        |screen| {
            let text = screen.region_text(Region::Pane);
            extract_nonce(&text).is_some_and(|n| n != old_nonce)
        },
        RECONNECT_WAIT,
    );

    // Must also carry the proxy env.
    let env_get2 = |key: &str| -> Option<String> {
        env_contents2.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k == key).then(|| v.to_owned())
        })
    };
    assert_eq!(
        env_get2("ANTHROPIC_BASE_URL").as_deref(),
        Some(expected_base_url.as_str()),
        "respawned: ANTHROPIC_BASE_URL wrong\nenv:\n{env_contents2}"
    );
    assert_eq!(
        env_get2("ANTHROPIC_AUTH_TOKEN").as_deref(),
        Some(token.as_str()),
        "respawned: ANTHROPIC_AUTH_TOKEN wrong"
    );
    assert_eq!(
        env_get2("CLAUDE_CODE_USE_GATEWAY").as_deref(),
        Some("1"),
        "respawned: CLAUDE_CODE_USE_GATEWAY not set"
    );
    assert_eq!(
        env_get2("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC").as_deref(),
        Some("1"),
        "respawned: CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC not set"
    );
    assert_eq!(
        env_get2("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY").as_deref(),
        Some("1"),
        "respawned: CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY not set"
    );

    tui2.quit(WAIT);
}

/// Gated by `CLAUDIO_E2E=1` + `CLAUDIO_PROXY_URL`.
///
/// Sends a prompt whose expected answer does NOT appear in the prompt
/// (`4242 + 1 = 4243`), proving the answer is from claude via the proxy.
#[test]
fn test_proxy_real_claude() {
    if std::env::var("CLAUDIO_E2E").as_deref() != Ok("1") {
        return;
    }
    let proxy_url = match std::env::var("CLAUDIO_PROXY_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            println!("SKIP test_proxy_real_claude: set CLAUDIO_PROXY_URL to run");
            return;
        }
    };

    let root = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        std::time::SystemTime::now()
            .elapsed()
            .unwrap_or_default()
            .subsec_nanos()
            .hash(&mut h);
        std::env::temp_dir().join(format!("cl-proxy-e2e-{:08x}", h.finish() as u32))
    };
    let runtime_dir = root.join("run");
    let config_home = root.join("cfg");
    let session_dir = root.join("sess");
    for p in [&runtime_dir, &config_home, &session_dir] {
        fs::create_dir_all(p).unwrap();
    }
    common::write_test_config(&config_home);

    let runtime_dir_g = runtime_dir.clone();
    let cleanup_root = root.clone();
    let _guard = scopeguard(move || {
        let lock = runtime_dir_g.join("claudio").join("daemon-v1.lock");
        if let Some(pid) = fs::read_to_string(&lock)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let _ = fs::remove_dir_all(&cleanup_root);
    });
    // Remove the project dir that claude creates in ~/.claude/projects/<encoded-sess>/.
    let _project_guard = ClaudeProjectGuard::new(&session_dir);

    let mut cmd = CommandBuilder::new(BINARY);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("CLAUDIO_PROXY_URL", &proxy_url);
    cmd.env("ANTHROPIC_MODEL", "claude-haiku-4-5");
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("COLORTERM");
    // Keep real HOME for claude credentials.

    let mut tui = TuiProcess::spawn(cmd);

    tui.wait_for("Alt+h help", Region::StatusBar, Duration::from_secs(30));

    // Wizard: local host, then session directory.
    tui.send_keys(ENTER);
    thread::sleep(Duration::from_millis(80));
    tui.send_paste(session_dir.to_str().unwrap());
    tui.send_keys(ENTER);

    // Wait for claude to be ready (handle trust dialog).
    let ready_dl = Instant::now() + Duration::from_secs(90);
    let mut trust_pressed = false;
    loop {
        let screen = tui.screen_text(Region::Pane);
        if !trust_pressed && screen.contains("Do you trust") {
            tui.send_keys(ENTER);
            trust_pressed = true;
            continue;
        }
        // Ready = claude's input prompt is drawn AND the SessionStart hook
        // marked the tab "needs input". In gateway mode claude keeps
        // initializing (model discovery) after its banner first appears, and
        // keys typed in that window are dropped.
        let tabs = tui.screen_text(Region::TabBar);
        if screen.contains("❯") && tabs.contains('?') {
            break;
        }
        assert!(
            Instant::now() < ready_dl,
            "timeout waiting for claude to be ready"
        );
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(500));

    // Send prompt; expected answer (4243) does NOT appear in the prompt.
    tui.send_paste("Reply with only the number 4242 plus 1");
    tui.send_keys(ENTER);

    tui.wait_for("4243", Region::Pane, Duration::from_secs(90));
    tui.wait_for("✓", Region::Screen, Duration::from_secs(30));

    // Kill the session and quit.
    tui.send_keys(ALT_X);
    tui.wait_for("Kill session", Region::Screen, WAIT);
    tui.send_keys(b"y");
    tui.wait_until(|s| !s.contains("Kill session", Region::Screen), WAIT);

    tui.quit(WAIT);
}

/// R19: a session another client started (a raw `Spawn` over the daemon
/// socket, carrying a proxy token as a web-started one would) is unknown
/// to a manager whose state.json does not have it: its status says so,
/// Alt+e refuses with a notice and never restarts it direct, and
/// state.json keeps it unknown.
#[test]
fn test_restart_of_a_session_started_elsewhere_never_goes_direct() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("other.env");
    harness.write_env_recording_fake_claude(&env_file, &["ANTHROPIC_AUTH_TOKEN"], &[]);
    // The daemon runs; this manager's state.json knows no session.
    let mut tui = harness.start_tui();
    tui.quit(WAIT);
    let reply = harness.daemon_request(serde_json::json!({
        "op": "spawn",
        "id": "6d1f6a52-3c55-4a8e-9a39-5d1c0b0e0019",
        "cwd": harness.dirs[0],
        "env": [["ANTHROPIC_AUTH_TOKEN", "other-client-token"]],
        "rows": 24,
        "cols": 80,
    }));
    assert_eq!(reply["op"], "spawned", "the daemon refused the spawn");
    let proxied = || {
        fs::read_to_string(&env_file)
            .unwrap_or_default()
            .lines()
            .any(|l| l == "ANTHROPIC_AUTH_TOKEN=other-client-token")
    };
    let deadline = Instant::now() + WAIT;
    while !proxied() {
        assert!(Instant::now() < deadline, "the session never started");
        thread::sleep(Duration::from_millis(50));
    }

    let mut tui = harness.start_tui();
    let n0 = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    tui.wait_for(n0, Region::TabBar, WAIT);
    tui.wait_for("route unknown", Region::StatusBar, WAIT);
    tui.wait_for("nonce=", Region::Pane, WAIT);
    let before = current_nonce(&tui);
    tui.send_keys(ALT_E);
    tui.wait_for("restart & resume", Region::Screen, WAIT);
    tui.send_keys(b"r");
    tui.wait_for("restart it where it was started", Region::StatusBar, WAIT);
    // Nothing restarted: the same process, still on its proxy token.
    thread::sleep(Duration::from_millis(500));
    assert_eq!(current_nonce(&tui), before);
    assert!(proxied(), "restarted without its proxy");
    tui.quit(WAIT);

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(harness.state_json()).unwrap()).unwrap();
    let entry = &state["sessions"][0];
    assert_eq!(entry["route_unknown"], true, "{state}");
    assert!(entry["proxy"].is_null(), "{state}");
}

/// Alt+h lists exactly the seven plain keys (and the stats popup's `n`, and
/// the leader's line); any key closes it.
#[test]
fn test_plain_help_lists_only_the_plain_keys() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_plain_with(&["--no-proxy"], |_| {});

    tui.send_keys(ALT_H);
    tui.wait_for("claudio keys", Region::Screen, WAIT);
    let screen = tui.screen_text(Region::Screen);
    for key in [
        "Alt+h",
        "Alt+s",
        "Alt+l",
        "Alt+e",
        "Alt+u",
        "Alt+z",
        "Alt+i",
        "Alt+z then:",
    ] {
        assert!(screen.contains(key), "{key} missing:\n{screen}");
    }
    assert_eq!(
        screen.matches("Alt+").count(),
        8,
        "exactly the plain keys:\n{screen}"
    );

    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("claudio keys", Region::Screen), WAIT);
    tui.send_keys(b"after\r");
    tui.wait_for("after", Region::Screen, WAIT);
    drop(tui);
}

/// Alt+s (proxy stats) and Alt+e (reset) work as in the manager.
#[test]
fn test_plain_keeps_proxy_stats_and_reset() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_plain_with(&["--no-proxy"], |_| {});

    tui.send_keys(b"\x1bs");
    tui.wait_for("No proxy configured", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("No proxy configured", Region::Screen), WAIT);

    let before = extract_nonce(&tui.screen_text(Region::Screen)).expect("banner nonce");
    tui.send_keys(ALT_E);
    tui.wait_for("new conversation", Region::Screen, WAIT);
    tui.send_keys(b"n");
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Screen)).is_some_and(|n| n != before),
        WAIT,
    );
    // Still the same single, full-screen session.
    assert!(!tui.screen_text(Region::Screen).contains("Alt+h help"));
    drop(tui);
}

/// With a proxy profile (an unreachable one, so the built-in model defaults
/// apply) claude gets the gateway env, loses the parent-session marker and the
/// API key, and keeps the user's args; the token never reaches argv.
#[test]
fn test_plain_injects_proxy_env() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("plain.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
        ],
        &["ANTHROPIC_API_KEY", "CLAUDECODE"],
    );

    let tui = harness.start_plain_with(&["-c", "hello"], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", "sekret@127.0.0.1:1");
    });
    // The banner lists claude's argv, which wraps at the screen's edge.
    let screen = tui.screen_text(Region::Screen).replace('\n', "");
    assert!(screen.contains("-c hello"), "{screen}");
    assert!(
        !screen.contains("sekret"),
        "the token must never reach argv:\n{screen}"
    );

    let env = fs::read_to_string(&env_file).expect("env recorded");
    assert!(env.contains("ANTHROPIC_AUTH_TOKEN=sekret"), "{env}");
    assert!(
        env.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:1"),
        "{env}"
    );
    assert!(
        env.contains("ANTHROPIC_DEFAULT_OPUS_MODEL=claude-opus"),
        "{env}"
    );
    assert!(
        !env.contains("ANTHROPIC_API_KEY"),
        "api key must be removed: {env}"
    );
    assert!(
        !env.contains("CLAUDECODE"),
        "session marker must be scrubbed: {env}"
    );
    drop(tui);
}

/// `--no-proxy` ignores the configured proxy: the API key stays and no
/// gateway env appears.
#[test]
fn test_plain_no_proxy_keeps_api_key() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("plain.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &["ANTHROPIC_API_KEY"],
        &["ANTHROPIC_AUTH_TOKEN", "CLAUDECODE"],
    );

    let tui = harness.start_plain_with(&["--no-proxy"], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", "sekret@127.0.0.1:1");
    });
    let env = fs::read_to_string(&env_file).expect("env recorded");
    assert!(env.contains("ANTHROPIC_API_KEY=test-key-not-real"), "{env}");
    assert!(!env.contains("ANTHROPIC_AUTH_TOKEN"), "{env}");
    assert!(!env.contains("CLAUDECODE"), "{env}");
    drop(tui);
}

/// R20: a `[proxy]` section that cannot be read (here mistyped; the rest of
/// config.toml still parses) fails `--plain`'s default launch with the load
/// error: claude never starts, and never direct.
#[test]
fn test_plain_with_unreadable_proxy_profiles_never_starts_direct() {
    let harness = ManagerHarness::new();
    let marker = harness.root.join("claude-ran");
    harness.write_marker_fake_claude(&marker);
    let config = harness.config_home.join("claudio").join("config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[proxy]\ndefault = 5\n");
    fs::write(&config, text).unwrap();

    let mut cmd = CommandBuilder::new(BINARY);
    cmd.arg("--plain");
    cmd.cwd(&harness.dirs[0]);
    harness.apply(&mut cmd);
    let mut tui = TuiProcess::spawn(cmd);
    assert_eq!(tui.wait_exit_code(DAEMON_WAIT), 1);
    let screen = tui.screen_text(Region::Screen);
    assert!(
        screen.contains("cannot read the proxy profiles"),
        "{screen}"
    );
    assert!(!marker.exists(), "claude started");

    // `--no-proxy` still starts it, direct.
    let tui = harness.start_plain_with(&["--no-proxy"], |_| {});
    assert!(marker.exists());
    drop(tui);
}

/// An old client's spawn (no `env_remove`) gets the compatibility scrub
/// from a new daemon: the proxy token in its env replaces the inherited
/// `ANTHROPIC_API_KEY`, and the session markers go (docs/daemon-protocol.md,
/// "Mixed versions"; B1). The spawn is the raw JSON an older client sends.
#[test]
fn test_old_client_spawn_is_scrubbed_by_a_new_daemon() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("old-client.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &["ANTHROPIC_AUTH_TOKEN"],
        &["ANTHROPIC_API_KEY", "CLAUDECODE"],
    );
    // The daemon inherits the TUI's env: the harness's API key and a marker.
    let tui = harness.start_tui_with(|cmd| {
        cmd.env("CLAUDECODE", "1");
        // Never inherit the developer's own gateway settings.
        cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
        cmd.env_remove("ANTHROPIC_BASE_URL");
    });
    let id = "6d1f6a52-3c55-4a8e-9a39-5d1c0b0e0001";
    let reply = harness.daemon_request(serde_json::json!({
        "op": "spawn",
        "id": id,
        "cwd": harness.dirs[0],
        "env": [["ANTHROPIC_AUTH_TOKEN", "old-client-token"]],
        "rows": 24,
        "cols": 80,
    }));
    assert_eq!(reply["op"], "spawned", "the daemon refused the spawn");
    let deadline = Instant::now() + WAIT;
    let env = loop {
        let env = fs::read_to_string(&env_file).unwrap_or_default();
        if env
            .lines()
            .any(|l| l == "ANTHROPIC_AUTH_TOKEN=old-client-token")
        {
            break env;
        }
        assert!(Instant::now() < deadline, "the session never started");
        thread::sleep(Duration::from_millis(50));
    };
    let set = |key: &str| env.lines().any(|l| l.starts_with(&format!("{key}=")));
    // Values are never printed (C1): the messages name the keys only.
    assert!(
        !set("ANTHROPIC_API_KEY"),
        "the inherited API key reached claude"
    );
    assert!(!set("CLAUDECODE"), "a session marker reached claude");
    drop(tui);
}

/// A proxy profile is gateway by default; `--first-party` makes one run
/// first-party. Nothing listens on the profile's port, so the config fetch
/// falls back to the built-in model defaults. An inherited
/// `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` is removed from the test env.
#[test]
fn test_plain_first_party_flag() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("plain.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &[
            "CLAUDE_CODE_USE_GATEWAY",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
            "DISABLE_AUTOUPDATER",
            "ANTHROPIC_BASE_URL",
            "ENABLE_TOOL_SEARCH",
        ],
        &[],
    );
    let proxy = "sekret@127.0.0.1:1";

    let tui = harness.start_plain_with(&[], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", proxy);
        cmd.env_remove("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC");
    });
    let env = fs::read_to_string(&env_file).expect("env recorded");
    assert!(env.contains("CLAUDE_CODE_USE_GATEWAY=1"), "{env}");
    assert!(
        env.contains("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1"),
        "{env}"
    );
    assert!(!env.contains("DISABLE_AUTOUPDATER="), "{env}");
    drop(tui);

    let _ = fs::remove_file(&env_file);
    let tui = harness.start_plain_with(&["--first-party"], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", proxy);
        cmd.env_remove("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC");
    });
    let env = fs::read_to_string(&env_file).expect("env recorded");
    assert!(!env.contains("CLAUDE_CODE_USE_GATEWAY="), "{env}");
    assert!(
        !env.contains("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="),
        "{env}"
    );
    assert!(
        env.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:1"),
        "{env}"
    );
    assert!(env.contains("DISABLE_AUTOUPDATER=1"), "{env}");
    assert!(env.contains("ENABLE_TOOL_SEARCH=true"), "{env}");
    drop(tui);
}

/// The configured default proxy profile routes a new session even when its
/// proxy config cannot be fetched (nothing listens on its port): the
/// session starts through that proxy with the built-in model defaults,
/// never direct (AC8; Routing::Required).
#[test]
fn test_default_profile_with_unreachable_config_starts_through_the_proxy() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("default.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &["ANTHROPIC_BASE_URL"],
        &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
    );
    let config = harness.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "[proxy]\ndefault = \"work\"\n[proxy.profiles.work]\n\
         url = \"http://127.0.0.1:1\"\ntoken = \"work-test-token\"\n",
    );
    fs::write(&config, text).unwrap();
    let mut tui = harness.start_tui();
    tui.wait_for("New session", Region::Screen, WAIT);
    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    let deadline = Instant::now() + WAIT;
    let env = loop {
        let env = fs::read_to_string(&env_file).unwrap_or_default();
        if env.lines().any(|l| l.starts_with("ANTHROPIC_BASE_URL=")) {
            break env;
        }
        assert!(Instant::now() < deadline, "no session started: {env}");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        env.lines()
            .any(|l| l == "ANTHROPIC_BASE_URL=http://127.0.0.1:1"),
        "{env}"
    );
    assert!(
        env.lines().any(|l| l == "ANTHROPIC_AUTH_TOKEN=<set>"),
        "{env}"
    );
    assert!(
        !env.lines().any(|l| l.starts_with("ANTHROPIC_API_KEY=")),
        "the API key reached a proxied session: {env}"
    );
    tui.wait_for("proxy:work", Region::StatusBar, WAIT);
    tui.quit(WAIT);
}

/// A proxy session whose daemon stopped (an upgrade replaces it) comes back,
/// when the TUI recovers it, through its proxy: never direct, which would
/// use whatever login the host has ("Not logged in").
#[test]
fn test_a_recovered_proxy_session_keeps_its_proxy() {
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("recovered.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &["ANTHROPIC_BASE_URL"],
        &["ANTHROPIC_AUTH_TOKEN"],
    );
    let config = harness.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "[proxy]\ndefault = \"work\"\n[proxy.profiles.work]\n\
         url = \"http://127.0.0.1:1\"\ntoken = \"work-test-token\"\n",
    );
    fs::write(&config, text).unwrap();
    let proxied = |env: &str| {
        env.lines()
            .any(|l| l == "ANTHROPIC_BASE_URL=http://127.0.0.1:1")
            && env.lines().any(|l| l == "ANTHROPIC_AUTH_TOKEN=<set>")
    };
    let started = || {
        let deadline = Instant::now() + WAIT;
        loop {
            let env = fs::read_to_string(&env_file).unwrap_or_default();
            if env.lines().any(|l| l.starts_with("ANTHROPIC_BASE_URL=")) {
                return env;
            }
            assert!(Instant::now() < deadline, "no session started: {env}");
            thread::sleep(Duration::from_millis(50));
        }
    };

    let mut tui = harness.start_tui();
    tui.wait_for("New session", Region::Screen, WAIT);
    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    let first = started();
    assert!(proxied(&first), "the first start: {first}");

    // The daemon is replaced under the running TUI, which reconnects.
    fs::remove_file(&env_file).unwrap();
    let restart = std::process::Command::new(BINARY)
        .args(["daemon", "restart"])
        .env("XDG_RUNTIME_DIR", &harness.runtime_dir)
        .env("XDG_CONFIG_HOME", &harness.config_home)
        .env("HOME", &harness.home)
        .env("CLAUDIO_CLAUDE_PATH", &harness.fake_claude)
        .output()
        .expect("claudio daemon restart");
    assert!(
        restart.status.success(),
        "{}",
        String::from_utf8_lossy(&restart.stdout)
    );
    let reconnected = started();
    assert!(
        proxied(&reconnected),
        "reconnected without its proxy: {reconnected}"
    );
    tui.quit(WAIT);

    // The daemon goes (as an upgrade replaces it): the session is dormant.
    let stop = std::process::Command::new(BINARY)
        .args(["daemon", "stop"])
        .env("XDG_RUNTIME_DIR", &harness.runtime_dir)
        .env("XDG_CONFIG_HOME", &harness.config_home)
        .env("HOME", &harness.home)
        .env("CLAUDIO_CLAUDE_PATH", &harness.fake_claude)
        .output()
        .expect("claudio daemon stop");
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stdout)
    );
    fs::remove_file(&env_file).unwrap();

    // A new TUI starts a daemon and recovers the session.
    let mut tui = harness.start_tui();
    let n0 = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    tui.wait_for(n0, Region::TabBar, WAIT);
    let recovered = started();
    assert!(
        proxied(&recovered),
        "recovered without its proxy: {recovered}"
    );
    tui.quit(WAIT);
}

/// A proxy session on a remote host whose daemon is replaced (an upgrade
/// uploads a new binary and restarts it) comes back through its proxy when
/// the TUI reconnects: never direct.
#[test]
fn test_a_recovered_remote_proxy_session_keeps_its_proxy() {
    let fake = FakeSsh::new();
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("remote.env");
    harness.write_env_recording_fake_claude(
        &env_file,
        &["ANTHROPIC_BASE_URL"],
        &["ANTHROPIC_AUTH_TOKEN"],
    );
    let config = harness.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "[proxy]\ndefault = \"work\"\n[proxy.profiles.work]\n\
         url = \"http://127.0.0.1:1\"\ntoken = \"work-test-token\"\n",
    );
    fs::write(&config, text).unwrap();
    let started = || {
        let deadline = Instant::now() + DAEMON_WAIT;
        loop {
            let env = fs::read_to_string(&env_file).unwrap_or_default();
            if env.starts_with("# env") {
                return env;
            }
            assert!(Instant::now() < deadline, "no session started");
            thread::sleep(Duration::from_millis(50));
        }
    };
    let proxied = |env: &str| {
        env.lines()
            .any(|l| l == "ANTHROPIC_BASE_URL=http://127.0.0.1:1")
            && env.lines().any(|l| l == "ANTHROPIC_AUTH_TOKEN=<set>")
    };

    let path = fake.path();
    let mut tui = harness.start_tui_with(|cmd| cmd.env("PATH", &path));
    tui.send_paste("fakehost");
    tui.wait_for("connect to fakehost", Region::Screen, WAIT);
    tui.send_keys(ENTER);
    tui.wait_for("New session on fakehost", Region::Screen, DAEMON_WAIT);
    tui.send_paste(harness.dirs[0].to_str().unwrap());
    thread::sleep(Duration::from_millis(50));
    tui.send_keys(ENTER);
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, DAEMON_WAIT);
    let first = started();
    assert!(proxied(&first), "the first start: {first}");

    // The remote daemon is replaced; the TUI reconnects and recovers.
    fs::remove_file(&env_file).unwrap();
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let v = claudio_core::version::PROTO;
    let lock = fake.root.join(format!("t/claudio-{uid}/daemon-v{v}.lock"));
    let pid: i32 = fs::read_to_string(&lock).unwrap().trim().parse().unwrap();
    // SAFETY: kill(2) has no memory-safety preconditions.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let recovered = started();
    assert!(
        proxied(&recovered),
        "recovered without its proxy: {recovered}"
    );
    tui.quit(WAIT);
}

// ── Proxy plugins ────────────────────────────────────────────────────────────

/// The fake plugin and the secret its proxy env carries.
const PLUGIN: &str = "vocdoni-test";

const PLUGIN_KEY: &str = "VOCDONI_TEST_TOKEN";

/// The manifest the fake proxy serves: plugins `tool` (to install) and
/// `have` (installed already), servers `kb` (needs the secret) and
/// `extra`, agents `reviewer` and `mine` (the member's own), instructions.
const PLUGIN_MANIFEST: &str = r#"format = 1
name = "vocdoni-test"
version = "1.0.0"
description = "Test setup"
instructions = "Manifest instructions for the test."

[[marketplace]]
name = "market"
source = "github:acme/market"

[[plugin]]
name = "tool"
marketplace = "market"

[[plugin]]
name = "have"
marketplace = "market"

[[mcp]]
name = "kb"
type = "http"
url = "https://kb.example.org/mcp"
headers = { Authorization = "Bearer ${VOCDONI_TEST_TOKEN}" }
secrets = ["VOCDONI_TEST_TOKEN"]

[[mcp]]
name = "extra"
type = "sse"
url = "https://extra.example.org/sse"

[[agent]]
name = "reviewer"
description = "Reviews"
prompt = "Review the change."

[[agent]]
name = "mine"
description = "Mine"
prompt = "Mine."
"#;

/// A fake claude-proxy on loopback: `GET /v1/claudio/plugins` lists
/// [`PLUGIN`] with its manifest at `GET /vocdoni-test.toml` (plain http on
/// loopback, which a debug build takes with `CLAUDIO_TEST_LOOPBACK_HTTP`);
/// every other path is a 404. Returns its `host:port`.
fn fake_plugin_proxy() -> String {
    use sha2::Digest;
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let sha = format!("{:x}", sha2::Sha256::digest(PLUGIN_MANIFEST.as_bytes()));
    let list = serde_json::json!({"plugins": [{
        "name": PLUGIN, "version": "1.0.0",
        "bundle": {"url": format!("http://{addr}/{PLUGIN}.toml"), "sha256": sha},
        "env": {PLUGIN_KEY: "s3cret"}}]})
    .to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&head);
            let (status, reply) = if line.starts_with("GET /v1/claudio/plugins ") {
                ("200 OK", list.as_str())
            } else if line.starts_with("GET /v1/claudio ") {
                // `claudio vocdoni login`'s check of the token.
                ("200 OK", "{}")
            } else if line.starts_with(&format!("GET /{PLUGIN}.toml ")) {
                ("200 OK", PLUGIN_MANIFEST)
            } else {
                ("404 Not Found", "{}")
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    addr
}

/// A fake claude whose `--help` lists `--agents` and `--mcp-config`. Its
/// `plugin …` runs are logged to `<dumps>/plugin.log` and act on
/// `$CLAUDE_CONFIG_DIR` as Claude Code would (the marketplace is added, the
/// plugin installed beside `have@market`). Every other launch is recorded
/// in `<dumps>/<n>/` (`argv`, one argument per line, and `env`: the plugin
/// secret's key only, [`record_env_sh`]) before the usual banner and `cat`.
fn write_plugin_claude(h: &ManagerHarness, dumps: &std::path::Path) {
    write_plugin_claude_noting(h, dumps, &[]);
}

/// [`write_plugin_claude`], also noting whether each of `present` is set.
fn write_plugin_claude_noting(h: &ManagerHarness, dumps: &std::path::Path, present: &[&str]) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(dumps.join("launches")).unwrap();
    let record = record_env_sh("$d/env", &[PLUGIN_KEY], present);
    let dumps = dumps.display();
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
if [ "$1" = "--help" ]; then
  printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'
  exit 0
fi
if [ "$1" = "plugin" ]; then
  echo "$*" >> "{dumps}/plugin.log"
  p="$CLAUDE_CONFIG_DIR/plugins"
  mkdir -p "$p"
  [ "$2" = "marketplace" ] && printf '{{"market":{{}}}}\n' > "$p/known_marketplaces.json"
  [ "$2" = "install" ] && printf '{{"version":2,"plugins":{{"have@market":[{{"scope":"user"}}],"%s":[{{"scope":"user"}}]}}}}\n' "$3" > "$p/installed_plugins.json"
  echo '{{"ok":true}}'
  exit 0
fi
d="{dumps}/launches/$(ls "{dumps}/launches" | wc -l | tr -d ' ')"
mkdir -p "$d"
{record}printf '%s\n' "$@" > "$d/argv"
echo "FAKE_CLAUDE_BANNER pid=$$"
exec cat
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Launch `n`'s recorded argv and env, once both are written.
fn launch_dump(dumps: &std::path::Path, n: usize) -> (Vec<String>, String) {
    let dir = dumps.join("launches").join(n.to_string());
    let deadline = Instant::now() + WAIT;
    loop {
        let argv = fs::read_to_string(dir.join("argv"));
        let env = fs::read_to_string(dir.join("env"));
        if let (Ok(argv), Ok(env)) = (argv, env) {
            if !env.is_empty() {
                return (argv.lines().map(str::to_owned).collect(), env);
            }
        }
        assert!(
            Instant::now() < deadline,
            "launch {n} not recorded in {dir:?}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// The value after `flag` in `argv`.
fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let at = argv.iter().position(|a| a == flag)?;
    argv.get(at + 1).map(String::as_str)
}

/// The text of every session context file the daemon wrote.
fn context_files(h: &ManagerHarness) -> String {
    fn walk(dir: &std::path::Path, out: &mut String) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.parent().is_some_and(|p| p.ends_with("context")) {
                out.push_str(&fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut out = String::new();
    walk(&h.runtime_dir, &mut out);
    out
}

/// A proxy plugin's manifest: the setup screen comes first and nothing is
/// applied before it closes; after Enter a new session gets the agents and
/// the MCP servers it kept (placeholders on argv, the secret in the env),
/// the instructions in its context, and the plugin is installed through
/// the CLI; the extensions screen shows each item's outcome; turned off,
/// the next session has none of it.
/// AC7: `claudio vocdoni login --token` (the URL given, the token on
/// stdin) saves the default profile and caches its plugins; a new session
/// then gets the proxy env (the token), the plugin args and the
/// instructions in its context file.
#[test]
fn test_vocdoni_login_token_then_a_session_gets_the_proxy_and_plugins() {
    use std::io::Write;
    let h = ManagerHarness::new();
    let dumps = h.root.join("dumps");
    write_plugin_claude_noting(&h, &dumps, &["ANTHROPIC_AUTH_TOKEN"]);
    let claude_config = h.root.join("claude-config");
    fs::create_dir_all(claude_config.join("plugins")).unwrap();
    let cache = h.root.join("xdg-cache");
    let proxy = fake_plugin_proxy();
    let url = format!("http://{proxy}");
    let mut login = std::process::Command::new(BINARY)
        .args(["vocdoni", "login", &url, "--name", "work", "--token"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("XDG_RUNTIME_DIR", &h.runtime_dir)
        .env("XDG_CONFIG_HOME", &h.config_home)
        .env("XDG_CACHE_HOME", &cache)
        .env("HOME", &h.home)
        .env("CLAUDIO_CLAUDE_PATH", &h.fake_claude)
        .env("CLAUDIO_TEST_LOOPBACK_HTTP", "1")
        .env("CLAUDIO_NO_BROWSER", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run claudio vocdoni login");
    login
        .stdin
        .take()
        .unwrap()
        .write_all(b"member-token\n")
        .unwrap();
    let out = login.wait_with_output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Profile \"work\" saved as default."),
        "{stdout}"
    );
    assert!(stdout.contains(PLUGIN), "its plugins are listed: {stdout}");
    assert!(!stdout.contains("member-token") && !stderr.contains("member-token"));
    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains("default = \"work\""), "{config}");

    let mut tui = h.start_tui_with(|cmd| {
        cmd.env("XDG_CACHE_HOME", &cache);
        cmd.env("CLAUDE_CONFIG_DIR", &claude_config);
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    });
    tui.wait_for("New session", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_for("Set up vocdoni-test 1.0.0", Region::Screen, WAIT);
    // Past the arming, keep everything.
    thread::sleep(Duration::from_millis(1200));
    tui.send_keys(ENTER);
    tui.wait_for(
        &format!("plugin {PLUGIN} set up (saved)"),
        Region::StatusBar,
        WAIT,
    );
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &h.dirs[0]);
    let (argv, env) = launch_dump(&dumps, 0);
    assert!(
        env.contains("ANTHROPIC_AUTH_TOKEN=<set>"),
        "the proxy env: {env}"
    );
    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
    assert!(flag_value(&argv, "--agents").is_some(), "{argv:?}");
    let mcp = flag_value(&argv, "--mcp-config").expect("--mcp-config");
    assert!(mcp.contains("Bearer ${VOCDONI_TEST_TOKEN}"), "{mcp}");
    assert!(!argv.iter().any(|a| a.contains("member-token")), "{argv:?}");
    let context = context_files(&h);
    assert!(
        context.contains("Manifest instructions for the test."),
        "{context}"
    );
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

#[test]
fn test_plugin_manifest_setup_then_sessions_get_what_was_kept() {
    let h = ManagerHarness::new();
    let dumps = h.root.join("dumps");
    write_plugin_claude(&h, &dumps);
    let claude_config = h.root.join("claude-config");
    fs::create_dir_all(claude_config.join("agents")).unwrap();
    fs::create_dir_all(claude_config.join("plugins")).unwrap();
    fs::write(claude_config.join("agents/mine.md"), "my own agent").unwrap();
    fs::write(
        claude_config.join("plugins/installed_plugins.json"),
        r#"{"version":2,"plugins":{"have@market":[{"scope":"user"}]}}"#,
    )
    .unwrap();
    let proxy = fake_plugin_proxy();
    let mut tui = h.start_tui_with(|cmd| {
        cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
        cmd.env("XDG_CACHE_HOME", h.root.join("xdg-cache"));
        cmd.env("CLAUDE_CONFIG_DIR", &claude_config);
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    });

    // The wizard is open at start; the setup screen waits its turn.
    tui.wait_for("New session", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_for("Set up vocdoni-test 1.0.0", Region::Screen, WAIT);
    tui.wait_for(
        "[x] kb (http https://kb.example.org/mcp)",
        Region::Screen,
        WAIT,
    );
    tui.wait_for("[x] reviewer: Reviews", Region::Screen, WAIT);
    assert!(
        !fs::read_to_string(dumps.join("plugin.log")).is_ok_and(|l| !l.is_empty()),
        "nothing applied before the setup closes"
    );
    // Past the arming; `extra` (the fourth item) off, then Enter.
    thread::sleep(Duration::from_millis(1200));
    for _ in 0..3 {
        tui.send_keys(DOWN_ARROW);
    }
    tui.send_keys(b" ");
    tui.wait_for(
        "[ ] extra (sse https://extra.example.org/sse)",
        Region::Screen,
        WAIT,
    );
    tui.send_keys(ENTER);
    tui.wait_for(
        &format!("plugin {PLUGIN} set up (saved)"),
        Region::StatusBar,
        WAIT,
    );
    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
    assert!(
        config.contains(&format!(
            "[plugins.{PLUGIN}]\ndisabled = [\"mcp:extra\"]\nreviewed = \"1.0.0\""
        )),
        "{config}"
    );

    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &h.dirs[0]);
    let (argv, env) = launch_dump(&dumps, 0);
    let agents = flag_value(&argv, "--agents").expect("--agents");
    assert!(
        agents.contains("\"reviewer\"") && !agents.contains("\"mine\""),
        "{agents}"
    );
    let mcp = flag_value(&argv, "--mcp-config").expect("--mcp-config");
    assert!(mcp.contains("Bearer ${VOCDONI_TEST_TOKEN}"), "{mcp}");
    assert!(!mcp.contains("extra"), "a server turned off: {mcp}");
    assert!(
        !argv.iter().any(|a| a.contains("s3cret")),
        "no secret in argv"
    );
    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
    let context = context_files(&h);
    assert!(
        context.contains("Manifest instructions for the test."),
        "{context}"
    );

    // The plugin is installed in the background, never with -y.
    let deadline = Instant::now() + WAIT;
    let log = loop {
        let log = fs::read_to_string(dumps.join("plugin.log")).unwrap_or_default();
        if log.contains("plugin install tool@market --scope user --json") {
            break log;
        }
        assert!(Instant::now() < deadline, "no install: {log}");
        thread::sleep(Duration::from_millis(100));
    };
    assert!(
        log.contains("plugin marketplace add acme/market --scope user --json"),
        "{log}"
    );
    assert!(
        !log.contains("have@market") && !log.contains(" -y"),
        "{log}"
    );
    let installed = claude_config.join("plugins/installed_plugins.json");
    while !fs::read_to_string(&installed).is_ok_and(|s| s.contains("tool@market")) {
        assert!(Instant::now() < deadline, "the install never finished");
        thread::sleep(Duration::from_millis(100));
    }
    // The daemon notes the outcome once the CLI exits.
    thread::sleep(Duration::from_millis(500));

    // The extensions screen: `r` asks again, and each outcome shows.
    tui.send_keys(ALT_I);
    tui.wait_for(&format!("[x] {PLUGIN}"), Region::Screen, WAIT);
    // After the built-ins search and snippets (P3.18: latex and diagram are
    // the web addon's rows, after the plugins).
    for _ in 0..2 {
        tui.send_keys(DOWN_ARROW);
    }
    tui.wait_for("1 items disabled (Enter to change)", Region::Screen, WAIT);
    tui.send_keys(b"r");
    tui.wait_for(
        "local: ready · 1 installed, 1 present, 0 installing, 1 skipped",
        Region::Screen,
        WAIT,
    );
    tui.wait_for("agent:mine: yours", Region::Screen, WAIT);
    // Turned off: the next session has none of it.
    tui.send_keys(b" ");
    tui.wait_for(&format!("[ ] {PLUGIN}"), Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &h.dirs[1]);
    let (argv, env) = launch_dump(&dumps, 1);
    assert!(
        !argv.iter().any(|a| a == "--agents" || a == "--mcp-config"),
        "{argv:?}"
    );
    assert!(!env.contains(PLUGIN_KEY), "{env}");
    tui.quit(WAIT);
}

/// The wizard's host and directory steps for `dir`, without waiting for
/// the session (a review may come first).
fn wizard_enter_dir(tui: &mut TuiProcess, dir: &std::path::Path) {
    tui.send_keys(ENTER);
    thread::sleep(Duration::from_millis(80));
    tui.send_paste(dir.to_str().expect("UTF-8 dir"));
    thread::sleep(Duration::from_millis(50));
    tui.send_keys(ENTER);
}

/// The first-sight offer put off (Esc), a launch still reviews the unsaved
/// plugin: its checklist opens before the session starts, `s` skips it for
/// that launch (the session starts without the plugin's secret or flags),
/// and the next launch asks again; Esc then cancels that launch and its tab.
#[test]
fn test_a_launch_reviews_an_unsaved_plugin_and_skip_starts_without_it() {
    let h = ManagerHarness::new();
    let dumps = h.root.join("dumps");
    write_plugin_claude(&h, &dumps);
    let claude_config = h.root.join("claude-config");
    fs::create_dir_all(&claude_config).unwrap();
    let proxy = fake_plugin_proxy();
    let mut tui = h.start_tui_with(|cmd| {
        cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
        cmd.env("XDG_CACHE_HOME", h.root.join("xdg-cache"));
        cmd.env("CLAUDE_CONFIG_DIR", &claude_config);
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    });
    tui.wait_for("New session", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_for("Set up vocdoni-test 1.0.0", Region::Screen, WAIT);
    tui.wait_for("Esc later", Region::Screen, WAIT);
    thread::sleep(Duration::from_millis(1200));
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Set up vocdoni-test", Region::Screen), WAIT);

    tui.send_keys(ALT_N);
    wizard_enter_dir(&mut tui, &h.dirs[0]);
    tui.wait_for("Set up vocdoni-test 1.0.0", Region::Screen, WAIT);
    tui.wait_for("s skip · Esc cancel", Region::Screen, WAIT);
    assert!(
        !dumps.join("launches/0").exists(),
        "nothing starts before the review"
    );
    thread::sleep(Duration::from_millis(1200));
    tui.send_keys(b"s");
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);
    let (argv, env) = launch_dump(&dumps, 0);
    assert!(
        !argv.iter().any(|a| a == "--agents" || a == "--mcp-config"),
        "{argv:?}"
    );
    assert!(
        !env.contains(PLUGIN_KEY),
        "skipped: no plugin secret: {env}"
    );
    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap_or_default();
    assert!(!config.contains("[plugins."), "nothing saved: {config}");

    // The next launch asks again; Esc cancels it, and its tab goes.
    tui.send_keys(ALT_N);
    wizard_enter_dir(&mut tui, &h.dirs[1]);
    tui.wait_for("s skip · Esc cancel", Region::Screen, WAIT);
    thread::sleep(Duration::from_millis(1200));
    tui.send_keys(ESC);
    tui.wait_for("launch cancelled", Region::StatusBar, WAIT);
    thread::sleep(Duration::from_millis(500));
    assert!(!dumps.join("launches/1").exists(), "nothing started");
    tui.quit(WAIT);
}

/// `--plain` with a plugin never set up: its checklist comes before the
/// session starts; Enter saves it and the session gets the plugin.
#[test]
fn test_plain_reviews_before_starting() {
    let h = ManagerHarness::new();
    let dumps = h.root.join("dumps");
    write_plugin_claude(&h, &dumps);
    let claude_config = h.root.join("claude-config");
    fs::create_dir_all(&claude_config).unwrap();
    let proxy = fake_plugin_proxy();
    let mut cmd = CommandBuilder::new(BINARY);
    cmd.arg("--plain");
    cmd.cwd(&h.dirs[0]);
    h.apply(&mut cmd);
    cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
    cmd.env("XDG_CACHE_HOME", h.root.join("xdg-cache"));
    cmd.env("CLAUDE_CONFIG_DIR", &claude_config);
    cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
    let mut tui = TuiProcess::spawn(cmd);
    tui.wait_for("Set up vocdoni-test 1.0.0", Region::Screen, DAEMON_WAIT);
    tui.wait_for(
        "[x] kb (http https://kb.example.org/mcp)",
        Region::Screen,
        WAIT,
    );
    assert!(
        !dumps.join("launches/0").exists(),
        "nothing starts before the review"
    );
    thread::sleep(Duration::from_millis(1200));
    tui.send_keys(ENTER);
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Screen, WAIT);
    let (argv, env) = launch_dump(&dumps, 0);
    assert!(env.contains(&format!("{PLUGIN_KEY}=s3cret")), "{env}");
    let mcp = flag_value(&argv, "--mcp-config").expect("--mcp-config");
    assert!(mcp.contains("Bearer ${VOCDONI_TEST_TOKEN}"), "{mcp}");
    let config = fs::read_to_string(h.config_home.join("claudio/config.toml")).unwrap();
    assert!(config.contains(&format!("[plugins.{PLUGIN}]")), "{config}");
    drop(tui);
}

/// A daemon of the previous release (no `addon:vocdoni`): a new client
/// starts the session through the proxy without the plugins, and says why
/// (spec "Mixed versions", row 2; AC8). The old daemon's own scrub drops
/// the inherited API key; no plugin secret reaches the session.
#[test]
fn test_new_client_old_daemon_gets_proxy_env_without_plugins() {
    let h = ManagerHarness::new();
    let env_file = h.root.join("old-daemon.env");
    h.write_env_recording_fake_claude(
        &env_file,
        &["ANTHROPIC_AUTH_TOKEN"],
        &[PLUGIN_KEY, "ANTHROPIC_API_KEY"],
    );
    // The plugin is set up already: no setup screen.
    let config = h.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "[plugins.{PLUGIN}]\ndisabled = []\nreviewed = \"1.0.0\"\n"
    ));
    fs::write(&config, text).unwrap();
    let proxy = fake_plugin_proxy();
    let mut tui = h.start_tui_with(|cmd| {
        cmd.env("CLAUDIO_PROXY_URL", format!("tok@{proxy}"));
        cmd.env("XDG_CACHE_HOME", h.root.join("xdg-cache"));
        cmd.env("CLAUDIO_TEST_LOOPBACK_HTTP", "1");
        cmd.env("CLAUDIO_TEST_OLD_DAEMON", "1");
    });
    tui.wait_for("New session", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_for("No sessions", Region::Screen, WAIT);
    // The plugin is listed (its row) before the session starts.
    tui.send_keys(ALT_I);
    tui.wait_for(&format!("[x] {PLUGIN}"), Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &h.dirs[0]);
    let deadline = Instant::now() + WAIT;
    let env = loop {
        let env = fs::read_to_string(&env_file).unwrap_or_default();
        if env.lines().any(|l| l.starts_with("ANTHROPIC_AUTH_TOKEN=")) {
            break env;
        }
        assert!(Instant::now() < deadline, "no session started: {env}");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        env.lines().any(|l| l == "ANTHROPIC_AUTH_TOKEN=tok"),
        "{env}"
    );
    assert!(
        !env.lines()
            .any(|l| l.starts_with(&format!("{PLUGIN_KEY}="))),
        "a plugin secret reached the session: {env}"
    );
    assert!(
        !env.lines().any(|l| l.starts_with("ANTHROPIC_API_KEY=")),
        "the old scrub kept the API key: {env}"
    );
    tui.wait_for(
        "plugins after `claudio daemon restart`",
        Region::Screen,
        WAIT,
    );
    tui.quit(WAIT);
}

// ── Usage limits ─────────────────────────────────────────────────────────────

/// The conversation the usage-limit fake claude reports.
const LIMIT_SID: &str = "5e55-limit-test";

/// A fake claude-proxy on loopback that accepts every `POST
/// /v1/claudio/session/switch` (202, from `sub-a` to `sub-b`) and sends each
/// one's `authorization` header and body down the channel; every other path
/// is a 404. Returns its `host:port`.
fn fake_switch_proxy() -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake proxy");
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let header = |name: &str| {
                head.lines().find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                })
            };
            let len = header("content-length").and_then(|v| v.parse().ok());
            let mut body = vec![0u8; len.unwrap_or(0).min(4096)];
            let _ = stream.read_exact(&mut body);
            let (status, reply) = if head.starts_with("POST /v1/claudio/session/switch ") {
                let body = String::from_utf8_lossy(&body).into_owned();
                let _ = tx.send((header("authorization").unwrap_or_default(), body));
                let reply = serde_json::json!({"session_id": LIMIT_SID, "state": "pending",
                    "from": {"id": "c1", "label": "sub-a"}, "to": {"id": "c2", "label": "sub-b"}});
                ("202 Accepted", reply.to_string())
            } else {
                ("404 Not Found", "{}".to_owned())
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    (addr, rx)
}

/// A fake claude whose turn ends on a usage limit: a second after its banner
/// it reports conversation [`LIMIT_SID`] (`SessionStart`) and then the limit
/// (`StopFailure`), through the hook commands of its merged `--settings`, as
/// claude does; then it echoes each line it reads as `GOT <line>`.
fn write_limit_claude(h: &ManagerHarness) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
[ "$1" = "--help" ] && exit 0
echo "FAKE_CLAUDE_BANNER pid=$$"
START=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart [^"]*\)".*/\1/p')
FAIL=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook StopFailure [^"]*\)".*/\1/p')
sleep 1
printf '{{"session_id":"{LIMIT_SID}","source":"startup"}}' | eval "$START"
eval "$FAIL" <<'JSON'
{{"error":"rate_limit","last_assistant_message":"You've hit your 5-hour limit · resets 4pm (UTC)"}}
JSON
while IFS= read -r line; do printf 'GOT %s\n' "$line"; done
"#
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(&h.fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
}

/// AC4: a proxy session whose turn ends on a usage limit is moved by its
/// host's daemon, with no one asking: the fake proxy gets the switch for the
/// session's conversation with the profile's token, the session gets
/// `continue` and Enter, and the TUI shows the notice. The token is in no
/// notice and no daemon log line.
#[test]
fn test_a_usage_limit_switches_the_subscription_and_continues() {
    let h = ManagerHarness::new();
    write_limit_claude(&h);
    let (proxy, switches) = fake_switch_proxy();
    let config = h.config_home.join("claudio/config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "[proxy]\ndefault = \"work\"\n[proxy.profiles.work]\n\
         url = \"http://{proxy}\"\ntoken = \"work-test-token\"\n"
    ));
    fs::write(&config, text).unwrap();
    let mut tui = h.start_tui();
    tui.wait_for("New session", Region::Screen, WAIT);
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    let (auth, body) = switches
        .recv_timeout(DAEMON_WAIT)
        .expect("no switch request reached the proxy");
    assert_eq!(auth, "Bearer work-test-token");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body, serde_json::json!({ "id": LIMIT_SID }));
    // The notice first: it leaves the status bar after a few seconds.
    tui.wait_for(
        "usage limit: switched sub-a → sub-b, continuing (1/3)",
        Region::StatusBar,
        WAIT,
    );
    tui.wait_for("GOT continue", Region::Pane, WAIT);
    assert!(switches.try_recv().is_err(), "one switch per limit hit");
    assert!(!tui.screen_text(Region::Screen).contains("work-test-token"));
    let log = fs::read_to_string(h.runtime_dir.join("claudio/daemon.log")).unwrap_or_default();
    assert!(
        !log.contains("work-test-token"),
        "the token in the daemon log"
    );
    tui.quit(WAIT);
}
