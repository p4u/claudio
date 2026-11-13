# claudio in its own window: app window, launcher, sign-in tickets: implementation plan

Spec: `docs/specs/2026-11-13-desktop-design.md` (audited 2026-11-13; `.delivery/2026-11-13-desktop/audit-spec.md` F1-F16 folded in, F1 option A by the owner)
Class: architectural
Repos and branches: /home/p4u/claudio → feat/app-window (base eb850fd)

Implementers work in a git worktree of that branch, `~/.cache/claudio/wt/app-window`; every
patch below is repo-relative and is applied from that worktree's root.

Every patch below was generated from a scratch worktree of eb850fd and replayed in a second,
fresh one with `git apply --whitespace=error-all`, task after task, extracted from this file
with the awk command in Global Constraints. For each task, step 1 fails as written under
"Expected" and the implementation step passes with the counts given. After each code task
`cargo fmt --all --check`, `cargo clippy --locked --workspace --all-targets --all-features -- -D
warnings` and `scripts/check_rules.py` passed. On the final tree these passed too: `make
check`'s sequence (fmt, clippy, rules, the core-only build, the workspace unit tests, each crate
alone), the five `make matrix` cells (with the core-only manager suite), the `manager`,
`vocdoni_manager`, `web`, `web_manager` and `vocdoni_web` suites, `node --test
addons/web/web/tests/*.test.mjs`, and the whole Playwright e2e against a debug build (as `make
test-web` runs it). Counts are under "Merge and release". All of it ran on Linux (rustup stable
1.99.1, Node 22, Playwright 1.64.1 with its cached Chromium 1248). Not run: anything on macOS
(no Mac here: the macOS launcher writer and its tests ran on Linux over a temp `HOME`; the
`open` and `osascript` branches of the integration tests are compiled but only their Linux
twins ran), and AC9's manual checks, which need the owner's Linux laptop (a display, a real
Chrome). AC9(a) is done: the owner's key check in `google-chrome --app=https://z6:7337/` showed
`Ctrl+W`, `Ctrl+T` and `Ctrl+N` reach the terminal (`key=w code=KeyW mods=ctrl → sent to the
session: \x17`, `key=t … \x14`, `key=n … \x0e`); Task 7's "Installed window" paragraph is
written from it.

## Global Constraints
- Commit rule (verbatim): conventional commits `type(scope): lowercase summary`; no AI
  attribution, no Co-Authored-By or similar trailers; commit with explicit paths only (`git add
  <new files>` first, then `git commit -m … -- <paths>`); never `git add -A`; never push.
- Build output under the worktree's `target/` or `~/.cache/claudio/`, never `/tmp` and never
  `/home/p4u/claudio/target`.
- Subagents never dispatch subagents.
- Apply each patch exactly. The patches are numbered 1-14 in order (each step says which).
  Extract patch N with its final newline, then apply it from the worktree root:
  `awk -v n=N '/^````diff$/{c++; if(c==n){f=1; next}} f&&/^````$/{f=0} f' /home/p4u/claudio/docs/plans/2026-11-13-desktop-plan.md > ~/.cache/claudio/app-N.patch`
  then `git apply --whitespace=error-all ~/.cache/claudio/app-N.patch`. Patch 8 holds three
  PNGs as a git binary patch: `git apply` takes it as it is. If a patch does not apply, stop and
  report: do not hand-merge it.
- `make check` passes at every commit (CI runs it with the latest stable toolchain; this plan
  was checked with rustup stable 1.99.1).
- ACCEPTANCE.md throughout:
  - A2/A3: the core names no addon. The core gains two generic changes, each in its own
    commit: `app::browser::open_browser` starts its opener in a process group of its own
    (Task 2) and `app::browser::display_present` (Task 5). Browser discovery, tickets, the
    app window and the launchers live in the web addon.
  - A8: `addons/web/src/lib.rs` goes from 692 to 732 production lines (budget 800) and
    `cli.rs` from 362 to 412, as `scripts/check_rules.py` counts; every new file is far under
    800 and every new function under 80. `hub.rs`, `ws.rs` and `scripts/size-baseline.txt` get
    no edit.
  - B2: `docs/web-protocol.md` lands first (Task 1), alone.
  - C1: no token in argv, logs, notices or the no-terminal output; the ticket never in argv
    but in the sandboxed-browser fallback (a stated residual risk), never in a log.
  - C3: the browser, the openers and the notifiers start without a shell, stdin null, output
    dropped; the notifier has a 3 s deadline; `SESSION_MARKERS` are scrubbed from the browser's
    and the notifier's env.
  - C4: ticket files 0600 in a 0700 directory; redirect files, `web-app.log` and launcher files
    0600 through `paths::write_atomic` (missing directories 0700); the macOS launcher script is
    then made 0755; nothing chmods a directory it did not make, but `ensure_private_dir`'s
    repair of claudio's own runtime directories.
  - D4/D2: no `allow(dead_code)`, no new `unwrap` outside tests: every task's code is used by
    that task.
  - E2: no test weakened or rewritten. Task 5 turns the two lines of `cli.rs`'s "did not
    start" failure into one message (decision 11); no test asserts on them.
  - E3: tests are hermetic (below).
- Tests are hermetic. Fake browsers, openers (`xdg-open`, `open`) and notifiers (`notify-send`,
  `osascript`) are scripts in one directory that is the whole `PATH` of the command under test,
  and every test that reaches the browser search sets `CLAUDIO_BROWSER` to the fake browser
  (`addons/web/tests/web_harness/app.rs`, `desktop_env`). Never launch the owner's real browser,
  never write to the real `~/.local/share/applications` or `~/Applications` (launcher tests use
  temp dirs, or `HOME` = the harness's with `XDG_DATA_HOME` removed), never touch the owner's
  running `claudio --daemon` or its web service on port 7337 (every test port is a free one;
  every daemon is the harness's, in its own `XDG_RUNTIME_DIR`).
- Linux and macOS: no `/proc`, no `setsid` in tests. Shell fakes use POSIX `sh` builtins only
  (`printf`), never `echo` for data.
- Do not reformat, reorder or "tidy" unrelated code.

**Decisions this plan made where the spec left them open (binding).**

1. **Module layout.** `addons/web/src/ticket.rs`: tickets (mint, redeem, sweep), redirect files
   and `open_tab`, the browser tab a plain `claudio web` opens. `addons/web/src/app_window.rs`:
   `--app` and `--app=<url>` (the browser search, the argv, the spawn, the URL check, the
   no-terminal output, the desktop failure report). `addons/web/src/launcher/{mod,linux,
   macos}.rs`: the launchers. `lib.rs` keeps only flags and wiring.
2. **Browser discovery lives in the web addon**, not in `app::browser`: only the web uses it,
   and the core's browser service stays two functions plus the spec's `display_present`.
3. **`open_browser` gets a process group of its own** (Task 2, a core fix in its own commit).
   A browser that `claudio web --foreground` started died with the terminal's Ctrl+C, and
   under the PTY harness the hangup at the CLI's exit killed the fake opener before it wrote
   its argv in 1 of 5 runs of the AC6 test (measured: 0 failures in 6 runs with the fix).
   `app_window::spawn` does the same for the app window.
4. **The runtime dir both sides use** is `paths::runtime_dir()`. The CLI reaches the daemon at
   `paths::daemon_socket()`, which is in that directory, and the service inherits the daemon's
   environment, so the two resolve the same directory by construction; `--foreground` mints in
   its own process.
5. **The redirect file** is exactly the spec's HTML plus a newline, named `<32 hex>.html`, in
   `<runtime>/web-open/`. `ticket::redirect_html` refuses any URL byte outside
   `[A-Za-z0-9:/.[]#=]`, so nothing in it ever needs escaping. Cleanup: each write first removes
   files older than 60 s and the oldest beyond 15; every server start removes the old ones too
   (`sweep`).
   Nothing deletes it right after the open: the browser reads it after `claudio` exits.
6. **Ticket lifetime** is measured on the file's mtime against `now - 30 s ..= now + 2 s`.
   Tests age files with `std::fs::File::set_modified` on a read-only handle (works for the
   owner; checked on Linux).
7. **Sandboxed browsers.** The spec tests the resolved path; a snap's `/snap/bin/<name>` resolves
   to `/usr/bin/snap`, so that test alone misses every snap. `app_window::sandboxed` is true when
   the path as found or the resolved path is under `/snap`, or contains `/flatpak/exports/bin/`,
   or the resolved file is named `snap`.
8. **The chosen browser** (`[web] browser`, else `$CLAUDIO_BROWSER`) always wins: a value with a
   `/` as given; a name looked up in the search directories, else given to the spawn as it is
   (which reports a missing one). Empty values count as unset. Search order: names outer,
   directories inner (`google-chrome` anywhere beats `chromium` earlier on `PATH`). Relative
   `PATH` entries are skipped.
9. **The `--app=<url>` grammar.** `http://` or `https://` (ASCII case-insensitive); a host of
   `[A-Za-z0-9.-]` not starting with `-` or `.`, or a bracketed IPv6 literal; an optional port
   1-65535; an optional path of RFC 3986 path characters (unreserved, `%`, sub-delims, `:`,
   `@`, `/`). No userinfo, no query (the spec lists host, port and path only), no fragment.
   The refusal prints exactly the spec's line: `app_window::line` prints a message that is
   already a whole `claudio web …` line as it is, and any other as `claudio web: <message>`.
   The same rule gives "no display" its exact line.
10. **Flags.** `WebArgs` gains `app: Option<AppTarget>` (`Local` for bare `--app`, `Url` for
    `--app=<url>`) and `launcher: Option<LauncherOp>`. `service::run_args` copies `app`; `Unit`
    never holds it (a test pins it). `--app` is refused with `--no-open` and `--print-url`
    ("--app opens a window: drop --no-open and --print-url"), with `--set-password` and
    `--print-cert` by their one-argument rule, and with `--service` by its rule. `--app=URL`
    with anything else: "--app=URL takes no other argument". The launcher flags take no other
    argument but `--install-launcher --app=URL`. A second `--app` replaces the first, as the
    other flags do. `USAGE` gains `[--app]`, `claudio web --app=URL` and `claudio web
    --install-launcher [--app=URL] | --remove-launcher` (the spec's line with the URL form it
    allows).
11. **`--app` failures.** `cli.rs` sends every failure of a start through `failed(app, code,
    why)`. With `--app` the line is also appended to `<runtime>/web-app.log` (one line of at
    most 4 KiB, with a Unix time; past 64 KiB the file starts over) and, when stderr is not a
    terminal, shown with `notify-send -a claudio claudio <line>` (Linux; skipped when it is not
    on `PATH`) or `osascript -e 'on run argv' -e 'display notification (item 1 of argv) with
    title "claudio"' -e 'end run' <line>` (macOS). The "did not start" failure becomes one
    message carrying the tail and the log path, so the desktop sees the reason. `claudio web
    --app=<url>` reports the same way. `--foreground --app` reports on stderr and keeps
    serving when the window cannot open: the spec's "exits 1" is for the command that only
    starts the service and opens the window; a foreground server is the point of the command.
12. **No terminal on stdout, with `--app`** (`app_window::announce`): `claudio web:
    http://<listen>/`, or with `--remote` the banner's stdout line (the public URL, no token)
    and its stderr lines without any line that holds `#t=`.
13. **An older running service** (spec, Compatibility). `cli::current` compares the `apply`
    reply's `since` with this binary's mtime (`current_exe()`'s metadata; not `Probe::own`,
    which also hashes the whole binary for a value not needed here), and only when this start
    opens a browser on a ticket (`app_window::opens`). It then asks `apply {force: true}`. The
    spec asked the plan to confirm that this re-execs `Config::claudio` from disk: it does.
    `Config::claudio` is a path, taken once at the daemon's start
    (`crates/core/src/daemon/mod.rs:133`), and `service.rs::spawn` runs
    `Command::new(&config.claudio)` for every start (`crates/core/src/daemon/service.rs:505`),
    so the file on disk, this binary. If the restart fails: the base URL on stdout, then "the
    web service is older than this claudio: run `claudio daemon restart`", exit 1. The test
    runs a copy of the binary and sets the copy's mtime 120 s ahead: same bytes, so
    `freshness::compare` says `Current` and the daemon stays, as a busy one would.
14. **`StartupWMClass` is left out** of the `.desktop` (the spec's "if not" branch). The class
    could not be observed here (no display; it needs the owner's laptop, AC9(b)), and
    Chromium probably derives an `--app` window's instance from its URL, here a redirect file
    with a random name (unverified). docs/web.md says the window may group under the browser and
    recommends the browser's Install. If AC9(b) finds one class that covers a browser already
    running and not, a follow-up adds the line in `launcher/linux.rs::entry` and its test.
15. **Launchers.** Install also refuses a launcher that claudio did not write, or a symlink at
    its path (the spec says so for removal; "installing again replaces it" is about claudio's
    own). The icons come from the binary's embedded page (`assets::embedded`), so a launcher
    shows exactly the served icons. In `Exec`, the exe is always double-quoted with the spec's
    escaping; the word after `web` (`--app` or `--app=<url>`) is quoted the same way only when
    it holds a character the Desktop Entry spec reserves, and `%` is `%%` either way, so AC14's
    literal `Exec="<exe>" web --app=<url>` holds for a plain URL. The macOS script
    single-quotes the exe and the `--app=<url>` word. A removal also takes the three icon files
    it wrote, regular files only. Both writers are compiled and tested on every OS; `cfg!`
    picks one at run time.
16. **Icons.** `addons/web/web/dev/render-icons.mjs` renders them once with Playwright's
    Chromium (headless shell 1248, `@playwright/test` 1.64.1 as pinned by
    `addons/web/web/e2e/package-lock.json`; `dev/` is never embedded): `icons/claudio.svg` at
    192 and 512 px, and `dev/icons/claudio-maskable.svg` (full-bleed `#0b0e12`, the glyph scaled
    0.8 about the centre) at 512 px. The farthest glyph pixel of the maskable PNG is 183.1 px
    from the centre (the safe circle's radius is 204.8). Patch 8 carries the PNGs; re-rendering
    gives the same bytes (SHA-256 under Task 4). The rounded "any" icon keeps its 12/64 corner
    radius: DESIGN.md's 1px radius is for UI surfaces, not app icons.
17. **The page.** `addons/web/web/app/signin.js` (pure, node-tested: `ticketOf`,
    `afterRefusal`, `TICKET_EXPIRED`); `boot.js` imports it by a relative path. The import map
    is unchanged, so the CSP hash is too.
18. **Playwright.** `addons/web/web/e2e/tests/signin.spec.mjs` mints a ticket by writing its
    file into the server's isolated runtime dir (`<srv.root>/run/claudio/web-tickets`), as
    `claudio` does. Its 429 test is last, because it spends the loopback peer's minute. The
    manifest checks extend `keys.spec.mjs`'s existing manifest test (no new server start).
19. **Rust integration tests** go in `addons/web/tests/web.rs` (the `web` suite; test files have
    no A8 budget), with their fakes and helpers in a new `addons/web/tests/web_harness/app.rs`.
    Each fake appends its argv, `\x1f` after each argument, as one line of `<name>.log`. `DISPLAY`
    is `:99` and `WAYLAND_DISPLAY` removed. Cases that need a terminal run in the PTY harness.
20. **Risk recorded for AC9(d).** A plain `claudio web` now opens the default browser on a
    `file://` page in the runtime dir. A snap browser as the default (Ubuntu's Firefox) cannot
    read it and would show its own "access denied" page; claudio cannot know the default
    browser's packaging. AC9(d) checks `xdg-open` on the file on the owner's laptop; if it
    fails, the owner decides (the spec's argv fallback, or no auto sign-in).

## Review Focus
Line numbers are those of the branch's final tree (after Task 7).

- **Minting**, `addons/web/src/ticket.rs:70-82` (`mint`): `private` (55-58) makes `<runtime>`
  and `<runtime>/web-tickets` private with `ensure_private_dir` on every mint, before anything
  is read; `prune` (130-152) keeps at most 15 (then the new one: 16) and never follows a
  symlink (`symlink_metadata`, `remove_file`); the file is created `create_new` (O_EXCL) with
  `O_NOFOLLOW` and mode 0600, empty, named by the ticket's SHA-256 (`file_name`, 47-52). The
  ticket is 32 bytes of `/dev/urandom` (`auth::random_hex`).
- **Redeeming**, `addons/web/src/login.rs:52-63` (the body: exactly one of token, password or
  ticket, else 400; the ticket path in `spawn_blocking`) and `login.rs:111-136`
  (`ticket_login`): the limiter slot first, as a token's (`!caller.local` for the global
  window), and `429` touches nothing; then `token_allowed` (with `--remote`, a non-local
  request's ticket file is never looked at); then `ticket.rs:97-111` (`redeem`): the 64-hex
  form, the two directories checked again (an error is an ERROR log naming the path, 130-133
  of login.rs, and a 401), `symlink_metadata` of the file (regular, our uid, `fresh`: mtime in
  `now - 30 s ..= now + 2 s`, 62-65), a failing file removed, and `remove_file` as the single
  use (`NotFound` for the loser of a race is a 401). Only `Ok` releases the slot; every 401
  keeps it. Nothing logs the ticket (only `status` and `local`, as before).
- **Sweep at a server start**, `ticket.rs:115-124`, called from `addons/web/src/lib.rs:352`
  (before the runtime starts): it prunes a directory only if it exists and passes
  `ensure_private_dir` (a symlinked `web-tickets` is never read through).
- **The redirect file**, `ticket.rs:157-180`: `redirect_html` refuses any byte outside
  `[A-Za-z0-9:/.[]#=]`, so the URL is pasted into HTML with nothing to escape; `write_redirect`
  checks the directory, prunes (60 s, 16) and writes 0600 through `paths::write_atomic` under a
  random 32-hex name. `file_url` (184-195) percent-encodes every byte but unreserved ones and
  `/`. argv carries only that `file://` URL: `ticket.rs:223-246` (`open_tab`) for the tab,
  `addons/web/src/app_window.rs:259-298` (`window`) for the app window.
- **The process group**, `crates/core/src/app/browser.rs:21-42` (`open_browser`, line 33) and
  `app_window.rs:236-253` (`spawn`): no shell, null stdio, `process_group(0)`, reaped on a
  thread; `spawn` also removes `SESSION_MARKERS` from the browser's env.
- **No token when stdout is not a terminal**, `app_window.rs:349-368` (`announce`,
  `token_free`): with `--app` and no terminal, the stdout line is the base URL and any banner
  line with `#t=` is dropped; with a terminal the lines are `crate::announce`'s, unchanged.
  `cli.rs:244-269` (`announce`) and `lib.rs:596-610` (`started`) are its two callers.
- **Desktop failure reports**, `app_window.rs:372-463`: `line` (whole `claudio web …` lines as
  they are), `tell_desktop` (log always, notify only when stderr is not a terminal),
  `append_log` (`ensure_private_dir`, `O_APPEND|O_CREAT|O_NOFOLLOW` 0600, 4 KiB per line, a
  restart past 64 KiB through `write_atomic`), `notifier`/`notify` (the message one argv item,
  never a shell; `osascript` reads it as `item 1 of argv`; 3 s deadline then kill and reap). The
  callers pass only lines that hold no token or ticket: `cli.rs:221-228` (`failed`) and
  `app_window.rs:302-312` (`open_url`).
- **`--app=<url>`**, `app_window.rs:88-135` (`url_ok`, `authority_ok`, `path_byte`): no
  userinfo (`@` is not a host byte; a port must be digits), no `#`, no `?`, no control or
  non-ASCII byte; `target` (77-83) is the parse-time gate (`lib.rs:177`), so a refused URL never
  reaches a spawn. `open_url` starts no daemon and reads no token or `[web]` but its `browser`
  key.
- **The browser search**, `app_window.rs:148-200` (`Search::here`, `find_browser`): no login
  shell, relative `PATH` entries skipped, `[web] browser` (read by `configured`, 223-231) before
  `CLAUDIO_BROWSER`; `sandboxed` (206-210) decides the one argv with `#k=`, after printing the
  spec's warning (window, 280-284).
- **The older-service restart**, `addons/web/src/cli.rs:186-212` (`current`, `older_than_me`):
  only when a browser opens on a ticket; `apply {force: true}`; on failure the base URL (no
  token) and the spec's message.
- **Flags**, `lib.rs:177-179` and `lib.rs:188-211` (`combination_ok`): `--app` never reaches
  `Unit` (`service.rs:145-156` copies it into the run args only) or the service's argv.
- **Launchers**, `addons/web/src/launcher/mod.rs:90-110` (`exe_ok`: absolute, UTF-8, no control
  character, not ` (deleted)`, a file; as `current_exe` returns it, not canonicalized),
  `mod.rs:131-167` (`found`: a symlink is foreign; the marker read with `O_NOFOLLOW`, 64 KiB at
  most; `write` through `write_atomic`), `launcher/linux.rs:19-57` (the reserved set, `quoted`'s
  two layers, `arg`, `entry`), `linux.rs:61-96` (install refuses a foreign or symlinked entry;
  remove takes regular files only), `launcher/macos.rs:34-59` (`sh_quoted`, `script`, `icns`),
  `macos.rs:63-104` (a foreign or symlinked bundle refused; claudio's replaced whole; the script
  made 0755 after its 0600 write).
- **The page**, `addons/web/web/app/boot.js:111-135` (`ticketBoot`: a malformed `#k=` posts
  nothing; the fragment is cleared after the request whatever happened; `429`; then `/api/me`)
  and `addons/web/web/app/signin.js:11-25`.

## Task order and parallelism
| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 Protocol doc: sign-in tickets | none | docs/web-protocol.md | 4 |
| 2 Tickets: mint, redeem, the tab on a redirect file | 1 | crates/core/src/app/browser.rs; addons/web/src/{ticket,login,lib,cli,remote}.rs; addons/web/tests/{web.rs,web_harness/mod.rs,web_harness/app.rs,web_harness/service.rs} | 4 |
| 3 The page signs in with `#k=` | 1, 2 | addons/web/web/app/{signin,boot}.js; addons/web/web/tests/signin.test.mjs; addons/web/web/e2e/tests/signin.spec.mjs | 4 |
| 4 PWA icons and manifest | none | addons/web/web/{icons/*,manifest.webmanifest,dev/icons/claudio-maskable.svg,dev/render-icons.mjs}; addons/web/web/e2e/tests/keys.spec.mjs | 1, 2, 3 |
| 5 `--app` and `--app=<url>` | 2 | crates/core/src/app/browser.rs; addons/web/src/{app_window,lib,cli,service}.rs; addons/web/tests/{web.rs,web_harness/service.rs} | none |
| 6 The launchers | 4, 5 | addons/web/src/{launcher/mod,launcher/linux,launcher/macos,assets,lib}.rs; addons/web/tests/web.rs | none |
| 7 Docs: web.md, README, CLAUDE.md | 5, 6 | docs/web.md, README.md, CLAUDE.md | none |

The commits land in task order on one branch: 1, 2 (two commits), 3, 4, 5 (two commits), 6, 7.
Task 4 touches only page files and may be done beside 1-3, but it commits after Task 3 (its
patches were generated on top of it). Task 6 embeds Task 4's PNGs, and both 5 and 6 edit
`lib.rs`'s flags.

## Tasks
### Task 1: Protocol doc: sign-in tickets
Model: haiku   Repo: /home/p4u/claudio   Files: `docs/web-protocol.md`
Goal: B2, the protocol doc first and in its own commit (spec §1 and §5, AC10): the `ticket`
login body, the redeem order, the redirect file, and what the page does with `#k=`.

Step 1: apply the patch (patch 1)
````diff
diff --git a/docs/web-protocol.md b/docs/web-protocol.md
index d44b4a1..fd98aaa 100644
--- a/docs/web-protocol.md
+++ b/docs/web-protocol.md
@@ -21,7 +21,7 @@ is in git history.
 | Method and path | Cookie | Origin check | Answer |
 |---|---|---|---|
 | `GET`/`HEAD` `/`, `/index.html`, any embedded asset path | no | no | the asset; `404` when unknown |
-| `POST /api/login` body `{"token":"<64 hex>"}` or `{"password":"<8–1024 bytes>"}` (≤ 4 KiB) | no | yes | `204` + `Set-Cookie`; `401` wrong token or password, or a token from a non-local request with `--remote`; `429` locked; `400` bad body (neither or both fields, or a password without `--remote`); `413` too large; `503` no hashing slot within 2 s (does not count) |
+| `POST /api/login` body `{"token":"<64 hex>"}`, `{"password":"<8–1024 bytes>"}` or `{"ticket":"<64 hex>"}` (≤ 4 KiB) | no | yes | `204` + `Set-Cookie`; `401` wrong token or password, a ticket that is unknown, spent or expired, or a token or ticket from a non-local request with `--remote`; `429` locked; `400` bad body (not exactly one of the three fields, or a password without `--remote`); `413` too large; `503` no hashing slot within 2 s (does not count) |
 | `POST /api/logout` | yes | yes | `204` + an expiring `Set-Cookie` |
 | `GET /api/me` **(decided here)** | yes | no | `204`: the cookie is live (the page asks this at load when the URL has no token). `401` without one, body `{"login":[…]}`: the methods this request may use, `"token"` and/or `"password"` (below) |
 | `GET /ws` (WebSocket upgrade) | yes | yes | `101`; `503` past 16 tabs **(decided here)** |
@@ -76,6 +76,47 @@ request with it, `["password"]` for any other request.
   (`#t=<token>`), posts it to `/api/login`, then removes the fragment with
   `history.replaceState`.
 
+**Sign-in tickets.** A `claudio` that opens a browser on this machine (`claudio web`,
+`--foreground`, `--app`) signs it in with a single-use ticket, never the token in argv. The
+ticket body is additive: the protocol stays version 2.
+
+- **Minting** (by that `claudio`). The ticket is 32 random bytes as 64 lowercase hex. Minting
+  makes `<runtime>` and `<runtime>/web-tickets` private (0700, ours, not a symlink; `<runtime>`
+  is the runtime dir the daemon socket is in, which the CLI and the service the daemon runs
+  share), removes the files there older than 30 s and the oldest beyond 15 (so at most 16 are
+  live), then creates the empty file `<runtime>/web-tickets/<SHA-256 hex of the ticket>` with
+  `O_CREAT|O_EXCL|O_NOFOLLOW`, mode 0600. A server start removes the expired ones too.
+- **The redirect file.** The browser is opened on `file://<runtime>/web-open/<32 hex>.html`
+  (0600, written atomically in a directory checked as above), whose whole text is
+  `<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=URL"><title>claudio</title><a href="URL">Open claudio</a>`
+  and a newline, where URL is `<base URL>#k=<ticket>`. The base URL is `http://<listen>/`, or with
+  `--remote` `https://127.0.0.1:<port>/` (`https://[::1]:<port>/` for `[::]`, the address itself
+  when it is loopback). URL holds only a scheme, an IP literal, a port, `/`, `#k=` and hex:
+  nothing in it needs escaping. Redirect files older than 60 s go at each one written and at a
+  server start; at most 16 are kept. Only a browser that cannot read the runtime dir (its path,
+  or where it resolves, is under `/snap/`, is the snap launcher, or is in a flatpak
+  `exports/bin`) is given `<base URL>#k=<ticket>` in argv instead, and `claudio` says "this
+  browser is sandboxed: the sign-in link is visible to other users of this machine for up to
+  30 s". A specific non-loopback `--remote` address has no base URL: no ticket is minted, and
+  the page asks for the password.
+- **Redeeming** `{"ticket": T}`, in this order: (1) the rate-limit slot, taken as a token
+  attempt takes it (`429` when full: nothing below runs and the ticket is untouched); (2) with
+  `--remote`, a request that is not local gets `401` and the ticket file is not touched; (3) `T`
+  not 64 lowercase hex: `401`; (4) `<runtime>` and `<runtime>/web-tickets` checked as at
+  minting, on every redeem (an error: `401`, and an ERROR log naming the path); (5) the file
+  must be a regular file owned by the server's uid with an mtime from 30 s ago to 2 s ahead,
+  else `401`, and a file that fails is removed; (6) removing the file must succeed: the other
+  of two racing attempts gets `401`; (7) the slot is given back and the session made as a token
+  login makes it (a cookie the request came with loses its session). Every `401` keeps its
+  slot. A ticket is found by its hash, never compared as a string.
+- **The page**, on a fragment that starts with `#k=`: anything but exactly
+  `#k=<64 lowercase hex>` is a refused ticket, and no request is made. Otherwise it posts
+  `{"ticket": …}` (also with a live cookie, so the ticket is spent), then clears the fragment
+  with `history.replaceState`, also when the request throws. `204` starts the app; `429`
+  shows "Too many attempts: wait a minute."; any other answer, a thrown request or a refused
+  fragment asks `/api/me`: `204` starts the app, a 401 that offers the password shows the
+  password form, anything else shows "This sign-in link has expired. Run `claudio web` again."
+
 ## 2. WebSocket framing
 
 One socket per browser tab, `ws://<host>/ws` (`wss://` with `--remote` or behind a TLS proxy; the page picks the scheme from its own URL).
````
Run: `grep -c "ticket" docs/web-protocol.md`
Expected: `17`

Step 2: commit
`git commit -m "docs(web-protocol): sign-in tickets and the #k= fragment" -- docs/web-protocol.md`

Interfaces this task provides to later tasks: none (the contract Tasks 2 and 3 implement).

### Task 2: Tickets: minted, redeemed once, and the browser tab on a redirect file
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/app/browser.rs`,
`addons/web/src/ticket.rs` (new), `addons/web/src/login.rs`, `addons/web/src/lib.rs`,
`addons/web/src/cli.rs`, `addons/web/src/remote.rs`, `addons/web/tests/web.rs`,
`addons/web/tests/web_harness/mod.rs`, `addons/web/tests/web_harness/app.rs` (new),
`addons/web/tests/web_harness/service.rs`
Goal: spec §1 and Goal 3 for the browser `claudio web` and `--foreground` already open: AC3
(ticket units), AC4 (the login), AC6 (the tab opens a `file://` redirect file, never `#t=`).
`remote::base_url` is the spec's shared helper. The core fix (decision 3) is its own commit.

Step 1: write the failing tests: the harness's fakes and ticket helpers, AC4's two tests and
AC6's (patch 2)
````diff
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index 8379ba9..0bafee4 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -15,6 +15,7 @@ use common::{FakeSsh, ManagerHarness, BINARY, DAEMON_WAIT, WAIT};
 use futures::SinkExt;
 use serde_json::{json, Value};
 use tokio_tungstenite::tungstenite::Message;
+use web_harness::app::*;
 use web_harness::service::*;
 use web_harness::tls::*;
 use web_harness::*;
@@ -2223,3 +2224,89 @@ async fn hello_is_v2() {
     assert_eq!(tab.hello["v"], 2);
     assert_eq!(tab.hello["features"], json!([]));
 }
+
+// ── Sign-in tickets (docs/specs/2026-11-13-desktop-design.md) ────────────────
+
+/// AC4: a ticket signs in once; a body with a ticket and anything else is
+/// a bad one.
+#[test]
+fn web_a_ticket_signs_in_once() {
+    let web = Web::start();
+    let ticket = mint_ticket(web.h());
+    let post = |body: Value| web.post("/api/login", Some(&web.origin()), &body.to_string(), &[]);
+    assert_eq!(
+        post(json!({"ticket": ticket, "token": web.token})).status,
+        400
+    );
+    assert_eq!(post(json!({"ticket": ticket, "password": "x"})).status, 400);
+    assert!(
+        ticket_file(web.h(), &ticket).exists(),
+        "a 400 spends nothing"
+    );
+    let first = post(json!({ "ticket": ticket }));
+    assert_eq!(first.status, 204, "{first:?}");
+    let cookie = cookie_of(first.header("set-cookie").expect("a cookie"));
+    assert_eq!(me_with(&web, &cookie), 204);
+    assert!(!ticket_file(web.h(), &ticket).exists(), "spent");
+    assert_eq!(post(json!({ "ticket": ticket })).status, 401, "a replay");
+}
+
+/// AC4: with `--remote`, a ticket from a peer that is not local is refused
+/// and counts, and its file stays: the same ticket then signs in locally.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_remote_tickets_are_for_local_requests_only() {
+    // Linux routes all of 127/8 to lo; macOS only 127.0.0.1 unless aliased.
+    if std::net::TcpListener::bind("127.0.0.2:0").is_err() {
+        return;
+    }
+    let r = RemoteWeb::start(ManagerHarness::new(), &[], |cmd| {
+        cmd.env("CLAUDIO_TEST_REMOTE_PEER", "127.0.0.2");
+    });
+    let ticket = mint_ticket(r.web.h());
+    let body = json!({ "ticket": ticket }).to_string();
+    let (host, origin) = (r.host(), r.origin());
+    let headers = [("Origin", origin.as_str())];
+    let from_2 = || r.req_as("127.0.0.2", &host, "POST", "/api/login", &headers, &body);
+    for i in 0..5 {
+        assert_eq!(from_2().await.status, 401, "attempt {i}");
+    }
+    assert_eq!(from_2().await.status, 429, "the refusals counted");
+    assert!(ticket_file(r.web.h(), &ticket).exists(), "the file stays");
+    let local = r.login(&json!({ "ticket": ticket })).await;
+    assert_eq!(local.status, 204, "{local:?}");
+    assert!(local.header("set-cookie").unwrap().contains("; Secure"));
+}
+
+/// AC6: `claudio web` opens the browser on a `file://` redirect file that
+/// carries a ticket; neither argv nor the file holds the token.
+#[test]
+fn web_opens_the_browser_on_a_redirect_file_never_the_token() {
+    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
+    let token = svc.token();
+    let fake = FakeBin::new(&svc.h().root);
+    let mut cmd = portable_pty::CommandBuilder::new(BINARY);
+    cmd.arg("web");
+    svc.h().apply(&mut cmd);
+    desktop_env_pty(&mut cmd, &fake);
+    let mut tty = common::TuiProcess::spawn(cmd);
+    assert_eq!(tty.wait_exit_code(DAEMON_WAIT), 0);
+    let opener = match cfg!(target_os = "macos") {
+        true => "open",
+        false => "xdg-open",
+    };
+    let calls = fake.wait_calls(opener, 1, WAIT);
+    assert_eq!(calls.len(), 1, "{calls:?}");
+    let [url] = calls[0].as_slice() else {
+        panic!("one argument: {calls:?}");
+    };
+    assert!(!url.contains('#') && !url.contains(&token), "{url}");
+    let html = redirect_of(svc.h(), url);
+    assert!(
+        html.contains(&format!("url=http://127.0.0.1:{}/#k=", svc.port)),
+        "{html}"
+    );
+    assert!(!html.contains("#t=") && !html.contains(&token), "{html}");
+    let ticket = ticket_in(&html);
+    assert_eq!(svc.ticket_login(&ticket), 204, "the ticket signs in");
+    svc.finish();
+}
diff --git a/addons/web/tests/web_harness/app.rs b/addons/web/tests/web_harness/app.rs
new file mode 100644
index 0000000..c75c1fd
--- /dev/null
+++ b/addons/web/tests/web_harness/app.rs
@@ -0,0 +1,165 @@
+//! Fakes for what `claudio web` starts on the desktop: a browser, the
+//! openers (`xdg-open`, `open`) and the notifiers (`notify-send`,
+//! `osascript`). Each records its argv; none opens anything. Put
+//! [`FakeBin::dir`] alone on `PATH`, so no real one is reached, and the
+//! browser in `CLAUDIO_BROWSER`.
+
+use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
+use std::path::{Path, PathBuf};
+use std::time::{Duration, Instant};
+use std::{fs, thread};
+
+use sha2::{Digest, Sha256};
+
+use crate::common::ManagerHarness;
+
+/// The fake programs, all in one directory.
+pub struct FakeBin {
+    dir: PathBuf,
+}
+
+/// The programs a `FakeBin` holds.
+const NAMES: [&str; 5] = [
+    "fake-browser",
+    "xdg-open",
+    "open",
+    "notify-send",
+    "osascript",
+];
+
+impl FakeBin {
+    /// The fakes under `root/fakebin`. Each invocation of `<name>` appends
+    /// one line to `<name>.log`: its arguments, each followed by `\x1f`
+    /// (shell builtins only: the `PATH` reaches nothing else).
+    pub fn new(root: &Path) -> FakeBin {
+        let dir = root.join("fakebin");
+        fs::create_dir_all(&dir).unwrap();
+        for name in NAMES {
+            let log = dir.join(format!("{name}.log"));
+            let script = format!(
+                "#!/bin/sh\nprintf '%s\\037' \"$@\" >> '{}'\nprintf '\\n' >> '{}'\n",
+                log.display(),
+                log.display()
+            );
+            let path = dir.join(name);
+            fs::write(&path, script).unwrap();
+            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
+        }
+        FakeBin { dir }
+    }
+
+    /// The directory, to be the whole `PATH`.
+    pub fn dir(&self) -> &Path {
+        &self.dir
+    }
+
+    /// The fake browser, for `CLAUDIO_BROWSER`.
+    pub fn browser(&self) -> PathBuf {
+        self.dir.join("fake-browser")
+    }
+
+    /// Every recorded invocation of `name`, its arguments in order.
+    pub fn calls(&self, name: &str) -> Vec<Vec<String>> {
+        let text = fs::read_to_string(self.dir.join(format!("{name}.log"))).unwrap_or_default();
+        text.lines()
+            .map(|l| {
+                l.split('\x1f')
+                    .filter(|a| !a.is_empty())
+                    .map(str::to_owned)
+                    .collect()
+            })
+            .collect()
+    }
+
+    /// Wait until `name` was run `n` times (it is started detached).
+    pub fn wait_calls(&self, name: &str, n: usize, within: Duration) -> Vec<Vec<String>> {
+        let deadline = Instant::now() + within;
+        loop {
+            let calls = self.calls(name);
+            if calls.len() >= n {
+                return calls;
+            }
+            assert!(
+                Instant::now() < deadline,
+                "{name} ran {} times",
+                calls.len()
+            );
+            thread::sleep(Duration::from_millis(50));
+        }
+    }
+}
+
+/// The env of a `claudio web` on a desktop: a display, the fakes alone on
+/// `PATH`, the fake browser chosen, `CLAUDIO_NO_BROWSER` unset.
+pub fn desktop_env(cmd: &mut std::process::Command, fake: &FakeBin) {
+    cmd.env_remove("CLAUDIO_NO_BROWSER")
+        .env_remove("WAYLAND_DISPLAY")
+        .env("DISPLAY", ":99")
+        .env("PATH", fake.dir())
+        .env("CLAUDIO_BROWSER", fake.browser());
+}
+
+/// [`desktop_env`] for a command run in a PTY (stdout a terminal).
+pub fn desktop_env_pty(cmd: &mut portable_pty::CommandBuilder, fake: &FakeBin) {
+    cmd.env_remove("CLAUDIO_NO_BROWSER");
+    cmd.env_remove("WAYLAND_DISPLAY");
+    cmd.env("DISPLAY", ":99");
+    cmd.env("PATH", fake.dir());
+    cmd.env("CLAUDIO_BROWSER", fake.browser());
+}
+
+/// `h`'s `<runtime>/claudio`.
+pub fn runtime(h: &ManagerHarness) -> PathBuf {
+    h.runtime_dir.join("claudio")
+}
+
+/// Where the file of `ticket` lives.
+pub fn ticket_file(h: &ManagerHarness, ticket: &str) -> PathBuf {
+    let name: String = Sha256::digest(ticket.as_bytes())
+        .iter()
+        .map(|b| format!("{b:02x}"))
+        .collect();
+    runtime(h).join("web-tickets").join(name)
+}
+
+/// A sign-in ticket minted as `claudio` mints one: 64 lowercase hex, its
+/// empty 0600 file named by its SHA-256 in `<runtime>/web-tickets` (0700).
+pub fn mint_ticket(h: &ManagerHarness) -> String {
+    let ticket = format!(
+        "{}{}",
+        uuid::Uuid::new_v4().simple(),
+        uuid::Uuid::new_v4().simple()
+    );
+    let file = ticket_file(h, &ticket);
+    let dir = file.parent().unwrap();
+    for d in [runtime(h).as_path(), dir] {
+        fs::create_dir_all(d).unwrap();
+        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
+    }
+    fs::OpenOptions::new()
+        .write(true)
+        .create_new(true)
+        .mode(0o600)
+        .open(&file)
+        .unwrap();
+    ticket
+}
+
+/// The ticket in a redirect file's text (`…#k=<64 hex>"…`).
+pub fn ticket_in(html: &str) -> String {
+    let at = html.find("#k=").expect("a #k= link") + 3;
+    html[at..at + 64].to_owned()
+}
+
+/// The redirect file a `file://` URL names (the harness paths need no
+/// percent-decoding), checked to be in `h`'s `<runtime>/claudio/web-open`.
+pub fn redirect_of(h: &ManagerHarness, url: &str) -> String {
+    let path = url.strip_prefix("file://").expect("a file:// URL");
+    assert!(
+        Path::new(path).starts_with(runtime(h).join("web-open")),
+        "{path}"
+    );
+    let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
+    assert_eq!(mode, 0o600, "{path}");
+    fs::read_to_string(path).unwrap()
+}
diff --git a/addons/web/tests/web_harness/mod.rs b/addons/web/tests/web_harness/mod.rs
index 2188def..3d44d3a 100644
--- a/addons/web/tests/web_harness/mod.rs
+++ b/addons/web/tests/web_harness/mod.rs
@@ -5,6 +5,8 @@
 
 #![allow(dead_code)]
 
+/// Fake browsers, openers and notifiers; sign-in tickets as `claudio` mints them.
+pub mod app;
 /// `claudio web` handed to the harness daemon (the web service).
 pub mod service;
 /// `claudio web --remote`: a TLS server and a pinned client.
diff --git a/addons/web/tests/web_harness/service.rs b/addons/web/tests/web_harness/service.rs
index 29c76f8..53ff997 100644
--- a/addons/web/tests/web_harness/service.rs
+++ b/addons/web/tests/web_harness/service.rs
@@ -162,6 +162,14 @@ impl ServiceWeb {
         super::tls::cookie_of(&set)
     }
 
+    /// A sign-in with `ticket` on the loopback listener: its status.
+    pub fn ticket_login(&self, ticket: &str) -> u16 {
+        let (host, origin) = (self.host(), format!("http://{}", self.host()));
+        let body = serde_json::json!({ "ticket": ticket }).to_string();
+        let headers = [("Origin", origin.as_str())];
+        http(self.port, "POST", "/api/login", &host, &headers, &body).status
+    }
+
     /// `/api/me` with `cookie`.
     pub fn me(&self, cookie: &str) -> u16 {
         let with = format!("claudio_web={cookie}");
````
Run: `cargo test --locked --test web -- --test-threads=1 web_a_ticket_signs_in_once web_opens_the_browser_on_a_redirect_file_never_the_token`
Expected: `test result: FAILED. 0 passed; 2 failed`, with

```
thread 'web_a_ticket_signs_in_once' panicked at addons/web/tests/web.rs:2237:5:
assertion `left == right` failed
  left: 204
 right: 400
```
(the old server ignores `"ticket"` and signs the token in), and
`web_opens_the_browser_on_a_redirect_file_never_the_token` panicking at
`addons/web/tests/web.rs:2302:5` with the opener's one argument,
`http://127.0.0.1:<port>/#t=<token>`: the token in argv, the bug this task removes. (Without
patch 3 the opener may also die on the PTY's hangup before it records anything; the test then
fails with `xdg-open ran 0 times`.)

Step 2: the opener's process group, a core fix in its own commit (patch 3)
````diff
diff --git a/crates/core/src/app/browser.rs b/crates/core/src/app/browser.rs
index 7eaa8ce..af326d4 100644
--- a/crates/core/src/app/browser.rs
+++ b/crates/core/src/app/browser.rs
@@ -11,8 +11,11 @@ pub fn browser_wanted(os: &str, env: impl Fn(&str) -> Option<String>, tty: bool)
 }
 
 /// Open `url` with `open` (macOS) or `xdg-open`, without waiting for it;
-/// whether it could be started.
+/// whether it could be started. The opener gets a process group of its
+/// own: a browser it starts is not the terminal's to stop, so a Ctrl+C in
+/// it (`claudio web --foreground`) or its hangup never reaches that browser.
 pub fn open_browser(url: &str) -> bool {
+    use std::os::unix::process::CommandExt;
     let opener = if cfg!(target_os = "macos") {
         "open"
     } else {
@@ -23,6 +26,7 @@ pub fn open_browser(url: &str) -> bool {
         .stdin(std::process::Stdio::null())
         .stdout(std::process::Stdio::null())
         .stderr(std::process::Stdio::null())
+        .process_group(0)
         .spawn();
     match child {
         Ok(mut child) => {
````
Run: `cargo test --locked -p claudio-core --lib app::browser`
Expected: `test result: ok. 1 passed; 0 failed`
Then: `git commit -m "fix(core): open_browser starts its opener in a process group of its own" -- crates/core/src/app/browser.rs`

Step 3: implement: `ticket.rs` (with its unit tests), the ticket login (with its unit test),
the sweep at a server's start, the tab on a redirect file, `remote::base_url` (patch 4)
````diff
diff --git a/addons/web/src/cli.rs b/addons/web/src/cli.rs
index f158d52..20501e3 100644
--- a/addons/web/src/cli.rs
+++ b/addons/web/src/cli.rs
@@ -7,7 +7,7 @@
 //! only, never on an error's text.
 
 use std::future::Future;
-use std::io::{self, IsTerminal};
+use std::io;
 use std::path::Path;
 use std::process::ExitCode;
 
@@ -17,7 +17,7 @@ use claudio_core::proto::Msg;
 use serde_json::{json, Value};
 
 use crate::service::{self, Reported, Unit};
-use crate::{auth, parse_args, password, remote, start_url, tls, WebArgs};
+use crate::{auth, parse_args, password, remote, start_url, ticket, tls, WebArgs};
 
 /// `claudio web` against a daemon without `addon:web`.
 const NO_SERVICE: &str = "the daemon has no web service (older, or built without the web \
@@ -190,7 +190,8 @@ fn fail_after_token(run: &WebArgs, why: &str) -> ExitCode {
 }
 
 /// What a foreground start prints, for what runs (the reply's argv: two
-/// `claudio web` at once each print the one that won), then the browser.
+/// `claudio web` at once each print the one that won), then the browser,
+/// signed in with a ticket (`ticket::open_tab`).
 fn announce(argv: &[String], token: &str, asked: &WebArgs) -> ExitCode {
     let Some(Ok(running)) = argv.get(2..).map(parse_args) else {
         return fail("the daemon runs a web service this claudio cannot read");
@@ -207,14 +208,7 @@ fn announce(argv: &[String], token: &str, asked: &WebArgs) -> ExitCode {
         }
     };
     crate::announce(&url, remote.as_ref());
-    let wanted = claudio_core::app::browser::browser_wanted(
-        std::env::consts::OS,
-        |k| std::env::var(k).ok(),
-        io::stdout().is_terminal(),
-    );
-    if !asked.no_open && wanted {
-        claudio_core::app::browser::open_browser(&url);
-    }
+    ticket::open_tab(&running, asked.no_open);
     ExitCode::SUCCESS
 }
 
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index a816ec8..49d5b3a 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -28,6 +28,7 @@ mod session_events;
 mod sessions;
 mod spawn;
 mod state;
+mod ticket;
 mod tls;
 mod ws;
 
@@ -318,6 +319,10 @@ fn serve_here(args: WebArgs, addons: Arc<claudio_core::addon::Addons>) -> ExitCo
         }
     };
     let sessions = load_sessions(&args, &token, &password);
+    ticket::sweep(
+        &claudio_core::paths::runtime_dir(),
+        std::time::SystemTime::now(),
+    );
     init_tracing();
     let runtime = match tokio::runtime::Builder::new_multi_thread()
         .enable_all()
@@ -547,7 +552,7 @@ async fn serve(
 /// Once the listener is bound and the hub runs: `--service` (no `url`)
 /// writes `ready` on stdout, the line the daemon waits for, and nothing
 /// else (never the token); a foreground start prints its URL (and banner)
-/// and opens a browser.
+/// and opens a browser, signed in with a ticket (`ticket::open_tab`).
 fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
     if !env!("CLAUDIO_BUILD_LABEL").is_empty() {
         eprintln!("claudio web: local build {}", version());
@@ -559,14 +564,7 @@ fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
         return;
     };
     announce(url, remote);
-    let wanted = claudio_core::app::browser::browser_wanted(
-        std::env::consts::OS,
-        |k| std::env::var(k).ok(),
-        io::stdout().is_terminal(),
-    );
-    if !args.no_open && wanted {
-        claudio_core::app::browser::open_browser(url);
-    }
+    ticket::open_tab(args, args.no_open);
 }
 
 /// The start lines: with `--remote` the banner's stderr lines first, then
diff --git a/addons/web/src/login.rs b/addons/web/src/login.rs
index 6b353f2..1e83f71 100644
--- a/addons/web/src/login.rs
+++ b/addons/web/src/login.rs
@@ -1,12 +1,14 @@
-//! `POST /api/login`: a token (local requests, or any without `--remote`)
-//! or the password (`--remote`). Every attempt reserves its rate-limit slot
-//! before anything is compared or hashed (`limit.rs`); argon2 runs off the
-//! executor under `auth::HASH_PERMITS` slots. The guard already checked
-//! Host and Origin, so only same-origin attempts reach it. Nothing a body
-//! carries is ever logged.
+//! `POST /api/login`: a token or a sign-in ticket (local requests, or any
+//! without `--remote`) or the password (`--remote`). Every attempt reserves
+//! its rate-limit slot before anything is compared, looked up or hashed
+//! (`limit.rs`); argon2 runs off the executor under `auth::HASH_PERMITS`
+//! slots, a ticket's file checks in `spawn_blocking`. The guard already
+//! checked Host and Origin, so only same-origin attempts reach it. Nothing a
+//! body carries is ever logged.
 
+use std::path::Path;
 use std::sync::Arc;
-use std::time::{Duration, Instant};
+use std::time::{Duration, Instant, SystemTime};
 
 use axum::extract::{Request, State};
 use axum::http::{header, StatusCode};
@@ -14,16 +16,18 @@ use axum::response::{IntoResponse, Response};
 use serde::Deserialize;
 
 use crate::auth::{self, Auth, Caller};
+use crate::ticket;
 use claudio_core::paths;
 
 /// How long a password login waits for a hashing slot before `503`.
 pub(crate) const HASH_WAIT: Duration = Duration::from_secs(2);
 
-/// Exactly one of the two. Unknown fields are ignored, as before.
+/// Exactly one of the three. Unknown fields are ignored, as before.
 #[derive(Deserialize)]
 struct LoginBody {
     token: Option<String>,
     password: Option<String>,
+    ticket: Option<String>,
 }
 
 pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
@@ -38,11 +42,25 @@ pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Respon
         Ok(LoginBody {
             token: Some(token),
             password: None,
+            ticket: None,
         }) => token_login(&auth, &caller, &token, Instant::now()),
         Ok(LoginBody {
             token: None,
             password: Some(pw),
+            ticket: None,
         }) => password_login(&auth, &caller, pw, Instant::now()).await,
+        Ok(LoginBody {
+            token: None,
+            password: None,
+            ticket: Some(t),
+        }) => {
+            let (auth, runtime) = (Arc::clone(&auth), paths::runtime_dir());
+            tokio::task::spawn_blocking(move || {
+                ticket_login(&auth, &caller, &t, Instant::now(), &runtime)
+            })
+            .await
+            .unwrap_or(Err(StatusCode::INTERNAL_SERVER_ERROR))
+        }
         _ => Err(StatusCode::BAD_REQUEST),
     };
     if let Err(status) = status {
@@ -85,6 +103,38 @@ fn token_login(auth: &Auth, caller: &Caller, token: &str, now: Instant) -> Resul
     }
 }
 
+/// A ticket attempt (docs/web-protocol.md, "Sign-in tickets"): its slot as
+/// a token attempt takes it, the token's rule (with `--remote`, local
+/// requests only: a refused ticket's file is not touched), then
+/// [`ticket::redeem`] under `runtime`. Every refusal keeps its slot.
+/// Blocking: the login runs it in `spawn_blocking`.
+fn ticket_login(
+    auth: &Auth,
+    caller: &Caller,
+    t: &str,
+    now: Instant,
+    runtime: &Path,
+) -> Result<(), StatusCode> {
+    let slot = auth
+        .limiter
+        .reserve(caller.key, !caller.local, now)
+        .map_err(|()| StatusCode::TOO_MANY_REQUESTS)?;
+    if !auth.token_allowed(caller) {
+        return Err(StatusCode::UNAUTHORIZED);
+    }
+    match ticket::redeem(runtime, t, SystemTime::now()) {
+        Ok(()) => {
+            auth.limiter.release(slot);
+            Ok(())
+        }
+        Err(ticket::Refused::Dir(why)) => {
+            tracing::error!(error = %why, "web: the sign-in ticket directory failed its check");
+            Err(StatusCode::UNAUTHORIZED)
+        }
+        Err(ticket::Refused::Invalid) => Err(StatusCode::UNAUTHORIZED),
+    }
+}
+
 /// A password attempt: `400` without `--remote`; its slot (peer and global
 /// windows) before any hashing; a hashing slot within [`HASH_WAIT`] or
 /// `503` (the attempt is given back: it did not count); argon2 in
@@ -363,6 +413,42 @@ mod tests {
         let _ = std::fs::remove_dir_all(&dir);
     }
 
+    /// A ticket redeems once from a local caller; a refused one counts like
+    /// a wrong token; with `--remote` a non-local caller is refused before
+    /// the file is looked at, so the same ticket still redeems locally.
+    #[test]
+    fn tickets_redeem_once_count_when_refused_and_stay_local() {
+        let rt = std::env::temp_dir().join(format!("claudio-login-{}", uuid::Uuid::new_v4()));
+        paths::ensure_private_dir(&rt).unwrap();
+        let a = plain();
+        let t0 = Instant::now();
+        let t = ticket::mint(&rt, SystemTime::now()).unwrap();
+        assert_eq!(ticket_login(&a, &LOCAL, &t, t0, &rt), Ok(()));
+        for _ in 0..5 {
+            assert_eq!(
+                ticket_login(&a, &LOCAL, &t, t0, &rt),
+                Err(StatusCode::UNAUTHORIZED)
+            );
+        }
+        let fresh = ticket::mint(&rt, SystemTime::now()).unwrap();
+        assert_eq!(
+            ticket_login(&a, &LOCAL, &fresh, t0, &rt),
+            Err(StatusCode::TOO_MANY_REQUESTS),
+            "the replays counted"
+        );
+        let file = ticket::tickets_dir(&rt).join(ticket::file_name(&fresh));
+        assert!(file.exists(), "a 429 leaves the ticket untouched");
+        let r = remote();
+        let t60 = t0 + Duration::from_secs(61);
+        assert_eq!(
+            ticket_login(&r, &REMOTE, &fresh, t60, &rt),
+            Err(StatusCode::UNAUTHORIZED)
+        );
+        assert!(file.exists(), "a non-local refusal does not touch the file");
+        assert_eq!(ticket_login(&r, &LOCAL, &fresh, t60, &rt), Ok(()));
+        std::fs::remove_dir_all(&rt).unwrap();
+    }
+
     #[test]
     fn successes_do_not_count() {
         let a = plain();
diff --git a/addons/web/src/remote.rs b/addons/web/src/remote.rs
index 3b89cfd..0e19f24 100644
--- a/addons/web/src/remote.rs
+++ b/addons/web/src/remote.rs
@@ -100,17 +100,22 @@ fn url_host(ip: IpAddr) -> String {
     }
 }
 
-/// The URL a local browser uses with the token, when a loopback request
-/// can reach the listener: the address itself when loopback, `127.0.0.1`
-/// for `0.0.0.0`, `[::1]` for `[::]`.
-pub(crate) fn token_url(listen: SocketAddr, token: &str) -> Option<String> {
+/// The URL a local browser uses, without a fragment, when a loopback
+/// request can reach the listener: the address itself when loopback,
+/// `127.0.0.1` for `0.0.0.0`, `[::1]` for `[::]`.
+pub(crate) fn base_url(listen: SocketAddr) -> Option<String> {
     let host = match listen.ip() {
         ip if ip.is_loopback() => url_host(ip),
         IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_owned(),
         IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_owned(),
         _ => return None,
     };
-    Some(format!("https://{host}:{}/#t={token}", listen.port()))
+    Some(format!("https://{host}:{}/", listen.port()))
+}
+
+/// [`base_url`] with the token.
+pub(crate) fn token_url(listen: SocketAddr, token: &str) -> Option<String> {
+    base_url(listen).map(|base| format!("{base}#t={token}"))
 }
 
 /// The one stdout URL: the specific address, else the host name.
@@ -293,6 +298,8 @@ mod tests {
             Some("https://127.0.0.1:5/#t=t")
         );
         assert_eq!(token_url("192.168.1.20:5".parse().unwrap(), "t"), None);
+        assert_eq!(base_url(any).as_deref(), Some("https://127.0.0.1:7337/"));
+        assert_eq!(base_url("192.168.1.20:5".parse().unwrap()), None);
         assert_eq!(public_url(any, "box"), "https://box:7337/");
         assert_eq!(
             public_url("127.0.0.1:5".parse().unwrap(), "box"),
diff --git a/addons/web/src/ticket.rs b/addons/web/src/ticket.rs
new file mode 100644
index 0000000..8353612
--- /dev/null
+++ b/addons/web/src/ticket.rs
@@ -0,0 +1,481 @@
+//! Sign-in tickets (docs/web-protocol.md §1, "Sign-in tickets"). The
+//! `claudio` that opens a browser on this machine mints a single-use ticket
+//! and hands the browser `<base URL>#k=<ticket>` in a 0600 redirect file, so
+//! argv holds only `file://<path>`; the server redeems the ticket once. A
+//! ticket is an empty 0600 file in `<runtime>/web-tickets/`, named by the
+//! SHA-256 of the ticket: the CLI and the service the daemon runs both
+//! resolve `paths::runtime_dir()`, as the daemon socket in it relies on, so
+//! nothing secret travels between them and a listing shows no usable ticket.
+//! Blocking I/O throughout.
+
+use std::fs;
+use std::io::{self, IsTerminal};
+use std::os::unix::ffi::OsStrExt;
+use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
+use std::path::{Path, PathBuf};
+use std::time::{Duration, SystemTime};
+
+use claudio_core::paths;
+use sha2::{Digest, Sha256};
+
+use crate::WebArgs;
+
+/// A ticket's life from its mint.
+pub(crate) const LIFE: Duration = Duration::from_secs(30);
+/// How far ahead of the clock a ticket's mtime may be.
+const SKEW: Duration = Duration::from_secs(2);
+/// Live tickets at most; a mint removes the oldest beyond.
+pub(crate) const MAX_LIVE: usize = 16;
+/// A redirect file's life.
+pub(crate) const OPEN_LIFE: Duration = Duration::from_secs(60);
+/// Redirect files at most.
+pub(crate) const MAX_OPEN: usize = 16;
+/// Entries a prune looks at: both directories are ours and small.
+const SCAN: usize = 1024;
+
+/// `<runtime>/web-tickets`.
+pub(crate) fn tickets_dir(runtime: &Path) -> PathBuf {
+    runtime.join("web-tickets")
+}
+
+/// `<runtime>/web-open`.
+pub(crate) fn open_dir(runtime: &Path) -> PathBuf {
+    runtime.join("web-open")
+}
+
+/// A ticket file's name: the SHA-256 of the ticket, lowercase hex.
+pub(crate) fn file_name(ticket: &str) -> String {
+    Sha256::digest(ticket.as_bytes())
+        .iter()
+        .map(|b| format!("{b:02x}"))
+        .collect()
+}
+
+/// `runtime` and `dir` in it, private as `ensure_private_dir` makes them.
+fn private(runtime: &Path, dir: &Path) -> io::Result<()> {
+    paths::ensure_private_dir(runtime)?;
+    paths::ensure_private_dir(dir)
+}
+
+/// Whether `mtime` lies in the window that ends `life` before `now`
+/// and [`SKEW`] after it.
+fn fresh(mtime: SystemTime, now: SystemTime, life: Duration) -> bool {
+    let from = now.checked_sub(life).unwrap_or(SystemTime::UNIX_EPOCH);
+    from <= mtime && mtime <= now + SKEW
+}
+
+/// A new ticket. Its directory is checked first, expired files and the
+/// oldest beyond `MAX_LIVE - 1` go, then its empty file is created 0600,
+/// exclusively and never through a symlink.
+pub(crate) fn mint(runtime: &Path, now: SystemTime) -> io::Result<String> {
+    let dir = tickets_dir(runtime);
+    private(runtime, &dir)?;
+    prune(&dir, now, LIFE, MAX_LIVE - 1);
+    let ticket = crate::auth::random_hex(32)?;
+    fs::OpenOptions::new()
+        .write(true)
+        .create_new(true)
+        .mode(0o600)
+        .custom_flags(libc::O_NOFOLLOW)
+        .open(dir.join(file_name(&ticket)))?;
+    Ok(ticket)
+}
+
+/// Why a ticket was refused (each one a `401`).
+#[derive(Debug, PartialEq, Eq)]
+pub(crate) enum Refused {
+    /// Not 64 lowercase hex, unknown, spent, expired, or not a file of ours.
+    Invalid,
+    /// The directory failed its check: the path and why.
+    Dir(String),
+}
+
+/// Steps 3 to 6 of a redeem (docs/web-protocol.md): the form, the
+/// directories (on every redeem), the file (regular, ours, `now - 30 s ..=
+/// now + 2 s`; one that fails is removed), and its removal, which only one
+/// of two racing attempts wins.
+pub(crate) fn redeem(runtime: &Path, ticket: &str, now: SystemTime) -> Result<(), Refused> {
+    if !crate::auth::token_ok(ticket) {
+        return Err(Refused::Invalid);
+    }
+    let dir = tickets_dir(runtime);
+    private(runtime, &dir).map_err(|e| Refused::Dir(format!("{}: {e}", dir.display())))?;
+    let path = dir.join(file_name(ticket));
+    let meta = fs::symlink_metadata(&path).map_err(|_| Refused::Invalid)?;
+    let live = meta.modified().is_ok_and(|t| fresh(t, now, LIFE));
+    if !(meta.file_type().is_file() && meta.uid() == paths::uid() && live) {
+        let _ = fs::remove_file(&path);
+        return Err(Refused::Invalid);
+    }
+    fs::remove_file(&path).map_err(|_| Refused::Invalid)
+}
+
+/// At a server's start: expired tickets and old redirect files go. A
+/// directory that fails its check is left alone.
+pub(crate) fn sweep(runtime: &Path, now: SystemTime) {
+    for (dir, life, keep) in [
+        (tickets_dir(runtime), LIFE, MAX_LIVE),
+        (open_dir(runtime), OPEN_LIFE, MAX_OPEN),
+    ] {
+        if dir.exists() && private(runtime, &dir).is_ok() {
+            prune(&dir, now, life, keep);
+        }
+    }
+}
+
+/// Remove from `dir` every entry that is not a regular file or is outside
+/// its `life`, then the oldest until at most `keep` stay. Best effort: a
+/// file that stays is refused at its redeem anyway. A directory entry stays
+/// (`remove_file` refuses it).
+fn prune(dir: &Path, now: SystemTime, life: Duration, keep: usize) {
+    let Ok(entries) = fs::read_dir(dir) else {
+        return;
+    };
+    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
+    for entry in entries.flatten().take(SCAN) {
+        let path = entry.path();
+        let Ok(meta) = fs::symlink_metadata(&path) else {
+            continue;
+        };
+        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
+        if meta.file_type().is_file() && fresh(mtime, now, life) {
+            kept.push((mtime, path));
+        } else {
+            let _ = fs::remove_file(&path);
+        }
+    }
+    kept.sort();
+    let excess = kept.len().saturating_sub(keep);
+    for (_, path) in kept.into_iter().take(excess) {
+        let _ = fs::remove_file(path);
+    }
+}
+
+/// The redirect page that sends a browser to `url`, if `url` holds only
+/// what a sign-in URL holds (a scheme, an IP literal, a port, `/`, `#k=`
+/// and hex): nothing in it then needs escaping in HTML.
+pub(crate) fn redirect_html(url: &str) -> Option<String> {
+    let safe = !url.is_empty()
+        && url
+            .bytes()
+            .all(|b| b.is_ascii_alphanumeric() || b":/.[]#=".contains(&b));
+    safe.then(|| {
+        format!(
+            "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" \
+             content=\"0;url={url}\"><title>claudio</title><a href=\"{url}\">Open claudio</a>\n"
+        )
+    })
+}
+
+/// Write `<runtime>/web-open/<32 hex>.html` (0600, atomically) sending the
+/// browser to `url`; its path. Old files go first (60 s, 16 at most).
+pub(crate) fn write_redirect(runtime: &Path, url: &str, now: SystemTime) -> io::Result<PathBuf> {
+    let html = redirect_html(url).ok_or_else(|| io::Error::other("not a sign-in URL"))?;
+    let dir = open_dir(runtime);
+    private(runtime, &dir)?;
+    prune(&dir, now, OPEN_LIFE, MAX_OPEN - 1);
+    let path = dir.join(format!("{}.html", crate::auth::random_hex(16)?));
+    paths::write_atomic(&path, html.as_bytes())?;
+    Ok(path)
+}
+
+/// `file://` and `path`, every byte but an unreserved one and `/`
+/// percent-encoded.
+pub(crate) fn file_url(path: &Path) -> String {
+    let mut url = String::from("file://");
+    for &b in path.as_os_str().as_bytes() {
+        match b {
+            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
+                url.push(char::from(b))
+            }
+            _ => url.push_str(&format!("%{b:02X}")),
+        }
+    }
+    url
+}
+
+/// The URL this machine's browser reaches `running` at, without a
+/// fragment: `http://<listen>/`, or with `--remote` the loopback one
+/// (`None` for a specific non-loopback address: no local URL, no ticket).
+pub(crate) fn base_url(running: &WebArgs) -> Option<String> {
+    match running.remote {
+        None => Some(format!("http://{}/", running.listen)),
+        Some(addr) => crate::remote::base_url(addr),
+    }
+}
+
+/// A new ticket's sign-in link: `<base>#k=<ticket>`.
+pub(crate) fn link(base: &str) -> io::Result<String> {
+    let ticket = mint(&paths::runtime_dir(), SystemTime::now())?;
+    Ok(format!("{base}#k={ticket}"))
+}
+
+/// The `file://` URL of a new redirect file for `link`.
+pub(crate) fn redirect(link: &str) -> io::Result<String> {
+    let path = write_redirect(&paths::runtime_dir(), link, SystemTime::now())?;
+    Ok(file_url(&path))
+}
+
+/// `claudio web`'s browser tab (no `--app`): when one is wanted (not
+/// `--no-open`, `browser_wanted`), the default browser on a redirect file
+/// that signs it in with a ticket; for a `--remote` web with no local URL,
+/// the public one. The printed URL lines are unchanged; this only opens.
+pub(crate) fn open_tab(running: &WebArgs, no_open: bool) {
+    let wanted = claudio_core::app::browser::browser_wanted(
+        std::env::consts::OS,
+        |k| std::env::var(k).ok(),
+        io::stdout().is_terminal(),
+    );
+    if no_open || !wanted {
+        return;
+    }
+    let target = match base_url(running) {
+        Some(base) => match link(&base).and_then(|l| redirect(&l)) {
+            Ok(target) => target,
+            Err(e) => {
+                eprintln!("claudio web: cannot make a sign-in link for the browser: {e}");
+                return;
+            }
+        },
+        None => match running.remote {
+            Some(addr) => crate::remote::public_url(addr, &claudio_core::sys::hostname()),
+            None => return,
+        },
+    };
+    claudio_core::app::browser::open_browser(&target);
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use std::os::unix::fs::PermissionsExt;
+
+    fn scratch() -> PathBuf {
+        let dir = std::env::temp_dir().join(format!("claudio-ticket-{}", uuid::Uuid::new_v4()));
+        paths::ensure_private_dir(&dir).unwrap();
+        dir
+    }
+
+    fn mode(p: &Path) -> u32 {
+        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
+    }
+
+    fn age(path: &Path, by: Duration, ahead: bool) {
+        let t = match ahead {
+            true => SystemTime::now() + by,
+            false => SystemTime::now() - by,
+        };
+        fs::File::open(path).unwrap().set_modified(t).unwrap();
+    }
+
+    fn files(dir: &Path) -> usize {
+        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
+    }
+
+    #[test]
+    fn a_ticket_redeems_once_and_its_files_are_private() {
+        let rt = scratch();
+        let now = SystemTime::now();
+        let t = mint(&rt, now).unwrap();
+        assert!(crate::auth::token_ok(&t), "{t}");
+        let file = tickets_dir(&rt).join(file_name(&t));
+        assert_eq!(fs::metadata(&file).unwrap().len(), 0, "empty");
+        assert_eq!(mode(&file), 0o600);
+        assert_eq!(mode(&tickets_dir(&rt)), 0o700);
+        assert_eq!(redeem(&rt, &t, now), Ok(()));
+        assert!(!file.exists(), "spent");
+        assert_eq!(redeem(&rt, &t, now), Err(Refused::Invalid), "a replay");
+        let unknown = "0123456789abcdef".repeat(4);
+        assert_eq!(redeem(&rt, &unknown, now), Err(Refused::Invalid));
+        for bad in [
+            "",
+            "AB".repeat(32).as_str(),
+            &t[..63],
+            format!("{t}0").as_str(),
+        ] {
+            assert_eq!(redeem(&rt, bad, now), Err(Refused::Invalid), "{bad:?}");
+        }
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn an_expired_or_future_ticket_fails_and_its_file_goes() {
+        let rt = scratch();
+        let old = mint(&rt, SystemTime::now()).unwrap();
+        let file = tickets_dir(&rt).join(file_name(&old));
+        age(&file, Duration::from_secs(31), false);
+        assert_eq!(redeem(&rt, &old, SystemTime::now()), Err(Refused::Invalid));
+        assert!(!file.exists(), "an expired file is removed");
+        let ahead = mint(&rt, SystemTime::now()).unwrap();
+        let file = tickets_dir(&rt).join(file_name(&ahead));
+        age(&file, Duration::from_secs(10), true);
+        assert_eq!(
+            redeem(&rt, &ahead, SystemTime::now()),
+            Err(Refused::Invalid)
+        );
+        assert!(!file.exists(), "a future file is removed");
+        let edge = mint(&rt, SystemTime::now()).unwrap();
+        age(
+            &tickets_dir(&rt).join(file_name(&edge)),
+            Duration::from_secs(25),
+            false,
+        );
+        assert_eq!(redeem(&rt, &edge, SystemTime::now()), Ok(()), "25 s old");
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn a_ticket_that_is_a_symlink_or_a_directory_fails() {
+        let rt = scratch();
+        let dir = tickets_dir(&rt);
+        paths::ensure_private_dir(&dir).unwrap();
+        let target = rt.join("elsewhere");
+        fs::write(&target, b"").unwrap();
+        let link = "1".repeat(64);
+        std::os::unix::fs::symlink(&target, dir.join(file_name(&link))).unwrap();
+        assert_eq!(redeem(&rt, &link, SystemTime::now()), Err(Refused::Invalid));
+        assert!(target.exists(), "never followed");
+        assert!(
+            fs::symlink_metadata(dir.join(file_name(&link))).is_err(),
+            "the link is removed"
+        );
+        let sub = "2".repeat(64);
+        fs::create_dir(dir.join(file_name(&sub))).unwrap();
+        assert_eq!(redeem(&rt, &sub, SystemTime::now()), Err(Refused::Invalid));
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn the_ticket_directory_is_checked_and_repaired_at_each_redeem() {
+        let rt = scratch();
+        let t = mint(&rt, SystemTime::now()).unwrap();
+        fs::set_permissions(tickets_dir(&rt), fs::Permissions::from_mode(0o750)).unwrap();
+        assert_eq!(redeem(&rt, &t, SystemTime::now()), Ok(()));
+        assert_eq!(mode(&tickets_dir(&rt)), 0o700, "repaired");
+        let t = mint(&rt, SystemTime::now()).unwrap();
+        let real = rt.join("real");
+        fs::rename(tickets_dir(&rt), &real).unwrap();
+        std::os::unix::fs::symlink(&real, tickets_dir(&rt)).unwrap();
+        let refused = redeem(&rt, &t, SystemTime::now());
+        assert!(
+            matches!(&refused, Err(Refused::Dir(why)) if why.contains("web-tickets")),
+            "{refused:?}"
+        );
+        assert!(real.join(file_name(&t)).exists(), "nothing done through it");
+        assert!(mint(&rt, SystemTime::now()).is_err(), "no mint through it");
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn at_most_16_tickets_live_and_the_oldest_go_first() {
+        let rt = scratch();
+        let first = mint(&rt, SystemTime::now()).unwrap();
+        age(
+            &tickets_dir(&rt).join(file_name(&first)),
+            Duration::from_secs(5),
+            false,
+        );
+        let rest: Vec<String> = (0..20)
+            .map(|_| mint(&rt, SystemTime::now()).unwrap())
+            .collect();
+        assert_eq!(files(&tickets_dir(&rt)), MAX_LIVE);
+        assert_eq!(
+            redeem(&rt, &first, SystemTime::now()),
+            Err(Refused::Invalid),
+            "the oldest went first"
+        );
+        let newest = rest.last().unwrap();
+        assert_eq!(redeem(&rt, newest, SystemTime::now()), Ok(()));
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn a_start_sweeps_expired_tickets_and_old_redirects() {
+        let rt = scratch();
+        let old = mint(&rt, SystemTime::now()).unwrap();
+        let live = mint(&rt, SystemTime::now()).unwrap();
+        age(
+            &tickets_dir(&rt).join(file_name(&old)),
+            Duration::from_secs(40),
+            false,
+        );
+        let page = write_redirect(&rt, "http://127.0.0.1:1/#k=ab", SystemTime::now()).unwrap();
+        let stale = write_redirect(&rt, "http://127.0.0.1:1/#k=cd", SystemTime::now()).unwrap();
+        age(&stale, Duration::from_secs(61), false);
+        sweep(&rt, SystemTime::now());
+        assert!(!tickets_dir(&rt).join(file_name(&old)).exists());
+        assert!(tickets_dir(&rt).join(file_name(&live)).exists());
+        assert!(page.exists() && !stale.exists());
+        let empty = scratch();
+        sweep(&empty, SystemTime::now());
+        assert!(!tickets_dir(&empty).exists(), "a sweep makes nothing");
+        for dir in [rt, empty] {
+            fs::remove_dir_all(dir).unwrap();
+        }
+    }
+
+    #[test]
+    fn the_redirect_file_holds_the_link_and_nothing_to_escape() {
+        let rt = scratch();
+        let url = format!("http://127.0.0.1:7999/#k={}", "a1".repeat(32));
+        let path = write_redirect(&rt, &url, SystemTime::now()).unwrap();
+        assert_eq!(
+            fs::read_to_string(&path).unwrap(),
+            format!(
+                "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" \
+                 content=\"0;url={url}\"><title>claudio</title><a href=\"{url}\">Open claudio</a>\n"
+            )
+        );
+        assert_eq!(mode(&path), 0o600);
+        assert_eq!(mode(&open_dir(&rt)), 0o700);
+        let name = path.file_name().unwrap().to_str().unwrap();
+        assert!(name.len() == 37 && name.ends_with(".html"), "{name}");
+        assert!(redirect_html("https://[::1]:8443/#k=00").is_some());
+        for bad in [
+            "",
+            "http://x/\"><script>",
+            "http://h/#k=a b",
+            "http://h/?q&r",
+            "http://h/'",
+        ] {
+            assert_eq!(redirect_html(bad), None, "{bad}");
+        }
+        for _ in 0..20 {
+            write_redirect(&rt, &url, SystemTime::now()).unwrap();
+        }
+        assert_eq!(files(&open_dir(&rt)), MAX_OPEN);
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn file_urls_percent_encode_what_is_not_plain() {
+        assert_eq!(
+            file_url(Path::new("/run/user/1000/claudio/web-open/ab.html")),
+            "file:///run/user/1000/claudio/web-open/ab.html"
+        );
+        assert_eq!(
+            file_url(Path::new("/tmp/a b/#?%é")),
+            "file:///tmp/a%20b/%23%3F%25%C3%A9"
+        );
+    }
+
+    #[test]
+    fn base_urls_carry_no_fragment() {
+        let args = |list: &[&str]| {
+            crate::parse_args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
+        };
+        assert_eq!(
+            base_url(&args(&["--listen", "127.0.0.1:9000"])).as_deref(),
+            Some("http://127.0.0.1:9000/")
+        );
+        assert_eq!(
+            base_url(&args(&["--listen", "[::1]:9000"])).as_deref(),
+            Some("http://[::1]:9000/")
+        );
+        assert_eq!(
+            base_url(&args(&["--remote", ":8443"])).as_deref(),
+            Some("https://127.0.0.1:8443/")
+        );
+        assert_eq!(base_url(&args(&["--remote", "192.168.1.2:8443"])), None);
+    }
+}
````
Run: `cargo test --locked -p claudio-web --lib -- ticket login remote`
Expected: `test result: ok. 26 passed; 0 failed` (the 9 `ticket::tests`,
`login::tests::tickets_redeem_once_count_when_refused_and_stay_local`, and the existing tests
the filter also matches: `login::tests`, `remote::tests` with `urls_and_the_banner`'s new
`base_url` lines, and five `sessions` and `auth` tests with "login" or "remote" in their names)
Run: `cargo test --locked --test web -- --test-threads=1 ticket redirect_file`
Expected: `test result: ok. 3 passed; 0 failed` (`web_a_ticket_signs_in_once`,
`web_opens_the_browser_on_a_redirect_file_never_the_token`,
`web_remote_tickets_are_for_local_requests_only`; the last returns early where `127.0.0.2`
cannot be bound, on macOS, as `web_remote_peer_on_another_address_needs_the_password` does)
Run: `cargo fmt --all --check && cargo clippy --locked --workspace --all-targets --all-features -- -D warnings && scripts/check_rules.py`
Expected: `rules: 0 problem(s); over budget: 12293 file lines, 4287 fn lines, 20 unwraps`

Step 4: commit
`git add addons/web/src/ticket.rs addons/web/tests/web_harness/app.rs`
`git commit -m "feat(web): sign-in tickets, redeemed once, opened through a redirect file" -- addons/web/src/ticket.rs addons/web/src/login.rs addons/web/src/lib.rs addons/web/src/cli.rs addons/web/src/remote.rs addons/web/tests/web.rs addons/web/tests/web_harness/mod.rs addons/web/tests/web_harness/app.rs addons/web/tests/web_harness/service.rs`

Interfaces this task provides to later tasks: `ticket::{mint(runtime, now) -> io::Result<String>,
redeem(runtime, ticket, now) -> Result<(), Refused>, sweep(runtime, now), link(base) ->
io::Result<String>, redirect(link) -> io::Result<String>, file_url(&Path) -> String,
base_url(&WebArgs) -> Option<String>, open_tab(&WebArgs, no_open: bool), tickets_dir, file_name,
Refused}`, `remote::base_url(SocketAddr) -> Option<String>`; in the harness
`web_harness::app::{FakeBin (new, dir, browser, calls, wait_calls), desktop_env,
desktop_env_pty, runtime, ticket_file, mint_ticket, ticket_in, redirect_of}` and
`ServiceWeb::ticket_login(&self, ticket) -> u16`.

### Task 3: The page signs in with `#k=`
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/web/app/signin.js` (new),
`addons/web/web/app/boot.js`, `addons/web/web/tests/signin.test.mjs` (new),
`addons/web/web/e2e/tests/signin.spec.mjs` (new)
Goal: spec §1 "The page": AC8's sign-in half (a ticket signs in and the fragment clears; a
replayed, expired or malformed one shows the expiry message; with a live cookie the old
sign-in goes; a `429` shows the wait message; no CSP violation).

Step 1: write the failing tests (patch 5)
````diff
diff --git a/addons/web/web/e2e/tests/signin.spec.mjs b/addons/web/web/e2e/tests/signin.spec.mjs
new file mode 100644
index 0000000..953f47b
--- /dev/null
+++ b/addons/web/web/e2e/tests/signin.spec.mjs
@@ -0,0 +1,95 @@
+import { test, expect } from '@playwright/test';
+import crypto from 'node:crypto';
+import fs from 'node:fs';
+import path from 'node:path';
+import { startServer } from '../fixtures/server.mjs';
+
+// The rate-limit test spends this peer's minute: it stays last.
+let srv;
+test.beforeAll(async () => { srv = await startServer(); });
+test.afterAll(async () => { await srv?.stop(); });
+
+/** A ticket as `claudio` mints one (docs/web-protocol.md, "Sign-in tickets"), its file `age` s old. */
+function mint(age = 0) {
+  const ticket = crypto.randomBytes(32).toString('hex');
+  const dir = path.join(srv.root, 'run/claudio/web-tickets');
+  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
+  fs.chmodSync(dir, 0o700);
+  const file = path.join(dir, crypto.createHash('sha256').update(ticket).digest('hex'));
+  fs.writeFileSync(file, '', { mode: 0o600, flag: 'wx' });
+  if (age) { const t = Date.now() / 1000 - age; fs.utimesSync(file, t, t); }
+  return { ticket, file };
+}
+
+const EXPIRED = 'This sign-in link has expired. Run claudio web again.';
+const csp = (page) => page.evaluate(() => window.__claudioCsp ?? []);
+const hash = (page) => page.evaluate(() => location.hash);
+const cookieIn = async (context) => (await context.cookies()).find((c) => c.name === 'claudio_web')?.value;
+
+test('a ticket link signs in once and clears the fragment', async ({ page, browser }) => {
+  const { ticket, file } = mint();
+  await page.goto(`${srv.origin}/#k=${ticket}`);
+  await expect(page.getByTestId('rail')).toBeVisible();
+  expect(await hash(page)).toBe('');
+  expect(fs.existsSync(file)).toBe(false);
+  expect(await cookieIn(page.context())).toMatch(/^[0-9a-f]{64}$/);
+  expect(await csp(page)).toEqual([]);
+  const other = await browser.newContext();
+  const replay = await other.newPage();
+  await replay.goto(`${srv.origin}/#k=${ticket}`);
+  await expect(replay.getByText(EXPIRED)).toBeVisible();
+  expect(await hash(replay)).toBe('');
+  expect(await cookieIn(other)).toBeUndefined();
+  expect(await csp(replay)).toEqual([]);
+  await other.close();
+});
+
+test('an expired or malformed ticket shows the expiry message', async ({ page }) => {
+  const posted = [];
+  page.on('request', (r) => { if (r.url().endsWith('/api/login')) posted.push(r.url()); });
+  const { ticket, file } = mint(60);
+  await page.goto(`${srv.origin}/#k=${ticket}`);
+  await expect(page.getByText(EXPIRED)).toBeVisible();
+  expect(await hash(page)).toBe('');
+  expect(fs.existsSync(file)).toBe(false);
+  expect(posted).toHaveLength(1);
+  await page.goto('about:blank');
+  await page.goto(`${srv.origin}/#k=${ticket.toUpperCase()}`);
+  await expect(page.getByText(EXPIRED)).toBeVisible();
+  expect(await hash(page)).toBe('');
+  expect(posted).toHaveLength(1);
+  expect(await csp(page)).toEqual([]);
+});
+
+test('a ticket opened with a live cookie replaces that sign-in', async ({ page, request }) => {
+  await page.goto(srv.url);
+  await expect(page.getByTestId('rail')).toBeVisible();
+  const old = await cookieIn(page.context());
+  const { ticket, file } = mint();
+  await page.goto('about:blank');
+  await page.goto(`${srv.origin}/#k=${ticket}`);
+  await expect(page.getByTestId('rail')).toBeVisible();
+  expect(fs.existsSync(file)).toBe(false);
+  const now = await cookieIn(page.context());
+  expect(now).toMatch(/^[0-9a-f]{64}$/);
+  expect(now).not.toBe(old);
+  const me = (value) => request.get(`${srv.origin}/api/me`, { headers: { Cookie: `claudio_web=${value}` } });
+  expect((await me(old)).status()).toBe(401);
+  expect((await me(now)).status()).toBe(204);
+  expect(await csp(page)).toEqual([]);
+});
+
+test('a ticket refused by the rate limit shows the wait message', async ({ page, request }) => {
+  const wrong = () => request.post(`${srv.origin}/api/login`, {
+    data: { ticket: crypto.randomBytes(32).toString('hex') }, headers: { Origin: srv.origin },
+  });
+  let status = 0;
+  for (let i = 0; i < 6 && status !== 429; i++) status = (await wrong()).status();
+  expect(status).toBe(429);
+  const { ticket, file } = mint();
+  await page.goto(`${srv.origin}/#k=${ticket}`);
+  await expect(page.getByText('Too many attempts: wait a minute.')).toBeVisible();
+  expect(await hash(page)).toBe('');
+  expect(fs.existsSync(file)).toBe(true);
+  expect(await csp(page)).toEqual([]);
+});
diff --git a/addons/web/web/tests/signin.test.mjs b/addons/web/web/tests/signin.test.mjs
new file mode 100644
index 0000000..3cb689d
--- /dev/null
+++ b/addons/web/web/tests/signin.test.mjs
@@ -0,0 +1,25 @@
+import test from 'node:test';
+import assert from 'node:assert/strict';
+import { ticketOf, afterRefusal, TICKET_EXPIRED } from '../app/signin.js';
+
+const HEX = '0123456789abcdef'.repeat(4);
+
+test('only #k= followed by 64 lowercase hex is a ticket to post', () => {
+  assert.deepEqual(ticketOf(`#k=${HEX}`), { ticket: HEX });
+  for (const bad of ['#k=', `#k=${HEX.slice(1)}`, `#k=${HEX}0`, `#k=${HEX.toUpperCase()}`, `#k=${HEX}&x`, `#k=${HEX.slice(2)}zz`]) {
+    assert.deepEqual(ticketOf(bad), { ticket: null }, bad);
+  }
+});
+
+test('other fragments are not ticket fragments', () => {
+  for (const hash of ['', '#', `#t=${HEX}`, `#K=${HEX}`, '#k', `#x#k=${HEX}`]) assert.equal(ticketOf(hash), null, hash);
+});
+
+test('a refused ticket starts the app with a cookie, else the password, else expires', () => {
+  assert.equal(afterRefusal(204, []), 'start');
+  assert.equal(afterRefusal(401, ['token', 'password']), 'password');
+  assert.equal(afterRefusal(401, ['password']), 'password');
+  assert.equal(afterRefusal(401, ['token']), 'expired');
+  assert.equal(afterRefusal(500, []), 'expired');
+  assert.match(TICKET_EXPIRED, /expired\. Run `claudio web` again\.$/);
+});
````
Run: `node --test addons/web/web/tests/signin.test.mjs`
Expected: `# fail 1`, with `Cannot find module` naming `addons/web/web/app/signin.js`

Step 2: implement (patch 6)
````diff
diff --git a/addons/web/web/app/boot.js b/addons/web/web/app/boot.js
index 8d68460..412b819 100644
--- a/addons/web/web/app/boot.js
+++ b/addons/web/web/app/boot.js
@@ -1,3 +1,5 @@
+import { ticketOf, afterRefusal, TICKET_EXPIRED } from './signin.js';
+
 // First thing: record CSP violations so the e2e can assert there are none.
 document.addEventListener('securitypolicyviolation', (e) => {
   (window.__claudioCsp ||= []).push(e.violatedDirective);
@@ -101,7 +103,35 @@ async function start() {
   await import('./main.js');
 }
 
+/**
+ * `#k=<ticket>` (docs/web-protocol.md, "Sign-in tickets"): post it (a well-formed one only, and
+ * also with a live cookie, so it is spent), clear the fragment whatever happened, then start, or
+ * ask /api/me what is left: the cookie, the password form or the expiry message.
+ */
+async function ticketBoot(ticket) {
+  let r = null;
+  if (ticket) {
+    try {
+      r = await fetch('/api/login', {
+        method: 'POST', headers: { 'Content-Type': 'application/json' },
+        body: JSON.stringify({ ticket }), credentials: 'same-origin',
+      });
+    } catch { /* r stays null */ }
+  }
+  history.replaceState(null, '', location.pathname);
+  if (r?.status === 204) return start();
+  if (r?.status === 429) return show('claudio', LOGIN_FAILED[429]);
+  let me;
+  try { me = await fetch('/api/me', { credentials: 'same-origin' }); } catch { return show('claudio', 'Cannot reach the server.'); }
+  const next = afterRefusal(me.status, await methodsOf(me));
+  if (next === 'start') return start();
+  if (next === 'password') return showLogin('');
+  return show('claudio', TICKET_EXPIRED);
+}
+
 async function boot() {
+  const k = ticketOf(location.hash);
+  if (k) return ticketBoot(k.ticket);
   const m = /^#t=([A-Za-z0-9._~-]{1,128})$/.exec(location.hash);
   if (location.hash.startsWith('#t=')) {
     const token = m ? m[1] : '';
diff --git a/addons/web/web/app/signin.js b/addons/web/web/app/signin.js
new file mode 100644
index 0000000..1a81c23
--- /dev/null
+++ b/addons/web/web/app/signin.js
@@ -0,0 +1,25 @@
+// The sign-in ticket rules of docs/web-protocol.md §1 ("Sign-in tickets"). Imports nothing.
+
+/** What the page shows when a ticket was refused and the server offers no other way in. */
+export const TICKET_EXPIRED = 'This sign-in link has expired. Run `claudio web` again.';
+
+/**
+ * The ticket a fragment carries: null when it is not a ticket fragment (no `#k=`), else
+ * `{ ticket }`, where `ticket` is null unless the fragment is exactly `#k=<64 lowercase hex>`
+ * (a refused ticket, never posted).
+ */
+export function ticketOf(hash) {
+  if (!hash.startsWith('#k=')) return null;
+  const m = /^#k=([0-9a-f]{64})$/.exec(hash);
+  return { ticket: m ? m[1] : null };
+}
+
+/**
+ * After a refused or failed ticket, what `/api/me` leads to: 'start' (a live cookie), 'password'
+ * (the server offers the password) or 'expired'.
+ */
+export function afterRefusal(meStatus, methods) {
+  if (meStatus === 204) return 'start';
+  if (methods.includes('password')) return 'password';
+  return 'expired';
+}
````
Run: `node --test addons/web/web/tests/*.test.mjs`
Expected: `# tests 155`, `# pass 155`, `# fail 0` (152 before, and the 3 of `signin.test.mjs`)
Run: `cargo build --locked && (cd addons/web/web/e2e && npm ci --no-audit --no-fund && CLAUDIO_BIN=<target dir>/debug/claudio npx playwright test signin)`
(`CLAUDIO_BIN` is needed only when the target dir is not the worktree's `target/`; `npm ci`
with `PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1` as `make test-web` runs it.)
Expected: `4 passed`. Before patch 6 the same command gives `4 failed`, the first with
`Error: expect(locator).toBeVisible() failed` on `getByTestId('rail')`: the old page ignores
`#k=` and shows "Open the URL `claudio web` printed".

Step 3: commit
`git add addons/web/web/app/signin.js addons/web/web/tests/signin.test.mjs addons/web/web/e2e/tests/signin.spec.mjs`
`git commit -m "feat(web): the page signs in with a #k= ticket" -- addons/web/web/app/signin.js addons/web/web/app/boot.js addons/web/web/tests/signin.test.mjs addons/web/web/e2e/tests/signin.spec.mjs`

Interfaces this task provides to later tasks: `app/signin.js`: `ticketOf(hash)`,
`afterRefusal(meStatus, methods)`, `TICKET_EXPIRED`.

### Task 4: PWA icons and the manifest
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/web/icons/claudio.svg`,
`addons/web/web/icons/claudio-192.png` (new), `addons/web/web/icons/claudio-512.png` (new),
`addons/web/web/icons/claudio-maskable-512.png` (new),
`addons/web/web/dev/icons/claudio-maskable.svg` (new), `addons/web/web/dev/render-icons.mjs`
(new), `addons/web/web/manifest.webmanifest`, `addons/web/web/e2e/tests/keys.spec.mjs`
Goal: Goal 4 and spec §4: PNG icons at 192 and 512 px and a maskable one, DESIGN.md's Void
(`#0b0e12`) as the manifest colours and the icon background, the strokes Syntax Violet
(`#c58aff`) and Phosphor Green (`#00d892`); AC8's manifest half (the manifest lists the PNGs;
they are served as `image/png`).

Step 1: write the failing test, extending the existing manifest test (patch 7)
````diff
diff --git a/addons/web/web/e2e/tests/keys.spec.mjs b/addons/web/web/e2e/tests/keys.spec.mjs
index d4965e5..dd74c3e 100644
--- a/addons/web/web/e2e/tests/keys.spec.mjs
+++ b/addons/web/web/e2e/tests/keys.spec.mjs
@@ -81,5 +81,15 @@ test('the settings have Capture all keys and the manifest is served', async ({ p
   await expect(page.getByTestId('capture-keys')).toBeVisible();
   const r = await page.request.get(new URL('/manifest.webmanifest', srv.url).href);
   expect(r.headers()['content-type']).toContain('application/manifest+json');
-  expect((await r.json()).display).toBe('standalone');
+  const manifest = await r.json();
+  expect(manifest.display).toBe('standalone');
+  expect([manifest.background_color, manifest.theme_color]).toEqual(['#0b0e12', '#0b0e12']);
+  const pngs = manifest.icons.filter((i) => i.type === 'image/png');
+  expect(pngs.map((i) => [i.sizes, i.purpose])).toEqual([['192x192', 'any'], ['512x512', 'any'], ['512x512', 'maskable']]);
+  for (const icon of pngs) {
+    const png = await page.request.get(new URL(icon.src, srv.url).href);
+    expect(png.status()).toBe(200);
+    expect(png.headers()['content-type']).toBe('image/png');
+    expect([...(await png.body()).subarray(0, 8)]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
+  }
 });
````
Run: `cargo build --locked && (cd addons/web/web/e2e && CLAUDIO_BIN=<target dir>/debug/claudio npx playwright test keys)`
Expected: `1 failed`, `4 passed`: `the settings have Capture all keys and the manifest is
served` fails on `Error: expect(received).toEqual(expected) // deep equality`, `"#0b0f14"`
received twice where `"#0b0e12"` is expected.

Step 2: the icons, their sources and the manifest (patch 8; it carries the three PNGs as a git
binary patch)
````diff
diff --git a/addons/web/web/dev/icons/claudio-maskable.svg b/addons/web/web/dev/icons/claudio-maskable.svg
new file mode 100644
index 0000000..f1f0e77
--- /dev/null
+++ b/addons/web/web/dev/icons/claudio-maskable.svg
@@ -0,0 +1 @@
+<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"><rect width="64" height="64" fill="#0b0e12"/><g transform="translate(32 32) scale(0.8) translate(-32 -32)"><path d="M14 20l14 12-14 12" fill="none" stroke="#c58aff" stroke-width="6" stroke-linecap="square"/><path d="M34 46h18" stroke="#00d892" stroke-width="6" stroke-linecap="square"/></g></svg>
diff --git a/addons/web/web/dev/render-icons.mjs b/addons/web/web/dev/render-icons.mjs
new file mode 100644
index 0000000..4a05893
--- /dev/null
+++ b/addons/web/web/dev/render-icons.mjs
@@ -0,0 +1,34 @@
+// Renders the app's PNG icons from their SVG sources with Playwright's Chromium (headless, a
+// throwaway profile), once; the PNGs are committed. From the repository root:
+//   (cd addons/web/web/e2e && npm ci) && node addons/web/web/dev/render-icons.mjs
+//   icons/claudio.svg            -> icons/claudio-192.png, icons/claudio-512.png (purpose "any")
+//   dev/icons/claudio-maskable.svg -> icons/claudio-maskable-512.png (purpose "maskable": a
+//   full-bleed square, the glyph scaled 0.8 about the centre, inside the 40 % safe circle)
+import fs from 'node:fs';
+import { createRequire } from 'node:module';
+import { fileURLToPath } from 'node:url';
+
+const require = createRequire(new URL('../e2e/package.json', import.meta.url));
+const { chromium } = require('@playwright/test');
+const web = (rel) => fileURLToPath(new URL(`../${rel}`, import.meta.url));
+
+const jobs = [
+  ['icons/claudio.svg', 'icons/claudio-192.png', 192],
+  ['icons/claudio.svg', 'icons/claudio-512.png', 512],
+  ['dev/icons/claudio-maskable.svg', 'icons/claudio-maskable-512.png', 512],
+];
+
+const browser = await chromium.launch();
+try {
+  for (const [src, out, size] of jobs) {
+    const page = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
+    const svg = fs.readFileSync(web(src)).toString('base64');
+    await page.setContent(`<html><body style="margin:0;background:transparent"><img style="display:block" width="${size}" height="${size}" src="data:image/svg+xml;base64,${svg}"></body></html>`);
+    await page.locator('img').evaluate((img) => img.decode());
+    await page.screenshot({ path: web(out), omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
+    await page.close();
+    console.log(`${out}: ${size}x${size}, ${fs.statSync(web(out)).size} bytes`);
+  }
+} finally {
+  await browser.close();
+}
diff --git a/addons/web/web/icons/claudio-192.png b/addons/web/web/icons/claudio-192.png
new file mode 100644
index 0000000000000000000000000000000000000000..59b6ea60fca18daf84ae67a427424606b814be87
GIT binary patch
literal 4283
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

literal 0
HcmV?d00001

diff --git a/addons/web/web/icons/claudio-512.png b/addons/web/web/icons/claudio-512.png
new file mode 100644
index 0000000000000000000000000000000000000000..f619fdb622ab3ec4e5c583eb889f3c10b392bcf3
GIT binary patch
literal 11267
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

literal 0
HcmV?d00001

diff --git a/addons/web/web/icons/claudio-maskable-512.png b/addons/web/web/icons/claudio-maskable-512.png
new file mode 100644
index 0000000000000000000000000000000000000000..63fd8e6f7409758e97d385738026886987523d86
GIT binary patch
literal 5788
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

literal 0
HcmV?d00001

diff --git a/addons/web/web/icons/claudio.svg b/addons/web/web/icons/claudio.svg
index e5ac027..2d33712 100644
--- a/addons/web/web/icons/claudio.svg
+++ b/addons/web/web/icons/claudio.svg
@@ -1 +1 @@
-<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"><rect width="64" height="64" rx="12" fill="#0b0f14"/><path d="M14 20l14 12-14 12" fill="none" stroke="#8b7bff" stroke-width="6" stroke-linecap="square"/><path d="M34 46h18" stroke="#3ddc84" stroke-width="6" stroke-linecap="square"/></svg>
+<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"><rect width="64" height="64" rx="12" fill="#0b0e12"/><path d="M14 20l14 12-14 12" fill="none" stroke="#c58aff" stroke-width="6" stroke-linecap="square"/><path d="M34 46h18" stroke="#00d892" stroke-width="6" stroke-linecap="square"/></svg>
diff --git a/addons/web/web/manifest.webmanifest b/addons/web/web/manifest.webmanifest
index 473c62a..4169655 100644
--- a/addons/web/web/manifest.webmanifest
+++ b/addons/web/web/manifest.webmanifest
@@ -5,7 +5,12 @@
   "start_url": "/",
   "scope": "/",
   "display": "standalone",
-  "background_color": "#0b0f14",
-  "theme_color": "#0b0f14",
-  "icons": [{ "src": "/icons/claudio.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }]
+  "background_color": "#0b0e12",
+  "theme_color": "#0b0e12",
+  "icons": [
+    { "src": "/icons/claudio-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
+    { "src": "/icons/claudio-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
+    { "src": "/icons/claudio.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" },
+    { "src": "/icons/claudio-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
+  ]
 }
````
Run: `sha256sum addons/web/web/icons/*.png`
Expected:
```
77660c74281d18819ac71c3493ee17152c5f8035029d8fdba3c0873f3de291c2  addons/web/web/icons/claudio-192.png
2165974ccfce4d915f448df8530b5d3f1bd2cf5c3b8f7d32aff75ee286f0a420  addons/web/web/icons/claudio-512.png
1ec35891f60ae94e6df08999d3fbb2a119f7d68c275c80b4301050a2a9c4ce26  addons/web/web/icons/claudio-maskable-512.png
```
How they were made (for the record; re-running it rewrites the same bytes):
`(cd addons/web/web/e2e && npm ci --no-audit --no-fund) && node addons/web/web/dev/render-icons.mjs`
prints `icons/claudio-192.png: 192x192, 4283 bytes`, `icons/claudio-512.png: 512x512, 11267
bytes`, `icons/claudio-maskable-512.png: 512x512, 5788 bytes`.
Run: `cargo build --locked && (cd addons/web/web/e2e && CLAUDIO_BIN=<target dir>/debug/claudio npx playwright test keys)`
Expected: `5 passed`

Step 3: commit
`git add addons/web/web/icons/claudio-192.png addons/web/web/icons/claudio-512.png addons/web/web/icons/claudio-maskable-512.png addons/web/web/dev/icons/claudio-maskable.svg addons/web/web/dev/render-icons.mjs`
`git commit -m "feat(web): PNG and maskable icons, and DESIGN.md's Void in the manifest" -- addons/web/web/icons/claudio.svg addons/web/web/icons/claudio-192.png addons/web/web/icons/claudio-512.png addons/web/web/icons/claudio-maskable-512.png addons/web/web/dev/icons/claudio-maskable.svg addons/web/web/dev/render-icons.mjs addons/web/web/manifest.webmanifest addons/web/web/e2e/tests/keys.spec.mjs`

Interfaces this task provides to later tasks: the embedded `/icons/claudio.svg`,
`/icons/claudio-192.png`, `/icons/claudio-512.png` (Task 6's launcher icons).

### Task 5: `claudio web --app` and `--app=<url>`
Model: opus   Repo: /home/p4u/claudio   Files: `crates/core/src/app/browser.rs`,
`addons/web/src/app_window.rs` (new), `addons/web/src/lib.rs`, `addons/web/src/cli.rs`,
`addons/web/src/service.rs`, `addons/web/tests/web.rs`, `addons/web/tests/web_harness/service.rs`
Goal: spec §2 and §2b, Goals 0 and 1, and the compatibility rule for an older running service:
AC1 (the browser search), AC2 (the argv, the sandboxed fallback), AC5 (`--app` with and without
a terminal; failures on the desktop), AC11 (`--foreground --app`, `CLAUDIO_NO_BROWSER`, the
fallback line), AC13 (the URL check), AC14 (`--app=<url>` starts nothing else), and the forced
restart of a service older than the binary. `display_present` lands in the core first, in its
own commit (A3).

Step 1: write the failing integration tests (patch 9)
````diff
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index 0bafee4..190f107 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -2310,3 +2310,249 @@ fn web_opens_the_browser_on_a_redirect_file_never_the_token() {
     assert_eq!(svc.ticket_login(&ticket), 204, "the ticket signs in");
     svc.finish();
 }
+
+// ── The app window (docs/specs/2026-11-13-desktop-design.md) ─────────────────
+
+/// `--class=claudio` where the app argv has it.
+fn class() -> Vec<String> {
+    match cfg!(target_os = "linux") {
+        true => vec!["--class=claudio".to_owned()],
+        false => Vec::new(),
+    }
+}
+
+/// AC5: `claudio web --app`, in a terminal and without one, starts the
+/// browser once each with `--app=file://<redirect file>`, the file holding
+/// a ticket; without a terminal neither stdout nor stderr holds the token
+/// or the ticket.
+#[test]
+fn web_app_opens_a_signed_in_window_and_prints_no_secret_without_a_terminal() {
+    let mut svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
+    let token = svc.token();
+    let fake = FakeBin::new(&svc.h().root);
+    let mut cmd = portable_pty::CommandBuilder::new(BINARY);
+    cmd.args(["web", "--app"]);
+    svc.h().apply(&mut cmd);
+    desktop_env_pty(&mut cmd, &fake);
+    let mut tty = common::TuiProcess::spawn(cmd);
+    assert_eq!(tty.wait_exit_code(DAEMON_WAIT), 0);
+    fake.wait_calls("fake-browser", 1, WAIT);
+    let out = claudio(svc.h(), &["web", "--app"], |cmd| desktop_env(cmd, &fake));
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    let (stdout, stderr) = (
+        String::from_utf8_lossy(&out.stdout).into_owned(),
+        String::from_utf8_lossy(&out.stderr).into_owned(),
+    );
+    assert_eq!(
+        stdout,
+        format!("claudio web: http://127.0.0.1:{}/\n", svc.port)
+    );
+    assert!(
+        !stdout.contains(&token) && !stderr.contains(&token),
+        "{stderr}"
+    );
+    let calls = fake.wait_calls("fake-browser", 2, WAIT);
+    assert_eq!(calls.len(), 2, "once per run: {calls:?}");
+    for (i, call) in calls.iter().enumerate() {
+        assert_eq!(call[1..], class(), "{call:?}");
+        let url = call[0].strip_prefix("--app=").expect("--app=<url>");
+        assert!(!url.contains('#') && !url.contains(&token), "{url}");
+        let html = redirect_of(svc.h(), url);
+        assert!(
+            html.contains(&format!("url=http://127.0.0.1:{}/#k=", svc.port)),
+            "{html}"
+        );
+        let ticket = ticket_in(&html);
+        assert!(!call.iter().any(|a| a.contains(&ticket)), "{call:?}");
+        if i == 1 {
+            assert!(!stdout.contains(&ticket) && !stderr.contains(&ticket));
+        }
+        assert_eq!(svc.ticket_login(&ticket), 204, "its ticket signs in");
+    }
+    assert!(fake.calls("xdg-open").is_empty() && fake.calls("open").is_empty());
+    svc.finish();
+}
+
+/// AC5: without a terminal, an `--app` whose `[web]` cannot start (here
+/// `--remote` without a password) is said on the desktop (a fake
+/// `notify-send`, `osascript` on macOS) and in `<runtime>/web-app.log`.
+#[test]
+fn web_app_failures_reach_the_desktop_without_a_terminal() {
+    let h = ManagerHarness::new();
+    let fake = FakeBin::new(&h.root);
+    let remote = format!("127.0.0.1:{}", free_port());
+    let out = claudio(&h, &["web", "--app", "--remote", &remote], |cmd| {
+        desktop_env(cmd, &fake)
+    });
+    assert_eq!(out.status.code(), Some(1), "{out:?}");
+    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
+    assert!(stderr.contains("--set-password"), "{stderr}");
+    let notifier = match cfg!(target_os = "macos") {
+        true => "osascript",
+        false => "notify-send",
+    };
+    let calls = fake.wait_calls(notifier, 1, WAIT);
+    let message = calls[0].last().expect("the message").clone();
+    assert!(
+        message.starts_with("claudio web: ") && message.contains("--set-password"),
+        "{message}"
+    );
+    assert!(stderr.contains(&message), "{stderr}");
+    if notifier == "notify-send" {
+        assert_eq!(calls[0][..3], ["-a", "claudio", "claudio"]);
+    }
+    let log = fs::read_to_string(runtime(&h).join("web-app.log")).unwrap();
+    assert!(log.contains(&message), "{log}");
+    assert!(fake.calls("fake-browser").is_empty(), "no window");
+}
+
+/// AC11: `claudio web --foreground --app` opens the window once the server
+/// listens, on a ticket of its own; without a terminal its stdout line has
+/// no token.
+#[test]
+fn web_foreground_app_opens_the_window_once_it_listens() {
+    let h = ManagerHarness::new();
+    let fake = FakeBin::new(&h.root);
+    let port = free_port();
+    let mut cmd = Command::new(BINARY);
+    cmd.args(["web", "--foreground", "--app", "--listen"])
+        .arg(format!("127.0.0.1:{port}"));
+    apply_env(&h, &mut cmd);
+    desktop_env(&mut cmd, &fake);
+    let log = fs::File::create(h.root.join("web.log")).unwrap();
+    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
+    let mut child = cmd.spawn().expect("start claudio web");
+    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
+    // Dropped (killed) on every path from here.
+    let web = Web {
+        h: Some(h),
+        child,
+        port,
+        token: String::new(),
+    };
+    let line = lines.next().and_then(Result::ok).unwrap_or_default();
+    assert_eq!(line, format!("claudio web: http://127.0.0.1:{port}/"));
+    let calls = fake.wait_calls("fake-browser", 1, DAEMON_WAIT);
+    assert_eq!(web.get("/", &[]).status, 200, "it listens");
+    assert_eq!(calls[0][1..], class());
+    let html = redirect_of(web.h(), calls[0][0].strip_prefix("--app=").unwrap());
+    let body = json!({ "ticket": ticket_in(&html) }).to_string();
+    let reply = web.post("/api/login", Some(&web.origin()), &body, &[]);
+    assert_eq!(reply.status, 204, "{reply:?}");
+    let token = fs::read_to_string(web.h().config_home.join("claudio/web-token")).unwrap();
+    let stderr = fs::read_to_string(web.h().root.join("web.log")).unwrap();
+    assert!(!stderr.contains(token.trim()), "{stderr}");
+}
+
+/// AC11: `CLAUDIO_NO_BROWSER=1 claudio web --app` starts nothing and mints
+/// nothing.
+#[test]
+fn web_app_with_claudio_no_browser_opens_and_mints_nothing() {
+    let svc = ServiceWeb::start(ManagerHarness::new(), &[], |_| {});
+    let fake = FakeBin::new(&svc.h().root);
+    let out = claudio(svc.h(), &["web", "--app"], |cmd| {
+        desktop_env(cmd, &fake);
+        cmd.env("CLAUDIO_NO_BROWSER", "1");
+    });
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    thread::sleep(std::time::Duration::from_millis(300));
+    assert!(fake.calls("fake-browser").is_empty());
+    let tickets = fs::read_dir(runtime(svc.h()).join("web-tickets"));
+    assert_eq!(tickets.map(|d| d.count()).unwrap_or(0), 0, "nothing minted");
+    svc.finish();
+}
+
+/// AC14: `claudio web --app=<url>` starts the browser once on exactly that
+/// URL and nothing else: no daemon, no token, `config.toml` unchanged. A
+/// URL with credentials exits 2 with the message.
+#[test]
+fn web_app_url_opens_only_the_window() {
+    let h = ManagerHarness::new();
+    let fake = FakeBin::new(&h.root);
+    let config = h.config_home.join("claudio/config.toml");
+    let before = fs::read_to_string(&config).unwrap();
+    let url = "https://example.invalid:7337/";
+    let arg = format!("--app={url}");
+    let out = claudio(&h, &["web", &arg], |cmd| desktop_env(cmd, &fake));
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    let calls = fake.wait_calls("fake-browser", 1, WAIT);
+    let want: Vec<String> = std::iter::once(arg.clone()).chain(class()).collect();
+    assert_eq!(calls, [want]);
+    let sockets = fs::read_dir(runtime(&h))
+        .map(|d| {
+            d.flatten()
+                .filter(|e| e.path().extension().is_some_and(|x| x == "sock"))
+                .count()
+        })
+        .unwrap_or(0);
+    assert_eq!(sockets, 0, "no daemon");
+    assert_eq!(
+        fs::read_to_string(&config).unwrap(),
+        before,
+        "config.toml unchanged"
+    );
+    assert!(
+        !h.config_home.join("claudio/web-token").exists(),
+        "no token"
+    );
+    let bad = claudio(&h, &["web", "--app=https://u:p@example.invalid/"], |cmd| {
+        desktop_env(cmd, &fake)
+    });
+    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
+    assert_eq!(
+        String::from_utf8_lossy(&bad.stderr),
+        "claudio web --app: expected an http(s) URL without credentials or fragment\n"
+    );
+    assert_eq!(fake.calls("fake-browser").len(), 1);
+}
+
+/// Compatibility: a service that started before this binary's file changed
+/// (as when a busy daemon kept it) is restarted from the binary on disk
+/// before an `--app` window opens on a ticket; a current one is kept.
+#[test]
+fn web_app_restarts_a_service_older_than_its_binary() {
+    let h = ManagerHarness::new();
+    let bin = h.root.join("bin/claudio");
+    fs::create_dir_all(bin.parent().unwrap()).unwrap();
+    fs::copy(BINARY, &bin).unwrap();
+    let fake = FakeBin::new(&h.root);
+    let port = free_port();
+    let listen = format!("127.0.0.1:{port}");
+    let mut svc = ServiceWeb::adopt(h, port);
+    let out = claudio_at(
+        &bin,
+        svc.h(),
+        &["web", "--listen", &listen, "--no-open"],
+        |_| {},
+    );
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    let first = svc.running_other_than(None, WAIT);
+    let app = |svc: &ServiceWeb| {
+        claudio_at(&bin, svc.h(), &["web", "--app"], |cmd| {
+            desktop_env(cmd, &fake)
+        })
+    };
+    let out = app(&svc);
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    fake.wait_calls("fake-browser", 1, WAIT);
+    assert_eq!(
+        svc.running_other_than(None, WAIT),
+        first,
+        "a current service is kept"
+    );
+    // The same build (the daemon stays), its file newer than the service.
+    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
+    fs::File::open(&bin).unwrap().set_modified(later).unwrap();
+    let out = app(&svc);
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    let calls = fake.wait_calls("fake-browser", 2, WAIT);
+    svc.running_other_than(Some(first), WAIT);
+    let html = redirect_of(svc.h(), calls[1][0].strip_prefix("--app=").unwrap());
+    assert_eq!(
+        svc.ticket_login(&ticket_in(&html)),
+        204,
+        "the restarted service redeems it"
+    );
+    svc.token();
+    svc.finish();
+}
diff --git a/addons/web/tests/web_harness/service.rs b/addons/web/tests/web_harness/service.rs
index 53ff997..1765f52 100644
--- a/addons/web/tests/web_harness/service.rs
+++ b/addons/web/tests/web_harness/service.rs
@@ -2,6 +2,7 @@
 //! CLI, read `claudio web status`, and end every test with no process of
 //! its own left and no token in a log (AC5, AC11).
 
+use std::path::Path;
 use std::process::{Command, Output, Stdio};
 use std::time::{Duration, Instant};
 use std::{fs, thread};
@@ -12,7 +13,17 @@ use crate::common::{ManagerHarness, BINARY};
 /// `claudio <args>` in `h`'s isolated env, run to its end: at most 60 s,
 /// so a command that keeps running fails the test instead of hanging it.
 pub fn claudio(h: &ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> Output {
-    let mut cmd = Command::new(BINARY);
+    claudio_at(Path::new(BINARY), h, args, setup)
+}
+
+/// [`claudio`] with the binary at `bin` (a copy of it, say).
+pub fn claudio_at(
+    bin: &Path,
+    h: &ManagerHarness,
+    args: &[&str],
+    setup: impl FnOnce(&mut Command),
+) -> Output {
+    let mut cmd = Command::new(bin);
     cmd.args(args);
     apply_env(h, &mut cmd);
     setup(&mut cmd);
````
Run: `cargo test --locked --test web -- --test-threads=1 web_app_url_opens_only_the_window`
Expected: `test result: FAILED. 0 passed; 1 failed`, with

```
thread 'web_app_url_opens_only_the_window' panicked at addons/web/tests/web.rs:2477:5:
assertion `left == right` failed: Output { status: ExitStatus(unix_wait_status(512)), stdout: "", stderr: "claudio web: unknown argument \"--app\"\n…
  left: Some(2)
 right: Some(0)
```

Step 2: `display_present` in the core (patch 10)
````diff
diff --git a/crates/core/src/app/browser.rs b/crates/core/src/app/browser.rs
index af326d4..64419b9 100644
--- a/crates/core/src/app/browser.rs
+++ b/crates/core/src/app/browser.rs
@@ -1,13 +1,17 @@
 //! Opening a sign-in page in the member's browser.
 
+/// Whether a window can open here: always on macOS, else with a non-empty
+/// `DISPLAY` or `WAYLAND_DISPLAY` (Linux and the BSDs).
+pub fn display_present(os: &str, env: impl Fn(&str) -> Option<String>) -> bool {
+    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
+    os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY")
+}
+
 /// Whether to open the sign-in page in a browser: there is one to show it
-/// (a display on Linux and the BSDs, always on macOS), stdout is a terminal
-/// (someone is there to use it), and `CLAUDIO_NO_BROWSER` is unset. The URL
-/// is printed either way.
+/// ([`display_present`]), stdout is a terminal (someone is there to use
+/// it), and `CLAUDIO_NO_BROWSER` is unset. The URL is printed either way.
 pub fn browser_wanted(os: &str, env: impl Fn(&str) -> Option<String>, tty: bool) -> bool {
-    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
-    let display = os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY");
-    display && tty && env("CLAUDIO_NO_BROWSER").is_none()
+    display_present(os, &env) && tty && env("CLAUDIO_NO_BROWSER").is_none()
 }
 
 /// Open `url` with `open` (macOS) or `xdg-open`, without waiting for it;
@@ -66,4 +70,25 @@ mod tests {
         let opted_out = env(&[("DISPLAY", ":0"), ("CLAUDIO_NO_BROWSER", "1")]);
         assert!(!browser_wanted("linux", opted_out, true));
     }
+
+    #[test]
+    fn a_display_is_a_non_empty_display_variable_or_macos() {
+        let env = |vars: &'static [(&'static str, &'static str)]| {
+            move |k: &str| {
+                vars.iter()
+                    .find(|(n, _)| *n == k)
+                    .map(|(_, v)| v.to_string())
+            }
+        };
+        assert!(display_present("linux", env(&[("DISPLAY", ":0")])));
+        assert!(display_present("linux", env(&[("WAYLAND_DISPLAY", "w-0")])));
+        assert!(!display_present("linux", env(&[("DISPLAY", "")])));
+        assert!(!display_present("linux", env(&[])), "an ssh session");
+        assert!(display_present("macos", env(&[])));
+        let no_tty_needed = env(&[("DISPLAY", ":0"), ("CLAUDIO_NO_BROWSER", "1")]);
+        assert!(
+            display_present("linux", no_tty_needed),
+            "the opt-out is the caller's"
+        );
+    }
 }
````
Run: `cargo test --locked -p claudio-core --lib app::browser`
Expected: `test result: ok. 2 passed; 0 failed`
Then: `git commit -m "feat(core): display_present in app::browser" -- crates/core/src/app/browser.rs`

Step 3: implement `app_window.rs` (with its unit tests), the flags (with their unit tests),
the `cli.rs` wiring and the older-service restart (patch 11)
````diff
diff --git a/addons/web/src/app_window.rs b/addons/web/src/app_window.rs
new file mode 100644
index 0000000..5083b9e
--- /dev/null
+++ b/addons/web/src/app_window.rs
@@ -0,0 +1,744 @@
+//! `claudio web --app`: the manager in a chromeless window of a
+//! Chromium-family browser (docs/web.md, "App window and launcher"). Bare
+//! `--app` opens this machine's web service, signed in with a ticket through
+//! a redirect file (`ticket.rs`); `--app=<url>` opens a web on another
+//! machine and starts nothing else. Both need a display but no terminal: a
+//! desktop launcher runs them, so a failure also goes to `web-app.log` and a
+//! desktop notification. The browser is spawned without a shell; its argv
+//! holds `--app=` and the redirect file's `file://` URL, or the URL the user
+//! gave, never the token, and the ticket only for a sandboxed browser.
+
+use std::fs;
+use std::io::{self, IsTerminal, Write};
+use std::os::unix::fs::OpenOptionsExt;
+use std::os::unix::process::CommandExt;
+use std::path::{Path, PathBuf};
+use std::process::{Command, ExitCode, Stdio};
+use std::time::{Duration, Instant};
+
+use claudio_core::app::browser;
+use claudio_core::claude::SESSION_MARKERS;
+use claudio_core::paths;
+
+use crate::remote::Remote;
+use crate::{ticket, WebArgs};
+
+/// What `--app` opens.
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub(crate) enum AppTarget {
+    /// `--app`: this machine's web service, signed in with a ticket.
+    Local,
+    /// `--app=<url>`: an existing web elsewhere ([`url_ok`]).
+    Url(String),
+}
+
+/// `--app=<url>` refused (exit 2).
+pub(crate) const URL_BAD: &str =
+    "claudio web --app: expected an http(s) URL without credentials or fragment";
+/// No Chromium-family browser: the default one was opened.
+pub(crate) const FALLBACK: &str = "no Chromium-family browser found: opened the default browser \
+(an app window needs Chrome, Chromium, Brave or Edge)";
+/// A sandboxed browser was given the sign-in link in argv.
+pub(crate) const SANDBOXED: &str = "this browser is sandboxed: the sign-in link is visible to \
+other users of this machine for up to 30 s";
+
+/// The Chromium-family names looked for on Linux, in this order.
+const LINUX_NAMES: [&str; 7] = [
+    "google-chrome",
+    "google-chrome-stable",
+    "chromium",
+    "chromium-browser",
+    "brave-browser",
+    "microsoft-edge",
+    "microsoft-edge-stable",
+];
+/// Where a name is looked for after `$PATH` (then the user's flatpak exports).
+const LINUX_DIRS: [&str; 4] = [
+    "/usr/bin",
+    "/usr/local/bin",
+    "/snap/bin",
+    "/var/lib/flatpak/exports/bin",
+];
+/// The macOS bundles looked for, in this order.
+const MAC_BUNDLES: [&str; 4] = [
+    "Google Chrome",
+    "Chromium",
+    "Brave Browser",
+    "Microsoft Edge",
+];
+/// `web-app.log` starts over past this size.
+const LOG_MAX: u64 = 64 * 1024;
+/// One logged line at most, in bytes.
+const LOG_LINE: usize = 4096;
+/// A desktop notification's deadline.
+const NOTIFY_WAIT: Duration = Duration::from_secs(3);
+
+/// `--app`'s value: none is the local service; a URL must pass [`url_ok`].
+pub(crate) fn target(value: Option<String>) -> Result<AppTarget, String> {
+    match value {
+        None => Ok(AppTarget::Local),
+        Some(url) if url_ok(&url) => Ok(AppTarget::Url(url)),
+        Some(_) => Err(URL_BAD.to_owned()),
+    }
+}
+
+/// `--app=<url>`: `http` or `https`, a host, an optional port and path. No
+/// userinfo, no query and no fragment, so no password, token (`#t=`) or
+/// ticket (`#k=`) can reach argv this way.
+pub(crate) fn url_ok(url: &str) -> bool {
+    let rest = ["http://", "https://"].iter().find_map(|scheme| {
+        url.get(..scheme.len())
+            .filter(|head| head.eq_ignore_ascii_case(scheme))
+            .map(|_| &url[scheme.len()..])
+    });
+    let Some(rest) = rest else {
+        return false;
+    };
+    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
+    authority_ok(authority) && path.bytes().all(path_byte)
+}
+
+/// `host[:port]`: a name of `[A-Za-z0-9.-]` not starting with `-` or `.`
+/// (an IPv4 address is one), or `[<IPv6>]`; a port of 1 to 65535.
+fn authority_ok(authority: &str) -> bool {
+    let (host_ok, port) = match authority.strip_prefix('[') {
+        Some(v6) => match v6.split_once(']') {
+            Some((ip, tail)) => (ip.parse::<std::net::Ipv6Addr>().is_ok(), tail),
+            None => return false,
+        },
+        None => {
+            let at = authority.find(':').unwrap_or(authority.len());
+            let (host, tail) = authority.split_at(at);
+            let name = !host.is_empty()
+                && !host.starts_with(['-', '.'])
+                && host
+                    .bytes()
+                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
+            (name, tail)
+        }
+    };
+    let port_ok = match port.strip_prefix(':') {
+        None => port.is_empty(),
+        Some(p) => {
+            !p.is_empty()
+                && p.bytes().all(|b| b.is_ascii_digit())
+                && p.parse::<u16>().is_ok_and(|n| n > 0)
+        }
+    };
+    host_ok && port_ok
+}
+
+/// A byte RFC 3986 allows in a path: unreserved, `%`, sub-delims, `:`, `@`
+/// and `/`.
+fn path_byte(b: u8) -> bool {
+    b.is_ascii_alphanumeric() || b"-._~%!$&'()*+,;=:@/".contains(&b)
+}
+
+/// What the browser search looks at (pure: [`find_browser`]).
+pub(crate) struct Search {
+    pub os: &'static str,
+    /// `[web] browser`, else `$CLAUDIO_BROWSER`: a path or a name.
+    pub chosen: Option<String>,
+    /// Where a name is looked for: `$PATH`, then the fixed directories.
+    pub dirs: Vec<PathBuf>,
+    /// Where the macOS bundles are looked for.
+    pub bundles: Vec<PathBuf>,
+}
+
+impl Search {
+    /// This process's search: `configured` (`[web] browser`), the env and
+    /// `home`. No login shell is run.
+    pub(crate) fn here(
+        os: &'static str,
+        configured: Option<String>,
+        env: impl Fn(&str) -> Option<String>,
+        home: &Path,
+    ) -> Search {
+        let mut dirs: Vec<PathBuf> = env("PATH")
+            .map(|p| {
+                std::env::split_paths(&p)
+                    .filter(|d| d.is_absolute())
+                    .collect()
+            })
+            .unwrap_or_default();
+        dirs.extend(LINUX_DIRS.map(PathBuf::from));
+        dirs.push(home.join(".local/share/flatpak/exports/bin"));
+        let set = |v: Option<String>| v.filter(|v| !v.is_empty());
+        Search {
+            os,
+            chosen: set(configured).or_else(|| set(env("CLAUDIO_BROWSER"))),
+            dirs,
+            bundles: vec![PathBuf::from("/Applications"), home.join("Applications")],
+        }
+    }
+}
+
+/// The browser an app window opens in. A chosen one always wins (a name
+/// found in `dirs`, else as given, for the spawn to report); else on Linux
+/// the first of [`LINUX_NAMES`] in `dirs`, on macOS the executable of the
+/// first of [`MAC_BUNDLES`] in `bundles`. `exists` is the only look at the
+/// disk.
+pub(crate) fn find_browser(s: &Search, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
+    let lookup = |name: &str| s.dirs.iter().map(|d| d.join(name)).find(|p| exists(p));
+    if let Some(chosen) = &s.chosen {
+        return Some(match chosen.contains('/') {
+            true => PathBuf::from(chosen),
+            false => lookup(chosen).unwrap_or_else(|| PathBuf::from(chosen)),
+        });
+    }
+    match s.os {
+        "macos" => MAC_BUNDLES
+            .iter()
+            .flat_map(|b| {
+                s.bundles
+                    .iter()
+                    .map(move |root| root.join(format!("{b}.app/Contents/MacOS/{b}")))
+            })
+            .find(|p| exists(p)),
+        _ => LINUX_NAMES.iter().find_map(|name| lookup(name)),
+    }
+}
+
+/// Whether the browser found at `path`, resolving to `resolved`, is a snap
+/// or a flatpak, which cannot read the runtime dir: under `/snap`, the snap
+/// launcher (`/snap/bin/*` link to `/usr/bin/snap`), or in a flatpak
+/// `exports/bin`.
+pub(crate) fn sandboxed(path: &Path, resolved: &Path) -> bool {
+    let boxed =
+        |p: &Path| p.starts_with("/snap") || p.to_string_lossy().contains("/flatpak/exports/bin/");
+    boxed(path) || boxed(resolved) || resolved.file_name().is_some_and(|n| n == "snap")
+}
+
+/// An app window's argv on `target`: `--app=<target>`, and `--class=claudio`
+/// on Linux.
+pub(crate) fn app_argv(os: &str, target: &str) -> Vec<String> {
+    let mut argv = vec![format!("--app={target}")];
+    if os == "linux" {
+        argv.push("--class=claudio".to_owned());
+    }
+    argv
+}
+
+/// `[web] browser` in config.toml (written by hand, never by claudio).
+fn configured() -> Option<String> {
+    let text = fs::read_to_string(paths::config_file()).ok()?;
+    let table: toml::Table = text.parse().ok()?;
+    table
+        .get("web")?
+        .get("browser")?
+        .as_str()
+        .map(str::to_owned)
+}
+
+/// Start `browser` with `argv`, detached: no shell, null stdio, a process
+/// group of its own (a Ctrl+C in this terminal never reaches it), reaped
+/// but never waited for.
+fn spawn(browser: &Path, argv: &[String]) -> Result<(), String> {
+    let mut cmd = Command::new(browser);
+    cmd.args(argv)
+        .stdin(Stdio::null())
+        .stdout(Stdio::null())
+        .stderr(Stdio::null())
+        .process_group(0);
+    for marker in SESSION_MARKERS {
+        cmd.env_remove(marker);
+    }
+    match cmd.spawn() {
+        Ok(mut child) => {
+            std::thread::spawn(move || child.wait());
+            Ok(())
+        }
+        Err(e) => Err(format!("cannot start {}: {e}", browser.display())),
+    }
+}
+
+/// An app window on `url`. With `sign_in`, `url` is the local web's base and
+/// the window gets a new ticket in a redirect file (the link itself for a
+/// sandboxed browser). Nothing with `CLAUDIO_NO_BROWSER`; an error without
+/// a display; the default browser when no Chromium-family one is found.
+fn window(url: &str, sign_in: bool) -> Result<(), String> {
+    let env = |k: &str| std::env::var(k).ok();
+    let os = std::env::consts::OS;
+    if env("CLAUDIO_NO_BROWSER").is_some() {
+        return Ok(());
+    }
+    if !browser::display_present(os, env) {
+        return Err(format!(
+            "claudio web --app: no display here; open {url} in a browser on this machine"
+        ));
+    }
+    let search = Search::here(os, configured(), env, &paths::home());
+    let found = find_browser(&search, |p| p.is_file());
+    let boxed = found
+        .as_deref()
+        .is_some_and(|b| sandboxed(b, &fs::canonicalize(b).unwrap_or_else(|_| b.to_owned())));
+    let target = match sign_in {
+        false => url.to_owned(),
+        true => {
+            let link =
+                ticket::link(url).map_err(|e| format!("cannot make a sign-in ticket: {e}"))?;
+            match boxed {
+                true => {
+                    eprintln!("claudio web: {SANDBOXED}");
+                    link
+                }
+                false => ticket::redirect(&link)
+                    .map_err(|e| format!("cannot write the sign-in page: {e}"))?,
+            }
+        }
+    };
+    match found {
+        Some(b) => spawn(&b, &app_argv(os, &target)),
+        None if browser::open_browser(&target) => {
+            eprintln!("claudio web: {FALLBACK}");
+            Ok(())
+        }
+        None => Err("no Chromium-family browser found, and no default browser starts".to_owned()),
+    }
+}
+
+/// `claudio web --app=<url>`: the window and nothing else: no daemon, no
+/// token, no ticket, no `[web]`.
+pub(crate) fn open_url(url: &str) -> ExitCode {
+    match window(url, false) {
+        Ok(()) => ExitCode::SUCCESS,
+        Err(why) => {
+            let line = line(&why);
+            eprintln!("{line}");
+            tell_desktop(&line);
+            ExitCode::FAILURE
+        }
+    }
+}
+
+/// After a start printed its lines (`running`: what serves; `asked`: this
+/// command line): `--app`'s window, signed in (with no local URL, a
+/// specific non-loopback `--remote` address, the public one and no
+/// ticket); else `claudio web`'s tab (`ticket::open_tab`).
+pub(crate) fn open(running: &WebArgs, asked: &WebArgs) -> Result<(), String> {
+    if asked.app.is_none() {
+        ticket::open_tab(running, asked.no_open);
+        return Ok(());
+    }
+    match (ticket::base_url(running), running.remote) {
+        (Some(base), _) => window(&base, true),
+        (None, Some(addr)) => window(
+            &crate::remote::public_url(addr, &claudio_core::sys::hostname()),
+            false,
+        ),
+        (None, None) => Ok(()),
+    }
+}
+
+/// Whether [`open`] would open a browser on a ticket, for the check of an
+/// older service (`cli.rs`).
+pub(crate) fn opens(asked: &WebArgs) -> bool {
+    let env = |k: &str| std::env::var(k).ok();
+    match asked.app {
+        Some(_) => env("CLAUDIO_NO_BROWSER").is_none(),
+        None => {
+            let tty = io::stdout().is_terminal();
+            !asked.no_open && browser::browser_wanted(std::env::consts::OS, env, tty)
+        }
+    }
+}
+
+/// A start's lines (`crate::announce`); but with `--app` and stdout not a
+/// terminal (a desktop launcher), none that holds the token: the base URL
+/// alone on stdout, and the `--remote` banner without its token link.
+pub(crate) fn announce(running: &WebArgs, asked: &WebArgs, url: &str, remote: Option<&Remote>) {
+    if asked.app.is_none() || io::stdout().is_terminal() {
+        return crate::announce(url, remote);
+    }
+    let line = match remote {
+        Some(r) => {
+            token_free(&r.banner.stderr).for_each(|l| eprintln!("{l}"));
+            r.banner.stdout.clone()
+        }
+        None => format!("claudio web: http://{}/", running.listen),
+    };
+    let mut out = io::stdout();
+    let _ = writeln!(out, "{line}");
+    let _ = out.flush();
+}
+
+/// `lines` but those with a token link (`#t=`).
+fn token_free(lines: &[String]) -> impl Iterator<Item = &String> {
+    lines.iter().filter(|l| !l.contains("#t="))
+}
+
+/// A failure's line: `why` when it is one already (`claudio web --app: …`),
+/// else `claudio web: <why>`.
+pub(crate) fn line(why: &str) -> String {
+    match why.starts_with("claudio web") {
+        true => why.to_owned(),
+        false => format!("claudio web: {why}"),
+    }
+}
+
+/// An `--app` failure's `line`, already on stderr, also where a desktop
+/// user sees it: appended to `<runtime>/web-app.log` and, when stderr is
+/// not a terminal, a desktop notification. It holds no token or ticket.
+pub(crate) fn tell_desktop(line: &str) {
+    append_log(&paths::runtime_dir(), line);
+    if !io::stderr().is_terminal() {
+        notify(std::env::consts::OS, line);
+    }
+}
+
+/// Append `line` (one line, at most 4 KiB) to `<runtime>/web-app.log`,
+/// 0600, never through a symlink; a log that would pass 64 KiB starts over.
+/// Best effort: the line is on stderr already.
+fn append_log(runtime: &Path, line: &str) {
+    if paths::ensure_private_dir(runtime).is_err() {
+        return;
+    }
+    let mut end = line.len().min(LOG_LINE);
+    while !line.is_char_boundary(end) {
+        end -= 1;
+    }
+    let entry = format!("{} {}\n", paths::unix_now(), line[..end].replace('\n', " "));
+    let path = runtime.join("web-app.log");
+    let full = fs::symlink_metadata(&path).is_ok_and(|m| m.len() + entry.len() as u64 > LOG_MAX);
+    if full {
+        let _ = paths::write_atomic(&path, entry.as_bytes());
+        return;
+    }
+    let file = fs::OpenOptions::new()
+        .append(true)
+        .create(true)
+        .mode(0o600)
+        .custom_flags(libc::O_NOFOLLOW)
+        .open(&path);
+    if let Ok(mut f) = file {
+        let _ = f.write_all(entry.as_bytes());
+    }
+}
+
+/// The notifier and its argv: `message` is one argument, never through a
+/// shell (on macOS the item of an `on run argv` script).
+fn notifier(os: &str, message: &str) -> (&'static str, Vec<String>) {
+    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
+    match os {
+        "macos" => (
+            "osascript",
+            words(&[
+                "-e",
+                "on run argv",
+                "-e",
+                "display notification (item 1 of argv) with title \"claudio\"",
+                "-e",
+                "end run",
+                message,
+            ]),
+        ),
+        _ => ("notify-send", words(&["-a", "claudio", "claudio", message])),
+    }
+}
+
+/// `message` as a desktop notification: stdin null, output dropped, killed
+/// after 3 s; nothing when the notifier is not on `PATH`.
+fn notify(os: &str, message: &str) {
+    let (program, argv) = notifier(os, message);
+    let mut cmd = Command::new(program);
+    cmd.args(&argv)
+        .stdin(Stdio::null())
+        .stdout(Stdio::null())
+        .stderr(Stdio::null());
+    for marker in SESSION_MARKERS {
+        cmd.env_remove(marker);
+    }
+    let Ok(mut child) = cmd.spawn() else {
+        return;
+    };
+    let deadline = Instant::now() + NOTIFY_WAIT;
+    while Instant::now() < deadline {
+        if !matches!(child.try_wait(), Ok(None)) {
+            return;
+        }
+        std::thread::sleep(Duration::from_millis(50));
+    }
+    let _ = child.kill();
+    let _ = child.wait();
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    fn search(os: &'static str, chosen: Option<&str>, dirs: &[&str]) -> Search {
+        Search {
+            os,
+            chosen: chosen.map(str::to_owned),
+            dirs: dirs.iter().map(PathBuf::from).collect(),
+            bundles: vec!["/Applications".into(), "/home/u/Applications".into()],
+        }
+    }
+
+    fn on_disk(files: &'static [&'static str]) -> impl Fn(&Path) -> bool {
+        move |p: &Path| files.iter().any(|f| p == Path::new(f))
+    }
+
+    #[test]
+    fn the_chosen_browser_wins_and_config_beats_the_env() {
+        let env = |k: &str| match k {
+            "CLAUDIO_BROWSER" => Some("/opt/env-browser".to_owned()),
+            "PATH" => Some("/fake/bin:relative:/other".to_owned()),
+            _ => None,
+        };
+        let home = Path::new("/home/u");
+        let s = Search::here("linux", Some("/opt/configured".into()), env, home);
+        assert_eq!(s.chosen.as_deref(), Some("/opt/configured"));
+        let s = Search::here("linux", None, env, home);
+        assert_eq!(s.chosen.as_deref(), Some("/opt/env-browser"));
+        assert_eq!(
+            Search::here("linux", Some(String::new()), env, home)
+                .chosen
+                .as_deref(),
+            Some("/opt/env-browser"),
+            "an empty [web] browser is none"
+        );
+        let dirs: Vec<&str> = s.dirs.iter().map(|d| d.to_str().unwrap()).collect();
+        assert_eq!(
+            dirs,
+            [
+                "/fake/bin",
+                "/other",
+                "/usr/bin",
+                "/usr/local/bin",
+                "/snap/bin",
+                "/var/lib/flatpak/exports/bin",
+                "/home/u/.local/share/flatpak/exports/bin"
+            ]
+        );
+        assert_eq!(
+            find_browser(&s, |_| false),
+            Some(PathBuf::from("/opt/env-browser"))
+        );
+        let named = search("linux", Some("my-chrome"), &["/a", "/b"]);
+        let found = find_browser(&named, on_disk(&["/b/my-chrome", "/b/google-chrome"]));
+        assert_eq!(found, Some(PathBuf::from("/b/my-chrome")));
+        assert_eq!(
+            find_browser(&named, |_| false),
+            Some(PathBuf::from("my-chrome")),
+            "not found: as given, for the spawn to report"
+        );
+    }
+
+    #[test]
+    fn linux_looks_for_the_names_in_order_over_the_dirs() {
+        let s = search("linux", None, &["/a", "/b"]);
+        let found = find_browser(&s, on_disk(&["/a/chromium", "/b/google-chrome"]));
+        assert_eq!(
+            found,
+            Some(PathBuf::from("/b/google-chrome")),
+            "names first"
+        );
+        let found = find_browser(&s, on_disk(&["/b/brave-browser", "/a/brave-browser"]));
+        assert_eq!(found, Some(PathBuf::from("/a/brave-browser")), "then dirs");
+        let edge = find_browser(&s, on_disk(&["/b/microsoft-edge-stable"]));
+        assert_eq!(edge, Some(PathBuf::from("/b/microsoft-edge-stable")));
+        assert_eq!(find_browser(&s, |_| false), None);
+        let empty = search("linux", None, &[]);
+        assert_eq!(find_browser(&empty, |_| true), None, "no search roots");
+        assert!(FALLBACK.starts_with("no Chromium-family browser found: opened the default"));
+        assert_eq!(
+            FALLBACK,
+            "no Chromium-family browser found: opened the default browser (an app window \
+             needs Chrome, Chromium, Brave or Edge)"
+        );
+    }
+
+    #[test]
+    fn macos_looks_for_the_bundles_in_order() {
+        let s = search("macos", None, &["/usr/bin"]);
+        let found = find_browser(
+            &s,
+            on_disk(&[
+                "/home/u/Applications/Chromium.app/Contents/MacOS/Chromium",
+                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
+                "/usr/bin/google-chrome",
+            ]),
+        );
+        assert_eq!(
+            found,
+            Some(PathBuf::from(
+                "/home/u/Applications/Chromium.app/Contents/MacOS/Chromium"
+            ))
+        );
+        const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
+        let found = find_browser(
+            &s,
+            on_disk(&[
+                CHROME,
+                "/home/u/Applications/Chromium.app/Contents/MacOS/Chromium",
+            ]),
+        );
+        assert_eq!(found, Some(PathBuf::from(CHROME)));
+        assert_eq!(find_browser(&s, on_disk(&["/usr/bin/google-chrome"])), None);
+    }
+
+    #[test]
+    fn the_app_argv_holds_the_file_url_and_never_a_secret() {
+        let token = "ab".repeat(32);
+        let file = ticket::file_url(Path::new("/run/user/1000/claudio/web-open/0f.html"));
+        assert_eq!(
+            app_argv("linux", &file),
+            [
+                "--app=file:///run/user/1000/claudio/web-open/0f.html",
+                "--class=claudio"
+            ]
+        );
+        assert_eq!(
+            app_argv("macos", &file),
+            ["--app=file:///run/user/1000/claudio/web-open/0f.html"]
+        );
+        for arg in app_argv("linux", &file) {
+            assert!(!arg.contains("#t=") && !arg.contains("#k=") && !arg.contains(&token));
+        }
+        let link = format!("http://127.0.0.1:7337/#k={token}");
+        assert_eq!(
+            app_argv("linux", &link)[0],
+            format!("--app={link}"),
+            "the sandboxed fallback: the one argv with #k="
+        );
+    }
+
+    #[test]
+    fn snaps_and_flatpaks_are_sandboxed() {
+        let yes = |p: &str, r: &str| sandboxed(Path::new(p), Path::new(r));
+        assert!(yes("/snap/bin/chromium", "/usr/bin/snap"));
+        assert!(yes(
+            "/usr/bin/chromium",
+            "/snap/chromium/3000/usr/lib/chromium"
+        ));
+        assert!(yes(
+            "/var/lib/flatpak/exports/bin/com.google.Chrome",
+            "/var/lib/flatpak/app/com.google.Chrome/current/active/export/bin/com.google.Chrome"
+        ));
+        assert!(yes(
+            "/home/u/.local/share/flatpak/exports/bin/org.chromium.Chromium",
+            "/home/u/.local/share/flatpak/exports/bin/org.chromium.Chromium"
+        ));
+        assert!(!yes(
+            "/usr/bin/google-chrome",
+            "/opt/google/chrome/google-chrome"
+        ));
+        assert!(!yes("/usr/bin/snapper", "/usr/bin/snapper"));
+    }
+
+    #[test]
+    fn app_urls_are_http_with_a_host_and_no_secret() {
+        for ok in [
+            "https://z6:7337/",
+            "http://127.0.0.1:7337",
+            "https://[::1]:8443/a/b",
+            "HTTPS://Z6.example.org/claudio/",
+            "https://example.invalid:7337/",
+            "http://h/a;b=c/%41",
+        ] {
+            assert!(url_ok(ok), "{ok}");
+            assert_eq!(
+                target(Some(ok.to_owned())),
+                Ok(AppTarget::Url(ok.to_owned()))
+            );
+        }
+        for bad in [
+            "",
+            "https://",
+            "https:///x",
+            "https://user:pass@z6:7337/",
+            "https://user@z6/",
+            "https://z6:7337/#t=ab",
+            "https://z6:7337/#k=ab",
+            "https://z6/#x",
+            "https://z6#x",
+            "https://z6/?q=1",
+            "ftp://z6/",
+            "file:///etc/passwd",
+            "javascript:alert(1)",
+            "z6:7337",
+            "https://z6:0/",
+            "https://z6:65536/",
+            "https://z6:/",
+            "https://-z6/",
+            "https://[::1/",
+            "https://[z6]/",
+            "https://z6/a b",
+            "https://z6/\"",
+            "https://é.example/",
+        ] {
+            assert!(!url_ok(bad), "{bad}");
+            assert_eq!(
+                target(Some(bad.to_owned())),
+                Err(URL_BAD.to_owned()),
+                "{bad}"
+            );
+        }
+        assert_eq!(target(None), Ok(AppTarget::Local));
+        assert_eq!(
+            URL_BAD,
+            "claudio web --app: expected an http(s) URL without credentials or fragment"
+        );
+    }
+
+    #[test]
+    fn the_notifier_takes_the_message_as_one_argument() {
+        let msg = "claudio web: it failed; $(rm -rf ~) \"quoted\"";
+        assert_eq!(
+            notifier("linux", msg),
+            (
+                "notify-send",
+                vec![
+                    "-a".into(),
+                    "claudio".into(),
+                    "claudio".into(),
+                    msg.to_owned()
+                ]
+            )
+        );
+        let (program, argv) = notifier("macos", msg);
+        assert_eq!(program, "osascript");
+        assert_eq!(argv.last().map(String::as_str), Some(msg));
+        assert_eq!(argv[1], "on run argv");
+        assert!(argv[3].contains("(item 1 of argv)"));
+    }
+
+    #[test]
+    fn failures_are_lines_and_the_log_is_capped() {
+        assert_eq!(line("cannot start x"), "claudio web: cannot start x");
+        assert_eq!(line(URL_BAD), URL_BAD);
+        let rt = std::env::temp_dir().join(format!("claudio-appwin-{}", uuid::Uuid::new_v4()));
+        append_log(&rt, "claudio web: first\nsecond");
+        let log = rt.join("web-app.log");
+        let text = fs::read_to_string(&log).unwrap();
+        assert!(text.ends_with(" claudio web: first second\n"), "{text}");
+        use std::os::unix::fs::PermissionsExt;
+        assert_eq!(
+            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
+            0o600
+        );
+        let long = "é".repeat(3000);
+        for _ in 0..40 {
+            append_log(&rt, &long);
+        }
+        let size = fs::metadata(&log).unwrap().len();
+        assert!(size <= LOG_MAX, "{size}");
+        assert!(fs::read_to_string(&log)
+            .unwrap()
+            .lines()
+            .all(|l| l.len() < LOG_LINE + 32));
+        fs::remove_dir_all(&rt).unwrap();
+    }
+
+    #[test]
+    fn the_remote_banner_loses_its_token_line_only() {
+        let lines = [
+            "claudio web: TLS certificate SHA-256 AB".to_owned(),
+            "claudio web: on this machine: https://127.0.0.1:7337/#t=abc".to_owned(),
+            "claudio web: other machines log in with the password".to_owned(),
+        ];
+        let kept: Vec<&String> = token_free(&lines).collect();
+        assert_eq!(kept, [&lines[0], &lines[2]]);
+    }
+}
diff --git a/addons/web/src/cli.rs b/addons/web/src/cli.rs
index 20501e3..4df1a8d 100644
--- a/addons/web/src/cli.rs
+++ b/addons/web/src/cli.rs
@@ -4,7 +4,8 @@
 //! exits. Also `claudio web stop` and `status`, and the service restart
 //! `--set-password` and `--print-url --rotate-token` ask for. Whether the
 //! daemon has the service is decided on `Welcome.features` (`addon:web`)
-//! only, never on an error's text.
+//! only, never on an error's text. With `--app` the start opens a window
+//! (`app_window.rs`) and every failure also reaches the desktop.
 
 use std::future::Future;
 use std::io;
@@ -17,7 +18,7 @@ use claudio_core::proto::Msg;
 use serde_json::{json, Value};
 
 use crate::service::{self, Reported, Unit};
-use crate::{auth, parse_args, password, remote, start_url, ticket, tls, WebArgs};
+use crate::{app_window, auth, parse_args, password, remote, start_url, ticket, tls, WebArgs};
 
 /// `claudio web` against a daemon without `addon:web`.
 const NO_SERVICE: &str = "the daemon has no web service (older, or built without the web \
@@ -122,22 +123,20 @@ fn log_path() -> std::path::PathBuf {
 /// `claudio web [flags]`: check, save `[web]`, have the daemon serve it,
 /// print what a foreground start prints.
 pub(crate) fn start(args: WebArgs) -> ExitCode {
+    let app = args.app.is_some();
     let config = paths::config_file();
     let run = match stored(&config) {
-        Err(e) => return fail(&e),
+        Err(e) => return failed(app, 1, &e),
         Ok(web) => service::merge(&web, &args).and_then(|unit| service::run_args(&unit, &args)),
     };
     let run = match run {
         Ok(run) => run,
-        Err(e) => {
-            eprintln!("claudio web: {e}");
-            return ExitCode::from(2);
-        }
+        Err(e) => return failed(app, 2, &e),
     };
     // The token first: the service only ever loads it.
     let token = match auth::load_or_create_token(&auth::token_path(), run.rotate_token) {
         Ok(token) => token,
-        Err(e) => return fail(&format!("cannot read or write the token file: {e}")),
+        Err(e) => return failed(app, 1, &format!("cannot read or write the token file: {e}")),
     };
     if run.print_url {
         println!("{}", start_url(&run, &token));
@@ -159,23 +158,73 @@ pub(crate) fn start(args: WebArgs) -> ExitCode {
     }
     let force = run.rotate_token || run.rotate_cert;
     match block_on(ask("apply", json!({ "force": force }), true)) {
-        Ok(Answer::Replied(Reported::Running { argv, .. })) => announce(&argv, &token, &run),
-        Ok(Answer::Replied(Reported::Failed { tail, .. })) => {
-            eprintln!("claudio web: the web service did not start: {tail}");
-            fail(&format!(
-                "see {}; it is tried again (claudio web status)",
+        Ok(Answer::Replied(Reported::Running { argv, since, .. })) => {
+            match current(argv, since, &run) {
+                Ok(argv) => announce(&argv, &token, &run),
+                Err(code) => code,
+            }
+        }
+        Ok(Answer::Replied(Reported::Failed { tail, .. })) => failed(
+            app,
+            1,
+            &format!(
+                "the web service did not start: {tail}\nsee {}; it is tried again (claudio web status)",
                 log_path().display()
-            ))
+            ),
+        ),
+        Ok(Answer::Replied(other)) => failed(app, 1, &format!("the web service is {}", what(&other))),
+        Ok(Answer::NoService | Answer::NotRunning) => failed(app, 1, NO_SERVICE),
+        Err(e) => failed(app, 1, &e),
+    }
+}
+
+/// The argv of the service this start's browser reaches. One that started
+/// before this binary's file changed runs older code (a busy daemon kept
+/// it), whose page ignores `#k=`: when this start opens a browser on a
+/// ticket, it is restarted (`apply { force: true }`; the daemon starts the
+/// binary at `Config::claudio`, a path, so the one on disk, this one).
+fn current(argv: Vec<String>, since: u64, run: &WebArgs) -> Result<Vec<String>, ExitCode> {
+    if !older_than_me(since) || !app_window::opens(run) {
+        return Ok(argv);
+    }
+    match block_on(ask("apply", json!({ "force": true }), false)) {
+        Ok(Answer::Replied(Reported::Running { argv, .. })) => Ok(argv),
+        _ => {
+            let old = argv.get(2..).and_then(|a| parse_args(a).ok());
+            if let Some(base) = old.as_ref().and_then(ticket::base_url) {
+                println!("claudio web: {base}");
+            }
+            let why = "the web service is older than this claudio: run `claudio daemon restart`";
+            Err(failed(run.app.is_some(), 1, why))
         }
-        Ok(Answer::Replied(other)) => fail(&format!("the web service is {}", what(&other))),
-        Ok(Answer::NoService | Answer::NotRunning) => fail(NO_SERVICE),
-        Err(e) => fail(&e),
     }
 }
 
+/// Whether a service that started at `since` (Unix seconds) is older than
+/// this binary's file.
+fn older_than_me(since: u64) -> bool {
+    std::env::current_exe()
+        .and_then(std::fs::metadata)
+        .and_then(|m| m.modified())
+        .ok()
+        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
+        .is_some_and(|mtime| since < mtime.as_secs())
+}
+
 fn fail(why: &str) -> ExitCode {
-    eprintln!("claudio web: {why}");
-    ExitCode::FAILURE
+    failed(false, 1, why)
+}
+
+/// `why` on stderr as its line (`app_window::line`), with `--app` (`app`)
+/// also where a desktop user sees it (`app_window::tell_desktop`); then
+/// exit `code`.
+fn failed(app: bool, code: u8, why: &str) -> ExitCode {
+    let line = app_window::line(why);
+    eprintln!("{line}");
+    if app {
+        app_window::tell_desktop(&line);
+    }
+    ExitCode::from(code)
 }
 
 /// [`fail`] once the token is loaded. `--rotate-token` has then already
@@ -186,15 +235,20 @@ fn fail_after_token(run: &WebArgs, why: &str) -> ExitCode {
     if run.rotate_token {
         restart_if_enabled("a new token");
     }
-    fail(why)
+    failed(run.app.is_some(), 1, why)
 }
 
 /// What a foreground start prints, for what runs (the reply's argv: two
-/// `claudio web` at once each print the one that won), then the browser,
-/// signed in with a ticket (`ticket::open_tab`).
+/// `claudio web` at once each print the one that won), then the browser
+/// tab or (`--app`) the window, signed in with a ticket.
 fn announce(argv: &[String], token: &str, asked: &WebArgs) -> ExitCode {
+    let app = asked.app.is_some();
     let Some(Ok(running)) = argv.get(2..).map(parse_args) else {
-        return fail("the daemon runs a web service this claudio cannot read");
+        return failed(
+            app,
+            1,
+            "the daemon runs a web service this claudio cannot read",
+        );
     };
     let url = start_url(&running, token);
     let remote = match running.remote {
@@ -203,13 +257,15 @@ fn announce(argv: &[String], token: &str, asked: &WebArgs) -> ExitCode {
             let password = password::read(&password::path());
             match remote::prepare(&running, addr, token, &password) {
                 Ok(r) => Some(r),
-                Err(e) => return fail(&e),
+                Err(e) => return failed(app, 1, &e),
             }
         }
     };
-    crate::announce(&url, remote.as_ref());
-    ticket::open_tab(&running, asked.no_open);
-    ExitCode::SUCCESS
+    app_window::announce(&running, asked, &url, remote.as_ref());
+    match app_window::open(&running, asked) {
+        Ok(()) => ExitCode::SUCCESS,
+        Err(why) => failed(app, 1, &why),
+    }
 }
 
 /// A state in words.
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 49d5b3a..4bba824 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -5,6 +5,7 @@
 mod addon;
 mod addon_feed;
 mod addons;
+mod app_window;
 pub(crate) mod assets;
 mod atomic;
 pub(crate) mod auth;
@@ -60,7 +61,8 @@ pub(crate) fn version() -> String {
 
 const USAGE: &str =
     "usage: claudio web [--foreground] [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
-[--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
+[--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token] [--app]
+       claudio web --app=URL
        claudio web stop | status
        claudio web --set-password
        claudio web --print-cert";
@@ -88,6 +90,9 @@ pub(crate) struct WebArgs {
     /// `--service`: the child the daemon runs (docs/daemon-protocol.md,
     /// "Addon services"): `ready` on stdout, never the token, stops at stdin EOF.
     pub service: bool,
+    /// `--app` or `--app=<url>`: an app window. This invocation's only: never
+    /// in `[web]` or the service's argv.
+    pub app: Option<app_window::AppTarget>,
 }
 
 /// Shared by every handler.
@@ -128,6 +133,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
         set_password: false,
         foreground: false,
         service: false,
+        app: None,
     };
     let mut it = args.iter().peekable();
     while let Some(arg) = it.next() {
@@ -163,6 +169,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
             "--set-password" => out.set_password = true,
             "--foreground" => out.foreground = true,
             "--service" => out.service = true,
+            "--app" => out.app = Some(app_window::target(inline)?),
             other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
         }
     }
@@ -173,12 +180,19 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
 /// The flags that do not go together.
 fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
     let one_shot = a.no_open || a.print_url || a.rotate_token || a.rotate_cert;
-    if a.service && (one_shot || a.set_password || a.print_cert || a.foreground) {
+    if a.service && (one_shot || a.set_password || a.print_cert || a.foreground || a.app.is_some())
+    {
         return Err("--service takes only --listen=, --remote= and --allow-host=".to_owned());
     }
     if (a.set_password || a.print_cert) && n_args != 1 {
         return Err("--set-password and --print-cert take no other argument".to_owned());
     }
+    if matches!(a.app, Some(app_window::AppTarget::Url(_))) && n_args != 1 {
+        return Err("--app=URL takes no other argument".to_owned());
+    }
+    if a.app.is_some() && (a.no_open || a.print_url) {
+        return Err("--app opens a window: drop --no-open and --print-url".to_owned());
+    }
     if a.remote.is_some() && a.listen_given {
         return Err("--remote is the one listener: drop --listen".to_owned());
     }
@@ -273,7 +287,7 @@ pub fn run(args: &[String], addons: Arc<claudio_core::addon::Addons>) -> ExitCod
     let args = match parse_args(args) {
         Ok(args) => args,
         Err(e) => {
-            eprintln!("claudio web: {e}");
+            eprintln!("{}", app_window::line(&e));
             return ExitCode::from(2);
         }
     };
@@ -448,7 +462,7 @@ fn load_sessions(
     sessions::Sessions::new(store, epoch, path)
 }
 
-/// `--set-password` or `--print-cert`: one thing, then exit.
+/// `--set-password`, `--print-cert` or `--app=<url>`: one thing, then exit.
 fn one_shot(args: &WebArgs) -> Option<ExitCode> {
     if args.set_password {
         return Some(match password::set_password() {
@@ -463,6 +477,9 @@ fn one_shot(args: &WebArgs) -> Option<ExitCode> {
             }
         });
     }
+    if let Some(app_window::AppTarget::Url(url)) = &args.app {
+        return Some(app_window::open_url(url));
+    }
     args.print_cert.then(print_cert)
 }
 
@@ -552,7 +569,8 @@ async fn serve(
 /// Once the listener is bound and the hub runs: `--service` (no `url`)
 /// writes `ready` on stdout, the line the daemon waits for, and nothing
 /// else (never the token); a foreground start prints its URL (and banner)
-/// and opens a browser, signed in with a ticket (`ticket::open_tab`).
+/// and opens a browser tab or (`--app`) a window, signed in with a ticket.
+/// A window that cannot open is said; the server keeps serving.
 fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
     if !env!("CLAUDIO_BUILD_LABEL").is_empty() {
         eprintln!("claudio web: local build {}", version());
@@ -563,8 +581,10 @@ fn started(args: &WebArgs, url: Option<&str>, remote: Option<&remote::Remote>) {
         let _ = out.flush();
         return;
     };
-    announce(url, remote);
-    ticket::open_tab(args, args.no_open);
+    app_window::announce(args, args, url, remote);
+    if let Err(why) = app_window::open(args, args) {
+        eprintln!("{}", app_window::line(&why));
+    }
 }
 
 /// The start lines: with `--remote` the banner's stderr lines first, then
@@ -822,9 +842,46 @@ mod tests {
             "--set-password",
             "--print-cert",
             "--foreground",
+            "--app",
         ] {
             let err = parse_args(&args(&["--service", "--remote", one_shot])).unwrap_err();
             assert!(err.contains("--service takes only"), "{one_shot}: {err}");
         }
     }
+
+    #[test]
+    fn app_goes_with_the_service_flags_and_app_url_with_none() {
+        use app_window::AppTarget;
+        assert_eq!(parse_args(&[]).unwrap().app, None);
+        for ok in [
+            &["--app"][..],
+            &["--app", "--listen", "127.0.0.1:9000"],
+            &["--app", "--remote", ":8443", "--rotate-cert"],
+            &["--app", "--allow-host", "a.example"],
+            &["--app", "--foreground"],
+            &["--rotate-token", "--app"],
+        ] {
+            let a = parse_args(&args(ok)).map(|a| a.app);
+            assert_eq!(a, Ok(Some(AppTarget::Local)), "{ok:?}");
+        }
+        for bad in [&["--app", "--no-open"][..], &["--print-url", "--app"]] {
+            let err = parse_args(&args(bad)).unwrap_err();
+            assert!(err.contains("--app opens a window"), "{bad:?}: {err}");
+        }
+        for bad in [&["--app", "--set-password"][..], &["--print-cert", "--app"]] {
+            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
+        }
+        let url = parse_args(&args(&["--app=https://z6:7337/"])).unwrap();
+        assert_eq!(url.app, Some(AppTarget::Url("https://z6:7337/".into())));
+        for bad in [
+            &["--app=https://z6:7337/", "--foreground"][..],
+            &["--listen", "127.0.0.1:1", "--app=https://z6/"],
+        ] {
+            let err = parse_args(&args(bad)).unwrap_err();
+            assert_eq!(err, "--app=URL takes no other argument", "{bad:?}");
+        }
+        for bad in ["--app=", "--app=https://u:p@z6/", "--app=https://z6/#t=x"] {
+            assert_eq!(parse_args(&args(&[bad])).unwrap_err(), app_window::URL_BAD);
+        }
+    }
 }
diff --git a/addons/web/src/service.rs b/addons/web/src/service.rs
index 278c290..22c75a2 100644
--- a/addons/web/src/service.rs
+++ b/addons/web/src/service.rs
@@ -151,6 +151,7 @@ pub(crate) fn run_args(unit: &Unit, args: &WebArgs) -> Result<WebArgs, String> {
     run.print_url = args.print_url;
     run.rotate_token = args.rotate_token;
     run.rotate_cert = args.rotate_cert;
+    run.app = args.app.clone();
     Ok(run)
 }
 
@@ -375,6 +376,14 @@ mod tests {
         );
         let err = run_args(&Unit::default(), &asked).unwrap_err();
         assert!(err.contains("--rotate-cert needs --remote"), "{err}");
+        let app = flags(&["--app", "--listen", "127.0.0.1:9000"]);
+        assert_eq!(
+            Unit::of(&app).flags(),
+            words(&["--listen=127.0.0.1:9000"]),
+            "--app is never stored nor in the service's argv"
+        );
+        let run = run_args(&Unit::of(&app), &app).unwrap();
+        assert_eq!(run.app, Some(crate::app_window::AppTarget::Local));
         let foreign = Unit {
             allow_hosts: words(&["a.example:8443"]),
             ..remote
````
Run: `cargo test --locked -p claudio-web --lib`
Expected: `test result: ok. 139 passed; 0 failed` (was 129: `app_window`'s 9 tests,
`tests::app_goes_with_the_service_flags_and_app_url_with_none`; the service and lib tests
extended in place)
Run: `cargo test --locked --test web -- --test-threads=1 web_app web_foreground_app`
Expected: `test result: ok. 7 passed; 0 failed` (`web_app_failures_reach_the_desktop_without_a_terminal`,
`web_app_opens_a_signed_in_window_and_prints_no_secret_without_a_terminal`,
`web_app_restarts_a_service_older_than_its_binary`, `web_app_url_opens_only_the_window`,
`web_app_with_claudio_no_browser_opens_and_mints_nothing`,
`web_foreground_app_opens_the_window_once_it_listens`, and the existing
`web_approve_is_guarded_by_state_kind_and_seq`)
(`web_approve_is_guarded_by_state_kind_and_seq`, an existing test, matches the filter too.)
Run: `cargo fmt --all --check && cargo clippy --locked --workspace --all-targets --all-features -- -D warnings && scripts/check_rules.py`
Expected: `rules: 0 problem(s); over budget: 12293 file lines, 4287 fn lines, 20 unwraps`

Step 4: commit
`git add addons/web/src/app_window.rs`
`git commit -m "feat(web): claudio web --app and --app=<url>, an app window" -- addons/web/src/app_window.rs addons/web/src/lib.rs addons/web/src/cli.rs addons/web/src/service.rs addons/web/tests/web.rs addons/web/tests/web_harness/service.rs`

Interfaces this task provides to later tasks: `app_window::{AppTarget {Local, Url(String)},
target, url_ok, URL_BAD, FALLBACK, SANDBOXED, open_url, open, opens, announce, line,
tell_desktop}`; `WebArgs.app`; `web_harness::service::claudio_at(bin, h, args, setup)`.

### Task 6: The launchers
Model: sonnet   Repo: /home/p4u/claudio   Files: `addons/web/src/launcher/mod.rs` (new),
`addons/web/src/launcher/linux.rs` (new), `addons/web/src/launcher/macos.rs` (new),
`addons/web/src/assets.rs`, `addons/web/src/lib.rs`, `addons/web/tests/web.rs`
Goal: spec §3, Goal 2: AC7 (the `.desktop` file with every hard exe path checked against the
expected bytes, the icons, the macOS bundle, its `Info.plist`, the 0755 script and the icns;
remove, twice; a foreign launcher refused) and AC14's second half (`--install-launcher
--app=<url>` writes `Exec="<exe>" web --app=<url>` under the test `HOME`).

Step 1: write the failing integration test (patch 12)
````diff
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index 190f107..ec2dc9d 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -2556,3 +2556,50 @@ fn web_app_restarts_a_service_older_than_its_binary() {
     svc.token();
     svc.finish();
 }
+
+/// AC7, AC14: `--install-launcher --app=<url>` writes a launcher running
+/// `web --app=<url>` under the test `HOME` (`XDG_DATA_HOME` unset);
+/// `--remove-launcher` takes it away, and again finds nothing to remove.
+#[test]
+fn web_install_launcher_writes_under_home_and_remove_takes_it_away() {
+    let h = ManagerHarness::new();
+    let no_data_home = |cmd: &mut Command| {
+        cmd.env_remove("XDG_DATA_HOME");
+    };
+    let url = "https://example.invalid:7337/";
+    let arg = format!("--app={url}");
+    let out = claudio(&h, &["web", "--install-launcher", &arg], no_data_home);
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    let exes = [
+        BINARY.to_owned(),
+        fs::canonicalize(BINARY).unwrap().display().to_string(),
+    ];
+    let (launcher, line) = match cfg!(target_os = "macos") {
+        true => (
+            h.home.join("Applications/claudio.app"),
+            exes.map(|e| format!("exec '{e}' web '{arg}'")),
+        ),
+        false => (
+            h.home.join(".local/share/applications/claudio.desktop"),
+            exes.map(|e| format!("Exec=\"{e}\" web {arg}")),
+        ),
+    };
+    let text = match cfg!(target_os = "macos") {
+        true => fs::read_to_string(launcher.join("Contents/MacOS/claudio")).unwrap(),
+        false => fs::read_to_string(&launcher).unwrap(),
+    };
+    assert!(text.lines().any(|l| line.contains(&l.to_owned())), "{text}");
+    let printed = String::from_utf8_lossy(&out.stdout);
+    assert!(
+        printed
+            .lines()
+            .all(|l| l.starts_with("claudio web: wrote ")),
+        "{printed}"
+    );
+    let out = claudio(&h, &["web", "--remove-launcher"], no_data_home);
+    assert_eq!(out.status.code(), Some(0), "{out:?}");
+    assert!(fs::symlink_metadata(&launcher).is_err(), "removed");
+    let again = claudio(&h, &["web", "--remove-launcher"], no_data_home);
+    assert_eq!(again.status.code(), Some(0), "{again:?}");
+    assert!(String::from_utf8_lossy(&again.stdout).contains("no launcher at "));
+}
````
Run: `cargo test --locked --test web -- --test-threads=1 web_install_launcher`
Expected: `test result: FAILED. 0 passed; 1 failed`, with

```
thread 'web_install_launcher_writes_under_home_and_remove_takes_it_away' panicked at addons/web/tests/web.rs:2572:5:
assertion `left == right` failed: Output { status: ExitStatus(unix_wait_status(512)), stdout: "", stderr: "claudio web: unknown argument \"--install-launcher\"\n…
  left: Some(2)
 right: Some(0)
```

Step 2: implement (patch 13)
````diff
diff --git a/addons/web/src/assets.rs b/addons/web/src/assets.rs
index 7ffa3dd..145fbfe 100644
--- a/addons/web/src/assets.rs
+++ b/addons/web/src/assets.rs
@@ -159,6 +159,11 @@ fn find(path: &str) -> Option<&'static Asset> {
     ASSETS.iter().find(|a| a.path == path)
 }
 
+/// An embedded file's bytes, decompressed: the launcher's icons. Blocking.
+pub(crate) fn embedded(path: &str) -> Option<Vec<u8>> {
+    find(path).and_then(|a| gunzip(a.gz).ok())
+}
+
 /// The asset fallback route: `GET`/`HEAD` of an app file (`/` is
 /// `/index.html`).
 pub(crate) async fn serve(
diff --git a/addons/web/src/launcher/linux.rs b/addons/web/src/launcher/linux.rs
new file mode 100644
index 0000000..b8a59c3
--- /dev/null
+++ b/addons/web/src/launcher/linux.rs
@@ -0,0 +1,206 @@
+//! The Linux launcher: `<data>/applications/claudio.desktop` and its icons
+//! under `<data>/icons/hicolor` (`<data>`: `$XDG_DATA_HOME`, else
+//! `~/.local/share`).
+
+use std::fs;
+use std::path::Path;
+
+use super::{app_arg, found, write, Found, Icons};
+
+/// The line that marks a `.desktop` file as claudio's.
+const MARKER: &str = "X-Claudio-Launcher=1";
+const ENTRY: &str = "applications/claudio.desktop";
+const ICONS: [&str; 3] = [
+    "icons/hicolor/scalable/apps/claudio.svg",
+    "icons/hicolor/192x192/apps/claudio.png",
+    "icons/hicolor/512x512/apps/claudio.png",
+];
+/// The characters the Desktop Entry spec reserves in an `Exec` argument.
+const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";
+
+/// `s` as a double-quoted `Exec` argument, both layers of escaping: `\`,
+/// `"`, `` ` `` and `$` take a backslash, which the value's string escaping
+/// then doubles (`\` is `\\\\`, `"` is `\\"`); `%` is `%%`.
+fn quoted(s: &str) -> String {
+    let mut out = String::from("\"");
+    for c in s.chars() {
+        match c {
+            '\\' => out.push_str(r"\\\\"),
+            '"' => out.push_str(r#"\\""#),
+            '`' => out.push_str(r"\\`"),
+            '$' => out.push_str(r"\\$"),
+            '%' => out.push_str("%%"),
+            c => out.push(c),
+        }
+    }
+    out.push('"');
+    out
+}
+
+/// An `Exec` argument: [`quoted`] when it holds a reserved character, else
+/// as it is with `%` as `%%`.
+fn arg(s: &str) -> String {
+    match s.contains(|c| RESERVED.contains(c)) {
+        true => quoted(s),
+        false => s.replace('%', "%%"),
+    }
+}
+
+/// The `.desktop` file running `"<exe>" web --app` (or `--app=<url>`).
+fn entry(exe: &str, url: Option<&str>) -> String {
+    format!(
+        "[Desktop Entry]\nType=Application\nName=claudio\nExec={} web {}\nIcon=claudio\n\
+         Categories=Development;\nTerminal=false\n{MARKER}\n",
+        quoted(exe),
+        arg(&app_arg(url))
+    )
+}
+
+/// The icons, then the entry; a `.desktop` that is not claudio's (or a
+/// symlink) is left alone.
+pub(super) fn install(
+    data: &Path,
+    exe: &str,
+    url: Option<&str>,
+    icons: &Icons,
+) -> Result<Vec<String>, String> {
+    let path = data.join(ENTRY);
+    if let Found::Foreign(why) = found(&path, &path, MARKER)? {
+        return Err(format!("{why}: not replaced"));
+    }
+    let mut wrote = Vec::new();
+    for (rel, bytes) in ICONS.iter().zip([&icons.svg, &icons.png192, &icons.png512]) {
+        wrote.push(write(&data.join(rel), bytes)?);
+    }
+    wrote.push(write(&path, entry(exe, url).as_bytes())?);
+    Ok(wrote)
+}
+
+/// claudio's entry and then its icons (regular files only); nothing there
+/// is fine; anything else is refused.
+pub(super) fn remove(data: &Path) -> Result<Vec<String>, String> {
+    let path = data.join(ENTRY);
+    match found(&path, &path, MARKER)? {
+        Found::Nothing => return Ok(vec![format!("no launcher at {}", path.display())]),
+        Found::Foreign(why) => return Err(format!("{why}: not removed")),
+        Found::Ours => {}
+    }
+    let mut removed = Vec::new();
+    for p in std::iter::once(path).chain(ICONS.iter().map(|rel| data.join(rel))) {
+        if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_file()) {
+            fs::remove_file(&p).map_err(|e| format!("cannot remove {}: {e}", p.display()))?;
+            removed.push(format!("removed {}", p.display()));
+        }
+    }
+    Ok(removed)
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use std::os::unix::fs::PermissionsExt;
+    use std::path::PathBuf;
+
+    fn icons() -> Icons {
+        Icons::embedded().unwrap()
+    }
+
+    fn scratch() -> PathBuf {
+        std::env::temp_dir().join(format!("claudio-launcher-{}", uuid::Uuid::new_v4()))
+    }
+
+    #[test]
+    fn exec_quotes_the_exe_with_both_layers_of_escaping() {
+        for (exe, exec) in [
+            ("/opt/claudio", r#"Exec="/opt/claudio" web --app"#),
+            (
+                "/opt/my apps/claudio",
+                r#"Exec="/opt/my apps/claudio" web --app"#,
+            ),
+            (
+                r#"/opt/a"b/claudio"#,
+                r#"Exec="/opt/a\\"b/claudio" web --app"#,
+            ),
+            ("/opt/a`b/claudio", r#"Exec="/opt/a\\`b/claudio" web --app"#),
+            (
+                "/opt/$HOME/claudio",
+                r#"Exec="/opt/\\$HOME/claudio" web --app"#,
+            ),
+            (
+                r"/opt/a\b/claudio",
+                r#"Exec="/opt/a\\\\b/claudio" web --app"#,
+            ),
+            (
+                "/opt/100%/claudio",
+                r#"Exec="/opt/100%%/claudio" web --app"#,
+            ),
+        ] {
+            let text = entry(exe, None);
+            assert!(text.contains(&format!("\n{exec}\n")), "{exe}: {text}");
+        }
+        assert_eq!(
+            entry("/opt/claudio", Some("https://example.invalid:7337/")),
+            "[Desktop Entry]\nType=Application\nName=claudio\n\
+             Exec=\"/opt/claudio\" web --app=https://example.invalid:7337/\nIcon=claudio\n\
+             Categories=Development;\nTerminal=false\nX-Claudio-Launcher=1\n"
+        );
+        let odd = entry("/opt/claudio", Some("https://h/a;b%41"));
+        assert!(
+            odd.contains(r#"Exec="/opt/claudio" web "--app=https://h/a;b%%41""#),
+            "{odd}"
+        );
+        assert_eq!(arg("--app=https://h/%41"), "--app=https://h/%%41");
+    }
+
+    #[test]
+    fn install_writes_the_entry_and_icons_and_remove_takes_them_away() {
+        let data = scratch();
+        let icons = icons();
+        let wrote = install(&data, "/opt/claudio", None, &icons).unwrap();
+        assert_eq!(wrote.len(), 4, "{wrote:?}");
+        let path = data.join(ENTRY);
+        let text = fs::read_to_string(&path).unwrap();
+        assert!(text.starts_with("[Desktop Entry]\n") && text.contains("\nX-Claudio-Launcher=1\n"));
+        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
+        assert_eq!(mode(&path), 0o600);
+        assert_eq!(mode(&data.join("applications")), 0o700);
+        assert_eq!(fs::read(data.join(ICONS[0])).unwrap(), icons.svg);
+        assert_eq!(fs::read(data.join(ICONS[1])).unwrap(), icons.png192);
+        assert_eq!(fs::read(data.join(ICONS[2])).unwrap(), icons.png512);
+        let url = Some("https://z6:7337/");
+        install(&data, "/opt/claudio", url, &icons).unwrap();
+        assert!(fs::read_to_string(&path)
+            .unwrap()
+            .contains(" web --app=https://z6:7337/\n"));
+        assert_eq!(remove(&data).unwrap().len(), 4);
+        assert!(!path.exists() && !data.join(ICONS[2]).exists());
+        let again = remove(&data).unwrap();
+        assert!(again[0].starts_with("no launcher at "), "{again:?}");
+        fs::remove_dir_all(&data).unwrap();
+    }
+
+    #[test]
+    fn a_foreign_entry_or_a_symlink_is_never_replaced_or_removed() {
+        let data = scratch();
+        let path = data.join(ENTRY);
+        fs::create_dir_all(path.parent().unwrap()).unwrap();
+        fs::write(&path, "[Desktop Entry]\nName=mine\n").unwrap();
+        assert!(remove(&data)
+            .unwrap_err()
+            .contains("is not claudio's launcher: not removed"));
+        let refused = install(&data, "/opt/claudio", None, &icons()).unwrap_err();
+        assert!(refused.contains("not replaced"), "{refused}");
+        assert_eq!(
+            fs::read_to_string(&path).unwrap(),
+            "[Desktop Entry]\nName=mine\n"
+        );
+        fs::remove_file(&path).unwrap();
+        let target = data.join("elsewhere.desktop");
+        fs::write(&target, format!("{MARKER}\n")).unwrap();
+        std::os::unix::fs::symlink(&target, &path).unwrap();
+        assert!(remove(&data).unwrap_err().contains("is a symlink"));
+        assert!(install(&data, "/opt/claudio", None, &icons()).is_err());
+        assert!(target.exists() && fs::symlink_metadata(&path).is_ok());
+        fs::remove_dir_all(&data).unwrap();
+    }
+}
diff --git a/addons/web/src/launcher/macos.rs b/addons/web/src/launcher/macos.rs
new file mode 100644
index 0000000..f9d55e2
--- /dev/null
+++ b/addons/web/src/launcher/macos.rs
@@ -0,0 +1,214 @@
+//! The macOS launcher: `~/Applications/claudio.app`, a bundle whose
+//! executable is a shell script running `claudio web --app` (or
+//! `--app=<url>`). It only launches: the window is the browser's, under its
+//! Dock icon.
+
+use std::fs;
+use std::os::unix::fs::PermissionsExt;
+use std::path::Path;
+
+use super::{app_arg, found, write, Found};
+
+/// The `Info.plist` line that marks a bundle as claudio's.
+const MARKER: &str = "<key>ClaudioLauncher</key>";
+const BUNDLE: &str = "Applications/claudio.app";
+
+/// The bundle's `Info.plist`.
+fn info_plist() -> String {
+    format!(
+        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
+         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
+         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
+         <plist version=\"1.0\">\n<dict>\n\
+         \t<key>CFBundleName</key>\n\t<string>claudio</string>\n\
+         \t<key>CFBundleIdentifier</key>\n\t<string>io.vocdoni.claudio</string>\n\
+         \t<key>CFBundleExecutable</key>\n\t<string>claudio</string>\n\
+         \t<key>CFBundleIconFile</key>\n\t<string>claudio</string>\n\
+         \t<key>CFBundlePackageType</key>\n\t<string>APPL</string>\n\
+         \t{MARKER}\n\t<true/>\n\
+         </dict>\n</plist>\n"
+    )
+}
+
+/// `s` in single quotes for `sh`, each `'` written `'\''`.
+fn sh_quoted(s: &str) -> String {
+    format!("'{}'", s.replace('\'', r"'\''"))
+}
+
+/// The bundle's executable: `exec '<exe>' web --app` (or `'--app=<url>'`).
+fn script(exe: &str, url: Option<&str>) -> String {
+    let app = match url {
+        None => app_arg(None),
+        Some(_) => sh_quoted(&app_arg(url)),
+    };
+    format!("#!/bin/sh\nexec {} web {app}\n", sh_quoted(exe))
+}
+
+/// An `.icns` of one `ic09` entry (a 512 px PNG): `icns`, the file's
+/// length, `ic09`, the entry's length (with its 8-byte header), the PNG;
+/// lengths 32-bit big-endian.
+fn icns(png: &[u8]) -> Vec<u8> {
+    let len = |n: usize| u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes();
+    let mut out = Vec::with_capacity(png.len() + 16);
+    out.extend_from_slice(b"icns");
+    out.extend_from_slice(&len(png.len() + 16));
+    out.extend_from_slice(b"ic09");
+    out.extend_from_slice(&len(png.len() + 8));
+    out.extend_from_slice(png);
+    out
+}
+
+/// The bundle under `home`: claudio's own is replaced whole; any other
+/// (or a symlink) is left alone. The script is made 0755.
+pub(super) fn install(
+    home: &Path,
+    exe: &str,
+    url: Option<&str>,
+    png512: &[u8],
+) -> Result<Vec<String>, String> {
+    let bundle = home.join(BUNDLE);
+    let plist = bundle.join("Contents/Info.plist");
+    match found(&bundle, &plist, MARKER)? {
+        Found::Foreign(why) => return Err(format!("{why}: not replaced")),
+        Found::Ours => fs::remove_dir_all(&bundle)
+            .map_err(|e| format!("cannot replace {}: {e}", bundle.display()))?,
+        Found::Nothing => {}
+    }
+    let run = bundle.join("Contents/MacOS/claudio");
+    let wrote = vec![
+        write(&plist, info_plist().as_bytes())?,
+        write(&run, script(exe, url).as_bytes())?,
+        write(
+            &bundle.join("Contents/Resources/claudio.icns"),
+            &icns(png512),
+        )?,
+    ];
+    fs::set_permissions(&run, fs::Permissions::from_mode(0o755))
+        .map_err(|e| format!("cannot make {} executable: {e}", run.display()))?;
+    Ok(wrote)
+}
+
+/// claudio's bundle, whole; nothing there is fine; anything else is
+/// refused.
+pub(super) fn remove(home: &Path) -> Result<Vec<String>, String> {
+    let bundle = home.join(BUNDLE);
+    match found(&bundle, &bundle.join("Contents/Info.plist"), MARKER)? {
+        Found::Nothing => Ok(vec![format!("no launcher at {}", bundle.display())]),
+        Found::Foreign(why) => Err(format!("{why}: not removed")),
+        Found::Ours => {
+            fs::remove_dir_all(&bundle)
+                .map_err(|e| format!("cannot remove {}: {e}", bundle.display()))?;
+            Ok(vec![format!("removed {}", bundle.display())])
+        }
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use std::path::PathBuf;
+
+    fn png() -> Vec<u8> {
+        crate::assets::embedded("/icons/claudio-512.png").unwrap()
+    }
+
+    fn scratch() -> PathBuf {
+        std::env::temp_dir().join(format!("claudio-launcher-{}", uuid::Uuid::new_v4()))
+    }
+
+    #[test]
+    fn the_script_quotes_the_exe_for_sh() {
+        assert_eq!(
+            script("/opt/claudio", None),
+            "#!/bin/sh\nexec '/opt/claudio' web --app\n"
+        );
+        assert_eq!(
+            script("/Users/o'brien/my apps/claudio", None),
+            "#!/bin/sh\nexec '/Users/o'\\''brien/my apps/claudio' web --app\n"
+        );
+        assert_eq!(
+            script("/opt/claudio", Some("https://z6:7337/")),
+            "#!/bin/sh\nexec '/opt/claudio' web '--app=https://z6:7337/'\n"
+        );
+        assert_eq!(
+            script("/opt/$x`y`\"z", None),
+            "#!/bin/sh\nexec '/opt/$x`y`\"z' web --app\n"
+        );
+    }
+
+    #[test]
+    fn install_writes_the_bundle_and_remove_takes_it_away() {
+        let home = scratch();
+        let png = png();
+        assert_eq!(install(&home, "/opt/claudio", None, &png).unwrap().len(), 3);
+        let bundle = home.join(BUNDLE);
+        let plist = fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
+        for (key, value) in [
+            ("CFBundleName", "<string>claudio</string>"),
+            ("CFBundleIdentifier", "<string>io.vocdoni.claudio</string>"),
+            ("CFBundleExecutable", "<string>claudio</string>"),
+            ("CFBundleIconFile", "<string>claudio</string>"),
+            ("CFBundlePackageType", "<string>APPL</string>"),
+            ("ClaudioLauncher", "<true/>"),
+        ] {
+            let pair = format!("\t<key>{key}</key>\n\t{value}\n");
+            assert!(plist.contains(&pair), "{key}: {plist}");
+        }
+        let run = bundle.join("Contents/MacOS/claudio");
+        assert_eq!(
+            fs::metadata(&run).unwrap().permissions().mode() & 0o777,
+            0o755
+        );
+        assert_eq!(
+            fs::read_to_string(&run).unwrap(),
+            script("/opt/claudio", None)
+        );
+        let icns = fs::read(bundle.join("Contents/Resources/claudio.icns")).unwrap();
+        assert_eq!(&icns[..4], b"icns");
+        assert_eq!(
+            u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize,
+            icns.len()
+        );
+        assert_eq!(&icns[8..12], b"ic09");
+        assert_eq!(
+            u32::from_be_bytes(icns[12..16].try_into().unwrap()) as usize,
+            png.len() + 8
+        );
+        assert_eq!(&icns[16..], png.as_slice(), "one entry: the 512 px PNG");
+        install(&home, "/opt/claudio", Some("https://z6:7337/"), &png).unwrap();
+        assert!(fs::read_to_string(&run)
+            .unwrap()
+            .ends_with("web '--app=https://z6:7337/'\n"));
+        assert_eq!(
+            remove(&home).unwrap(),
+            [format!("removed {}", bundle.display())]
+        );
+        assert!(!bundle.exists());
+        assert!(remove(&home).unwrap()[0].starts_with("no launcher at "));
+        fs::remove_dir_all(&home).unwrap();
+    }
+
+    #[test]
+    fn a_foreign_bundle_or_a_symlink_is_never_replaced_or_removed() {
+        let home = scratch();
+        let bundle = home.join(BUNDLE);
+        fs::create_dir_all(bundle.join("Contents")).unwrap();
+        fs::write(bundle.join("Contents/Info.plist"), "<plist></plist>\n").unwrap();
+        assert!(remove(&home)
+            .unwrap_err()
+            .contains("is not claudio's launcher: not removed"));
+        assert!(install(&home, "/opt/claudio", None, &png())
+            .unwrap_err()
+            .contains("not replaced"));
+        assert!(bundle.join("Contents/Info.plist").exists());
+        fs::remove_dir_all(&bundle).unwrap();
+        let real = home.join("real.app");
+        install(&home, "/opt/claudio", None, &png()).unwrap();
+        fs::rename(&bundle, &real).unwrap();
+        std::os::unix::fs::symlink(&real, &bundle).unwrap();
+        assert!(remove(&home).unwrap_err().contains("is a symlink"));
+        assert!(install(&home, "/opt/claudio", None, &png()).is_err());
+        assert!(real.join("Contents/Info.plist").exists(), "never followed");
+        fs::remove_dir_all(&home).unwrap();
+    }
+}
diff --git a/addons/web/src/launcher/mod.rs b/addons/web/src/launcher/mod.rs
new file mode 100644
index 0000000..9d752a4
--- /dev/null
+++ b/addons/web/src/launcher/mod.rs
@@ -0,0 +1,192 @@
+//! `claudio web --install-launcher` and `--remove-launcher` (docs/web.md,
+//! "App window and launcher"): a desktop menu entry (Linux, `linux.rs`) or
+//! `~/Applications/claudio.app` (macOS, `macos.rs`) that runs `claudio web
+//! --app`, or `--app=<url>`. One launcher at a time: installing again
+//! replaces claudio's own. Files go through `paths::write_atomic` (0600,
+//! missing directories 0700), under the user's home or `XDG_DATA_HOME`; a
+//! symlink at a launcher's path is never followed, and a launcher claudio
+//! did not write is neither replaced nor removed.
+
+mod linux;
+mod macos;
+
+use std::fs;
+use std::io::{self, Read};
+use std::os::unix::fs::OpenOptionsExt;
+use std::path::{Path, PathBuf};
+use std::process::ExitCode;
+
+use claudio_core::paths;
+
+/// `--install-launcher` or `--remove-launcher`.
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub(crate) enum LauncherOp {
+    Install,
+    Remove,
+}
+
+/// The icons a launcher installs: the page's own, from the binary.
+struct Icons {
+    svg: Vec<u8>,
+    png192: Vec<u8>,
+    png512: Vec<u8>,
+}
+
+impl Icons {
+    fn embedded() -> Result<Icons, String> {
+        let get =
+            |p: &str| crate::assets::embedded(p).ok_or_else(|| format!("this build has no {p}"));
+        Ok(Icons {
+            svg: get("/icons/claudio.svg")?,
+            png192: get("/icons/claudio-192.png")?,
+            png512: get("/icons/claudio-512.png")?,
+        })
+    }
+}
+
+/// `claudio web --install-launcher [--app=<url>]` or `--remove-launcher`;
+/// each line it wrote or removed is printed.
+pub(crate) fn run(op: LauncherOp, url: Option<&str>) -> ExitCode {
+    let home = paths::home();
+    let mac = cfg!(target_os = "macos");
+    let done = match op {
+        LauncherOp::Install => install(mac, &home, url),
+        LauncherOp::Remove if mac => macos::remove(&home),
+        LauncherOp::Remove => linux::remove(&data_home(&home)),
+    };
+    match done {
+        Ok(lines) => {
+            lines.iter().for_each(|l| println!("claudio web: {l}"));
+            ExitCode::SUCCESS
+        }
+        Err(why) => {
+            eprintln!("claudio web: {why}");
+            ExitCode::FAILURE
+        }
+    }
+}
+
+fn install(mac: bool, home: &Path, url: Option<&str>) -> Result<Vec<String>, String> {
+    let exe = std::env::current_exe().map_err(|e| format!("cannot find this claudio: {e}"))?;
+    let exe = exe_ok(&exe)?;
+    let icons = Icons::embedded()?;
+    match mac {
+        true => macos::install(home, exe, url, &icons.png512),
+        false => linux::install(&data_home(home), exe, url, &icons),
+    }
+}
+
+/// `$XDG_DATA_HOME` when set and absolute, else `~/.local/share`.
+fn data_home(home: &Path) -> PathBuf {
+    std::env::var_os("XDG_DATA_HOME")
+        .map(PathBuf::from)
+        .filter(|p| p.is_absolute())
+        .unwrap_or_else(|| home.join(".local/share"))
+}
+
+/// The exe a launcher runs, as `current_exe` gave it (not canonicalized):
+/// absolute, UTF-8, no control character, not a replaced binary (`
+/// (deleted)`), and there. Its text.
+fn exe_ok(exe: &Path) -> Result<&str, String> {
+    let Some(text) = exe.to_str() else {
+        return Err(format!(
+            "{}: not UTF-8, which a launcher cannot name",
+            exe.display()
+        ));
+    };
+    if text.chars().any(char::is_control) {
+        return Err(format!("{text:?}: a path with a control character"));
+    }
+    if !exe.is_absolute() {
+        return Err(format!("{text}: not an absolute path"));
+    }
+    if text.ends_with(" (deleted)") {
+        return Err(format!("{text}: this binary was replaced; run the new one"));
+    }
+    if !exe.is_file() {
+        return Err(format!("{text}: no such file"));
+    }
+    Ok(text)
+}
+
+/// The argument after `web`: `--app`, or `--app=<url>`.
+fn app_arg(url: Option<&str>) -> String {
+    match url {
+        Some(url) => format!("--app={url}"),
+        None => "--app".to_owned(),
+    }
+}
+
+/// What is at a launcher's path.
+enum Found {
+    Nothing,
+    /// claudio's: its `file` has the marker line.
+    Ours,
+    /// Not to be touched, and why.
+    Foreign(String),
+}
+
+/// What is at `path`, read from `file` (the path itself, or a bundle's
+/// `Info.plist`): a symlink is foreign, never followed.
+fn found(path: &Path, file: &Path, marker: &str) -> Result<Found, String> {
+    match fs::symlink_metadata(path) {
+        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Nothing),
+        Err(e) => return Err(format!("{}: {e}", path.display())),
+        Ok(m) if m.file_type().is_symlink() => {
+            return Ok(Found::Foreign(format!("{} is a symlink", path.display())))
+        }
+        Ok(_) => {}
+    }
+    match read_small(file) {
+        Some(text) if text.lines().any(|l| l.trim() == marker) => Ok(Found::Ours),
+        _ => Ok(Found::Foreign(format!(
+            "{} is not claudio's launcher",
+            path.display()
+        ))),
+    }
+}
+
+/// `path`'s text, never through a symlink, at most 64 KiB.
+fn read_small(path: &Path) -> Option<String> {
+    let file = fs::OpenOptions::new()
+        .read(true)
+        .custom_flags(libc::O_NOFOLLOW)
+        .open(path)
+        .ok()?;
+    let mut text = String::new();
+    file.take(64 * 1024).read_to_string(&mut text).ok()?;
+    Some(text)
+}
+
+/// `bytes` to `path` (`write_atomic`: 0600, missing directories 0700); the
+/// line that says so.
+fn write(path: &Path, bytes: &[u8]) -> Result<String, String> {
+    paths::write_atomic(path, bytes)
+        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
+    Ok(format!("wrote {}", path.display()))
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn the_exe_must_be_absolute_present_and_plain() {
+        let exe = std::env::current_exe().unwrap();
+        assert_eq!(exe_ok(&exe), Ok(exe.to_str().unwrap()));
+        for bad in [
+            "relative/claudio",
+            "/no/such/claudio",
+            "/usr/bin/claudio (deleted)",
+            "/opt/clau\ndio",
+            "/opt/clau\tdio",
+        ] {
+            assert!(exe_ok(Path::new(bad)).is_err(), "{bad:?}");
+        }
+        use std::os::unix::ffi::OsStrExt;
+        let latin1 = Path::new(std::ffi::OsStr::from_bytes(b"/opt/\xe9/claudio"));
+        assert!(exe_ok(latin1).unwrap_err().contains("not UTF-8"));
+        assert_eq!(app_arg(None), "--app");
+        assert_eq!(app_arg(Some("https://z6:7337/")), "--app=https://z6:7337/");
+    }
+}
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index 4bba824..c515a4f 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -18,6 +18,7 @@ mod hosts;
 mod hub;
 mod hub_io;
 mod input;
+mod launcher;
 mod lenses;
 mod limit;
 mod login;
@@ -63,6 +64,7 @@ const USAGE: &str =
     "usage: claudio web [--foreground] [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
 [--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token] [--app]
        claudio web --app=URL
+       claudio web --install-launcher [--app=URL] | --remove-launcher
        claudio web stop | status
        claudio web --set-password
        claudio web --print-cert";
@@ -93,6 +95,8 @@ pub(crate) struct WebArgs {
     /// `--app` or `--app=<url>`: an app window. This invocation's only: never
     /// in `[web]` or the service's argv.
     pub app: Option<app_window::AppTarget>,
+    /// `--install-launcher` or `--remove-launcher`: one thing, then exit.
+    pub launcher: Option<launcher::LauncherOp>,
 }
 
 /// Shared by every handler.
@@ -134,6 +138,7 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
         foreground: false,
         service: false,
         app: None,
+        launcher: None,
     };
     let mut it = args.iter().peekable();
     while let Some(arg) = it.next() {
@@ -170,6 +175,8 @@ pub(crate) fn parse_args(args: &[String]) -> Result<WebArgs, String> {
             "--foreground" => out.foreground = true,
             "--service" => out.service = true,
             "--app" => out.app = Some(app_window::target(inline)?),
+            "--install-launcher" => out.launcher = Some(launcher::LauncherOp::Install),
+            "--remove-launcher" => out.launcher = Some(launcher::LauncherOp::Remove),
             other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
         }
     }
@@ -187,7 +194,16 @@ fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
     if (a.set_password || a.print_cert) && n_args != 1 {
         return Err("--set-password and --print-cert take no other argument".to_owned());
     }
-    if matches!(a.app, Some(app_window::AppTarget::Url(_))) && n_args != 1 {
+    let url = matches!(a.app, Some(app_window::AppTarget::Url(_)));
+    let install_url = url && a.launcher == Some(launcher::LauncherOp::Install) && n_args == 2;
+    if a.launcher.is_some() && n_args != 1 && !install_url {
+        return Err(
+            "--install-launcher and --remove-launcher take no other argument \
+                    (but --install-launcher --app=URL)"
+                .to_owned(),
+        );
+    }
+    if url && n_args != 1 && a.launcher.is_none() {
         return Err("--app=URL takes no other argument".to_owned());
     }
     if a.app.is_some() && (a.no_open || a.print_url) {
@@ -462,7 +478,8 @@ fn load_sessions(
     sessions::Sessions::new(store, epoch, path)
 }
 
-/// `--set-password`, `--print-cert` or `--app=<url>`: one thing, then exit.
+/// `--set-password`, `--print-cert`, a launcher flag or `--app=<url>`: one
+/// thing, then exit.
 fn one_shot(args: &WebArgs) -> Option<ExitCode> {
     if args.set_password {
         return Some(match password::set_password() {
@@ -477,10 +494,15 @@ fn one_shot(args: &WebArgs) -> Option<ExitCode> {
             }
         });
     }
-    if let Some(app_window::AppTarget::Url(url)) = &args.app {
-        return Some(app_window::open_url(url));
+    let url = match &args.app {
+        Some(app_window::AppTarget::Url(url)) => Some(url.as_str()),
+        _ => None,
+    };
+    if let Some(op) = args.launcher {
+        return Some(launcher::run(op, url));
     }
-    args.print_cert.then(print_cert)
+    url.map(app_window::open_url)
+        .or_else(|| args.print_cert.then(print_cert))
 }
 
 /// `--print-cert`: the kept certificate (never the key) on stdout.
@@ -884,4 +906,32 @@ mod tests {
             assert_eq!(parse_args(&args(&[bad])).unwrap_err(), app_window::URL_BAD);
         }
     }
+
+    #[test]
+    fn launcher_flags_go_alone_but_install_with_an_app_url() {
+        use launcher::LauncherOp;
+        let a = parse_args(&args(&["--install-launcher"])).unwrap();
+        assert_eq!((a.launcher, a.app), (Some(LauncherOp::Install), None));
+        let a = parse_args(&args(&["--remove-launcher"])).unwrap();
+        assert_eq!(a.launcher, Some(LauncherOp::Remove));
+        let a = parse_args(&args(&["--app=https://z6:7337/", "--install-launcher"])).unwrap();
+        assert_eq!(a.launcher, Some(LauncherOp::Install));
+        assert_eq!(
+            a.app,
+            Some(app_window::AppTarget::Url("https://z6:7337/".into()))
+        );
+        for bad in [
+            &["--install-launcher", "--app"][..],
+            &["--remove-launcher", "--app=https://z6/"],
+            &["--install-launcher", "--remove-launcher"],
+            &["--install-launcher", "--foreground"],
+            &["--install-launcher", "--app=https://z6/", "--no-open"],
+            &["--remove-launcher", "--listen", "127.0.0.1:1"],
+        ] {
+            let err = parse_args(&args(bad)).unwrap_err();
+            assert!(err.contains("take no other argument"), "{bad:?}: {err}");
+        }
+        let err = parse_args(&args(&["--install-launcher", "--app=ftp://z6/"])).unwrap_err();
+        assert_eq!(err, app_window::URL_BAD);
+    }
 }
````
Run: `cargo test --locked -p claudio-web --lib`
Expected: `test result: ok. 147 passed; 0 failed` (was 139: `launcher::tests`, `launcher::linux::tests`
(3), `launcher::macos::tests` (3), `tests::launcher_flags_go_alone_but_install_with_an_app_url`)
Run: `cargo test --locked --test web -- --test-threads=1 web_install_launcher`
Expected: `test result: ok. 1 passed; 0 failed`
Run: `cargo fmt --all --check && cargo clippy --locked --workspace --all-targets --all-features -- -D warnings && scripts/check_rules.py`
Expected: `rules: 0 problem(s); over budget: 12293 file lines, 4287 fn lines, 20 unwraps`

Step 3: commit
`git add addons/web/src/launcher/mod.rs addons/web/src/launcher/linux.rs addons/web/src/launcher/macos.rs`
`git commit -m "feat(web): claudio web --install-launcher and --remove-launcher" -- addons/web/src/launcher/mod.rs addons/web/src/launcher/linux.rs addons/web/src/launcher/macos.rs addons/web/src/assets.rs addons/web/src/lib.rs addons/web/tests/web.rs`

Interfaces this task provides to later tasks: `launcher::{LauncherOp, run}`,
`assets::embedded(path) -> Option<Vec<u8>>`, `WebArgs.launcher`.

### Task 7: Docs: the app window, the launcher, tickets
Model: haiku   Repo: /home/p4u/claudio   Files: `docs/web.md`, `README.md`, `CLAUDE.md`
Goal: spec §5, F1, AC10 and AC12: docs/web.md's "App window and launcher" (`--app`,
`--app=<url>`, the launcher, the browser search and `CLAUDIO_BROWSER`, the fallback, the
sandboxed browser's residual risk in the spec's words, the certificate steps for z6, window
grouping), the "Installed window" paragraph kept and confirmed by AC9(a), the security model's
sign-in tickets; README's quick start; CLAUDE.md's "Web UI" line and the web constraint (no
token or ticket in argv).

Step 1: apply the patch (patch 14)
````diff
diff --git a/CLAUDE.md b/CLAUDE.md
index acf0d7b..5b5c63f 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -264,6 +264,9 @@ local daemon run `claudio web --service` (supervised: restarted with backoff, st
 daemon), then prints the URL and exits (`cli.rs`, with `stop`, `status`, `restart_if_enabled`;
 `service.rs` maps `[web]` to argv); `--foreground` serves from the terminal and never touches
 `[web]`. The server is one more client of the daemons, like the TUI, over the same socket and ssh bridge.
+A browser it opens signs in with a single-use ticket through a 0600 redirect file (`ticket.rs`); `--app`
+opens a Chromium-family app window instead of a tab, `--app=<url>` another machine's web (`app_window.rs`),
+and `--install-launcher` writes a desktop launcher for it (`launcher/`).
 `docs/web-protocol.md` is the contract between server and page (version 2: `addons.*` ops,
 the Session's `launch` and `segments`); change it first, in its own commit. User docs, the
 security model and the TUI parity table: `docs/web.md`.
@@ -271,7 +274,7 @@ security model and the TUI parity table: `docs/web.md`.
 - **Server** (axum, loopback only unless `--remote`): `auth.rs` (token, the cookie with
   `Max-Age=34560000`, `Host`/`Origin` checks, the CSP), `sessions.rs` (sign-ins as hashes in
   `web-sessions.json`, bound to an epoch; 30 days idle, 64 at most), `hub_io.rs` (the plain
-  connect for `--service`), `login.rs` + `limit.rs` + `password.rs` (token or argon2id
+  connect for `--service`), `login.rs` + `limit.rs` + `password.rs` (token, ticket or argon2id
   password login, rate-limited), `tls/` (`--remote`'s kept self-signed certificate), `ws.rs`
   (the WebSocket), `hub.rs` (one actor owning hosts, daemon connections, sessions, tabs and
   `web-state.json`), `dto.rs` (DTOs built field by field, never a serialized `Profile`,
@@ -401,6 +404,11 @@ security model and the TUI parity table: `docs/web.md`.
   daemon's environment).
 - **The suites' remote peer is a debug-only hook** (`CLAUDIO_TEST_REMOTE_PEER`); it only ever
   takes the token away.
+- **No token or ticket in a browser's argv.** A browser claudio opens gets
+  `file://<runtime>/web-open/<hex>.html` (0600), which carries a single-use ticket; only a snap or
+  flatpak browser gets the `#k=` link in argv, with a warning. With `--app` and stdout not a
+  terminal, nothing that holds the token is printed. Tests put fake browsers, openers and
+  notifiers on `PATH` and in `CLAUDIO_BROWSER` (`web_harness/app.rs`), never a real one.
 - **DTO fields are fixed by `docs/web-protocol.md`.** A new field or message changes the doc
   first. Session ids are `sid` in every request and event (an `id` argument would collide with
   the request id); only the Session DTO keeps `id`.
diff --git a/README.md b/README.md
index 64469d5..bd09f47 100644
--- a/README.md
+++ b/README.md
@@ -12,6 +12,8 @@ Run many [Claude Code](https://claude.ai/claude-code) sessions, on this machine
 
 ```bash
 claudio web          # serves http://127.0.0.1:7337 through the daemon and opens it in your browser
+claudio web --app    # the same, in an app window of Chrome, Chromium, Brave or Edge
+claudio web --install-launcher   # a "claudio" launcher in the desktop menu that runs claudio web --app
 claudio web status   # is it running? (claudio web stop turns it off)
 ```
 
@@ -177,7 +179,7 @@ An exported `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` is kept and turns the fea
 | Command | What it does |
 |---|---|
 | `claudio [--proxy NAME\|--no-proxy] [--first-party]` | The terminal UI |
-| `claudio web [--listen ADDR] [--no-open]` | The web UI, served by the daemon (also `stop`, `status`, `--foreground`, `--remote`, `--set-password`, `--print-cert`, `--print-url`, `--rotate-token`) |
+| `claudio web [--listen ADDR] [--no-open]` | The web UI, served by the daemon (also `stop`, `status`, `--foreground`, `--remote`, `--set-password`, `--print-cert`, `--print-url`, `--rotate-token`, `--app`, `--app=URL`, `--install-launcher`, `--remove-launcher`) |
 | `claudio --plain [claude args…]` | One full-screen claude session |
 | `claudio sessions` | List the local sessions as a table |
 | `claudio daemon status\|stop\|restart` | Inspect or control this host's daemon |
diff --git a/docs/web.md b/docs/web.md
index 33f73f9..c3dd02b 100644
--- a/docs/web.md
+++ b/docs/web.md
@@ -33,11 +33,16 @@ claudio web --remote             # TLS on 0.0.0.0:7337 for other machines; needs
 claudio web --remote [::]:8443   # IPv6 and IPv4, port 8443
 claudio web --print-cert         # the certificate, to install as trusted on a laptop or phone
 claudio web --remote --rotate-cert   # a new certificate (browsers warn once more)
+claudio web --app                # as claudio web, in an app window of Chrome, Chromium, Brave or Edge
+claudio web --app=https://z6:7337/   # another machine's claudio web in an app window; starts nothing here
+claudio web --install-launcher   # a "claudio" launcher that runs claudio web --app (--remove-launcher)
 ```
 
 It prints `claudio web: http://127.0.0.1:7337/#t=<token>` and exits; the server keeps running in
 the background (next section). Open that URL once: the page posts the token to `/api/login`,
-gets a cookie and removes the token from the address bar. Later visits need no token while the
+gets a cookie and removes the token from the address bar. A browser that claudio opens itself
+is not given that URL: it signs in with a single-use ticket instead (Security model, "Sign-in
+tickets"). Later visits need no token while the
 sign-in lives: until it goes unused for 30 days (an open tab counts as use), across server and
 daemon restarts and closed browsers. The palette's (`k`) **Sign out** ends this browser's
 sign-in; `--rotate-token` (or a new password) ends every browser's. Sign out closes every tab
@@ -46,7 +51,9 @@ opened with that sign-in, and a page whose sign-in has ended shows the login.
 
 Flags: `--listen ADDR` (loopback only: `127.0.0.0/8`, `::1`, `localhost`; default
 `127.0.0.1:7337`), `--allow-host NAME[:PORT]` (repeatable), `--no-open`, `--print-url`,
-`--rotate-token`, `--foreground`. `CLAUDIO_NO_BROWSER=1` also stops it from opening a browser.
+`--rotate-token`, `--foreground`, `--app` and `--app=URL`, `--install-launcher` and
+`--remove-launcher` ("App window and launcher", below). `CLAUDIO_NO_BROWSER=1` also stops it
+from opening a browser or a window.
 
 ### The web service
 
@@ -189,9 +196,77 @@ Without `--remote` the server never listens on a non-loopback address. Three way
    phone the warning cannot show the fingerprint: install the certificate instead of clicking
    through.
 
+### App window and launcher
+
+`claudio web --app` opens the manager in a chromeless app window: no tab strip, no address bar,
+a window of its own in the task switcher. It does what a bare `claudio web` does (saves `[web]`,
+has the daemon serve it), then opens the window instead of a tab. It needs a display
+(`DISPLAY` or `WAYLAND_DISPLAY` on Linux; always on macOS) but no terminal, so a desktop launcher
+can run it; without a display it says `claudio web --app: no display here; open <URL> in a
+browser on this machine` and exits 1. `CLAUDIO_NO_BROWSER=1` opens nothing. With `--foreground`
+the window opens once the server listens. `--app` goes with `--listen`, `--remote`,
+`--allow-host`, `--foreground`, `--rotate-token` and `--rotate-cert`, not with `--no-open` or
+`--print-url`, and is never stored in `[web]`.
+
+- **The browser.** An app window needs a Chromium-family browser. claudio takes, in order:
+  `[web] browser` in `config.toml` (a path or a name, written by hand), `$CLAUDIO_BROWSER`,
+  then on Linux the first of `google-chrome`, `google-chrome-stable`, `chromium`,
+  `chromium-browser`, `brave-browser`, `microsoft-edge` and `microsoft-edge-stable` found in
+  `PATH`, `/usr/bin`, `/usr/local/bin`, `/snap/bin`, `/var/lib/flatpak/exports/bin` or
+  `~/.local/share/flatpak/exports/bin` (no login shell is run), and on macOS `Google
+  Chrome.app`, `Chromium.app`, `Brave Browser.app` or `Microsoft Edge.app` in `/Applications`
+  or `~/Applications`. It runs `<browser> --app=<url>` (and `--class=claudio` on Linux) with your
+  default profile, so your cookie, notification permission and settings apply. Without such a
+  browser it opens the default one and says `no Chromium-family browser found: opened the
+  default browser (an app window needs Chrome, Chromium, Brave or Edge)`. Firefox and Safari
+  have no app windows (Safari's Add to Dock stays a manual step).
+- **Signing in.** The window signs in with a single-use ticket, never with the token on the
+  browser's command line (Security model, "Sign-in tickets"). A snap or flatpak browser cannot
+  read claudio's private runtime dir, so it gets the ticket link itself on its command line,
+  and claudio says "this browser is sandboxed: the sign-in link is visible to other users of
+  this machine for up to 30 s": another local user who watches the process list in those 30 s
+  can sign in first.
+- **Without a terminal** (a launcher), nothing that holds the token is printed: the line is
+  `claudio web: http://127.0.0.1:7337/`, and the `--remote` banner leaves out its token link. A
+  failure is also appended to `<runtime>/web-app.log` (0600, 64 KiB at most) and shown as a
+  desktop notification (`notify-send` when it is installed, `osascript` on macOS).
+- **With `--remote`** the window opens `https://127.0.0.1:<port>/`. The browser shows its
+  certificate warning first (above); a ticket that expired meanwhile falls through to the
+  password form. A `--remote` on a specific non-loopback address has no local URL: the window
+  opens the public one, and the page asks for the password.
+- **Another machine's web: `--app=<url>`.** `claudio web --app=https://z6:7337/` opens a
+  `claudio web --remote` that runs elsewhere (here on `z6`) in an app window and does nothing
+  else: no daemon, no token, no ticket, no `[web]`. The URL is `http` or `https` with a host, an
+  optional port and path, and no `user:pass@`, query or fragment; anything else exits 2 with
+  `claudio web --app: expected an http(s) URL without credentials or fragment`. Sign in with the
+  password; the cookie then keeps the window signed in (until 30 days unused). The self-signed
+  certificate warns until the laptop trusts it: on the host, `claudio web --print-cert >
+  z6.pem`; on a Linux laptop, `certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n claudio-z6 -i
+  z6.pem` (the NSS store Chrome and Chromium read). Or tunnel it, `ssh -L
+  7337:127.0.0.1:7337 z6`, and open `claudio web --app=https://127.0.0.1:7337/`, a name the
+  certificate already has. Neither is automated.
+- **The launcher.** `claudio web --install-launcher` adds a "claudio" launcher that runs
+  `claudio web --app`; with `--app=<url>` (`claudio web --install-launcher --app=<url>`) it runs
+  `claudio web --app=<url>` instead. One launcher at a time: installing again replaces it. On
+  Linux it writes `$XDG_DATA_HOME/applications/claudio.desktop` (default `~/.local/share`),
+  marked `X-Claudio-Launcher=1`, and its icons under `$XDG_DATA_HOME/icons/hicolor`; on macOS,
+  `~/Applications/claudio.app`, a bundle whose executable is a script that runs claudio (the
+  window still shows under the browser's Dock icon). It names this claudio by its path, quoted.
+  `claudio web --remove-launcher` removes what it wrote and nothing else: a `claudio.desktop` or
+  `claudio.app` that claudio did not write, or a symlink at either path, is reported and left
+  alone. Before a downgrade to a claudio without `--app`, remove the launcher: an older claudio
+  refuses `--app`, so the launcher would do nothing.
+- **Window grouping.** On Linux the window opened while the browser already runs is that
+  browser's, so the desktop may group it under the browser's icon instead of the launcher's
+  (the launcher has no `StartupWMClass` for this reason). For a window with an icon of its own
+  every time, install the page from the browser (next paragraph).
+
 **Installed window.** Chromium and Safari can install the page as an app (Install, or Add to
-Dock). An app window has no tab strip, so `Ctrl+W`, `Ctrl+T`, `Ctrl+N` and friends stay with
-the terminal in it, and the page's own keys are not fighting the browser's.
+Dock); the page has PNG icons at 192 and 512 px and a maskable one for that. An app window,
+installed or opened by `claudio web --app`, has no tab strip, so `Ctrl+W`, `Ctrl+T`, `Ctrl+N`
+and friends stay with the terminal in it, and the page's own keys are not fighting the
+browser's. Checked with the help's key check in a Chrome `--app` window on Linux: `Ctrl+W`,
+`Ctrl+T` and `Ctrl+N` each reached the session.
 
 ## Security model
 
@@ -230,10 +305,24 @@ the terminal in it, and the page's own keys are not fighting the browser's.
   is reused across runs and is compared in constant time. Treat it like an SSH private key:
   whoever has it can type into your sessions. `--rotate-token` replaces it. `claudio web` prints
   it on stdout, from the token file, and so does `--print-url`; nothing else does (never a log,
-  the web service's included, a query string, a WebSocket frame or an HTTP body). With
-  `--remote` it is printed on stderr instead, in the link for this machine (when the listen
-  address is loopback or every interface), and by `--print-url`, and accepted only from local
-  requests.
+  the web service's included, a query string, a WebSocket frame, an HTTP body or a browser's
+  command line: a browser claudio opens gets a ticket, below). With `--remote` it is printed on
+  stderr instead, in the link for this machine (when the listen address is loopback or every
+  interface), and by `--print-url`, and accepted only from local requests. With `--app` and
+  stdout not a terminal it is not printed at all.
+- **Sign-in tickets.** A browser claudio opens (`claudio web`, `--foreground`, `--app`) signs in
+  with a single-use ticket, never the token: 32 random bytes, good for 30 s, kept as an empty
+  0600 file named by its SHA-256 in `<runtime>/web-tickets/` (the private runtime dir, 0700), so
+  a listing of it shows no usable ticket. The browser is opened on
+  `file://<runtime>/web-open/<hex>.html`, a 0600 page that sends it on to
+  `http://127.0.0.1:<port>/#k=<ticket>`: its command line holds only that file's path, which
+  other users cannot read. The page posts the ticket once and removes it from the address bar;
+  the server checks the file (a regular file of ours, at most 30 s old) and deletes it, so a
+  second use fails. A ticket attempt takes a login slot as a token attempt does, and with
+  `--remote` only local requests may use one. At most 16 are live at once. The one exception is
+  a snap or flatpak browser, which cannot read the runtime dir: it gets the `#k=` link on its
+  command line, and claudio says "this browser is sandboxed: the sign-in link is visible to
+  other users of this machine for up to 30 s".
 - **Cookie.** `claudio_web=<64 hex>; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/`, plus
   `Secure` with `--remote` or when `--allow-host` is set. 400 days is the longest a browser keeps
   a cookie; the server decides when a sign-in ends: 30 days unused (an open tab marks it used
@@ -275,8 +364,9 @@ the terminal in it, and the page's own keys are not fighting the browser's.
 - **State.** The web writes `web-state.json` (layout, order, each session's `launch`, still
   with its `proxy`/`first_party` for one release, remote hosts), `web-sessions.json` and
   `web-token` in the config dir (with `--set-password` and `--remote`, also `web-password` and
-  `web-tls/`), and `claudio web` writes `[web]` in `config.toml`. It reads the TUI's
-  `state.json` and never writes it.
+  `web-tls/`), and `claudio web` writes `[web]` in `config.toml`; in the runtime dir,
+  `web-tickets/`, `web-open/` and `web-app.log`; `--install-launcher` writes the launcher
+  (above). It reads the TUI's `state.json` and never writes it.
 
 ## Keys
 
@@ -375,7 +465,7 @@ Rust test in `addons/web/tests/web.rs`; rows without an automated test say how t
 | Next attention | `Alt+a` | `Alt+a`, `a` | (client side) | manual check (`a` focuses the session that needs approval); approve path: `a permission prompt shows its text…` |
 | Help | `Alt+h` | `Alt+h`, `?` | (client side) | `the help has the key check and describes keys without sending them` |
 | Overview of all sessions | `Alt+g` | `Alt+g`, Board | (client side) | `the board lists sessions with their states` |
-| Login and security | n/a | token URL, cookie; password form with `--remote`; palette: Sign out | `/api/login`, `/api/me`, `/api/logout` | `logs in from the token URL and clears the fragment`; `over --remote the page asks for the password and the session works`; `the palette signs this browser out, and its cookie stays refused`; `web_auth_gates_api_and_ws` and the other `web_*auth*` tests; `web_remote_login_rules`, `web_remote_serves_tls_with_a_kept_certificate`, `web_sign_ins_and_sign_outs_survive_a_killed_server` |
+| Login and security | n/a | token URL, cookie; sign-in ticket (`#k=`); password form with `--remote`; palette: Sign out | `/api/login`, `/api/me`, `/api/logout` | `logs in from the token URL and clears the fragment`; `a ticket link signs in once and clears the fragment`; `web_a_ticket_signs_in_once`; `over --remote the page asks for the password and the session works`; `the palette signs this browser out, and its cookie stays refused`; `web_auth_gates_api_and_ws` and the other `web_*auth*` tests; `web_remote_login_rules`, `web_remote_serves_tls_with_a_kept_certificate`, `web_sign_ins_and_sign_outs_survive_a_killed_server` |
 | Layout and style | n/a | dark only ([DESIGN.md](../DESIGN.md)); Settings → Text size | `layout.save` | `the page follows DESIGN.md`; `web_order_and_layout_persist_in_web_state` |
 | Keys | `Alt+…` (see Keys) | the same Alt keys, `Ctrl+Shift+Space`, `k` | (client side) | `Ctrl+Shift+Space then k opens the palette`; `Alt+u in a terminal opens the reader and sends nothing to claude`; `Alt+a focuses the session that needs attention, Alt+Right switches session` |
 | Newline in the prompt | `Shift+Enter` | `Shift+Enter` in a terminal | `session.input` | `Shift+Enter puts ESC CR into the input, Enter submits` |
````
Run: `grep -n "Installed window" docs/web.md && grep -c "install-launcher" docs/web.md README.md CLAUDE.md`
Expected:
```
264:**Installed window.** Chromium and Safari can install the page as an app (Install, or Add to
docs/web.md:5
README.md:2
CLAUDE.md:1
```

Step 2: commit
`git commit -m "docs(web): the app window, the launcher and sign-in tickets" -- docs/web.md README.md CLAUDE.md`

Interfaces this task provides to later tasks: none.

## Merge and release
<!-- Not copied into briefs; the controller executes this after the gate audit. -->
1. Full suites, on the branch's last commit, in the worktree:
   - `make check` → every step passes (`rules: 0 problem(s); over budget: 12293 file lines,
     4287 fn lines, 20 unwraps`; the workspace unit tests: `claudio` 2, `claudio-api` 106, `claudio-core` 581, `claudio-tui`
     393, `claudio-vocdoni` 188, `claudio-web` 147 passed, and the same per crate).
   - `make matrix` → the five cells pass; the core-only manager suite: `test result: ok. 44 passed; 0 failed`.
   - `cargo test --locked --test manager --test vocdoni_manager --test web --test web_manager --test vocdoni_web -- --test-threads=1`
     → `manager` 44, `vocdoni_manager` 19, `vocdoni_web` 13, `web` 65, `web_manager` 4 passed,
     0 failed.
   - `make test-web` → node: `# tests 155`, `# pass 155`, `# fail 0`; Playwright: `54 passed`.
   - The owner's manual checks on the Linux laptop, each recorded in the ledger
     (`.delivery/2026-11-13-desktop/progress.md`) with what was seen. They do not gate the merge,
     except AC9(d):
     - AC9(a): done (the ledger quotes the key check; Task 7's paragraph is written from it).
     - AC9(b): `claudio web --install-launcher`; the menu entry starts the window, with Chrome
       already running and not; record the window's class or app id each time (`xprop WM_CLASS`
       on X11, the compositor's window info on Wayland) and whether the dock shows the claudio
       icon. If one value covers both, a follow-up adds `StartupWMClass` (decision 14).
     - AC9(c): `~/Applications/claudio.app` opens from Finder, when a Mac is at hand.
     - AC9(d), **gates the merge**: in Chrome and Chromium, the `--app=file://…` window that
       redirects to `http://127.0.0.1:<port>/` shows no extra toolbar and keeps Capture all keys;
       `xdg-open` on the `.html` file (and `open` on a Mac) starts the default browser, not an
       editor, and it signs in (decision 20). If it fails, the owner decides between the spec's
       argv fallback and no auto sign-in, and the branch waits for that ruling.
     - AC9(e): `claudio web --app=https://z6:7337/` opens z6's web, the password form signs in,
       and a second launch is already signed in.
   - Fable gate audit of the branch against the spec, this plan and ACCEPTANCE.md; its fixes are
     re-reviewed by the controller and the suites above re-run.
2. Merge order: one repo, one branch: `git checkout main && git merge --no-ff feat/app-window`
   (from `/home/p4u/claudio`).
3. Release: the next patch version, v0.9.6. In the root `Cargo.toml`, `[workspace.package]
   version = "0.9.5"` becomes `"0.9.6"`; `cargo update -w`; `git commit -m "chore(release):
   v0.9.6" -- Cargo.toml Cargo.lock`; `git tag -a v0.9.6 -m "v0.9.6"`; push over HTTPS with the
   gh credential helper:
   `git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push https://github.com/p4u/claudio.git main v0.9.6`.
   The tag starts the release workflow. Then the notes: `gh release edit v0.9.6 --notes-file
   <file>` (`claudio web --app` and `--app=<url>`; `--install-launcher` and `--remove-launcher`;
   a browser claudio opens signs in with a single-use ticket, never the token on its command
   line; PNG and maskable icons; the sandboxed-browser residual risk in the spec's words; "run
   `claudio web --remove-launcher` before a downgrade").
4. **Owner OK required before:** nothing beyond the gate: the owner has said to merge, tag,
   push and publish without asking once the gate passes. The one owner decision that can hold
   the branch is AC9(d)'s, above.
5. Post-release checks: both CI runs green (`gh run list --branch main --limit 1` and the tag's
   release run, `gh run list --workflow release.yml --limit 1` or the run `gh release view
   v0.9.6` links); the release lists its assets (`gh release view v0.9.6`).
