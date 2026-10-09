//! Integration tests for the claudio session manager.
//!
//! Drives the real binary inside a PTY (via `portable-pty`), against an
//! isolated daemon and a fake `claude` script, so tests run offline with no
//! real API keys.
//!
//! The fake claude prints a unique per-spawn nonce then `exec cat`, which lets
//! tests verify session output and check that a restart produced a NEW nonce
//! (not the pre-restart one — fixing the main finding in review T1).
//!
//! All `wait_for` calls operate on the CURRENT rendered screen (via an
//! alacritty_terminal::Term VT model), never on accumulated raw bytes.
//! All timeouts panic; nothing logs-and-continues (review T2).
//!
//! Run with:
//!   cargo test --test manager -- --test-threads=1
//!   make test-manager
//!
//! For the gated real-claude scenario:
//!   CLAUDIO_E2E=1 cargo test --test manager -- --test-threads=1

mod common;

use std::time::{Duration, Instant};
use std::{fs, thread};

use common::{
    current_nonce, extract_nonce, wizard_pick_dir, ClaudeProjectGuard, ManagerHarness, Region,
    TuiProcess, ALT_C, ALT_G, ALT_H, ALT_LEFT, ALT_N, ALT_Q, ALT_R, ALT_RIGHT, ALT_X, BINARY, CTRL_U,
    DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, RECONNECT_WAIT, UP_ARROW, WAIT,
};
use portable_pty::CommandBuilder;

// ── Test 1: Fresh start ───────────────────────────────────────────────────────

/// 1. Fresh start: no sessions → wizard → session spawns → banner and tab label
///    visible in the current screen; quit exits cleanly.
#[test]
fn test_1_fresh_start_wizard_and_session() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    // Wizard should open (no sessions); pick the first session directory.
    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // The directory basename should appear in the tab bar.
    let dir_name = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    tui.wait_for(dir_name, Region::TabBar, WAIT);

    // Quit cleanly.
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

// ── Test 2: Two sessions, switching ──────────────────────────────────────────

/// 2. Two sessions; Alt+←/→ switches between them.  After switching we verify
///    both tab labels are visible in the current tab bar.
#[test]
fn test_2_two_sessions_switching() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    // First session.
    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // Second session via Alt+n → wizard.
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &harness.dirs[1]);

    // Both session dir names must appear in the tab bar.
    let n0 = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    let n1 = harness.dirs[1].file_name().unwrap().to_str().unwrap();
    tui.wait_for(n0, Region::TabBar, WAIT);
    tui.wait_for(n1, Region::TabBar, WAIT);

    // Switch left (to first session).
    tui.send_keys(ALT_LEFT);
    tui.wait_for(n0, Region::TabBar, WAIT);

    // Switch right (back to second session).
    tui.send_keys(ALT_RIGHT);
    tui.wait_for(n1, Region::TabBar, WAIT);

    tui.quit(WAIT);
}

// ── Test 3: Rename ────────────────────────────────────────────────────────────

/// 3. Alt+r opens a rename modal; typing a new name and pressing Enter makes
///    the tab bar show the name, and state.json contains it.
#[test]
fn test_3_rename() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // Alt+r opens the rename modal.
    tui.send_keys(ALT_R);
    // Ctrl+U clears the input field; wait for the modal to render before typing.
    tui.wait_for("Rename session", Region::Screen, WAIT);
    tui.send_keys(CTRL_U);
    tui.send_keys(b"my-renamed-session");
    tui.send_keys(ENTER);

    // Tab bar should now show the new name.
    tui.wait_for("my-renamed-session", Region::TabBar, WAIT);

    // state.json must persist the name.
    harness.wait_state(
        |v| {
            v.get("sessions")
                .and_then(|s| s.as_array())
                .map(|a| {
                    a.iter().any(|s| {
                        s.get("name").and_then(|n| n.as_str()) == Some("my-renamed-session")
                    })
                })
                .unwrap_or(false)
        },
        WAIT,
    );

    tui.quit(WAIT);
}

// ── Test 4: Quit and reattach ─────────────────────────────────────────────────

/// 4. Alt+q leaves the daemon running; restarting the TUI restores sessions
///    (banner visible from the screen snapshot).
#[test]
fn test_4_quit_and_reattach() {
    let harness = ManagerHarness::new();

    // First launch: create a session.
    {
        let mut tui = harness.start_tui();
        wizard_pick_dir(&mut tui, &harness.dirs[0]);
        tui.send_keys(ALT_Q);
        tui.wait_exit(WAIT);
    }

    // Daemon must still be alive.
    assert!(
        harness.daemon_pid().is_some(),
        "daemon should still be running after quit"
    );

    // Second launch: reattach; banner must come back from the screen snapshot.
    let tui2 = harness.start_tui();
    tui2.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, DAEMON_WAIT);

    let dir_name = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    tui2.wait_for(dir_name, Region::TabBar, WAIT);

    // TuiProcess::drop kills the child.
}

// ── Test 5: Daemon restart ────────────────────────────────────────────────────

/// 5. SIGTERM the daemon; the TUI reconnects and re-spawns dormant sessions.
///    The restart test requires the NEW nonce (not the pre-restart one), fixing
///    the vacuous match identified in review T1.
#[test]
fn test_5_daemon_restart() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // Record the pre-restart nonce.
    let old_nonce = current_nonce(&tui);

    // Kill the daemon.
    harness.kill_daemon();

    // The TUI should reconnect and re-spawn the session.
    // We require the NEW nonce to appear (different from the old one).
    tui.wait_until(
        |screen| {
            let text = screen.region_text(Region::Pane);
            extract_nonce(&text).map_or(false, |n| n != old_nonce)
        },
        RECONNECT_WAIT,
    );

    tui.quit(WAIT);
}

// ── Test 6: Close session ─────────────────────────────────────────────────────

/// 6. Alt+x + y removes the session from the tab bar and state.json.
#[test]
fn test_6_close_session() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // Alt+x opens the close-confirm modal; 'y' confirms.
    tui.send_keys(ALT_X);
    tui.wait_for("Kill session", Region::Screen, WAIT);
    tui.send_keys(b"y");
    // Wait for the modal to dismiss before sending ESC to the wizard.
    tui.wait_until(|s| !s.contains("Kill session", Region::Screen), WAIT);

    // After closing the only session the wizard opens again; dismiss it.
    // Wait until the wizard is actually closed before quitting.
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("New session", Region::Screen), WAIT);

    wait_session_forgotten(&harness, &harness.dirs[0]);
    tui.quit(WAIT);
}

/// 6b. When claude exits cleanly (Ctrl+D twice, `/exit`) the tab closes on
///     its own, without the confirm modal. The fake claude is `cat`, which a
///     single ^D ends with status 0.
#[test]
fn test_6b_clean_exit_closes_session() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    tui.send_keys(b"\x04");

    // The only session is gone, so the wizard opens again.
    tui.wait_for("session ended", Region::Screen, WAIT);
    tui.wait_for("New session", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("New session", Region::Screen), WAIT);

    wait_session_forgotten(&harness, &harness.dirs[0]);
    tui.quit(WAIT);
}

/// Wait until state.json no longer lists a session in `dir`.
fn wait_session_forgotten(harness: &ManagerHarness, dir: &std::path::Path) {
    let dir = dir.to_str().unwrap();
    harness.wait_state(
        |v| match v.get("sessions").and_then(|s| s.as_array()) {
            None => true, // empty/absent sessions section
            Some(a) => !a
                .iter()
                .any(|s| s.get("cwd").and_then(|c| c.as_str()) == Some(dir)),
        },
        WAIT,
    );
}

// ── Test 7: Input echo ────────────────────────────────────────────────────────

/// 7. Keystrokes reach the session PTY and are echoed back (fake claude runs
///    `exec cat` which echoes stdin).
#[test]
fn test_7_input_echo() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // Type a unique phrase; `exec cat` echoes lines back.
    let phrase = "hello-from-claudio-test";
    tui.send_keys(phrase.as_bytes());
    tui.send_keys(ENTER);

    // The echo must appear in the rendered pane.
    tui.wait_for(phrase, Region::Pane, WAIT);

    tui.quit(WAIT);
}

// ── P4: Overview ──────────────────────────────────────────────────────────────

/// Overview popup (Alt+g) lists both sessions; ↑ navigates; Enter switches.
#[test]
fn test_overview_opens_lists_and_switches() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &harness.dirs[1]);

    let dir1_name = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    let dir2_name = harness.dirs[1].file_name().unwrap().to_str().unwrap();
    tui.wait_for(dir1_name, Region::TabBar, WAIT);
    tui.wait_for(dir2_name, Region::TabBar, WAIT);

    // Open overview.
    tui.send_keys(ALT_G);
    // Both session names should be in the overview popup.
    tui.wait_for(dir1_name, Region::Screen, WAIT);
    tui.wait_for(dir2_name, Region::Screen, WAIT);

    // Navigate up to select session 0, then Enter to switch.
    tui.send_keys(UP_ARROW);
    tui.send_keys(ENTER);

    // Overview closes; session 1 tab label should still be in the tab bar.
    tui.wait_for(dir1_name, Region::TabBar, WAIT);

    tui.quit(WAIT);
}

// ── P4: Help ──────────────────────────────────────────────────────────────────

/// Help popup (Alt+h) shows key bindings; Esc closes it.
#[test]
fn test_help_popup_opens_and_closes() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    tui.send_keys(ALT_H);
    // The popup should show a binding description.
    tui.wait_for("previous session", Region::Screen, WAIT);
    // Wizard-scoped bindings are listed too.
    tui.wait_for("Alt+.", Region::Screen, WAIT);
    tui.wait_for("hidden dirs", Region::Screen, WAIT);
    // The "any key closes" string appears in the popup title/footer.
    tui.wait_for("any key closes", Region::Screen, WAIT);

    // Close with Esc.  Wait until the popup's "any key closes" text is gone
    // before checking the status bar — the status bar may already contain
    // "Alt+q" while the popup is still rendered over it.
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("any key closes", Region::Screen), WAIT);

    // Status bar hints should be back.
    tui.wait_for("Alt+h", Region::StatusBar, WAIT);

    tui.quit(WAIT);
}

// ── Wizard: hidden directories ────────────────────────────────────────────────

/// The directory explorer hides dot-directories by default; Alt+. reveals them.
#[test]
fn test_wizard_hidden_dirs_toggle() {
    let harness = ManagerHarness::new();
    let browse = harness.root.join("browse");
    fs::create_dir_all(browse.join(".secret-dir")).expect("create hidden dir");
    fs::create_dir_all(browse.join("visible-dir")).expect("create visible dir");
    let mut tui = harness.start_tui();

    // Host step: "Explore local dirs…" (pre-selected). Then browse `browse/`.
    tui.send_keys(ENTER);
    tui.wait_for("Tab complete", Region::Screen, WAIT);
    tui.send_paste(&format!("{}/", browse.display()));

    // Listing arrived: the visible dir is shown, the hidden one is not, and the
    // state line says so.
    tui.wait_for("visible-dir", Region::Screen, WAIT);
    tui.wait_for("Alt+. hidden: off", Region::Screen, WAIT);
    assert!(
        !tui.screen_text(Region::Screen).contains(".secret-dir"),
        "hidden dir must not be listed by default"
    );

    // Alt+. (ESC .) reveals it.
    tui.send_keys(b"\x1b.");
    tui.wait_for(".secret-dir", Region::Screen, WAIT);
    tui.wait_for("Alt+. hidden: on", Region::Screen, WAIT);
    tui.wait_for("visible-dir", Region::Screen, WAIT);

    // Pressing it again hides it once more.
    tui.send_keys(b"\x1b.");
    tui.wait_until(|s| !s.contains(".secret-dir", Region::Screen), WAIT);
    tui.wait_for("Alt+. hidden: off", Region::Screen, WAIT);

    tui.quit(WAIT);
}

// ── P4: daemon CLI ────────────────────────────────────────────────────────────

/// `claudio daemon status` reports "running" and a session count.
#[test]
fn test_daemon_status_cmd() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();
    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    let out = std::process::Command::new(BINARY)
        .arg("daemon")
        .arg("status")
        .env("XDG_RUNTIME_DIR", &harness.runtime_dir)
        .env("XDG_CONFIG_HOME", &harness.config_home)
        .env("HOME", &harness.home)
        .env("CLAUDIO_CLAUDE_PATH", &harness.fake_claude)
        .output()
        .expect("claudio daemon status failed to spawn");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "daemon status should exit 0\nstdout: {stdout}"
    );
    assert!(
        stdout.contains("running"),
        "should say 'running'\nstdout: {stdout}"
    );
    assert!(
        stdout.contains("sessions:"),
        "should show session count\nstdout: {stdout}"
    );

    tui.quit(WAIT);
}

/// `claudio daemon stop` terminates the daemon; `claudio daemon restart` brings
/// it back with a new PID.  The TUI reconnects and re-spawns with a NEW nonce.
#[test]
fn test_daemon_stop_and_restart_cmd() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();
    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    let pid_before = harness.daemon_pid().expect("daemon should have a pid");
    let old_nonce = current_nonce(&tui);

    // Stop the daemon.
    let stop_out = std::process::Command::new(BINARY)
        .args(["daemon", "stop"])
        .env("XDG_RUNTIME_DIR", &harness.runtime_dir)
        .env("XDG_CONFIG_HOME", &harness.config_home)
        .env("HOME", &harness.home)
        .env("CLAUDIO_CLAUDE_PATH", &harness.fake_claude)
        .output()
        .expect("claudio daemon stop failed to spawn");
    let stop_stdout = String::from_utf8_lossy(&stop_out.stdout);
    assert!(
        stop_out.status.success(),
        "daemon stop should exit 0\nstdout: {stop_stdout}"
    );
    assert!(
        stop_stdout.contains("dormant"),
        "should mention dormant sessions\nstdout: {stop_stdout}"
    );

    // Restart.
    let restart_out = std::process::Command::new(BINARY)
        .args(["daemon", "restart"])
        .env("XDG_RUNTIME_DIR", &harness.runtime_dir)
        .env("XDG_CONFIG_HOME", &harness.config_home)
        .env("HOME", &harness.home)
        .env("CLAUDIO_CLAUDE_PATH", &harness.fake_claude)
        .env("ANTHROPIC_API_KEY", "test-key")
        .output()
        .expect("claudio daemon restart failed to spawn");
    let restart_stdout = String::from_utf8_lossy(&restart_out.stdout);
    assert!(
        restart_out.status.success(),
        "daemon restart should exit 0\nstdout: {restart_stdout}"
    );

    // New daemon should have a new PID.
    // Allow a moment for the new lock file to be written.
    let new_pid = {
        let dl = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(p) = harness.daemon_pid() {
                if p != pid_before {
                    break Some(p);
                }
            }
            assert!(Instant::now() < dl, "new daemon PID not seen after restart");
            thread::sleep(Duration::from_millis(100));
        }
    };
    assert!(
        new_pid.is_some(),
        "daemon should have a new pid after restart"
    );

    // The TUI must reconnect and re-spawn with a NEW nonce.
    tui.wait_until(
        |screen| {
            let text = screen.region_text(Region::Pane);
            extract_nonce(&text).map_or(false, |n| n != old_nonce)
        },
        RECONNECT_WAIT,
    );

    tui.quit(WAIT);
}

// ── SSH TUI test ──────────────────────────────────────────────────────────────

/// Gated by `CLAUDIO_SSH_TEST_HOST`. Drives the full wizard against a real SSH
/// host, verifies the tab label contains @host, and verifies `hosts.json` lists
/// the host first.  Kills the session before quitting so nothing persists on the remote host.
///
/// The remote host must have `claude` installed and SSH key auth (BatchMode).
#[test]
fn test_ssh_remote_session() {
    let host = match std::env::var("CLAUDIO_SSH_TEST_HOST") {
        Ok(h) if !h.is_empty() => h,
        _ => {
            println!("SKIP test_ssh_remote_session: set CLAUDIO_SSH_TEST_HOST to run");
            return;
        }
    };

    // Isolate claudio state; keep real HOME so SSH keys are available.
    let root = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        std::time::SystemTime::now()
            .elapsed()
            .unwrap_or_default()
            .subsec_nanos()
            .hash(&mut h);
        std::env::temp_dir().join(format!("cl-ssh-{:08x}", h.finish() as u32))
    };
    let runtime_dir = root.join("run");
    let config_home = root.join("cfg");
    let hosts_json = config_home.join("claudio").join("hosts.json");
    for p in [&runtime_dir, &config_home] {
        fs::create_dir_all(p).expect("create test dir");
    }

    // ── Create a unique remote temp directory ────────────────────────────────
    // mktemp -d gives us an isolated dir so we never touch anything else in /tmp.
    let remote_tmp = std::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=30",
            &host,
            "mktemp -d /tmp/cl-ssh-XXXXXX",
        ])
        .output()
        .expect("ssh mktemp -d failed");
    assert!(
        remote_tmp.status.success(),
        "ssh mktemp -d failed on {host}: {}",
        String::from_utf8_lossy(&remote_tmp.stderr)
    );
    let remote_dir = String::from_utf8(remote_tmp.stdout)
        .expect("mktemp output is not UTF-8")
        .trim()
        .to_owned();
    assert!(
        remote_dir.starts_with("/tmp/cl-ssh-"),
        "unexpected mktemp output: {remote_dir:?}"
    );
    // The encoded project-dir name: Claude replaces non-alphanumeric chars with '-'.
    // e.g. /tmp/cl-ssh-abc123 → -tmp-cl-ssh-abc123
    let encoded_project = remote_dir
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>();

    // Cleanup guard.
    let runtime_dir_g = runtime_dir.clone();
    let cleanup_root = root.clone();
    let host_g = host.clone();
    let remote_dir_g = remote_dir.clone();
    let encoded_g = encoded_project.clone();
    let _guard = scopeguard(move || {
        let lock = runtime_dir_g.join("claudio").join("daemon-v1.lock");
        if let Some(pid) = fs::read_to_string(&lock)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let _ = fs::remove_dir_all(&cleanup_root);
        // Remove ONLY the unique remote dir and its encoded project entry.
        // $HOME expands on the remote; single-quote the literal paths.
        let cleanup_cmd = format!(
            r#"rm -rf "$HOME/.claude/projects/{encoded_g}" {remote_dir_g_q}"#,
            encoded_g = encoded_g,
            remote_dir_g_q = shell_quote_ssh(&remote_dir_g),
        );
        let _ = std::process::Command::new("ssh")
            .args([
                "-o", "BatchMode=yes",
                "-o", "ConnectTimeout=10",
                &host_g,
                &cleanup_cmd,
            ])
            .status();
    });

    let mut cmd = CommandBuilder::new(BINARY);
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("XDG_CONFIG_HOME", &config_home);
    // Keep real HOME for SSH keys and known_hosts.
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("COLORTERM");
    // Never check for updates during the SSH test.
    cmd.env("CLAUDIO_NO_UPDATE_CHECK", "1");
    // Clear any inherited CLAUDIO_* vars.
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDIO_") {
            cmd.env_remove(&k);
        }
    }

    let mut tui = TuiProcess::spawn(cmd);

    // ── 1. Wait for TUI to start. ────────────────────────────────────────────
    tui.wait_for("Alt+h help", Region::StatusBar, Duration::from_secs(30));

    // ── 2. Host step: type the hostname and press Enter. ─────────────────────
    tui.send_keys(host.as_bytes());
    tui.send_keys(ENTER);

    // Bootstrap may upload the binary first; a debug build is ~150 MB, so give
    // the first run after a rebuild plenty of time.
    let dir_step = format!("on {host}");
    tui.wait_for(&dir_step, Region::Screen, Duration::from_secs(300));

    // ── 3. Directory step: paste the unique remote dir and confirm. ──────────
    tui.send_paste(&remote_dir);
    tui.send_keys(ENTER);

    // ── 4. Resume step or auto-spawn. ────────────────────────────────────────
    // If /tmp has no claude sessions it auto-spawns (tab appears directly).
    // If /tmp has existing sessions a resume picker appears; pressing Enter
    // selects "+ New session" (always the first item).
    //
    // Wait up to 120 s for the newly spawned session to become active.
    // Handles:
    //  - claude's trust dialog (press Down then Enter to pick "Yes, I trust")
    //  - the wizard's resume/new picker (press Enter)
    //  - pre-existing @host tabs (don't break until the new session is ready)
    let at_host = format!("@{host}");
    let deadline_spawn = Instant::now() + Duration::from_secs(120);
    let mut trust_pressed = false;
    loop {
        let screen = tui.screen_text(Region::Screen);
        let wizard_visible =
            screen.contains("New session") || screen.contains("Resume");
        // ① Wizard session picker: always handle first so Down/Enter go to the
        //    wizard, not to the session's trust dialog behind it.
        if wizard_visible {
            tui.send_keys(ENTER);
            thread::sleep(Duration::from_millis(500)); // wait for wizard to close
            continue;
        }
        // ② Claude trust dialog (only once wizard is gone).
        //    Default cursor is "❯ No, exit"; Down moves to "Yes, I trust".
        if !trust_pressed && screen.contains("Yes, I trust this folder") {
            tui.send_keys(DOWN_ARROW);
            thread::sleep(Duration::from_millis(300));
            tui.send_keys(ENTER);
            trust_pressed = true;
            thread::sleep(Duration::from_millis(1000)); // wait for dialog to dismiss
            continue;
        }
        // ③ Session is ready: @host in tab bar, no longer starting, no trust dialog.
        if screen.contains(&at_host)
            && !screen.contains("starting")
            && !screen.contains("Yes, I trust")
        {
            break;
        }
        assert!(
            Instant::now() < deadline_spawn,
            "timeout waiting for remote session to start;\nscreen:\n{}",
            screen
        );
        thread::sleep(Duration::from_millis(200));
    }

    // ── 5. Wait for tab to show @host and assert the dir step showed host. ───
    // The wait_for("on {host}") in step 2 already asserted the dir step title.
    tui.wait_for(&at_host, Region::TabBar, Duration::from_secs(90));

    // ── 6. Assert hosts.json lists the host first. ───────────────────────────
    let hosts_ok = {
        let dl = Instant::now() + Duration::from_secs(5);
        let mut ok = false;
        while Instant::now() < dl {
            if let Ok(content) = fs::read_to_string(&hosts_json) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    let first = v["hosts"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|h| h.as_str());
                    if first == Some(host.as_str()) {
                        ok = true;
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        ok
    };
    assert!(hosts_ok, "hosts.json should list {host} first");

    // ── 7. Kill the session so nothing persists remotely. ────────────────────
    tui.send_keys(ALT_X);
    // Wait for the close-confirm modal.
    tui.wait_for("Kill session", Region::Screen, WAIT);
    tui.send_keys(b"y");
    // The remote Kill may take time over SSH — use a generous timeout.
    tui.wait_until(
        |s| !s.contains("Kill session", Region::Screen),
        RECONNECT_WAIT,
    );

    // ── 8. Quit cleanly. ─────────────────────────────────────────────────────
    // ALT_Q is a global quit key — it works even if the wizard re-opened after
    // the last session was closed.  The SSH cleanup (bridge shutdown) may take
    // up to 30 s on slow links, so give a generous timeout.
    tui.send_keys(ALT_Q);
    tui.wait_exit(Duration::from_secs(30));
}

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
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../claude-proxy"));
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

    // ── 4. Isolated env with an env-dumping fake claude ───────────────────────
    let harness = ManagerHarness::new();
    let env_file = harness.root.join("claude-env.txt");
    // Overwrite fake claude: dump env (but not on --version), then banner+cat.
    harness.write_env_dumping_fake_claude(&env_file);

    // ── 5. Start claudio TUI with CLAUDIO_PROXY_URL ────────────────────────────
    let mut tui = harness.start_tui_with(|cmd| {
        cmd.env("CLAUDIO_PROXY_URL", &proxy_url);
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
    assert_eq!(
        env_get("CLAUDE_CODE_USE_GATEWAY").as_deref(),
        Some("1"),
        "CLAUDE_CODE_USE_GATEWAY not set"
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
            extract_nonce(&text).map_or(false, |n| n != old_nonce)
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

    tui2.quit(WAIT);
}

// ── Real-claude gate ───────────────────────────────────────────────────────────

/// Gated by `CLAUDIO_E2E=1`.  Sends a prompt whose expected answer does NOT
/// appear in the prompt text (`4242 + 1 = 4243`), so matching `4243` in the
/// pane proves the answer is from claude, not the echoed prompt.
#[test]
fn test_e2e_real_claude() {
    if std::env::var("CLAUDIO_E2E").as_deref() != Ok("1") {
        return;
    }

    let root = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        std::time::SystemTime::now()
            .elapsed()
            .unwrap_or_default()
            .subsec_nanos()
            .hash(&mut h);
        std::env::temp_dir().join(format!("cl-e2e-{:08x}", h.finish() as u32))
    };
    let runtime_dir = root.join("run");
    let config_home = root.join("cfg");
    let session_dir = root.join("sess");
    for p in [&runtime_dir, &config_home, &session_dir] {
        fs::create_dir_all(p).unwrap();
    }

    let runtime_dir_g = runtime_dir.clone();
    let cleanup_root = root.clone();
    let session_dir_g = session_dir.clone();
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
    // Isolate claudio state; keep real HOME so claude finds ~/.claude.
    cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
    cmd.env("XDG_CONFIG_HOME", &config_home);
    cmd.env("ANTHROPIC_MODEL", "claude-haiku-4-5");
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("COLORTERM");
    // Clear CLAUDIO_* but keep real HOME for credentials.
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDIO_") {
            cmd.env_remove(&k);
        }
    }

    let mut tui = TuiProcess::spawn(cmd);

    // ── 1. Wait for TUI to start. ────────────────────────────────────────────
    tui.wait_for("Alt+h help", Region::StatusBar, Duration::from_secs(30));

    // ── 2. Wizard: local host, then session directory. ───────────────────────
    tui.send_keys(ENTER);
    thread::sleep(Duration::from_millis(80));
    tui.send_paste(session_dir.to_str().unwrap());
    tui.send_keys(ENTER);

    // ── 3. Wait for claude to be ready (handle trust dialog). ────────────────
    // Use the same dual-signal readiness check as test_proxy_real_claude:
    // wait for claude's input prompt (❯) in the pane AND the SessionStart
    // hook's `?` needs-input glyph in the tab bar, so no keys are lost.
    let ready_dl = Instant::now() + Duration::from_secs(90);
    let mut trust_pressed = false;
    loop {
        let screen = tui.screen_text(Region::Pane);
        if !trust_pressed && screen.contains("Do you trust") {
            tui.send_keys(ENTER);
            trust_pressed = true;
            continue;
        }
        let tabs = tui.screen_text(Region::TabBar);
        if screen.contains("❯") && tabs.contains('?') {
            break;
        }
        assert!(
            Instant::now() < ready_dl,
            "timeout waiting for claude to be ready; pane:\n{screen}\ntabs:\n{tabs}"
        );
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(500));
    let _ = session_dir_g; // keep alive until readiness check

    // ── 4. Send prompt: the expected answer (4243) is NOT in the prompt. ─────
    tui.send_paste("Reply with only the number 4242 plus 1");
    tui.send_keys(ENTER);

    // ── 5. Assert the answer 4243 appears in the pane (not in the prompt). ───
    tui.wait_for("4243", Region::Pane, Duration::from_secs(90));

    // ── 6. Session should return to idle (✓ glyph). ──────────────────────────
    tui.wait_for("✓", Region::Screen, Duration::from_secs(30));

    // ── 7. Quit cleanly. ──────────────────────────────────────────────────────
    tui.quit(WAIT);
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

// ── Soak test ─────────────────────────────────────────────────────────────────

/// Soak test: stay idle for 75 s (longer than the 45 s liveness deadline) then
/// verify the session is still attached and input still echoes.
///
/// Gated by `CLAUDIO_SOAK=1` because it takes 75 real seconds.
#[test]
fn test_soak_connection_stays_alive() {
    if std::env::var("CLAUDIO_SOAK").as_deref() != Ok("1") {
        return; // skip unless explicitly requested
    }

    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    // Create a session backed by the fake claude (exec cat).
    wizard_pick_dir(&mut tui, &harness.dirs[0]);

    // Verify the session is up.
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);

    // Stay idle for 75 s — longer than LIVENESS_DEADLINE (45 s).
    // The heartbeat must keep the connection alive during this time.
    thread::sleep(Duration::from_secs(75));

    // Verify no reconnecting or disconnected notice.
    let status = tui.screen_text(Region::StatusBar);
    assert!(
        !status.contains("reconnect") && !status.contains("disconnected"),
        "status bar should not show reconnect/disconnected after 75 s: {status:?}"
    );

    // Session should still be attached (not showing a reconnecting spinner).
    let pane = tui.screen_text(Region::Pane);
    assert!(
        !pane.contains("reconnecting") && !pane.contains("Connecting"),
        "pane should not show reconnecting after 75 s: {pane:?}"
    );

    // Input must still echo — the connection is alive.
    let echo_phrase = "soak-echo-ok";
    tui.send_keys(echo_phrase.as_bytes());
    tui.send_keys(ENTER);
    tui.wait_for(echo_phrase, Region::Pane, WAIT);

    tui.quit(WAIT);
}

// ── Additional status bar and tab tests ──────────────────────────────────────

/// Status bar shows cpu sparkline and mem usage for a running local session.
#[test]
fn test_status_bar_machine_stats() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();
    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    // Wait up to 6s for the cpu sparkline and mem to appear in the status bar.
    tui.wait_for("cpu", Region::StatusBar, Duration::from_secs(6));
    tui.wait_for("mem", Region::StatusBar, Duration::from_secs(6));
    tui.quit(WAIT);
}

/// Tab label for a fake-claude session is the cwd basename, not "Claude Code".
#[test]
fn test_tab_label_is_basename() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();
    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    let dir_name = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    tui.wait_for(dir_name, Region::TabBar, WAIT);
    // Verify "Claude Code" does NOT appear in the tab bar.
    let tab_text = tui.screen_text(Region::TabBar);
    assert!(!tab_text.contains("Claude Code"), "tab should not say 'Claude Code', got: {tab_text:?}");
    tui.quit(WAIT);
}

// ── Terminal tabs ─────────────────────────────────────────────────────────────

/// Alt+c opens a `$ term@local` tab right after the active one (not at the
/// end), and a real shell runs in it.
#[test]
fn test_terminal_tab_opens_next_to_active_and_runs_commands() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &harness.dirs[1]);
    let n0 = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    let n1 = harness.dirs[1].file_name().unwrap().to_str().unwrap();
    tui.wait_for(n1, Region::TabBar, WAIT);

    // Back to the first tab, so "next to the active tab" differs from "last".
    tui.send_keys(ALT_LEFT);
    tui.send_keys(ALT_C);
    tui.wait_for("$ term@local", Region::TabBar, WAIT);
    let tabs = tui.screen_text(Region::TabBar);
    let at = |s: &str| tabs.find(s).unwrap_or_else(|| panic!("{s:?} not in {tabs:?}"));
    assert!(
        at(n0) < at("term@local") && at("term@local") < at(n1),
        "terminal must sit between the two sessions: {tabs:?}"
    );

    // A terminal's status line is minimal: no state, model or proxy.
    tui.wait_for("$ terminal", Region::StatusBar, WAIT);
    let status = tui.screen_text(Region::StatusBar);
    assert!(!status.contains("direct") && !status.contains("idle"), "{status:?}");

    // The shell is live. 6*7 only appears in the output, never in the typed line.
    tui.wait_for("$", Region::Pane, WAIT);
    tui.send_keys(b"echo hi-from-term-$((6*7))\r");
    tui.wait_for("hi-from-term-42", Region::Pane, WAIT);

    // Alt+x confirms like for claude, then the tab is gone.
    tui.send_keys(ALT_X);
    tui.wait_for("Kill session", Region::Screen, WAIT);
    tui.send_keys(b"y");
    tui.wait_until(|s| !s.contains("term@local", Region::TabBar), WAIT);

    tui.quit(WAIT);
}

/// A terminal survives a daemon restart as a *shell* (fresh process, same
/// tab), and claude is never launched for it.
#[test]
fn test_terminal_survives_daemon_restart_as_a_shell() {
    let harness = ManagerHarness::new();
    let claude_ran = harness.root.join("claude-ran");
    harness.write_marker_fake_claude(&claude_ran);
    let mut tui = harness.start_tui();

    // No claude session: dismiss the wizard, then Alt+c opens a terminal in $HOME.
    tui.wait_for("New session", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("New session", Region::Screen), WAIT);
    tui.send_keys(ALT_C);
    tui.wait_for("$ term@local", Region::TabBar, WAIT);
    tui.wait_for("$", Region::Pane, WAIT);
    tui.send_keys(b"echo before-$((6*7))\r");
    tui.wait_for("before-42", Region::Pane, WAIT);

    harness.kill_daemon();

    // The new daemon respawns it: the old screen is gone, a new shell prompts.
    tui.wait_until(|s| !s.contains("before-42", Region::Pane), RECONNECT_WAIT);
    tui.wait_for("$", Region::Pane, RECONNECT_WAIT);
    tui.send_keys(b"echo after-$((8*8))\r");
    tui.wait_for("after-64", Region::Pane, WAIT);
    tui.wait_for("$ term@local", Region::TabBar, WAIT);

    assert!(!claude_ran.exists(), "claude must never run for a terminal");
    tui.quit(WAIT);
}

// ── Scope guard ───────────────────────────────────────────────────────────────

/// Minimal scope guard: runs `f` when dropped.
struct ScopeGuard<F: FnOnce()>(Option<F>);
impl<F: FnOnce()> Drop for ScopeGuard<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}
fn scopeguard<F: FnOnce()>(f: F) -> ScopeGuard<F> {
    ScopeGuard(Some(f))
}

/// Single-quote a string for safe inclusion in an SSH remote command.
/// Any embedded single quote is escaped as `'\''`.
fn shell_quote_ssh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
