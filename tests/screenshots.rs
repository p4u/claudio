//! Screenshot capture integration test.
//!
//! Drives the **real** `claudio` binary (connected to the **real** `claude`
//! binary) inside a PTY and exports the rendered VT screen as SVG → PNG.
//!
//! # Usage
//! ```
//! CLAUDIO_SCREENSHOTS=docs/screenshots \
//!   cargo test --test screenshots -- --nocapture
//! ```
//!
//! Reads the proxy URL (format `<token>@<host>`) from `/tmp/claudio-demo-proxy-url`
//! at runtime.  The token and host are **never** printed, logged, or committed.
//!
//! # Requirements
//! - `/tmp/claudio-demo-proxy-url` must exist.
//! - `inkscape` or `magick` (ImageMagick) must be in `$PATH`.
//! - The `claude` binary must be in `$PATH`.

mod common;

use std::{fs, path::PathBuf, thread, time::Duration};

use common::{Region, TuiProcess, DAEMON_WAIT, ENTER, WAIT};
use portable_pty::CommandBuilder;

// ── Screenshot PTY dimensions ─────────────────────────────────────────────────

const SS_ROWS: u16 = 40;
const SS_COLS: u16 = 140;

// ── Test entry point ──────────────────────────────────────────────────────────

/// Capture 4 real screenshots from the running claudio TUI.
///
/// Gated by `CLAUDIO_SCREENSHOTS=<output-dir>`.  Skips silently when the env
/// var is absent so `cargo test` in CI doesn't require a live proxy.
#[test]
fn capture_screenshots() {
    let out_dir_str = match std::env::var("CLAUDIO_SCREENSHOTS") {
        Ok(d) if !d.is_empty() => d,
        _ => {
            eprintln!("[screenshots] CLAUDIO_SCREENSHOTS not set — skipping");
            return;
        }
    };
    let out_dir = PathBuf::from(&out_dir_str);
    fs::create_dir_all(&out_dir).expect("create output dir");

    // Read the proxy URL.  Never print or log its contents.
    let proxy_url = fs::read_to_string("/tmp/claudio-demo-proxy-url")
        .expect("missing /tmp/claudio-demo-proxy-url — create it as `token@host`")
        .trim()
        .to_owned();

    eprintln!("[screenshots] setting up demo environment...");
    let demo = DemoEnv::setup(&proxy_url);

    // ── Phase 1: wizard.png + directories.png ─────────────────────────────────

    eprintln!("[screenshots] phase 1: wizard and directory picker");
    {
        let mut tui = demo.start_tui();

        // Wait for the wizard's LOCAL section heading to appear.
        tui.wait_for("LOCAL", Region::Screen, DAEMON_WAIT);
        // Give rendering a single tick to settle.
        thread::sleep(Duration::from_millis(300));

        // ── wizard.png ────────────────────────────────────────────────────────
        save_shot(&tui, &demo, &out_dir.join("wizard.png"), "wizard");

        // Navigate to the directory picker: Enter on "Explore local dirs…".
        tui.send_keys(ENTER);
        // Wait for the dir picker, which opens browsing HOME (`~/`).
        tui.wait_for("start here", Region::Screen, WAIT);
        thread::sleep(Duration::from_millis(300));

        // The picker opened at "~/" and listed HOME asynchronously, so it is
        // populated with real filesystem entries. Type "pro" after it.
        tui.send_keys(b"pro");
        // Wait for the typed text to appear in the filter field.
        tui.wait_for("~/pro", Region::Screen, Duration::from_secs(5));

        // Wait for the async ListDir response to arrive and populate the list.
        // We expect "projects" (or a sub-path) to appear as a completion entry.
        tui.wait_until(
            |s| {
                let screen = s.region_text(Region::Screen);
                screen.contains("projects") || screen.contains("~/projects")
            },
            Duration::from_secs(10),
        );
        thread::sleep(Duration::from_millis(200));

        // ── directories.png ───────────────────────────────────────────────────
        save_shot(
            &tui,
            &demo,
            &out_dir.join("directories.png"),
            "directories",
        );

        // Escape the wizard (cancels it) and quit.
        tui.send_keys(common::ESC);
        thread::sleep(Duration::from_millis(150));
        tui.send_keys(common::ALT_Q);
        tui.wait_exit(WAIT);
    }

    // Kill the Phase-1 daemon so Phase 2 starts with a clean slate (no stale
    // sessions that would cause "attach failed" errors in the status bar).
    demo.kill_daemon();
    thread::sleep(Duration::from_millis(500));

    // ── Phase 2: sessions.png + overview.png ─────────────────────────────────

    eprintln!("[screenshots] phase 2: sessions and overview");
    {
        let mut tui = demo.start_tui();

        // Wait for the wizard to appear (fresh daemon, no sessions).
        tui.wait_for("LOCAL", Region::Screen, DAEMON_WAIT);

        // Session 1: webapp — we'll send a real prompt and capture the response.
        let webapp = demo.home.join("projects/webapp");
        eprintln!("[screenshots] creating session 1 (webapp)...");
        wizard_navigate_to_dir(&mut tui, &webapp);
        // dismiss_security_notes handles ALL onboarding dialogs in sequence:
        // theme selection → trust dialog → security notes → prompt.
        dismiss_security_notes(&mut tui);

        // Send a short prompt for session 1 while sessions 2 and 3 are created.
        tui.send_paste("Summarize what this project does in one sentence.");
        tui.send_keys(ENTER);

        // Session 2: api-gateway — leave idle.
        eprintln!("[screenshots] creating session 2 (api-gateway)...");
        tui.send_keys(common::ALT_N);
        tui.wait_for("LOCAL", Region::Screen, WAIT);
        let api_gw = demo.home.join("projects/api-gateway");
        wizard_navigate_to_dir(&mut tui, &api_gw);
        dismiss_security_notes(&mut tui);

        // Session 3: infra — leave idle.
        eprintln!("[screenshots] creating session 3 (infra)...");
        tui.send_keys(common::ALT_N);
        tui.wait_for("LOCAL", Region::Screen, WAIT);
        let infra = demo.home.join("projects/infra");
        wizard_navigate_to_dir(&mut tui, &infra);
        dismiss_security_notes(&mut tui);

        // Switch back to session 1 (two lefts from session 3).
        tui.send_keys(common::ALT_LEFT);
        thread::sleep(Duration::from_millis(150));
        tui.send_keys(common::ALT_LEFT);
        // Confirm session 1 is selected.  Tab format varies (e.g. "1 · webapp",
        // "1 ✓ task summary") so just look for the leading "1 " prefix.
        tui.wait_for("1 ", Region::TabBar, WAIT);
        thread::sleep(Duration::from_millis(200));

        // Wait for session 1 to show real claude content:
        //  • no security-notes screen ("Press Enter to continue")
        //  • the claude prompt (`❯` or `> `) is present (session is idle/responded)
        //  • at least a few non-trivial response lines above the prompt
        eprintln!("[screenshots] waiting for claude response in session 1...");
        tui.wait_until(
            |s| {
                let pane = s.region_text(Region::Pane);
                // Must not be stuck on any onboarding dialog.
                if pane.contains("Press Enter") || pane.contains("Security notes:")
                    || pane.contains("Yes, I trust this folder")
                    || pane.contains("Choose the text style")
                {
                    return false;
                }
                // The claude prompt must be present (session is idle/responding done).
                let has_prompt = pane.contains("\n> ")
                    || pane.trim_start().starts_with("> ")
                    || pane.contains('❯')
                    || pane.contains('\u{276F}');
                if !has_prompt {
                    return false;
                }
                // At least a few lines of real response content above the prompt.
                pane.lines().filter(|l| {
                    let t = l.trim();
                    !t.is_empty()
                        && t != ">"
                        && t != "❯"
                        && !t.starts_with("Welcome to Claude")
                        && !t.starts_with("Claude Code v")
                        && t.len() > 5
                }).count() >= 4
            },
            Duration::from_secs(120),
        );
        thread::sleep(Duration::from_millis(300));

        // ── sessions.png ──────────────────────────────────────────────────────
        save_shot(&tui, &demo, &out_dir.join("sessions.png"), "sessions");

        // Overview popup (Alt+g).
        tui.send_keys(common::ALT_G);
        // Wait for the overview to render (shows session index numbers and paths).
        tui.wait_until(
            |s| {
                let screen = s.region_text(Region::Screen);
                screen.contains(" 1 ") && screen.contains(" 2 ") && screen.contains("webapp")
            },
            WAIT,
        );
        thread::sleep(Duration::from_millis(300));

        // ── overview.png ──────────────────────────────────────────────────────
        save_shot(&tui, &demo, &out_dir.join("overview.png"), "overview");

        // Dismiss popup and quit.
        tui.send_keys(common::ESC);
        thread::sleep(Duration::from_millis(150));
        tui.send_keys(common::ALT_Q);
        tui.wait_exit(WAIT);
    }

    eprintln!(
        "[screenshots] Done — PNGs written to {}",
        out_dir.display()
    );
}

// ── Demo environment ──────────────────────────────────────────────────────────

struct DemoEnv {
    home: PathBuf,
    runtime: PathBuf,
    proxy_url: String,
    /// The token portion of the proxy URL — used only for privacy checks.
    token: String,
    /// The host portion of the proxy URL — used only for privacy checks.
    _proxy_host: String,
}

impl DemoEnv {
    fn setup(proxy_url: &str) -> Self {
        let root = PathBuf::from("/tmp/claudio-demo");
        let home = root.join("home");
        let runtime = root.join("runtime");

        // Split token@host.  Use the LAST '@' so tokens can contain '@'.
        let at = proxy_url
            .rfind('@')
            .expect("CLAUDIO_PROXY_URL must be token@host");
        let token = proxy_url[..at].to_owned();
        let proxy_host = proxy_url[at + 1..].to_owned();

        // Create directory tree.
        for rel in &[
            "",
            "projects",
            "projects/webapp",
            "projects/api-gateway",
            "projects/infra",
            "projects/mobile-app",
            "notes",
            ".ssh",
            ".config/claudio",
            ".claude",
        ] {
            if rel.is_empty() {
                fs::create_dir_all(&home).expect("create home");
            } else {
                fs::create_dir_all(home.join(rel)).expect("create demo dir");
            }
        }
        fs::create_dir_all(&runtime).expect("create runtime");

        // Demo projects.
        setup_demo_projects(&home);

        // Pre-seed the claude config to skip onboarding + trust dialogs.
        preseed_claude_config(&home);

        // SSH config with demo hosts (never connected).
        let ssh_cfg = "\
Host prod-db\n  HostName prod-db.example.com\n  User deploy\n\n\
Host build-01\n  HostName build-01.example.com\n  User ci\n\n\
Host staging\n  HostName staging.example.com\n  User ubuntu\n";
        fs::write(home.join(".ssh/config"), ssh_cfg).expect("write ssh config");

        DemoEnv {
            home,
            runtime,
            proxy_url: proxy_url.to_owned(),
            token,
            _proxy_host: proxy_host,
        }
    }

    /// Build a `CommandBuilder` for the real claudio binary in the demo env.
    fn cmd(&self) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(common::BINARY);

        // Clear any inherited CLAUDIO_* vars from the host environment.
        for (k, _) in std::env::vars() {
            if k.starts_with("CLAUDIO_") {
                cmd.env_remove(&k);
            }
        }
        cmd.env_remove("CLAUDIO_CLAUDE_PATH"); // use the real claude from PATH
        cmd.env_remove("ANTHROPIC_API_KEY"); // proxy supplies credentials

        cmd.env("HOME", &self.home);
        cmd.env("XDG_RUNTIME_DIR", &self.runtime);
        cmd.env("XDG_CONFIG_HOME", self.home.join(".config"));
        cmd.env("CLAUDE_CONFIG_DIR", self.home.join(".claude"));
        cmd.env("CLAUDIO_PROXY_URL", &self.proxy_url);
        cmd.env("CLAUDIO_NO_UPDATE_CHECK", "1");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd
    }

    /// Spawn a wide claudio TUI (140×40) in the demo environment.
    fn start_tui(&self) -> TuiProcess {
        TuiProcess::spawn_sized(self.cmd(), SS_ROWS, SS_COLS)
    }

    /// Kill the running daemon (if any) by reading the PID from the lock file.
    fn kill_daemon(&self) {
        let lock = self.runtime.join("claudio").join("daemon-v1.lock");
        if let Ok(s) = fs::read_to_string(&lock) {
            if let Ok(pid) = s.trim().parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }

    /// Verify the SVG source text contains no personal or sensitive strings.
    ///
    /// Panics on any violation so the test never silently writes leaky PNGs.
    fn privacy_check(&self, text: &str, label: &str) {
        let forbidden = ["/volumes", "p4u"];
        for pat in &forbidden {
            if text.contains(pat) {
                panic!(
                    "[screenshots] PRIVACY VIOLATION in {label}: pattern {:?} appeared in SVG/screen text",
                    pat
                );
            }
        }
        // Check the token without revealing it in the panic message.
        if !self.token.is_empty() && text.contains(&self.token) {
            panic!(
                "[screenshots] PRIVACY VIOLATION in {label}: proxy token appeared in SVG/screen text"
            );
        }
    }
}

impl Drop for DemoEnv {
    fn drop(&mut self) {
        self.kill_daemon();
        let _ = fs::remove_dir_all(self.runtime.parent().unwrap_or(&self.runtime));
        // Do NOT remove /tmp/claudio-demo-proxy-url.
    }
}

// ── Demo project setup ────────────────────────────────────────────────────────

fn setup_demo_projects(home: &PathBuf) {
    // webapp — a small git repo on main, one commit.
    let webapp = home.join("projects/webapp");
    git_init(&webapp, "main");
    fs::write(webapp.join("package.json"), r#"{
  "name": "webapp",
  "version": "0.1.0",
  "description": "Demo React application",
  "scripts": { "start": "react-scripts start", "build": "react-scripts build" }
}
"#).ok();
    fs::write(webapp.join("README.md"), "# webapp\nA demo React web application.\n").ok();
    fs::create_dir_all(webapp.join("src")).ok();
    fs::write(webapp.join("src/App.jsx"), "export default function App() { return <h1>Demo</h1>; }\n").ok();
    git_commit(&webapp, "Initial commit");

    // api-gateway — a git repo with a feature branch.
    let api_gw = home.join("projects/api-gateway");
    git_init(&api_gw, "main");
    fs::write(api_gw.join("main.go"), "package main\n\nfunc main() {}\n").ok();
    fs::write(api_gw.join("README.md"), "# api-gateway\nHTTP gateway service.\n").ok();
    git_commit(&api_gw, "Initial commit");
    // Create a feature branch.
    std::process::Command::new("git")
        .args(["-C", api_gw.to_str().unwrap(), "checkout", "-b", "feat/rate-limit"])
        .output()
        .ok();
    fs::write(api_gw.join("ratelimit.go"), "// rate-limit middleware\n").ok();
    git_commit(&api_gw, "Add rate-limit skeleton");
    // Switch back to main.
    std::process::Command::new("git")
        .args(["-C", api_gw.to_str().unwrap(), "checkout", "main"])
        .output()
        .ok();

    // infra — plain git repo.
    let infra = home.join("projects/infra");
    git_init(&infra, "main");
    fs::write(infra.join("Makefile"), "deploy:\n\techo 'deploying...'\n").ok();
    fs::write(infra.join("README.md"), "# infra\nDeployment tooling.\n").ok();
    git_commit(&infra, "Initial commit");

    // mobile-app — git repo.
    let mobile = home.join("projects/mobile-app");
    git_init(&mobile, "main");
    fs::write(mobile.join("pubspec.yaml"), "name: mobile_app\n").ok();
    git_commit(&mobile, "Initial commit");

    // notes — plain directory (not a git repo).
    let notes = home.join("notes");
    fs::write(notes.join("ideas.md"), "# Ideas\n- improve API\n- refactor auth\n").ok();
}

fn git_init(dir: &PathBuf, branch: &str) {
    fs::create_dir_all(dir).ok();
    std::process::Command::new("git")
        .args(["-C", dir.to_str().unwrap(), "init", "-b", branch])
        .env("HOME", dir.parent().unwrap_or(dir).parent().unwrap_or(dir))
        .output()
        .ok();
    // Set demo identity so git doesn't complain.
    std::process::Command::new("git")
        .args(["-C", dir.to_str().unwrap(), "config", "user.name", "demo"])
        .output()
        .ok();
    std::process::Command::new("git")
        .args([
            "-C",
            dir.to_str().unwrap(),
            "config",
            "user.email",
            "demo@example.com",
        ])
        .output()
        .ok();
}

fn git_commit(dir: &PathBuf, msg: &str) {
    std::process::Command::new("git")
        .args(["-C", dir.to_str().unwrap(), "add", "-A"])
        .output()
        .ok();
    std::process::Command::new("git")
        .args(["-C", dir.to_str().unwrap(), "commit", "-m", msg])
        .output()
        .ok();
}

// ── Claude config pre-seeding ─────────────────────────────────────────────────

/// Write a minimal `.claude.json` to skip onboarding and trust dialogs.
fn preseed_claude_config(home: &PathBuf) {
    // Build the projects map with hasTrustDialogAccepted for all demo dirs.
    let home_str = home.to_str().unwrap_or("/tmp/claudio-demo/home");
    let projects_json = format!(
        r#"{{
    "{home_str}/projects/webapp": {{
      "hasTrustDialogAccepted": true,
      "allowedTools": [],
      "hasClaudeMdExternalIncludesApproved": false,
      "hasClaudeMdExternalIncludesWarningShown": false
    }},
    "{home_str}/projects/api-gateway": {{
      "hasTrustDialogAccepted": true,
      "allowedTools": [],
      "hasClaudeMdExternalIncludesApproved": false,
      "hasClaudeMdExternalIncludesWarningShown": false
    }},
    "{home_str}/projects/infra": {{
      "hasTrustDialogAccepted": true,
      "allowedTools": [],
      "hasClaudeMdExternalIncludesApproved": false,
      "hasClaudeMdExternalIncludesWarningShown": false
    }},
    "{home_str}/projects/mobile-app": {{
      "hasTrustDialogAccepted": true,
      "allowedTools": [],
      "hasClaudeMdExternalIncludesApproved": false,
      "hasClaudeMdExternalIncludesWarningShown": false
    }},
    "{home_str}/notes": {{
      "hasTrustDialogAccepted": true,
      "allowedTools": [],
      "hasClaudeMdExternalIncludesApproved": false,
      "hasClaudeMdExternalIncludesWarningShown": false
    }}
  }}"#
    );

    let config_json = format!(
        r#"{{
  "hasCompletedOnboarding": true,
  "theme": "dark-ansi",
  "installMethod": "native",
  "numStartups": 100,
  "projects": {projects_json}
}}
"#
    );

    // Write the config to ~/.claude.json (used by claude CLI).
    fs::write(home.join(".claude.json"), &config_json).expect("write .claude.json");

    // Also write to the CLAUDE_CONFIG_DIR location in case claude reads
    // the main config from there when CLAUDE_CONFIG_DIR is set.
    let config_dir = home.join(".claude");
    fs::create_dir_all(&config_dir).ok();
    fs::write(config_dir.join("claude.json"), &config_json).ok();
    fs::write(config_dir.join("settings.json"), &config_json).ok();
}

// ── Wizard navigation helpers ─────────────────────────────────────────────────

/// Navigate the wizard to pick a specific local directory.
///
/// Assumes the wizard's host-selection screen is already visible (wait for
/// "LOCAL" before calling).  Presses Enter to confirm the local host, then
/// pastes the directory path and confirms with Enter.
///
/// Waits until the session's directory name appears in the tab bar.
fn wizard_navigate_to_dir(tui: &mut TuiProcess, dir: &PathBuf) {
    let path = dir.to_str().expect("UTF-8 path");
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session");

    // Step 0: confirm the local host (Enter on "Explore local dirs…").
    tui.send_keys(ENTER);
    thread::sleep(Duration::from_millis(100));

    // Step 1: wait for the directory picker (browsing `~/`, "start here" row).
    tui.wait_for("start here", Region::Screen, WAIT);

    // Clear any existing filter text, then paste the full path.
    tui.send_keys(common::CTRL_U);
    thread::sleep(Duration::from_millis(50));
    tui.send_paste(path);
    thread::sleep(Duration::from_millis(100));

    // Confirm.
    tui.send_keys(ENTER);

    // Wait for the session tab to appear in the tab bar.
    tui.wait_for(dir_name, Region::TabBar, DAEMON_WAIT);
}

/// If a claude trust/permission dialog is visible, accept it.
///
/// Checks once after a short settle delay; non-blocking.
fn maybe_accept_trust_dialog(tui: &mut TuiProcess) {
    thread::sleep(Duration::from_millis(600));
    let screen = tui.screen_text(Region::Screen);
    let low = screen.to_lowercase();
    if low.contains("trust") || low.contains("(y)") || low.contains("y/n") {
        eprintln!("[screenshots] accepting trust dialog");
        tui.send_keys(b"y");
        tui.send_keys(ENTER);
        thread::sleep(Duration::from_millis(300));
    }
}

/// Wait for the claude session to be fully ready and dismiss any onboarding
/// dialogs.  Returns when the `> ` prompt is visible in the pane.
///
/// The pane may show claude's UI even while claudio reports "starting", so we
/// poll the pane unconditionally.  Dialogs handled (in order they can appear):
///   1. Theme-selection screen ("Choose the text style…")
///   2. Security notes / gateway onboarding ("Press Enter to continue…")
///   3. Any other "Press Enter" gating
///   4. `> ` prompt — session ready, return.
///
/// Panics if the prompt doesn't appear within 120 s.
fn dismiss_security_notes(tui: &mut TuiProcess) {
    use std::time::Instant;

    let deadline = Instant::now() + Duration::from_secs(120);

    loop {
        assert!(
            Instant::now() < deadline,
            "dismiss_security_notes: timed out waiting for '>' prompt\n\
             status: {}\npane:\n{}",
            tui.screen_text(Region::StatusBar),
            tui.screen_text(Region::Pane),
        );

        let pane = tui.screen_text(Region::Pane);

        // Theme-selection dialog ("Choose the text style that looks best…").
        // The highlighted option is already correct — just press Enter.
        if pane.contains("Choose the text style") || pane.contains("run /theme") {
            eprintln!("[screenshots] accepting theme selection (Enter)");
            tui.send_keys(ENTER);
            thread::sleep(Duration::from_millis(400));
            continue;
        }

        // Project trust/safety dialog ("Quick safety check: Is this a project you …").
        // The cursor starts on "❯ No, exit"; move it down to "Yes, I trust this
        // folder" before confirming with Enter.  Use the dialog HEADER text for
        // the "shown/cleared" check so we don't confuse the option text with the
        // selection cursor (`❯` appears in both the dialog and the claude prompt).
        if pane.contains("Is this a project you") || pane.contains("Quick safety check") {
            let st = tui.screen_text(Region::StatusBar);
            eprintln!(
                "[screenshots] project trust dialog (status contains 'starting': {})",
                st.contains("starting")
            );

            // Give a moment for the PTY to become fully interactive.
            thread::sleep(Duration::from_millis(500));

            // Navigate to "Yes, I trust this folder".  The default cursor is on
            // "❯ No, exit".  We try both normal-cursor-key mode (\x1b[B) and
            // application-cursor-key mode (\x1bOB) since the terminal may have DECCKM
            // set.  Send two presses so that even if the first is dropped we end on
            // "Yes" (the list has only 2 items and likely doesn't wrap, so 2 presses
            // from "No" → "Yes" → stays at "Yes").
            tui.send_keys(common::DOWN_ARROW);   // \x1b[B  normal mode
            thread::sleep(Duration::from_millis(150));
            tui.send_keys(b"\x1bOB");            // \x1bOB  application mode
            thread::sleep(Duration::from_millis(150));
            tui.send_keys(common::DOWN_ARROW);   // second normal DOWN (stay at Yes if no-wrap)
            thread::sleep(Duration::from_millis(300));

            eprintln!(
                "[screenshots] pane after DOWN: on-yes={}",
                tui.screen_text(Region::Pane)
                    .contains("Yes, I trust this folder")
            );

            // Confirm.
            tui.send_keys(ENTER);

            // Wait for the dialog header to disappear.
            let gone = Instant::now() + Duration::from_secs(20);
            loop {
                thread::sleep(Duration::from_millis(200));
                let p2 = tui.screen_text(Region::Pane);
                if !p2.contains("Is this a project you") && !p2.contains("Quick safety check") {
                    thread::sleep(Duration::from_millis(300));
                    break;
                }
                if Instant::now() > gone {
                    eprintln!("[screenshots] trust dialog didn't clear — continuing");
                    break;
                }
            }
            continue;
        }

        // Security notes / gateway onboarding ("Press Enter to continue…").
        if pane.contains("Press Enter") || pane.contains("to continue") {
            eprintln!("[screenshots] dismissing security notes (Enter)");
            tui.send_keys(ENTER);
            let gone = Instant::now() + Duration::from_secs(15);
            loop {
                thread::sleep(Duration::from_millis(100));
                let p2 = tui.screen_text(Region::Pane);
                if !p2.contains("Press Enter") && !p2.contains("to continue") {
                    thread::sleep(Duration::from_millis(300));
                    break;
                }
                if Instant::now() > gone {
                    eprintln!("[screenshots] security notes didn't clear — continuing");
                    break;
                }
            }
            continue;
        }

        // Session ready — the claude prompt is visible.
        // Claude uses `❯` (U+276F) as its prompt character.  The trust dialog also
        // shows `❯` as a cursor marker, but that case is handled above first.
        // Only treat `❯` as a ready-prompt when no dialog is active (already checked).
        if pane.contains("\n> ") || pane.trim_start().starts_with("> ")
            || pane.contains("❯") || pane.contains('\u{276F}')
        {
            return;
        }

        // Still loading / startup animation — keep polling.
        thread::sleep(Duration::from_millis(200));
    }
}

// ── Screenshot save helper ────────────────────────────────────────────────────

/// Export the current screen as SVG, run a privacy check, convert to PNG.
fn save_shot(tui: &TuiProcess, demo: &DemoEnv, png_path: &PathBuf, name: &str) {
    eprintln!("[screenshots] capturing {name}...");

    let svg = tui.screen_svg();

    // Privacy check on the SVG source.
    demo.privacy_check(&svg, name);

    // Also check the plain screen text.
    demo.privacy_check(&tui.screen_text(Region::Screen), name);

    // Write SVG to a temp file alongside the PNG.
    let svg_path = png_path.with_extension("svg");
    fs::write(&svg_path, svg.as_bytes()).expect("write SVG");

    // Convert SVG → PNG at 2× scale (192 DPI from the SVG's 96 DPI baseline).
    convert_svg_to_png(&svg_path, png_path);

    assert!(
        png_path.exists(),
        "PNG was not created at {}",
        png_path.display()
    );
    eprintln!("[screenshots] wrote {}", png_path.display());
}

/// Try inkscape first, fall back to ImageMagick `magick`.
fn convert_svg_to_png(svg: &PathBuf, png: &PathBuf) {
    let svg_s = svg.to_str().unwrap();
    let png_s = png.to_str().unwrap();

    // Try inkscape (preferred: better SVG renderer).
    let inkscape = std::process::Command::new("inkscape")
        .args(["--export-type=png", "--export-dpi=192", "-o", png_s, svg_s])
        .output();

    match inkscape {
        Ok(out) if out.status.success() => return,
        Ok(out) => eprintln!(
            "[screenshots] inkscape warn: {}",
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")
        ),
        Err(_) => eprintln!("[screenshots] inkscape not found, trying magick"),
    }

    // Fall back to ImageMagick.
    let magick = std::process::Command::new("magick")
        .args(["-density", "192", "-background", "#1e1e2e", svg_s, png_s])
        .output()
        .expect("neither inkscape nor magick found — install one of them");

    assert!(
        magick.status.success(),
        "magick failed: {}",
        String::from_utf8_lossy(&magick.stderr)
    );
}
