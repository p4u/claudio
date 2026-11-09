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
    current_nonce, encode_cwd, extract_nonce, run_git, wizard_pick_dir, ClaudeProjectGuard,
    ManagerHarness, Region, TuiProcess, ALT_C, ALT_E, ALT_G, ALT_H, ALT_L, ALT_LEFT, ALT_N, ALT_Q,
    ALT_R, ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2, ALT_U, ALT_X, BINARY, CTRL_U, DAEMON_WAIT,
    DOWN_ARROW, ENTER, ESC, LEFT_ARROW, RECONNECT_WAIT, RIGHT_ARROW, UP_ARROW, WAIT,
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

/// 2b. Alt+Shift+<digit> (kitty keyboard protocol encoding) jumps to that tab.
///     Each session is told apart by its per-spawn banner nonce in the pane.
#[test]
fn test_2b_alt_shift_digit_goes_to_session() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    let nonce1 = current_nonce(&tui);
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &harness.dirs[1]);
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Pane)).is_some_and(|n| n != nonce1),
        WAIT,
    );
    let nonce2 = current_nonce(&tui);

    // Alt+Shift+1 → first tab.
    tui.send_keys(ALT_SHIFT_1);
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Pane)).as_deref() == Some(nonce1.as_str()),
        WAIT,
    );
    // Alt+Shift+2 → second tab again.
    tui.send_keys(ALT_SHIFT_2);
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Pane)).as_deref() == Some(nonce2.as_str()),
        WAIT,
    );

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
            extract_nonce(&text).is_some_and(|n| n != old_nonce)
        },
        RECONNECT_WAIT,
    );

    tui.quit(WAIT);
}

/// 5b. A daemon left running by an older claudio is replaced, silently, when
///     a newer one starts and the sessions are idle: the session comes back
///     (a new process, reattached) and the host's CPU/memory chart shows.
///     The "older claudio" is a copy of this binary with a changed hash and
///     an older file time.
#[test]
fn test_5b_outdated_daemon_is_replaced() {
    let harness = ManagerHarness::new();
    let old_bin = harness.root.join("claudio-old");
    let mut bytes = fs::read(BINARY).unwrap();
    bytes.extend_from_slice(b"\0an older build");
    fs::write(&old_bin, bytes).unwrap();
    let old_file = fs::File::options().write(true).open(&old_bin).unwrap();
    let day_ago = std::time::SystemTime::now() - Duration::from_secs(86_400);
    old_file.set_modified(day_ago).unwrap();
    drop(old_file);
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&old_bin, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // The older claudio starts its daemon, with one session, and quits.
    let mut cmd = CommandBuilder::new(&old_bin);
    harness.apply(&mut cmd);
    let mut old_tui = TuiProcess::spawn(cmd);
    old_tui.wait_for("Alt+h help", Region::StatusBar, DAEMON_WAIT);
    wizard_pick_dir(&mut old_tui, &harness.dirs[0]);
    let old_nonce = current_nonce(&old_tui);
    old_tui.send_keys(ALT_Q);
    old_tui.wait_exit(WAIT);
    let old_pid = harness.daemon_pid().expect("the old daemon runs");
    #[cfg(target_os = "linux")]
    assert_eq!(
        fs::read_link(format!("/proc/{old_pid}/exe")).unwrap(),
        old_bin
    );

    // This claudio replaces it, and the session is back.
    let tui = harness.start_tui();
    tui.wait_until(
        |screen| {
            let text = screen.region_text(Region::Pane);
            extract_nonce(&text).is_some_and(|n| n != old_nonce)
        },
        DAEMON_WAIT,
    );
    let dir_name = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    tui.wait_for(dir_name, Region::TabBar, WAIT);
    let new_pid = harness.daemon_pid().unwrap();
    assert_ne!(new_pid, old_pid);
    #[cfg(target_os = "linux")]
    assert_eq!(
        fs::read_link(format!("/proc/{new_pid}/exe")).unwrap(),
        fs::canonicalize(BINARY).unwrap()
    );
    tui.wait_for("cpu", Region::StatusBar, DAEMON_WAIT);
    // No prompt, no notice about it.
    let screen = tui.screen_text(Region::StatusBar);
    assert!(!screen.contains("daemon"), "{screen}");
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

// ── Copy on select ────────────────────────────────────────────────────────────

/// A mouse drag over the fake claude's banner (which takes no mouse) selects
/// and copies it: the status bar says how much. claudio enables SGR mouse
/// reporting, so the terminal sends `ESC [ < b ; x ; y M/m`, 1-based.
#[test]
fn test_7b_drag_copies_the_selection() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);

    // The banner is the pane's first row: screen row 2 (1-based), below the
    // tab bar. Drag over its first 18 columns, "FAKE_CLAUDE_BANNER".
    tui.send_keys(b"\x1b[<0;1;2M");
    tui.send_keys(b"\x1b[<32;10;2M");
    tui.send_keys(b"\x1b[<32;18;2M");
    tui.send_keys(b"\x1b[<0;18;2m");
    tui.wait_for("copied 18 chars", Region::StatusBar, WAIT);

    tui.quit(WAIT);
}

// ── Overview ──────────────────────────────────────────────────────────────────

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

// ── Help ──────────────────────────────────────────────────────────────────────

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

    // Host step: "Explore local dirs…" (pre-selected). Then browse `browse/`
    // (a pasted path replaces the initial `~/`).
    tui.send_keys(ENTER);
    tui.wait_for("start here", Region::Screen, WAIT);
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

// ── Wizard: directory browser ─────────────────────────────────────────────────

/// "Explore local dirs…" opens a browser at $HOME: → descends into a child,
/// and Enter on the "start here" row starts the session in the directory
/// that was navigated to (not just one of the listed seeds).
#[test]
fn test_wizard_browse_from_home_and_start_here() {
    let harness = ManagerHarness::new();
    let proj = harness.home.join("work").join("proj");
    fs::create_dir_all(&proj).expect("create nested dir under HOME");
    let mut tui = harness.start_tui();

    // Explore local dirs… → browser at `~/`, "start here" on top.
    tui.send_keys(ENTER);
    tui.wait_for("start here: ~", Region::Screen, WAIT);
    tui.wait_for("work", Region::Screen, WAIT);

    // Down to `work`, → opens it.
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(RIGHT_ARROW);
    tui.wait_for("start here: ~/work", Region::Screen, WAIT);
    tui.wait_for("proj", Region::Screen, WAIT);

    // Down to `proj`, → opens it; "start here" is now the nested dir.
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(RIGHT_ARROW);
    tui.wait_for("start here: ~/work/proj", Region::Screen, WAIT);

    // ← goes back up one level, → comes back down.
    tui.send_keys(LEFT_ARROW);
    tui.wait_for("start here: ~/work", Region::Screen, WAIT);
    tui.wait_until(
        |s| !s.contains("start here: ~/work/proj", Region::Screen),
        WAIT,
    );
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(RIGHT_ARROW);
    tui.wait_for("start here: ~/work/proj", Region::Screen, WAIT);

    // Enter on "start here" (preselected after descending) starts the session.
    tui.send_keys(ENTER);
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);

    let want = proj.to_str().unwrap().to_owned();
    harness.wait_state(
        |v| {
            v.get("sessions")
                .and_then(|s| s.as_array())
                .map(|a| {
                    a.iter()
                        .any(|s| s.get("cwd").and_then(|c| c.as_str()) == Some(want.as_str()))
                })
                .unwrap_or(false)
        },
        WAIT,
    );

    tui.quit(WAIT);
}

// ── daemon CLI ────────────────────────────────────────────────────────────────

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
            extract_nonce(&text).is_some_and(|n| n != old_nonce)
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
    common::write_test_config(&config_home);

    // ── Create a unique remote temp directory ────────────────────────────────
    // mktemp -d gives us an isolated dir so we never touch anything else in /tmp.
    let remote_tmp = std::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=30",
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
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
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
        let wizard_visible = screen.contains("New session") || screen.contains("Resume");
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
    common::write_test_config(&config_home);

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
    assert!(
        !tab_text.contains("Claude Code"),
        "tab should not say 'Claude Code', got: {tab_text:?}"
    );
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
    let at = |s: &str| {
        tabs.find(s)
            .unwrap_or_else(|| panic!("{s:?} not in {tabs:?}"))
    };
    assert!(
        at(n0) < at("term@local") && at("term@local") < at(n1),
        "terminal must sit between the two sessions: {tabs:?}"
    );

    // A terminal's status line is minimal: no state, model or proxy.
    tui.wait_for("$ terminal", Region::StatusBar, WAIT);
    let status = tui.screen_text(Region::StatusBar);
    assert!(
        !status.contains("direct") && !status.contains("idle"),
        "{status:?}"
    );

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

// ── Git viewer ────────────────────────────────────────────────────────────────

/// Alt+l lists the session directory's commits; Enter opens one, Enter on a
/// file shows its colored patch, and Esc steps back out, one page at a time.
#[test]
fn test_git_viewer_log_commit_and_diff() {
    let harness = ManagerHarness::new();
    let repo = &harness.dirs[0];
    run_git(repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("a.txt"), "first line\n").unwrap();
    run_git(repo, &["add", "-A"]);
    run_git(repo, &["commit", "-q", "-m", "first commit"]);
    fs::write(repo.join("a.txt"), "first line\nhello world line\n").unwrap();
    run_git(repo, &["add", "-A"]);
    run_git(
        repo,
        &[
            "commit",
            "-q",
            "-m",
            "second change",
            "-m",
            "Body of the second.",
        ],
    );

    let mut tui = harness.start_tui();
    wizard_pick_dir(&mut tui, repo);

    // The log: both commits, the branch in the header.
    tui.send_keys(ALT_L);
    tui.wait_for("second change", Region::Pane, WAIT);
    tui.wait_for("first commit", Region::Pane, WAIT);
    tui.wait_for("⎇ main", Region::Pane, WAIT);
    tui.wait_for("HEAD → main", Region::Pane, WAIT);

    // Enter opens the selected (newest) commit: header, message and files.
    tui.send_keys(ENTER);
    tui.wait_for("Author: Tess", Region::Pane, WAIT);
    tui.wait_for("Body of the second.", Region::Pane, WAIT);
    tui.wait_for("a.txt", Region::Pane, WAIT);
    tui.wait_for("1 files changed", Region::Pane, WAIT);

    // Enter on the file shows its patch with colored line kinds.
    tui.send_keys(ENTER);
    tui.wait_for("+hello world line", Region::Pane, WAIT);
    tui.wait_for("diff --git", Region::Pane, WAIT);
    // ANSI palette indexes: 2 = green, 6 = cyan.
    tui.wait_until(
        |s| matches!(s.fg_of("+hello world line"), Some(c) if format!("{c:?}") == "Indexed(2)"),
        WAIT,
    );
    tui.wait_until(
        |s| matches!(s.fg_of("@@ -1"), Some(c) if format!("{c:?}") == "Indexed(6)"),
        WAIT,
    );

    // Esc: back to the commit, back to the log, then the view closes.
    tui.send_keys(ESC);
    tui.wait_for("Author: Tess", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_for("Enter open", Region::Pane, WAIT);
    tui.wait_for("first commit", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Enter open", Region::Pane), WAIT);
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);
    tui.quit(WAIT);
}

/// A directory that is not a repository shows git's error inside the view,
/// and a manager key (Alt+q) still works from there.
#[test]
fn test_git_viewer_not_a_repo_shows_the_error() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();
    wizard_pick_dir(&mut tui, &harness.dirs[1]);

    tui.send_keys(ALT_L);
    tui.wait_for("not a git repository", Region::Pane, WAIT);
    tui.wait_for("r retry", Region::Pane, WAIT);
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

// ── Reset (Alt+e) ─────────────────────────────────────────────────────────────

/// Alt+e then `n` restarts claude in the same tab (a new banner nonce shows up,
/// no tab is added or lost); Esc cancels; `r` restarts again.
#[test]
fn test_reset_restarts_claude_in_the_same_tab() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_tui();

    wizard_pick_dir(&mut tui, &harness.dirs[0]);
    tui.send_keys(ALT_N);
    wizard_pick_dir(&mut tui, &harness.dirs[1]);
    let n0 = harness.dirs[0].file_name().unwrap().to_str().unwrap();
    let n1 = harness.dirs[1].file_name().unwrap().to_str().unwrap();
    tui.wait_for(n1, Region::TabBar, WAIT);
    let before = current_nonce(&tui);

    // Esc leaves everything alone.
    tui.send_keys(ALT_E);
    tui.wait_for("new conversation", Region::Screen, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("new conversation", Region::Screen), WAIT);
    assert_eq!(current_nonce(&tui), before);

    // `n`: a new claude process in the same tab.
    tui.send_keys(ALT_E);
    tui.wait_for("new conversation", Region::Screen, WAIT);
    tui.send_keys(b"n");
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Pane)).is_some_and(|n| n != before),
        WAIT,
    );
    let after_new = current_nonce(&tui);

    // `r`: restarted once more.
    tui.send_keys(ALT_E);
    tui.wait_for("restart & resume", Region::Screen, WAIT);
    tui.send_keys(b"r");
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Pane)).is_some_and(|n| n != after_new),
        WAIT,
    );

    // Still two tabs, in the same order, and the other one is untouched.
    let tabs = tui.screen_text(Region::TabBar);
    assert_eq!(tabs.matches(n0).count(), 1, "{tabs:?}");
    assert_eq!(tabs.matches(n1).count(), 1, "{tabs:?}");
    assert!(tabs.find(n0) < tabs.find(n1), "{tabs:?}");
    tui.send_keys(ALT_LEFT);
    tui.wait_until(
        |s| extract_nonce(&s.region_text(Region::Pane)).is_some_and(|n| n != before),
        WAIT,
    );

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

// ── `claudio --plain` ─────────────────────────────────────────────────────────

/// A fake claude that waits for a line, then exits with `status`.
fn write_exiting_claude(harness: &ManagerHarness, status: i32) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\n[ \"$1\" = \"--help\" ] && exit 0\n[ \"$1\" = \"--version\" ] && echo 'claude 0.0.0-fake' && exit 0\n\
         echo FAKE_CLAUDE_BANNER\nread line\nexit {status}\n"
    );
    fs::write(&harness.fake_claude, script).expect("write exiting claude");
    fs::set_permissions(&harness.fake_claude, fs::Permissions::from_mode(0o755))
        .expect("chmod exiting claude");
}

/// A fake claude that prints the arguments the user gave (not the ones the
/// daemon injects) as `ARG<..>`, then behaves like the default one.
fn write_argv_claude(harness: &ManagerHarness) {
    use std::os::unix::fs::PermissionsExt;
    let script = r#"#!/bin/sh
[ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
[ "$1" = "--help" ] && exit 0
printf 'FAKE_CLAUDE_BANNER nonce=%s_%s' "$$" "$RANDOM"
for a in "$@"; do
    case "$a" in '{'*|--settings|--allow-dangerously-skip-permissions) ;; *) printf ' ARG<%s>' "$a" ;; esac
done
echo
exec cat
"#;
    fs::write(&harness.fake_claude, script).expect("write argv claude");
    fs::set_permissions(&harness.fake_claude, fs::Permissions::from_mode(0o755))
        .expect("chmod argv claude");
}

/// Wait (bounded) until the daemon has no session left.
fn wait_no_sessions(harness: &ManagerHarness) {
    let deadline = Instant::now() + WAIT;
    loop {
        let listed = harness.claudio_output(&["sessions"]);
        if listed.contains("no sessions") && harness.journal_sessions().is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "a session is left in the daemon:\n{listed}\njournal: {:?}",
            harness.journal_sessions()
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// The session fills the screen (no tab bar, no status bar), runs in the
/// current directory with claude's args, and leaves no state.json behind.
#[test]
fn test_plain_is_the_bare_session_full_screen() {
    let harness = ManagerHarness::new();
    write_argv_claude(&harness);
    let mut tui = harness.start_plain_with(&["--no-proxy", "-c", "hello"], |_| {});

    // The banner is on the first row: nothing is drawn above the pane.
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::TabBar, WAIT);
    let screen = tui.screen_text(Region::Screen);
    assert!(screen.contains("ARG<-c> ARG<hello>"), "{screen}");
    assert!(!screen.contains("Alt+h help"), "no status bar:\n{screen}");
    assert!(!screen.contains("│"), "no tab bar:\n{screen}");

    let listed = harness.claudio_output(&["sessions"]);
    assert!(
        listed.contains(harness.dirs[0].to_str().unwrap()),
        "claude runs in the current directory:\n{listed}"
    );
    assert!(
        !harness.state_json().exists(),
        "plain mode never writes state.json"
    );

    // Input reaches claude.
    tui.send_keys(b"typed\r");
    tui.wait_for("typed", Region::Screen, WAIT);
    assert!(!harness.state_json().exists());
    drop(tui);
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
        "Alt+s → n",
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
        9,
        "exactly the plain keys:\n{screen}"
    );

    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("claudio keys", Region::Screen), WAIT);
    tui.send_keys(b"after\r");
    tui.wait_for("after", Region::Screen, WAIT);
    drop(tui);
}

/// Manager keys are claude's here: Alt+n opens no wizard and Alt+q does not
/// quit; what is typed next reaches the session.
#[test]
fn test_plain_forwards_manager_keys_to_claude() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_plain_with(&["--no-proxy"], |_| {});

    tui.send_keys(ALT_N);
    tui.send_keys(b"after-n\r");
    tui.wait_for("after-n", Region::Screen, WAIT);
    let screen = tui.screen_text(Region::Screen);
    assert!(!screen.contains("New session"), "no wizard:\n{screen}");

    tui.send_keys(ALT_Q);
    tui.send_keys(b"after-q\r");
    tui.wait_for("after-q", Region::Screen, WAIT);
    assert!(
        matches!(tui.child.try_wait(), Ok(None)),
        "Alt+q must not quit"
    );

    // Alt+Shift+1 is not a tab switch either: it is just forwarded.
    tui.send_keys(ALT_SHIFT_1);
    tui.send_keys(b"after-1\r");
    tui.wait_for("after-1", Region::Screen, WAIT);
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
    harness.write_env_dumping_fake_claude(&env_file);

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
    harness.write_env_dumping_fake_claude(&env_file);

    let tui = harness.start_plain_with(&["--no-proxy"], |cmd| {
        cmd.env("CLAUDIO_PROXY_URL", "sekret@127.0.0.1:1");
    });
    let env = fs::read_to_string(&env_file).expect("env recorded");
    assert!(env.contains("ANTHROPIC_API_KEY=test-key-not-real"), "{env}");
    assert!(!env.contains("ANTHROPIC_AUTH_TOKEN"), "{env}");
    assert!(!env.contains("CLAUDECODE"), "{env}");
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
    harness.write_env_dumping_fake_claude(&env_file);
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

/// When claude ends (Ctrl+D), the UI exits with status 0 and no session
/// remains in the daemon or its journal, and no state.json was written.
#[test]
fn test_plain_exits_when_claude_ends_and_leaves_nothing() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_plain_with(&["--no-proxy"], |_| {});
    assert!(!harness
        .claudio_output(&["sessions"])
        .contains("no sessions"));

    tui.send_keys(b"\x04");
    assert_eq!(tui.wait_exit_code(WAIT), 0);
    wait_no_sessions(&harness);
    assert!(!harness.state_json().exists());

    // A bare manager started afterwards has nothing to show but its wizard.
    let mut manager = harness.start_tui();
    manager.wait_for("New session", Region::Screen, WAIT);
    manager.quit(WAIT);
}

/// A plain session is not the manager's: the daemon does not journal it, and
/// a manager started next to it neither shows it nor resumes it.
#[test]
fn test_a_plain_session_is_not_a_manager_tab() {
    let harness = ManagerHarness::new();
    let mut plain = harness.start_plain_with(&["--no-proxy"], |_| {});
    plain.wait_for("FAKE_CLAUDE_BANNER", Region::Screen, WAIT);
    assert!(!harness
        .claudio_output(&["sessions"])
        .contains("no sessions"));
    assert!(harness.journal_sessions().is_empty(), "not journaled");

    // With no session of its own, the manager opens its wizard.
    let mut manager = harness.start_tui();
    manager.wait_for("New session", Region::Screen, WAIT);
    let tabs = manager.screen_text(Region::TabBar);
    assert!(
        tabs.trim().is_empty(),
        "no tab for the plain session: {tabs:?}"
    );
    manager.send_keys(ESC);
    manager.wait_until(|s| !s.contains("New session", Region::Screen), WAIT);
    manager.quit(WAIT);

    // The plain session ran on, untouched.
    plain.send_keys(b"still-here\r");
    plain.wait_for("still-here", Region::Screen, WAIT);
    drop(plain);
}

/// Claude's non-zero status is the UI's, and the dormant session it leaves is
/// killed rather than recovered later.
#[test]
fn test_plain_exit_status_is_claudes() {
    let harness = ManagerHarness::new();
    write_exiting_claude(&harness, 7);
    let mut tui = harness.start_plain_with(&["--no-proxy"], |_| {});

    tui.send_keys(b"go\r");
    assert_eq!(tui.wait_exit_code(WAIT), 7);
    wait_no_sessions(&harness);
}

/// A closed terminal (SIGHUP) takes the session with it.
#[test]
fn test_plain_hangup_kills_the_session() {
    let harness = ManagerHarness::new();
    let mut tui = harness.start_plain_with(&["--no-proxy"], |_| {});
    assert!(!harness
        .claudio_output(&["sessions"])
        .contains("no sessions"));

    let pid = tui.child.process_id().expect("claudio pid");
    // SAFETY: kill(2) has no preconditions.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGHUP) };
    tui.wait_exit(WAIT);
    wait_no_sessions(&harness);
    assert!(!harness.state_json().exists());
}

/// `--plain -p` (and --print, --api) is rejected with a hint, without
/// starting claude.
#[test]
fn test_plain_rejects_print_mode() {
    let harness = ManagerHarness::new();
    let marker = harness.root.join("claude-ran");
    harness.write_marker_fake_claude(&marker);

    let res = std::process::Command::new(BINARY)
        .args(["--plain", "-p", "hi"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("XDG_RUNTIME_DIR", &harness.runtime_dir)
        .env("XDG_CONFIG_HOME", &harness.config_home)
        .env("HOME", &harness.home)
        .env("CLAUDIO_CLAUDE_PATH", &harness.fake_claude)
        .output()
        .expect("run claudio --plain -p");
    assert_eq!(res.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&res.stderr);
    assert!(
        stderr.contains("-p is not supported with --plain"),
        "{stderr}"
    );
    assert!(!marker.exists(), "claude must not run");
}

// ── Resume ───────────────────────────────────────────────────────────────────

/// Write one claude transcript for `dir` in `home`, filed the way claude
/// files it (under the resolved path).
fn write_transcript(home: &std::path::Path, dir: &std::path::Path) {
    let real = fs::canonicalize(dir).unwrap();
    let encoded: String = real
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let pdir = home.join(".claude/projects").join(encoded);
    fs::create_dir_all(&pdir).unwrap();
    let rec = serde_json::json!({
        "type": "user", "cwd": real.to_string_lossy(),
        "message": {"role": "user", "content": "PROBE-PROMPT hello"},
    });
    fs::write(
        pdir.join("11111111-2222-3333-4444-555555555555.jsonl"),
        format!("{rec}\n"),
    )
    .unwrap();
}

/// A directory with an earlier claude conversation offers to resume it
/// (picked with the browser's "start here").
#[test]
fn test_wizard_start_here_offers_resume() {
    let h = ManagerHarness::new();
    write_transcript(&h.home, &h.dirs[0]);
    let mut tui = h.start_tui();
    tui.wait_for("Explore local dirs", Region::Screen, WAIT);
    tui.send_keys(ENTER); // Explore → browser at ~/
    tui.wait_for("start here", Region::Screen, WAIT);
    tui.send_keys(CTRL_U);
    thread::sleep(Duration::from_millis(100));
    tui.send_paste(&format!("{}/", h.dirs[0].display()));
    thread::sleep(Duration::from_millis(500));
    tui.send_keys(ENTER);
    thread::sleep(Duration::from_millis(1500));
    tui.wait_for("resume?", Region::Screen, WAIT);
}

/// The same when the directory is picked from the first screen's recent list.
#[test]
fn test_wizard_recent_dir_offers_resume() {
    let h = ManagerHarness::new();
    write_transcript(&h.home, &h.dirs[0]);
    let state = h.config_home.join("claudio/state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    let dir = h.dirs[0].to_string_lossy().into_owned();
    fs::write(
        &state,
        serde_json::json!({"recent_dirs": {"local": [dir]}}).to_string(),
    )
    .unwrap();
    let mut tui = h.start_tui();
    tui.wait_for("Explore local dirs", Region::Screen, WAIT);
    thread::sleep(Duration::from_millis(500));
    tui.send_keys(DOWN_ARROW);
    thread::sleep(Duration::from_millis(200));
    tui.send_keys(ENTER);
    thread::sleep(Duration::from_millis(1500));
    tui.wait_for("resume?", Region::Screen, WAIT);
}

// ── LaTeX formulas ────────────────────────────────────────────────────────────

/// Tab, as the terminal sends it.
const TAB: &[u8] = b"\t";

/// A fake claude that answers with a formula, as real claude does by
/// default: it saves its `--settings`, writes the assistant record to its
/// transcript (under the test's `$HOME`), prints the answer with the tag,
/// then runs its own SessionStart hook so the daemon reads it.
fn write_latex_fake_claude(h: &ManagerHarness, settings: &std::path::Path) {
    let sid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    // claude (and now the daemon) file the transcript under the resolved cwd.
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
    let record = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "m1",
            "model": "claude-fake",
            "content": [{"type": "text", "text": "Energy: <latex>E = mc^2</latex> as promised."}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    });
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
if [ "$1" = "--help" ]; then exit 0; fi
printf '%s' "$2" > "{settings}"
mkdir -p "{project}"
printf '%s\n' '{record}' > "{project}/{sid}.jsonl"
echo "FAKE_CLAUDE_BANNER pid=$$"
echo 'Energy: <latex>E = mc^2</latex> as promised.'
HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
exec cat
"#,
        settings = settings.display(),
        project = project.display(),
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(
        &h.fake_claude,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// G5: a formula in claude's answer stays in the pane as claude printed
/// it, `<latex>` tags and all (formulas render in the web UI only), also
/// after the daemon has sent its formula list and in a TUI started later
/// (which gets the list replayed on attach). The reader has no formulas
/// list any more: Tab changes nothing. The default config still gives the
/// session the context-file SessionStart hook (the web UI needs the tags).
#[test]
fn test_latex_formula_is_shown_as_claude_printed_it() {
    const PRINTED: &str = "Energy: <latex>E = mc^2</latex> as promised.";
    let h = ManagerHarness::new();
    // The default test config has no `latex` key: the hook is on by default.
    let settings = h.root.join("settings.json");
    write_latex_fake_claude(&h, &settings);

    {
        let mut tui = h.start_tui();
        wizard_pick_dir(&mut tui, &h.dirs[0]);
        tui.wait_for(PRINTED, Region::Pane, DAEMON_WAIT);
        let hooks = fs::read_to_string(&settings).unwrap();
        assert!(hooks.contains("__context file"), "{hooks}");
        // The daemon read the transcript (its formula list is sent by
        // now): the pane is unchanged.
        thread::sleep(Duration::from_millis(1500));
        assert!(tui.screen_text(Region::Pane).contains(PRINTED));
        assert!(!tui.screen_text(Region::Pane).contains("mc²"));

        // The transcript has no prompt, so no answer; Tab lists nothing.
        tui.send_keys(ALT_U);
        tui.wait_for("No answers yet", Region::Pane, WAIT);
        tui.send_keys(TAB);
        thread::sleep(Duration::from_millis(300));
        let pane = tui.screen_text(Region::Pane);
        assert!(pane.contains("No answers yet"), "{pane}");
        assert!(!pane.contains("formulas"), "{pane}");
        tui.send_keys(ALT_U);
        tui.wait_for(PRINTED, Region::Pane, WAIT);
        tui.send_keys(ALT_Q);
        tui.wait_exit(WAIT);
    }

    // A new TUI gets the daemon's replay on attach: still claude's text.
    let tui = h.start_tui();
    tui.wait_for(PRINTED, Region::Pane, DAEMON_WAIT);
    thread::sleep(Duration::from_millis(1000));
    assert!(tui.screen_text(Region::Pane).contains(PRINTED));
}

// ── Reader view ───────────────────────────────────────────────────────────────

/// The answer the reader test's fake claude writes: text, an inline and a
/// display formula, and a list.
const ANSWER: &str = "The roots of <latex>ax^2 + bx + c = 0</latex> are\n\n\
    <latex>x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}</latex>\n\n\
    - first item\n- second item";

/// A fake claude with a conversation of two answers in its transcript. It
/// prints neither, so what the reader shows comes from the transcript (via
/// the daemon); its SessionStart hook tells the daemon which transcript.
fn write_answers_fake_claude(h: &ManagerHarness) {
    let sid = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
    let prompt = |text: &str| serde_json::json!({"type": "user", "message": {"role": "user", "content": text}});
    let answer = |text: &str| {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "model": "claude-fake",
                "content": [{"type": "text", "text": text}],
                "usage": {"input_tokens": 1, "output_tokens": 1},
            },
        })
    };
    let records = [
        prompt("Say hi"),
        answer("Hello there."),
        prompt("Explain the quadratic formula"),
        answer(ANSWER),
    ];
    let lines: Vec<String> = records.iter().map(|r| format!("'{r}'")).collect();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
if [ "$1" = "--help" ]; then exit 0; fi
mkdir -p "{project}"
printf '%s\n' {lines} > "{project}/{sid}.jsonl"
echo "FAKE_CLAUDE_BANNER pid=$$"
HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
exec cat
"#,
        project = project.display(),
        lines = lines.join(" "),
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(
        &h.fake_claude,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// AC10: Alt+u opens the reader on the newest answer, laid out from the
/// transcript: its prompt in the header, text with the inline formula and
/// the display formula as their `<latex>` source (G5: formulas render in
/// the web UI only), the list; ← shows the older one, and Esc closes it.
#[test]
fn test_reader_shows_the_answers_with_formulas_as_source() {
    let h = ManagerHarness::new();
    write_answers_fake_claude(&h);
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    tui.send_keys(ALT_U);
    tui.wait_for(
        "answer 2/2 · Explain the quadratic formula",
        Region::Pane,
        DAEMON_WAIT,
    );
    tui.wait_for(
        "The roots of <latex>ax^2 + bx + c = 0</latex> are",
        Region::Pane,
        WAIT,
    );
    let pane = tui.screen_text(Region::Pane);
    let display = "<latex>x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}</latex>";
    assert!(pane.contains(display), "missing {display:?} in:\n{pane}");
    assert!(!pane.contains("ax²") && !pane.contains('─'), "{pane}");
    assert!(
        pane.contains("• first item") && pane.contains("• second item"),
        "{pane}"
    );

    tui.send_keys(LEFT_ARROW);
    tui.wait_for("answer 1/2 · Say hi", Region::Pane, WAIT);
    tui.wait_for("Hello there.", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_for("FAKE_CLAUDE_BANNER", Region::Pane, WAIT);
    assert!(!tui.screen_text(Region::Pane).contains("Hello there."));
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

// ── Extensions ────────────────────────────────────────────────────────────────

const ALT_Z: &[u8] = b"\x1bz";

/// The external extension the extension tests install: a tag lens `keys`
/// drawn as `⌨ <source>`, and actions that paste (with the API key it
/// sees, which must be none), list, show a document, and hang.
const FAKE_EXTENSION: &str = r#"name = "fake"
description = "a fake extension"
command = ["./fake.sh"]
[[lens]]
tag = "keys"
instruction = "Write keys as <keys>Ctrl+C</keys>. EXT-INSTRUCTION"
[[action]]
key = "k"
title = "paste a stamp"
id = "stamp"
[[action]]
key = "l"
title = "pick one"
id = "pick"
[[action]]
key = "d"
title = "show a document"
id = "doc"
[[action]]
key = "h"
title = "hang"
id = "hang"
"#;

const FAKE_EXTENSION_PROGRAM: &str = r##"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/^{"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"render"'*)
      src=$(printf '%s' "$line" | sed 's/.*"source":"\([^"]*\)".*/\1/')
      printf '{"id":%s,"result":{"inline":"⌨ %s"}}\n' "$id" "$src" ;;
    *'"action":"stamp"'*)
      printf '{"id":%s,"result":{"kind":"paste","text":"PASTED-%s"}}\n' "$id" "${ANTHROPIC_API_KEY:-nokey}" ;;
    *'"action":"pick"'*)
      printf '{"id":%s,"result":{"kind":"list","title":"Fake list","items":[{"label":"first","result":{"kind":"paste","text":"PICKED-FIRST"}},{"label":"second","detail":"the other one","result":{"kind":"paste","text":"PICKED-SECOND"}}]}}\n' "$id" ;;
    *'"action":"doc"'*)
      printf '{"id":%s,"result":{"kind":"document","title":"Fake doc","markdown":"# Heading\\n\\nPress <keys>Esc</keys> now."}}\n' "$id" ;;
    *) ;;
  esac
done
"##;

/// Install the fake extension in the harness's config dir.
fn install_fake_extension(h: &ManagerHarness) {
    let dir = h.config_home.join("claudio/extensions/fake");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("extension.toml"), FAKE_EXTENSION).unwrap();
    let program = dir.join("fake.sh");
    fs::write(&program, FAKE_EXTENSION_PROGRAM).unwrap();
    fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// A fake claude whose answer has a `<keys>` tag: it saves its `--settings`
/// and argv, writes the transcript (a prompt and the answer), prints the
/// answer, turns bracketed paste on, runs its SessionStart hook, then
/// echoes lines; a line ending in `off` turns bracketed paste off.
fn write_keys_fake_claude(h: &ManagerHarness, settings: &std::path::Path, argv: &std::path::Path) {
    let sid = "cccccccc-dddd-4eee-8fff-000000000000";
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
    let prompt = serde_json::json!({"type": "user", "message": {"role": "user", "content": "How do I stop it?"}});
    let answer = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "m1",
            "model": "claude-fake",
            "content": [{"type": "text", "text": "Press <keys>Ctrl+C</keys> to stop."}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    });
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
if [ "$1" = "--help" ]; then exit 0; fi
printf '%s' "$2" > "{settings}"
printf '%s\n' "$@" > "{argv}"
mkdir -p "{project}"
printf '%s\n' '{prompt}' '{answer}' > "{project}/{sid}.jsonl"
echo "FAKE_CLAUDE_BANNER pid=$$"
echo 'Press <keys>Ctrl+C</keys> to stop.'
printf '\033[?2004h'
HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
while IFS= read -r line; do
  case "$line" in
    *off) printf '\033[?2004l' ;;
    *) printf 'GOT %s\n' "$line" ;;
  esac
done
"#,
        settings = settings.display(),
        argv = argv.display(),
        project = project.display(),
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(
        &h.fake_claude,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

/// AC2, AC6, AC7: an external extension's tag lens leaves the pane and the
/// reader alone (G5: lenses render in the web UI only, the tag text is
/// shown unchanged); its instruction reaches claude through the context
/// file only; Alt+z lists the built-in and external actions, Esc closes it
/// with nothing sent, Alt+z u is the reader; the program's paste reaches
/// claude without Enter (and is refused once bracketed paste is off), its
/// list picks, its document shows (its own tag as written), and a hanging
/// action times out with a notice.
#[test]
fn test_external_extension_lens_leader_and_actions() {
    let h = ManagerHarness::new();
    install_fake_extension(&h);
    let (settings, argv) = (h.root.join("settings.json"), h.root.join("argv.txt"));
    write_keys_fake_claude(&h, &settings, &argv);
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    // G5: the pane shows the tag text unchanged, also once the daemon has
    // sent its lens list.
    tui.wait_for(
        "Press <keys>Ctrl+C</keys> to stop.",
        Region::Pane,
        DAEMON_WAIT,
    );
    thread::sleep(Duration::from_millis(1500));
    let pane = tui.screen_text(Region::Pane);
    assert!(
        pane.contains("Press <keys>Ctrl+C</keys> to stop."),
        "{pane}"
    );
    assert!(!pane.contains('⌨'), "{pane}");

    // AC2: the hook has the context file's path; the text is only in it.
    let hooks = fs::read_to_string(&settings).unwrap();
    assert!(hooks.contains("__context file"), "{hooks}");
    assert!(!hooks.contains("EXT-INSTRUCTION"), "{hooks}");
    assert!(!fs::read_to_string(&argv)
        .unwrap()
        .contains("EXT-INSTRUCTION"));
    let context_dir = h.runtime_dir.join("claudio/context");
    let context: Vec<String> = fs::read_dir(&context_dir)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert_eq!(context.len(), 1);
    assert!(context[0].contains("EXT-INSTRUCTION"), "{context:?}");
    assert!(context[0].contains("<latex>"), "the built-in lenses too");

    // G5: the reader shows it as written too.
    tui.send_keys(ALT_U);
    tui.wait_for("answer 1/1 · How do I stop it?", Region::Pane, DAEMON_WAIT);
    tui.wait_for("Press <keys>Ctrl+C</keys> to stop.", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);

    // AC6: the leader lists built-in and external actions; Esc sends nothing.
    tui.send_keys(ALT_Z);
    tui.wait_for("search conversations", Region::Screen, WAIT);
    let shown = tui.screen_text(Region::Screen);
    for row in [
        "u  reader view",
        "p  insert snippet",
        "k  paste a stamp  (fake)",
    ] {
        assert!(shown.contains(row), "missing {row:?} in:\n{shown}");
    }
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("paste a stamp", Region::Screen), WAIT);
    thread::sleep(Duration::from_millis(300));
    assert!(
        !tui.screen_text(Region::Pane).contains("^["),
        "nothing reached claude"
    );
    tui.send_keys(ALT_Z);
    tui.wait_for("paste a stamp", Region::Screen, WAIT);
    tui.send_keys(b"u");
    tui.wait_for("answer 1/1 · How do I stop it?", Region::Pane, DAEMON_WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);

    // AC7: a paste reaches claude without Enter (no `GOT` line), and no API
    // key reaches the program.
    tui.send_keys(ALT_Z);
    tui.wait_for("paste a stamp", Region::Screen, WAIT);
    tui.send_keys(b"k");
    tui.wait_for("PASTED-nokey", Region::Pane, WAIT);
    thread::sleep(Duration::from_millis(300));
    let pane = tui.screen_text(Region::Pane);
    assert_eq!(pane.matches("PASTED-nokey").count(), 1, "{pane}");
    assert!(
        !pane.contains("GOT"),
        "no Enter followed the paste:\n{pane}"
    );

    // A list: Down, Enter pastes the second item's text.
    tui.send_keys(ALT_Z);
    tui.wait_for("pick one", Region::Screen, WAIT);
    tui.send_keys(b"l");
    tui.wait_for("Fake list", Region::Screen, WAIT);
    tui.wait_for("the other one", Region::Screen, WAIT);
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(ENTER);
    tui.wait_for("PICKED-SECOND", Region::Pane, WAIT);

    // A document, its own tag as written.
    tui.send_keys(ALT_Z);
    tui.wait_for("show a document", Region::Screen, WAIT);
    tui.send_keys(b"d");
    tui.wait_for("Fake doc", Region::Pane, WAIT);
    tui.wait_for("Press <keys>Esc</keys> now.", Region::Pane, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Fake doc", Region::Pane), WAIT);

    // Bracketed paste off: the paste is refused with a notice.
    tui.send_keys(b"off\r");
    thread::sleep(Duration::from_millis(300));
    tui.send_keys(ALT_Z);
    tui.wait_for("paste a stamp", Region::Screen, WAIT);
    tui.send_keys(b"k");
    tui.wait_for("bracketed paste is off", Region::StatusBar, WAIT);

    // A hanging action times out (15 s) with a notice.
    tui.send_keys(ALT_Z);
    tui.wait_for("hang", Region::Screen, WAIT);
    tui.send_keys(b"h");
    tui.wait_for(
        "extension fake timed out",
        Region::StatusBar,
        Duration::from_secs(25),
    );
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

// ── Built-in extensions: diagram, search, snippets ───────────────────────────

/// A fake claude for the built-in extension tests: logs `<pwd> <args>` per
/// launch to `log`; unless resuming, writes `records` (JSON lines) as
/// conversation `sid` under its directory's project; prints a banner, turns
/// bracketed paste on, runs its SessionStart hook, then echoes lines.
fn write_conversation_fake_claude(
    h: &ManagerHarness,
    sid: &str,
    records: &[serde_json::Value],
    log: &std::path::Path,
) {
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let project = h.home.join(".claude/projects").join(encode_cwd(&cwd));
    let lines: Vec<String> = records.iter().map(|r| format!("'{r}'")).collect();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "claude 0.0.0-fake"; exit 0; fi
if [ "$1" = "--help" ]; then exit 0; fi
echo "$(pwd) $*" >> "{log}"
case "$*" in
  *--resume*) ;;
  *) mkdir -p "{project}"; printf '%s\n' {lines} > "{project}/{sid}.jsonl" ;;
esac
echo "FAKE_CLAUDE_BANNER pid=$$"
printf '\033[?2004h'
HOOK=$(printf '%s' "$2" | sed -n 's/.*"command":"\([^"]*__hook SessionStart[^"]*\)".*/\1/p')
printf '{{"session_id":"{sid}","source":"startup"}}' | eval "$HOOK"
while IFS= read -r line; do printf 'GOT %s\n' "$line"; done
"#,
        log = log.display(),
        project = project.display(),
        lines = lines.join(" "),
    );
    fs::write(&h.fake_claude, script).unwrap();
    fs::set_permissions(
        &h.fake_claude,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
}

fn user_record(text: &str, cwd: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "type": "user",
        "cwd": cwd.display().to_string(),
        "message": {"role": "user", "content": text},
    })
}

fn assistant_record(text: &str, cwd: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "cwd": cwd.display().to_string(),
        "message": {
            "model": "claude-fake",
            "content": [{"type": "text", "text": text}],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    })
}

/// G5: a ```mermaid answer is its source in the reader, a code block
/// (diagrams are drawn in the web UI only).
#[test]
fn test_reader_shows_a_mermaid_diagram_as_its_source() {
    let h = ManagerHarness::new();
    let cwd = fs::canonicalize(&h.dirs[0]).unwrap();
    let answer = "Here it is:\n\n```mermaid\nflowchart LR\n  alpha --> beta\n```\n";
    let records = [user_record("Draw it", &cwd), assistant_record(answer, &cwd)];
    let log = h.root.join("launches.log");
    write_conversation_fake_claude(&h, "dddddddd-eeee-4fff-8000-111111111111", &records, &log);
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    tui.send_keys(ALT_U);
    tui.wait_for("answer 1/1 · Draw it", Region::Pane, DAEMON_WAIT);
    tui.wait_for("  flowchart LR", Region::Pane, WAIT);
    let pane = tui.screen_text(Region::Pane);
    assert!(pane.contains("    alpha --> beta"), "{pane}");
    assert!(
        !pane.contains('┌') && !pane.contains('╭'),
        "no boxes:\n{pane}"
    );
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("answer 1/1", Region::Pane), WAIT);
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

/// AC9: Alt+z s finds a conversation of another directory by a word in its
/// prompt, and Enter resumes it there in a new tab (`--resume <id>` reaches
/// claude).
#[test]
fn test_search_finds_and_resumes_a_conversation() {
    let h = ManagerHarness::new();
    let here = fs::canonicalize(&h.dirs[0]).unwrap();
    let there = fs::canonicalize(&h.dirs[1]).unwrap();
    let log = h.root.join("launches.log");
    let records = [user_record("Say hi", &here), assistant_record("Hi.", &here)];
    write_conversation_fake_claude(&h, "eeeeeeee-ffff-4000-8111-222222222222", &records, &log);
    // Another conversation, in the other directory.
    let other = "ffffffff-0000-4111-8222-333333333333";
    let project = h.home.join(".claude/projects").join(encode_cwd(&there));
    fs::create_dir_all(&project).unwrap();
    let lines = [
        user_record("Where is the needle in this haystack?", &there),
        assistant_record("Under the hay.", &there),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(project.join(format!("{other}.jsonl")), text).unwrap();

    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);
    tui.send_keys(ALT_Z);
    tui.wait_for("search conversations", Region::Screen, WAIT);
    tui.send_keys(b"s");
    tui.wait_for("(Enter run · Esc cancel)", Region::Screen, WAIT);
    tui.send_keys(b"needle");
    tui.send_keys(ENTER);
    tui.wait_for("1 conversations · \"needle\"", Region::Screen, DAEMON_WAIT);
    tui.send_keys(ENTER);
    // The new launch runs in the other directory, resuming that conversation.
    let resumed = |log: &str| {
        log.lines().any(|l| {
            l.starts_with(&format!("{} ", there.display()))
                && l.ends_with(&format!("--resume {other}"))
        })
    };
    let deadline = Instant::now() + DAEMON_WAIT;
    while !resumed(&fs::read_to_string(&log).unwrap_or_default()) {
        assert!(
            Instant::now() < deadline,
            "no resume of {other} in {}",
            there.display()
        );
        thread::sleep(Duration::from_millis(100));
    }
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

/// AC10: Alt+z p lists the snippets, the filter picks one, Enter pastes it
/// into claude without Enter; Alt+z P saves the last prompt as a snippet.
#[test]
fn test_snippets_paste_and_save_the_last_prompt() {
    let h = ManagerHarness::new();
    let here = fs::canonicalize(&h.dirs[0]).unwrap();
    let snippets = h.config_home.join("claudio/snippets");
    fs::create_dir_all(&snippets).unwrap();
    fs::write(snippets.join("review.md"), "Review this change carefully.").unwrap();
    fs::write(snippets.join("deploy.txt"), "Deploy to staging.").unwrap();
    let log = h.root.join("launches.log");
    let records = [
        user_record("Say hello to the world", &here),
        assistant_record("Hello, world.", &here),
    ];
    write_conversation_fake_claude(&h, "00000000-1111-4222-8333-444444444444", &records, &log);
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    tui.send_keys(ALT_Z);
    tui.wait_for("insert snippet", Region::Screen, WAIT);
    tui.send_keys(b"p");
    tui.wait_for("Snippets", Region::Screen, WAIT);
    tui.wait_for("Deploy to staging.", Region::Screen, WAIT);
    tui.send_keys(b"rev");
    tui.wait_until(|s| !s.contains("Deploy to staging.", Region::Screen), WAIT);
    tui.send_keys(ENTER);
    tui.wait_for("Review this change carefully.", Region::Pane, WAIT);
    thread::sleep(Duration::from_millis(300));
    assert!(
        !tui.screen_text(Region::Pane).contains("GOT"),
        "no Enter after the paste"
    );

    tui.send_keys(ALT_Z);
    tui.wait_for("save last prompt as snippet", Region::Screen, WAIT);
    tui.send_keys(b"P");
    tui.wait_for("saved snippet ", Region::StatusBar, DAEMON_WAIT);
    let saved: Vec<String> = fs::read_dir(&snippets)
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert!(
        saved.iter().any(|t| t.contains("Say hello to the world")),
        "{saved:?}"
    );
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
}

// ── Extensions screen ────────────────────────────────────────────────────────

const ALT_I: &[u8] = b"\x1bi";

/// The extensions screen (Alt+i) lists the built-ins and a loaded external
/// extension with their keys and lenses; Space turns the selected one off
/// at once (`[ ]`) and saves it in config.toml, the file's comment kept;
/// Esc closes it; the leader no longer offers a turned-off extension's
/// actions.
#[test]
fn test_extensions_screen_turns_extensions_off_and_saves_it() {
    let h = ManagerHarness::new();
    install_fake_extension(&h);
    let config = h.config_home.join("claudio/config.toml");
    let prior = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("# my settings, keep me\n{prior}")).unwrap();
    let mut tui = h.start_tui();
    wizard_pick_dir(&mut tui, &h.dirs[0]);

    tui.send_keys(ALT_I);
    tui.wait_for(
        "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)",
        Region::Screen,
        WAIT,
    );
    let shown = tui.screen_text(Region::Screen);
    for row in [
        "[x] latex     built-in  —              <latex>",
        "[x] diagram   built-in  —              ```mermaid",
        "[x] search    built-in  Alt+z s",
        "[x] snippets  built-in  Alt+z p/P",
        "[x] fake      external  Alt+z k/l/d/h  <keys>",
        "math typeset from <latex> tags",
        "changes apply now",
    ] {
        assert!(shown.contains(row), "missing {row:?} in:\n{shown}");
    }

    // Space on diagram: off now, and saved.
    tui.send_keys(DOWN_ARROW);
    tui.wait_for("Mermaid diagrams drawn in the web UI", Region::Screen, WAIT);
    tui.send_keys(b" ");
    tui.wait_for("[ ] diagram", Region::Screen, WAIT);
    tui.wait_for("extension diagram off (saved)", Region::StatusBar, WAIT);
    let saved = fs::read_to_string(&config).unwrap();
    assert!(saved.starts_with("# my settings, keep me\n"), "{saved}");
    assert!(saved.contains(&prior), "the rest is kept:\n{saved}");
    assert!(
        saved.ends_with("[extensions]\ndiagram = false\n"),
        "{saved}"
    );

    // Snippets off: its keys leave the leader.
    tui.send_keys(DOWN_ARROW);
    tui.send_keys(DOWN_ARROW);
    tui.wait_for("saved prompts", Region::Screen, WAIT);
    tui.send_keys(b" ");
    tui.wait_for("[ ] snippets", Region::Screen, WAIT);
    tui.wait_for("extension snippets off (saved)", Region::StatusBar, WAIT);
    tui.send_keys(ESC);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
    tui.send_keys(ALT_Z);
    tui.wait_for("search conversations", Region::Screen, WAIT);
    let leader = tui.screen_text(Region::Screen);
    assert!(leader.contains("paste a stamp"), "{leader}");
    assert!(leader.contains("e  manage extensions"), "{leader}");
    assert!(
        !leader.contains("insert snippet"),
        "snippets is off:\n{leader}"
    );
    let saved = fs::read_to_string(&config).unwrap();
    assert!(
        saved.ends_with("[extensions]\ndiagram = false\nsnippets = false\n"),
        "{saved}"
    );

    // `e` opens the screen too.
    tui.send_keys(b"e");
    tui.wait_for(
        "Extensions (Space toggle · Enter plugin setup · r refresh · Esc close)",
        Region::Screen,
        WAIT,
    );
    tui.send_keys(ALT_I);
    tui.wait_until(|s| !s.contains("Space toggle", Region::Screen), WAIT);
    tui.send_keys(ALT_Q);
    tui.wait_exit(WAIT);
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
/// in `<dumps>/<n>/` (`argv`, one argument per line, and `env`) before the
/// usual banner and `cat`.
fn write_plugin_claude(h: &ManagerHarness, dumps: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(dumps.join("launches")).unwrap();
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
printf '%s\n' "$@" > "$d/argv"
env > "$d/env"
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
    for _ in 0..4 {
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

// ── Installing a missing claude ──────────────────────────────────────────────

/// A machine without claude gets it from the official installer at start
/// (`[claude] install = "auto"`), and its sessions use it. The installer is
/// faked and nothing real is reachable: `PATH` is one directory of the
/// test's, with a fake `curl` (its script puts a fake claude in
/// `~/.local/bin`, where the real installer puts claude) and links to the
/// few system tools the run needs, and the login shell is a stand-in that
/// only prints that `PATH`.
#[test]
fn test_missing_claude_is_installed_at_start() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::Path;
    // The daemon also looks for claude in these: one there means this
    // machine cannot play one without claude.
    for dir in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
        if Path::new(dir).join("claude").exists() {
            eprintln!("skipped: {dir}/claude exists");
            return;
        }
    }
    let h = ManagerHarness::new();
    fs::write(
        h.config_home.join("claudio/config.toml"),
        "[claude]\nupdate_check = \"off\"\nremote_check = \"off\"\ninstall = \"auto\"\n",
    )
    .unwrap();
    let script = |path: &Path, text: &str| {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    let bin = h.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for tool in ["sh", "bash", "cat", "cp", "mkdir"] {
        let real = ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(tool))
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("no {tool} in /usr/bin or /bin"));
        symlink(real, bin.join(tool)).unwrap();
    }
    let fake = h.root.join("installed-claude");
    script(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo '2.1.296 (Claude Code)'; exit 0; fi\n\
         if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
         echo \"FAKE_CLAUDE_BANNER args=$*\"\n\
         exec cat\n",
    );
    let log = h.root.join("curl.log");
    script(
        &bin.join("curl"),
        &format!(
            "#!/bin/sh\n\
             echo \"$*\" >> '{}'\n\
             echo 'mkdir -p \"$HOME/.local/bin\" && cp \"{}\" \"$HOME/.local/bin/claude\"'\n\
             echo 'echo claude installed'\n",
            log.display(),
            fake.display()
        ),
    );
    // Run as `$SHELL -l -i -c <script>`: the script, without rc files.
    let shell = h.root.join("login-shell");
    script(
        &shell,
        "#!/bin/sh\nfor arg; do script=$arg; done\nexec /bin/sh -c \"$script\"\n",
    );

    let mut tui = h.start_tui_with(|cmd| {
        cmd.env_remove("CLAUDIO_CLAUDE_PATH");
        cmd.env("PATH", &bin);
        cmd.env("SHELL", &shell);
    });
    tui.wait_for(
        "claude 2.1.296 installed on local; new sessions use it",
        Region::Screen,
        WAIT,
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "-fsSL https://claude.ai/install.sh\n"
    );
    assert!(h.home.join(".local/bin/claude").is_file());
    // The wizard is open (no sessions yet); the session runs the new claude.
    wizard_pick_dir(&mut tui, &h.dirs[0]);
    tui.quit(WAIT);
}
