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
//! Reads the proxy URL (format `<token>@<host>`) from `CLAUDIO_PROXY_URL`.
//! The token and host are **never** printed, logged, or committed.
//!
//! # Requirements
//! - `CLAUDIO_PROXY_URL` must be set.
//! - `inkscape` or `magick` (ImageMagick) must be in `$PATH`.
//! - The `claude` binary must be in `$PATH`.

mod common;

use std::{fs, path::PathBuf, thread, time::Duration};

use common::{Region, TuiProcess, DAEMON_WAIT, ENTER, WAIT};
use portable_pty::CommandBuilder;

// ── Screenshot PTY dimensions ─────────────────────────────────────────────────

const SS_ROWS: u16 = 40;
const SS_COLS: u16 = 140;

/// Alt+s: the proxy stats popup.
const ALT_S: &[u8] = b"\x1bs";

// ── Test entry point ──────────────────────────────────────────────────────────

/// Capture the real screenshots from the running claudio TUI.
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

    // Read the proxy URL (`token@host`) from the environment.  Never print or
    // log its contents.
    let proxy_url = std::env::var("CLAUDIO_PROXY_URL")
        .map(|v| v.trim().to_owned())
        .ok()
        .filter(|v| !v.is_empty())
        .expect("set CLAUDIO_PROXY_URL=<token>@<host> to capture the screenshots");

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
        // Tab opens the highlighted `~/projects`: its subdirectories are listed.
        tui.wait_for("~/projects", Region::Screen, Duration::from_secs(10));
        tui.send_keys(b"\t");
        tui.wait_for("webapp", Region::Screen, Duration::from_secs(10));

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
        thread::sleep(Duration::from_secs(4));

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

        // Dismiss the popup; session 1 (webapp, the demo repo) is still active.
        tui.send_keys(common::ESC);
        thread::sleep(Duration::from_millis(300));

        // ── git.png / git-diff.png: the git viewer (Alt+l) ───────────────────
        eprintln!("[screenshots] phase 2: git viewer");
        tui.send_keys(common::ALT_L);
        tui.wait_for("chore(release)", Region::Pane, WAIT);
        tui.wait_for("feat(auth)", Region::Pane, WAIT);
        thread::sleep(Duration::from_millis(500));
        save_shot(&tui, &demo, &out_dir.join("git.png"), "git");

        // Open a commit with a real patch: filter the log for it, open it,
        // then open its first file's diff.
        tui.send_keys(b"/");
        thread::sleep(Duration::from_millis(150));
        tui.send_paste("resolve paths");
        thread::sleep(Duration::from_millis(300));
        tui.send_keys(ENTER);
        thread::sleep(Duration::from_millis(300));
        tui.send_keys(ENTER);
        tui.wait_for("Author:", Region::Pane, WAIT);
        tui.send_keys(ENTER);
        tui.wait_for("diff --git", Region::Pane, WAIT);
        thread::sleep(Duration::from_millis(500));
        save_shot(&tui, &demo, &out_dir.join("git-diff.png"), "git-diff");
        // Esc: diff → commit → log → (filter cleared) → closed.
        for _ in 0..4 {
            tui.send_keys(common::ESC);
            thread::sleep(Duration::from_millis(300));
        }

        // ── stats.png: the proxy stats popup (Alt+s), Overview page ──────────
        eprintln!("[screenshots] phase 2: proxy stats");
        tui.send_keys(ALT_S);
        tui.wait_for("Overview", Region::Screen, WAIT);
        // The pages load from the proxy; give the requests time to land.
        thread::sleep(Duration::from_secs(4));
        save_shot(&tui, &demo, &out_dir.join("stats.png"), "stats");
        tui.send_keys(common::ESC);
        thread::sleep(Duration::from_millis(300));

        // ── terminal.png: a terminal tab (Alt+c) next to the claude tab ──────
        eprintln!("[screenshots] phase 2: terminal tab");
        tui.send_keys(common::ALT_C);
        tui.wait_for("$ term@local", Region::TabBar, WAIT);
        tui.wait_for("$", Region::Pane, WAIT);
        thread::sleep(Duration::from_millis(500));
        tui.send_keys(b"git log --oneline --graph --decorate -n 14\r");
        tui.wait_for("chore(release)", Region::Pane, WAIT);
        thread::sleep(Duration::from_millis(300));
        tui.send_keys(b"ls\r");
        thread::sleep(Duration::from_millis(300));
        tui.send_keys(b"git status -sb\r");
        thread::sleep(Duration::from_millis(600));
        save_shot(&tui, &demo, &out_dir.join("terminal.png"), "terminal");

        tui.send_keys(common::ALT_Q);
        tui.wait_exit(WAIT);
    }

    // ── Phase 3: wizard.png ───────────────────────────────────────────────────
    // The sessions above were recorded as recent directories. Restart with no
    // sessions so the wizard opens on its first screen over an empty pane.

    eprintln!("[screenshots] phase 3: wizard with recent directories");
    demo.kill_daemon();
    demo.forget_sessions();
    thread::sleep(Duration::from_millis(500));
    {
        let mut tui = demo.start_tui();
        tui.wait_for("LOCAL", Region::Screen, DAEMON_WAIT);
        tui.wait_for("api-gateway", Region::Screen, WAIT);
        // The git-branch and claude-activity badges arrive asynchronously.
        thread::sleep(Duration::from_secs(3));
        thread::sleep(Duration::from_millis(500));
        save_shot(&tui, &demo, &out_dir.join("wizard.png"), "wizard");
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

        // The terminal tab runs a login bash: a short, colored prompt and `ls`
        // colors, so the shot shows neither the host's shell setup nor its paths.
        fs::write(
            home.join(".bash_profile"),
            "[ -f ~/.bashrc ] && . ~/.bashrc\n",
        )
        .expect("write .bash_profile");
        fs::write(
            home.join(".bashrc"),
            "PS1='\\[\\e[1;34m\\]\\w\\[\\e[0m\\] \\[\\e[1;32m\\]$\\[\\e[0m\\] '\n\
             alias ls='ls --color=auto'\n\
             alias grep='grep --color=auto'\n\
             export GIT_PAGER=cat\n\n",
        )
        .expect("write .bashrc");

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
        cmd.env("SHELL", "/bin/bash");
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

    /// Drop the daemon's journaled sessions, so a new daemon starts empty.
    /// (`state.json`, which holds the recent directories, is kept.)
    fn forget_sessions(&self) {
        for root in [&self.runtime, &self.home] {
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = fs::read_dir(&dir) else { continue };
                for entry in rd.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("daemon-sessions"))
                    {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }

    /// Verify the SVG source text contains no personal or sensitive strings.
    ///
    /// Panics on any violation so the test never silently writes leaky PNGs.
    fn privacy_check(&self, text: &str, label: &str) {
        let forbidden = ["/volumes", "p4u", "vocdoni", "z6"];
        for pat in &forbidden {
            if let Some(at) = text.find(pat) {
                let from = text[..at].char_indices().rev().nth(30).map_or(0, |(i, _)| i);
                let to = (at + pat.len() + 30).min(text.len());
                let to = (to..=text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
                panic!(
                    "[screenshots] PRIVACY VIOLATION in {label}: pattern {:?} appeared in SVG/screen text: …{}…",
                    pat,
                    &text[from..to]
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
    }
}

// ── Demo project setup ────────────────────────────────────────────────────────

fn setup_demo_projects(home: &PathBuf) {
    // webapp — a small React app with a believable history (see below).
    let webapp = home.join("projects/webapp");
    setup_webapp_history(&webapp);

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

/// Give `webapp` 12 conventional commits from three authors, two tags, a
/// merged feature branch and one unmerged branch, spread over a few weeks.
fn setup_webapp_history(dir: &PathBuf) {
    git_init(dir, "main");
    let ana = ("Ana Ruiz", "ana@example.com");
    let sam = ("Sam Okafor", "sam@example.com");
    let lee = ("Lee Chen", "lee@example.com");

    let commit = |files: &[(&str, &str)], msg: &str, who: (&str, &str), days: u64| {
        for (path, body) in files {
            let full = dir.join(path);
            fs::create_dir_all(full.parent().unwrap()).ok();
            fs::write(full, body).expect("write demo file");
        }
        git_run(dir, &["add", "-A"], who, days);
        git_run(dir, &["commit", "-q", "-m", msg], who, days);
    };

    let package = |version: &str| {
        format!(
            "{{\n  \"name\": \"webapp\",\n  \"version\": \"{version}\",\n  \"description\": \"Demo React application\",\n  \"scripts\": {{ \"start\": \"vite\", \"build\": \"vite build\" }}\n}}\n"
        )
    };
    let session = |extra: &str| {
        format!(
            "const KEY = \"webapp.token\";\n\nexport const saveToken = (t) => localStorage.setItem(KEY, t);\nexport const loadToken = () => localStorage.getItem(KEY);\n{extra}"
        )
    };
    let api = |base: bool| {
        let (decl, sig, url, msg) = if base {
            ("const BASE = \"/api\";\n\n", "path", "BASE + path", "path")
        } else {
            ("", "url", "url", "url")
        };
        format!(
            "{decl}export async function request({sig}, opts = {{}}, retries = 3) {{\n  for (let attempt = 0; ; attempt++) {{\n    try {{\n      const res = await fetch({url}, opts);\n      if (res.status < 500) return res;\n    }} catch (err) {{\n      if (attempt >= retries) throw err;\n    }}\n    if (attempt >= retries) throw new Error(`request failed: ${{{msg}}}`);\n    await new Promise((r) => setTimeout(r, 2 ** attempt * 100));\n  }}\n}}\n"
        )
    };

    commit(
        &[
            ("package.json", &package("0.1.0")),
            ("README.md", "# webapp\nA demo React web application.\n"),
            ("src/App.jsx", "export default function App() {\n  return <h1>Demo</h1>;\n}\n"),
        ],
        "chore: scaffold the app with Vite",
        ana,
        34,
    );
    commit(
        &[("src/Login.jsx", "export function Login({ onSubmit }) {\n  return (\n    <form onSubmit={onSubmit}>\n      <input name=\"email\" />\n      <input name=\"password\" type=\"password\" />\n      <button>Sign in</button>\n    </form>\n  );\n}\n")],
        "feat(auth): add the login form",
        ana,
        31,
    );
    commit(&[("src/session.js", &session(""))], "feat(auth): persist the session token", sam, 27);
    commit(
        &[
            ("src/session.js", &session("export const clearToken = () => localStorage.removeItem(KEY);\n")),
            ("src/App.jsx", "import { loadToken } from \"./session\";\nimport { Login } from \"./Login\";\n\nexport default function App() {\n  if (!loadToken()) return <Login />;\n  return <h1>Demo</h1>;\n}\n"),
        ],
        "fix(auth): show the login form when the token is missing",
        sam,
        24,
    );
    commit(
        &[("README.md", "# webapp\nA demo React web application.\n\n## Setup\n\n    npm install\n    npm start\n")],
        "docs: add setup instructions to the README",
        lee,
        21,
    );
    git_run(dir, &["tag", "-a", "v0.1.0", "-m", "v0.1.0"], ana, 21);

    // A feature branch, merged later with a merge commit.
    git_run(dir, &["checkout", "-q", "-b", "feat/dark-mode"], ana, 18);
    commit(
        &[("src/theme.js", "import { createContext, useContext } from \"react\";\n\nexport const ThemeContext = createContext(\"light\");\nexport const useTheme = () => useContext(ThemeContext);\n")],
        "feat(ui): add a theme context",
        lee,
        18,
    );
    commit(
        &[("src/Navbar.jsx", "import { useTheme } from \"./theme\";\n\nexport function Navbar({ onToggle }) {\n  const theme = useTheme();\n  return <nav className={theme}><button onClick={onToggle}>Dark mode</button></nav>;\n}\n")],
        "feat(ui): add a dark mode toggle to the navbar",
        lee,
        15,
    );
    // An unmerged branch, off the dark-mode work.
    git_run(dir, &["branch", "fix/nav-overflow"], lee, 15);

    git_run(dir, &["checkout", "-q", "main"], sam, 14);
    commit(&[("src/api.js", &api(false))], "fix(api): retry failed requests with backoff", sam, 14);
    commit(&[("src/api.js", &api(true))], "refactor(api): resolve paths against a base URL", sam, 11);
    git_run(
        dir,
        &["merge", "--no-ff", "-q", "feat/dark-mode", "-m", "Merge branch 'feat/dark-mode'"],
        ana,
        9,
    );
    commit(
        &[("src/App.test.jsx", "import { render } from \"@testing-library/react\";\nimport App from \"./App\";\n\ntest(\"renders without crashing\", () => {\n  render(<App />);\n});\n")],
        "test: add an App smoke test",
        lee,
        5,
    );
    commit(&[("package.json", &package("0.2.0"))], "chore(release): 0.2.0", ana, 2);
    git_run(dir, &["tag", "-a", "v0.2.0", "-m", "v0.2.0"], ana, 2);
}

/// Run git in `dir` as `who`, `days` days ago, so the log has believable dates.
fn git_run(dir: &PathBuf, args: &[&str], who: (&str, &str), days: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let when = format!("{} +0000", now.saturating_sub(days * 86_400 + 3_600 * (days % 7 + 1)));
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", who.0)
        .env("GIT_AUTHOR_EMAIL", who.1)
        .env("GIT_COMMITTER_NAME", who.0)
        .env("GIT_COMMITTER_EMAIL", who.1)
        .env("GIT_AUTHOR_DATE", &when)
        .env("GIT_COMMITTER_DATE", &when)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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

        // Bypass-permissions warning (claudio launches claude with
        // `--allow-dangerously-skip-permissions`). The cursor starts on
        // "No, exit"; move to "Yes, I accept" and confirm.
        if pane.contains("Bypass Permissions mode") && pane.contains("Yes, I accept") {
            eprintln!("[screenshots] accepting the bypass-permissions warning");
            thread::sleep(Duration::from_millis(500));
            tui.send_keys(common::DOWN_ARROW);
            thread::sleep(Duration::from_millis(300));
            tui.send_keys(ENTER);
            let gone = Instant::now() + Duration::from_secs(15);
            loop {
                thread::sleep(Duration::from_millis(200));
                if !tui.screen_text(Region::Pane).contains("Yes, I accept") {
                    thread::sleep(Duration::from_millis(300));
                    break;
                }
                if Instant::now() > gone {
                    eprintln!("[screenshots] bypass warning didn't clear — continuing");
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

/// Hide what identifies the real proxy account: e-mail addresses become
/// `demo@example.com` and the account name's "p4u" becomes "dmo". Both keep
/// the text's width so the render's layout does not move.
fn redact(text: &str) -> String {
    redact_emails(text).replace("p4u", "dmo")
}

/// Replace every e-mail address in `text` with `demo@example.com`, padded or
/// truncated to the same width.
fn redact_emails(text: &str) -> String {
    const DEMO: &str = "demo@example.com";
    let is_mail = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-');
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' {
            // Local part already copied to `out`: peel it back off.
            let mut start = out.len();
            for (idx, c) in out.char_indices().rev() {
                if is_mail(c) { start = idx } else { break }
            }
            let local = out.len() - start;
            let mut end = i + 1;
            while end < chars.len() && (is_mail(chars[end])) {
                end += 1;
            }
            let domain = end - (i + 1);
            if local > 0 && domain > 2 && chars[i + 1..end].contains(&'.') {
                out.truncate(start);
                let width = local + 1 + domain;
                let mut repl: String = DEMO.chars().take(width).collect();
                while repl.chars().count() < width {
                    repl.push(' ');
                }
                out.push_str(&repl);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Export the current screen as SVG, run a privacy check, convert to PNG.
fn save_shot(tui: &TuiProcess, demo: &DemoEnv, png_path: &PathBuf, name: &str) {
    eprintln!("[screenshots] capturing {name}...");

    let screen = redact(&tui.screen_text(Region::Screen));
    // The status bar shows the proxy account's real e-mail address; swap it for
    // a same-width placeholder (everything else is the real render).
    let svg = redact(&tui.screen_svg());

    // Privacy check on the SVG source.
    demo.privacy_check(&svg, name);

    // Also check the plain screen text.
    demo.privacy_check(&screen, name);

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
