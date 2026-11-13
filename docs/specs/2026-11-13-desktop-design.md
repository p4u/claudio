# claudio in its own window: app window, launcher, sign-in tickets: design

Status: audited (2026-11-13; audit-spec.md F1-F16 folded in, F1 option A by the owner)   Class: architectural (changes how a browser signs in: a security path)
Owner decisions so far (2026-11-13; the owner's desktop is a Linux laptop; z6, where the
sessions live, has no desktop):
- A desktop experience, released as one artefact per platform: the `claudio` binary itself, no
  separate `claudio-desktop` download (Fable consult, `.delivery/2026-11-13-desktop/consult-desktop.md`).
- Linux and macOS only for now. Windows (WSL2, remote client or a native port) is decided later.
- Both modes: `--app` (a web service on the laptop reaching the hosts over ssh) and
  `--app=<url>` (an existing remote web, such as z6's, in an app window).
- Sign-in through a 0600 redirect file, never a secret in argv (audit F1, option A).
- No Electron, no native webview shell in this work. A wry+tao shell is a later option, built
  only if the evidence the consult lists appears.

## Problem

- To use claudio in a browser today, the owner runs `claudio web`, which opens a browser tab.
  There is no "claudio" icon to click, no window of its own, and the tab competes with the
  browser's own shortcuts.
- The page already supports installation as an app:
  - `addons/web/web/manifest.webmanifest` declares `display: standalone`;
  - docs/web.md:192 documents "Installed window";
  - `app/ui/capture.js` implements "Capture all keys" (fullscreen and the Keyboard Lock API).

  What is missing is a way to open that window directly, and a launcher.
- **Security gap found by the consult.** `app::browser::open_browser(url)`
  (crates/core/src/app/browser.rs:15-30) passes the start URL as an argument to `xdg-open`/`open`.
  That URL carries the long-lived web token (`lib.rs::start_url`, `#t=<token>`, lib.rs:362-369).
  When the browser was not running, the URL also becomes the browser's own argv for its
  lifetime. Other local users can read argv (`/proc/<pid>/cmdline`). This breaks ACCEPTANCE C1,
  which says secrets never go in argv.
- The manifest has one SVG icon (`sizes: any`), so some Chromium versions may not offer to
  install it. Its colours (`#0b0f14`) are not DESIGN.md's Void (`#0b0e12`).

## Goals

0. `claudio web --app=<url>` opens an existing `claudio web` on another machine in a chromeless
   app window (Design 2b); nothing secret goes in argv.
1. `claudio web --app` opens the manager in a chromeless app window of the installed
   Chromium-family browser.
   - It first makes sure the web service runs, as a bare `claudio web` does, saving `[web]` and
     having the daemon apply it.
   - It works when started without a terminal, from a desktop launcher.
2. `claudio web --install-launcher` puts a "claudio" entry in the desktop menu (Linux) or in
   `~/Applications` (macOS) that runs `claudio web --app`, or `claudio web --app=<url>` when
   installed with `--app=<url>`. `--remove-launcher` takes it away.
3. No long-lived web token and no ticket goes in an argv that claudio builds, except in the
   sandboxed-browser fallback (a stated residual risk, see Failure modes). A browser that claudio
   opens signs in with a single-use sign-in ticket valid for 30 s, carried in a redirect file in
   the private runtime dir; argv holds only `file://<path>` of that file.
   - This covers `--app` and the existing browser opening of `claudio web` and
     `--foreground`.
   - `--print-url` and the printed URL lines are unchanged: they go to stdout, not argv.
4. The page is installable by the Chromium versions in use: PNG icons at 192 and 512 px,
   including a maskable one. The manifest and icon background colours use DESIGN.md's Void.
5. The docs say what an app window does with browser-reserved keys, as the owner's key check in
   a Chrome `--app` window found.

## Non-goals

- A native webview shell (wry, Tauri, Electron), a tray icon, launch at login, global hotkeys.
- Windows.
- Firefox or Safari app windows. Without a Chromium-family browser, `--app` falls back to the
  default browser and says why. Safari's "Add to Dock" stays a manual step.
- A service worker or offline mode.
- On macOS the app window appears under the browser's Dock icon; `claudio.app` is only a launcher.
- Changing the token login, the password login, cookies or sign-in sessions beyond adding the
  ticket.
- Trusting the `--remote` self-signed certificate on the user's behalf.

## Current state

- Flags: `WebArgs` and `parse_args` (addons/web/src/lib.rs:68-180). `--no-open`, `--print-url`,
  `--rotate-token` and the other one-shot flags are mutually constrained there (`combination_ok`, lib.rs:173-200).
- Service start: `cli::start` (addons/web/src/cli.rs:124-173) saves `[web]`, asks the daemon to
  `apply`, then calls `announce` (cli.rs:194-219). `announce` prints the URL and opens the
  browser when `browser_wanted` (browser.rs:7-11: a display, a TTY and no
  `CLAUDIO_NO_BROWSER`) and not `--no-open`. The foreground path opens it at lib.rs:567.
- Token login: the page reads `#t=<token>` (boot.js:105-122) and POSTs `{token}` to
  `/api/login`.
  - `login::login` (login.rs:29-70) calls `token_login` (login.rs:75-86).
  - The token is accepted only from a local caller when `--remote` is on (`token_allowed`),
    behind the login rate limiter, compared in constant time.
  - On success `create_session` sets the cookie.
- Runtime dir: `paths::runtime_dir()` (crates/core/src/paths.rs:50), private
  (`ensure_private_dir`). The CLI and the service it asks the daemon to run resolve it the same
  way, as the daemon socket already relies on.
- Assets: the page is embedded by addons/web/build.rs and served by assets.rs, which has the png
  MIME type. The icon is `addons/web/web/icons/claudio.svg`.
- Secret files are written with `paths::write_atomic` (0600), as sign-ins and the token are.

## Design

### 1. Sign-in tickets (protocol change: docs/web-protocol.md first, in its own commit)

**Minting** (in the `claudio` process that opens a browser):
1. Make 32 random bytes and hex-encode them as the ticket (64 characters).
2. Create the file `<runtime>/web-tickets/<sha256-hex of the ticket>`: empty, inside a directory
   made with `ensure_private_dir`. Minting runs `ensure_private_dir(<runtime>)` and
   `ensure_private_dir(<runtime>/web-tickets)`, and creates the file with
   `O_CREAT|O_EXCL|O_NOFOLLOW`, mode 0600 (not `write_atomic`: there is no content to replace).
3. The ticket never goes in argv. The CLI writes `<runtime>/web-open/<32 random hex>.html`
   (0600, `write_atomic`, directory checked as the ticket directory is) holding only
   `<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=URL">`
   `<title>claudio</title><a href="URL">Open claudio</a>`, where URL is `<base URL>#k=<ticket>`
   (scheme, IP literal, port and hex only: nothing to escape, asserted in a unit test). What it
   opens is `file://<that path>` (percent-encoded). Other users cannot read the file (the
   runtime dir is 0700), so argv holds no secret. Files older than 60 s are removed at each mint
   and at service start; at most 16. It is never `#t=<token>`.
4. Fallback, when the browser cannot read the runtime dir (its resolved path is under `/snap/`
   or a flatpak `exports/bin`): the URL in argv is `<base URL>#k=<ticket>`, and claudio prints
   "this browser is sandboxed: the sign-in link is visible to other users of this machine for
   up to 30 s". Docs state this residual risk in these words.

**Redeeming** (the page and the server):
- The page, on `#k=<64 lowercase hex>`, POSTs `{"ticket": "<ticket>"}` to `/api/login`, then
  clears the fragment, as it does for `#t=`.
- The server handles `{"ticket": T}` in this order, in `spawn_blocking`:
  1. reserve the rate-limit slot exactly as a token attempt does
     (`limiter.reserve(caller.key, !caller.local, now)`); `429` when full. Nothing below runs
     then; the ticket is untouched.
  2. `token_allowed(caller)` false (a loopback peer and Host are needed when `--remote` is on)
     gives `401`; the ticket file is not touched.
  3. `T` is not 64 lowercase hex gives `401`.
  4. `ensure_private_dir(<runtime>)` and `ensure_private_dir(<runtime>/web-tickets)`, on every
     redeem; an error gives `401` and an ERROR log naming the path.
  5. `symlink_metadata(<runtime>/web-tickets/<sha256 hex of T>)`: it must be a regular file,
     owned by our uid, with `now - 30 s <= mtime <= now + 2 s`. Otherwise `401`; a file that
     fails the test is removed.
  6. `remove_file` must return `Ok`. `NotFound` (the other of two racing attempts won) gives
     `401`.
  7. Success: release the slot, create the session as a token login does.

  Every `401` keeps its slot, like a wrong token.
- Expired files are removed by each mint (which lists the directory to keep at most 16,
  removing the oldest first) and once at service start. A mint that removes a live ticket of a
  concurrent `claudio web --app` is accepted: that window says the link expired.
- A ticket is checked by file lookup, not by string comparison, so there is no timing leak to
  guard against.

**The page**, on `#k=`: a fragment that is not exactly `#k=<64 lowercase hex>` is treated as a
refused ticket without a request. Otherwise it posts the ticket, even when a cookie exists (so
the ticket is spent), then clears the fragment, also when the request throws. `204` starts the
app. `429` shows "Too many attempts: wait a minute." Any other status or a thrown request asks
`/api/me`: `204` starts the app; a password offered shows the password form; else it shows "This
sign-in link has expired. Run `claudio web` again." A successful ticket login replaces the
session of the cookie it came with, as a token login does.

### 2. `claudio web --app`

- **Flags.** `--app` is accepted with the service flags (`--listen`, `--remote`,
  `--allow-host`) and with `--foreground`. It is refused with `--no-open`, `--print-url`,
  `--set-password`, `--print-cert`, `--service`, and the launcher flags. `--app` is a property of
  this invocation only: it is not part of `Unit`, never written to `[web]` and never in the
  service's argv. It is accepted with `--rotate-token` and `--rotate-cert` (the ticket is minted
  after the `apply` reply). `stop` and `status` take no flags. `USAGE` gains `[--app]` and the
  line `claudio web --install-launcher | --remove-launcher`. `--set-password`, `--print-cert`
  and the launcher flags each require `n_args == 1`, as `combination_ok` does today.
- **Start or find the service.** As `cli::start` does today. Then, instead of `open_browser`,
  it mints a ticket and opens the app window on the redirect file's `file://` URL (Design 1,
  Minting).
  - It needs a display (`DISPLAY` or `WAYLAND_DISPLAY` non-empty on Linux; always on macOS) but
    not a TTY. Without a display it mints nothing, prints "claudio web --app: no display here;
    open <base URL> in a browser on this machine" and exits 1. `browser_wanted` gains a sibling
    `display_present(os, env)` in `app::browser`, unit-tested.
  - `CLAUDIO_NO_BROWSER=1` still means no window at all.
  - With `--foreground`, the same happens once the server listens.
  - With `--app`, when stdout is not a terminal, nothing that holds the token is printed: the
    line is `claudio web: <base URL>` (no fragment) and the `--remote` banner's token line is
    left out. On a terminal the lines are as today.
- **Errors without a terminal.** When stderr is not a terminal and `--app` fails, the message
  also goes to the desktop: `notify-send -a claudio "claudio" "<message>"` on Linux (skipped
  when it is not on `PATH`), `osascript -e 'display notification …'` on macOS, with a 3 s
  deadline, stdin null, the message passed as one argument (never through a shell; for
  `osascript` as an `argv` item of `on run argv`). The message holds no token or ticket. It is
  always appended to `<runtime>/web-app.log` (0600, capped at 64 KiB).
- **Finding the browser** is a pure function over the env and a file-exists check, unit-tested.
  It tries, in order:
  1. `[web] browser` in `config.toml` (a path or a name; written by hand, never by claudio),
     else `$CLAUDIO_BROWSER`.
  2. On Linux, the first of the names below found in the process `PATH` followed by
     `/usr/bin`, `/usr/local/bin`, `/snap/bin`, `/var/lib/flatpak/exports/bin` and
     `~/.local/share/flatpak/exports/bin`. No login shell is run. The names are
     `google-chrome`, `google-chrome-stable`, `chromium`, `chromium-browser`, `brave-browser`,
     `microsoft-edge`, `microsoft-edge-stable`.
  3. On macOS, the executable inside the first of `Google Chrome.app`, `Chromium.app`,
     `Brave Browser.app`, `Microsoft Edge.app` under `/Applications` or `~/Applications`.
- **Spawning.** argv is `--app=file://<path>` (plus `--class=claudio` on Linux), or the same file
  URL as the one argument of `xdg-open`/`open`; in the sandboxed-browser fallback the argument
  is the `<base URL>#k=<ticket>` URL instead (Design 1, Minting step 4). stdin, stdout and
  stderr are null, and the child is not waited for, as `open_browser` does. It uses the
  browser's default profile, so the existing cookie, notification permission and settings apply.
- **No such browser.** It falls back to `open_browser` on the redirect file's `file://` URL and
  prints one line: "no Chromium-family browser found: opened the default browser (an app window
  needs Chrome, Chromium, Brave or Edge)".
- **Remote.** The base URL is `start_url`'s without the fragment: `https://127.0.0.1:<port>/` or
  `https://[::1]:<port>/`. When the listener has a specific non-loopback address there is no
  local URL: `--app` mints no ticket and opens the public URL, where the page asks for the
  password. A new `remote::base_url(addr) -> Option<String>` is shared with `token_url`.
  The browser handles the self-signed certificate as it does today.
  - The browser shows its certificate warning first. A ticket that expired meanwhile is not an
    error state of its own: the page falls through to the password form (it is offered to every
    `--remote` caller).

### 2b. `claudio web --app=<url>`: an existing web on another machine (owner decision 2026-11-13)

The owner's sessions live on a host without a desktop (z6, reached over ssh), whose own
`claudio web --remote` service the laptop's browser uses today. `--app=<url>` opens that web in
an app window and does nothing else:
- **No service.** It starts or contacts no daemon or service, reads no token, mints no ticket
  and writes no `[web]`. It runs only the browser search and the spawn of Design 2, on `<url>`.
  The display rule, the no-terminal error path and the fallback to `open_browser` apply as in
  Design 2.
- **Accepted `<url>`**, checked by a pure, unit-tested function:
  - scheme `http` or `https`, a host, an optional port and path;
  - no userinfo (`user:pass@`);
  - no fragment at all, so a token (`#t=`) or ticket (`#k=`) can never be put in argv this way.

  Anything else exits 2 with "claudio web --app: expected an http(s) URL without credentials
  or fragment".
- **Flags.** `--app=<url>` takes no other flag, apart from the launcher's (Design 3).
- **Sign-in** is the page's: the password form of a `--remote` web. The cookie then keeps the
  window signed in (30 days idle, renewed by use), as in a browser tab.
- **The self-signed certificate** of a `--remote` web: the window shows the browser's warning
  until the certificate is trusted on the laptop. docs/web.md says how: on the host,
  `claudio web --print-cert > z6.pem`; on a Linux laptop,
  `certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n claudio-z6 -i z6.pem` (Chrome/Chromium's NSS
  store); or an ssh tunnel (`ssh -L 7337:127.0.0.1:7337 z6`) and
  `claudio web --app=https://127.0.0.1:7337/`, which the certificate already names. Neither is
  automated.
- **Bare `--app` vs `--app=<url>`.** `--app` with no value is Design 2 (the local service).
  `--app=` with an empty value exits 2.

### 3. `claudio web --install-launcher` / `--remove-launcher`

These are one-shot commands, refused with any other flag except `--app=<url>`
(`--install-launcher --app=<url>`: the launcher opens that web, Design 2b; its `Exec` and the
macOS script then run `web --app=<url>`, the URL validated as in Design 2b and quoted like the
exe). Without it the launcher runs bare `web --app`. One launcher at a time: installing again
replaces it, whichever form. The exe is `std::env::current_exe()`
as returned (not canonicalized); a path that is not absolute, does not exist, ends in
` (deleted)` or holds a control character is refused with a message. Files go through
`paths::write_atomic` (0600; missing parent directories 0700); the macOS script is then `chmod`
0755 (C4: executables keep their mode). Every path is under the user's home or `XDG_DATA_HOME`.
Running it again replaces the files; removing what is absent succeeds.

- **Linux:**
  - `$XDG_DATA_HOME/applications/claudio.desktop` (default `~/.local/share`) with
    `Type=Application`, `Name=claudio`, `Exec="<exe>" web --app` (or `web --app=<url>`), `Icon=claudio`,
    `StartupWMClass=<the class observed in AC9(b)>` (see below), `Categories=Development;` and `Terminal=false`;
  - icons in `$XDG_DATA_HOME/icons/hicolor/{scalable,192x192,512x512}/apps/claudio.{svg,png}`.

  `.desktop` quoting, one pure function with a table test: the exe is wrapped in double quotes;
  inside them `\` becomes `\\\\`, `"` becomes `\\"`, `` ` `` becomes `` \\` ``, `$` becomes
  `\\$`, and `%` becomes `%%`. The file starts with `[Desktop Entry]` and has
  `X-Claudio-Launcher=1`.

  `StartupWMClass=<the class observed in AC9(b)>`: the plan records, for Chrome and Chromium on
  X11 and on Wayland, the window class or app id of an `--app` window opened while the browser
  already runs and while it does not. If one value covers both, the launcher uses it; if not,
  the line is left out and docs/web.md says the window groups with the browser when it is
  already running, and recommends the browser's Install for a window with its own icon.
- **macOS:** `~/Applications/claudio.app` containing:
  - `Contents/Info.plist` (`CFBundleName` claudio, `CFBundleIdentifier` `io.vocdoni.claudio`,
    `CFBundleExecutable` claudio, `CFBundleIconFile` claudio);
  - `Contents/MacOS/claudio`, a 0755 POSIX shell script: `#!/bin/sh` and
    `exec '<exe>' web --app`, the exe in single quotes with each `'` written `'\''`;
  - `Contents/Resources/claudio.icns`, the header plus one `ic09` entry holding
    `claudio-512.png`.

  `Info.plist` has `CFBundlePackageType` `APPL` and `ClaudioLauncher` `true`.

  A bundle created locally has no quarantine attribute, so Gatekeeper does not block it. This is
  unverified on current macOS: the plan includes a manual check by the owner.
- `--remove-launcher` removes only what claudio wrote: a `.desktop` that has
  `X-Claudio-Launcher=1`, a bundle whose `Info.plist` has `ClaudioLauncher`; otherwise it says
  the file is not its own and exits 1. It never follows a symlink: a symlink at either path is
  reported, not removed.
- They print what they wrote or removed.

### 4. PWA polish

- Commit `icons/claudio-192.png`, `icons/claudio-512.png` and `icons/claudio-maskable-512.png`.
  They are rendered once from the SVG, whose background becomes `#0b0e12`. The plan names the
  tool.
- The SVG's strokes become `#c58aff` and `#00d892`. The maskable PNG is rendered from a second
  source with a full-bleed `#0b0e12` square (no rounded corners) and the glyph scaled into the
  central 80 %. The manifest entries are `purpose: "any"` for the 192 and 512 PNGs and the SVG,
  `purpose: "maskable"` for the third.
- The manifest lists them. Its `background_color` and `theme_color` become `#0b0e12`.
- The embedded page grows by a few KB. The CSP is unchanged: images are same-origin.

### 5. Docs

- docs/web-protocol.md (first commit): the `ticket` login body, `#k=`, and its rules.
- docs/web.md:
  - "App window and launcher": `--app`, `--install-launcher`/`--remove-launcher`, the browser
    search order and `CLAUDIO_BROWSER`, the fallback;
  - the corrected "Installed window" key paragraph, following the owner's key check;
  - the security model's sign-in ticket.
- README: the quick start mentions `claudio web --app` and `--install-launcher`.
- CLAUDE.md: one line each in "Web UI" (tickets, `--app`) and in the web constraints (no token
  in argv).

### Alternatives considered

| Option | Why not |
|---|---|
| Separate claudio-desktop (Electron, Tauri, wry+tao) | The consult's verdict. Each moves the terminal to an engine we have never tested (WebKitGTK, WKWebView), or ships ~100 MB of Chromium. Each adds packaging, signing and a GUI dependency tree, for gains an app window already gives. Kept as phase 2, on evidence. |
| Open the app window with no fragment, relying on the cookie | Works only for a browser profile that is already signed in. A fresh profile shows "open the URL claudio web printed". |
| Ticket minted in the service's memory, fetched by the CLI over HTTP(S) | The CLI would need an HTTP client and, with `--remote`, would have to pin the kept certificate. The runtime-dir file needs neither, and both processes already share that private directory. |
| Keep `#t=<token>` in argv | It breaks C1. Any local user can read the token from `/proc` and keep it until it is rotated. |
| The ticket URL in argv | A process polling `/proc` reads it and redeems it in milliseconds, before the browser's page can. The redirect file in the 0700 runtime dir keeps it out of argv. |

## Compatibility and rollout

- **Page and server** ship in one binary. An old page never sees a `#k=` URL, because only a new
  `claudio` mints them.
- **New `claudio web`, old running service** (the daemon was outdated but busy, so it and its
  service were left running). The old page ignores `#k=`. The CLI detects this: the `apply`
  reply's `since` is earlier than the mtime of this binary (`Probe::own`). It then asks `apply`
  again with `force: true`, so the daemon restarts the service from the binary on disk
  (`Config::claudio`), which is the new one; open tabs reconnect. A new `claudio web --service`
  under an older daemon is the mixed-version case the daemon protocol already supports. AC: a
  web suite test with a service started before the binary's mtime is bumped. If the forced
  restart fails, `claudio web --app` prints the token-free base URL and "the web service is
  older than this claudio: run `claudio daemon restart`". (The plan must confirm that a forced
  `apply` re-execs `Config::claudio` from disk.)
- **The web protocol** stays version 2. The login body gains an optional alternative, which is
  additive.
- **Rollback:** an older claudio ignores `<runtime>/web-tickets/` and `<runtime>/web-open/`.
  Launchers keep pointing at the exe path; an older claudio refuses `--app`, so the launcher
  does nothing until claudio is upgraded again. Run `claudio web --remove-launcher` before a
  downgrade (docs/web.md says so).

## Failure modes and security

- **Argv holds no secret**, only `file://<path>` of a redirect file that other users cannot read
  (the runtime dir is 0700). The residual risk is the sandboxed-browser fallback (a browser
  whose resolved path is under `/snap/` or a flatpak `exports/bin`, which cannot read the
  runtime dir): there the `#k=` URL is in argv and claudio prints "this browser is sandboxed:
  the sign-in link is visible to other users of this machine for up to 30 s". Another local
  user who polls `/proc` can redeem it first. Docs state this in these words.
- **The ticket directory** is private (0700, owned, not a symlink, via `ensure_private_dir`,
  checked on every mint and every redeem, with the parent runtime dir).
  Ticket files are 0600 and empty. Their names are hashes, so listing the directory reveals no
  usable ticket.
- **Bounded:** 16 live tickets, a 30 s life, the existing login rate limiter and body cap.
- **Launchers** are written only under the user's own directories and never follow a symlink
  out (they use `write_atomic`). An exe path with spaces, quotes or other reserved
  characters is quoted or escaped correctly, and a test covers it.
- **`--app=<url>`** puts only a URL the user gave in argv, and refuses userinfo and any
  fragment, so neither a password, a token nor a ticket can travel that way.
- **The browser** is spawned without a shell, so nothing in the URL is interpreted. The URL is
  built only from the service's own address and the hex ticket.
- **C1:** the token never appears in argv, logs or notices. The ticket never appears in argv
  either: argv carries `file://<path>` only. The one exception is the sandboxed-browser
  fallback, a stated residual risk (above). The ticket is never in logs.

## Acceptance criteria

- [ ] AC1: unit tests of the browser search: `[web] browser`, then `CLAUDIO_BROWSER`, win; the Linux order over a fake
  PATH; macOS bundle paths; none found gives `None`.
- [ ] AC2: unit tests of the argv built for `--app` (`--app=file://<path>`, `--class=claudio` on
  Linux), and that it never contains `#t=`, `#k=` or the token; the redirect file's content, with
  nothing to escape in URL; the sandboxed-browser fallback argv (the only one with `#k=`).
- [ ] AC3: unit tests of minting and redeeming tickets:
  - a ticket redeems once and a second redeem fails;
  - an expired file (mtime over 30 s) fails and is removed;
  - an unknown ticket fails;
  - at most 16 live tickets;
  - the directory is 0700 and files are 0600;
  - a ticket file that is a symlink, a directory, or has a future mtime fails; a `web-tickets`
    that is a symlink or group-readable is refused or repaired as `ensure_private_dir` does.
- [ ] AC4: web Rust test (addons/web/tests/web.rs):
  - a minted ticket POSTed as `{"ticket": …}` gives 204 and a cookie, and replaying it gives
    401;
  - with `--remote` and a non-local peer (`CLAUDIO_TEST_REMOTE_PEER`), a valid ticket gives 401
    and counts in the limiter; the non-local `401` leaves the ticket file in place, and the same
    ticket then redeems from a local caller;
  - a body with both `token` and `ticket` gives 400.
- [ ] AC5: web suite test: `claudio web --app` with a fake browser on `PATH` (a script that
  records its argv) starts it once with `--app=file://<path>`; the redirect file holds
  `http://127.0.0.1:<port>/#k=<64 hex>`, and the recorded argv holds neither the token nor the
  ticket. The same holds with no TTY, and with no TTY stdout and stderr hold neither the token
  nor the ticket. With no TTY and a `[web]` that cannot start, a fake `notify-send` on `PATH`
  records the message and the log file has it.
- [ ] AC6: the existing browser-opening path (`claudio web` with a fake `xdg-open` on `PATH`)
  opens a `file://` URL of a redirect file that holds `#k=`, and never `#t=` in
  argv or in the file. The fake opener is `xdg-open` on Linux and `open` on macOS, first on a
  `PATH` that reaches no real one, under the PTY harness with `DISPLAY` set (`browser_wanted`
  needs a TTY).
- [ ] AC7: launcher tests over a temp `HOME`/`XDG_DATA_HOME`:
  - Linux `.desktop` content, with exe paths containing a space, `"`, `` ` ``, `$`, `\` and `%`,
    each checked against the expected bytes (a path with a newline is refused), and the icon
    files;
  - macOS bundle layout, `Info.plist` keys, script mode 0755 and a valid icns header; the icns has one `ic09` entry whose bytes are the
    512 PNG;
  - removing works and running twice is idempotent; removing a foreign `claudio.desktop` or
    `claudio.app` is refused.
- [ ] AC8: Playwright: opening `/#k=<valid ticket>` signs in and clears the fragment. A replayed
  or expired ticket shows the expiry message. A `#k=` URL opened with a live cookie signs in and
  the old cookie's session is gone; a `429` shows the wait message. Zero CSP violations. The manifest lists the PNG
  icons and they are served with `image/png`.
- [ ] AC9: manual checks by the owner on the Linux laptop (z6 has no desktop), recorded in the
  ledger. They are not CI gates:
  - (a) the key check in a Chrome `--app` window (`google-chrome --app=https://z6:7337/` is
    enough), run before the docs paragraph is written;
  - (b) `--install-launcher` on the Linux desktop starts the app window from the menu, and the
    window shows the claudio icon in the dock/taskbar, with the browser already running and
    not;
  - (c) on a Mac, the `~/Applications/claudio.app` stub opens from Finder (when a Mac is
    available; the owner's laptop is Linux);
  - (e) `claudio web --app=https://z6:7337/` on the laptop opens z6's web, the password form
    signs in, and a second launch is already signed in;
  - (d) in Chrome and Chromium, an `--app=file://…` window that redirects to
    `http://127.0.0.1:<port>/` shows no extra toolbar and keeps Capture all keys; `xdg-open` and
    `open` on the `.html` file start the default browser, not an editor. If (d) fails, the owner
    decides between the argv fallback and no auto sign-in.
- [ ] AC13: unit tests of the `--app=<url>` check: http and https with host, port and path
  pass; userinfo, any fragment (`#t=…`, `#k=…`, `#x`), other schemes, an empty value and a
  missing host exit 2 with the stated message.
- [ ] AC14: web suite: `claudio web --app=https://example.invalid:7337/` with a fake browser on
  `PATH` starts it once with exactly `--app=https://example.invalid:7337/` (plus `--class` on
  Linux); no daemon is started (no socket appears in the test runtime dir) and `config.toml` is
  unchanged. `--install-launcher --app=<url>` writes `Exec="<exe>" web --app=<url>` (AC7's temp
  `HOME`).
- [ ] AC10: docs updated as §5. `make check`, `make matrix`, the web suites and `make test-web`
  pass.
- [ ] AC11: web suite: `claudio web --foreground --app` starts the fake browser once the server
  listens; `CLAUDIO_NO_BROWSER=1 claudio web --app` starts nothing and mints nothing
  (`web-tickets` is empty); with `CLAUDIO_BROWSER` unset and injected empty search roots (unit
  level) the fallback line is the exact text of §Design 2.
- [ ] AC12: `grep -n "Installed window" docs/web.md` shows the paragraph, and the ledger entry of
  AC9(a) quotes the Key check lines it was written from.

## Constraints

- ACCEPTANCE.md throughout:
  - A2/A3: the core names no addon. Browser discovery lives in the web addon, or as a generic
    service in `app::browser`.
  - A8: `lib.rs` is at 692 of 800 lines and `hub.rs`/`ws.rs::dispatch` are baselined: tickets,
    the app window and the launchers are new modules (`ticket.rs`, `app_window.rs`,
    `launcher/`).
  - A10: bounded.
  - B2: the protocol doc comes first.
  - C1: no token in argv or logs; the ticket stays out of argv too, bar the sandboxed-browser
    fallback (a stated residual risk).
  - C4: secret files are 0600, in private directories.
  - E2: no weakened tests.
- DESIGN.md tokens. The import map, and thus the CSP hash, is unchanged.
- Tests are hermetic. Fake browsers are scripts on a test `PATH`. Never launch the owner's real
  browser, and never touch their daemon, web service or desktop menus.
  Every test that reaches the browser search sets `CLAUDIO_BROWSER` to a fake script; on macOS
  the bundle roots are a parameter of the pure function and no integration test uses the real
  ones. "None found" is a unit test only.
- Linux and macOS.
- Commits: `type(scope): lowercase summary`, no AI attribution or Co-Authored-By trailers.

## Open questions

- The key check result (AC9a) shapes only the docs paragraph. It does not block the build.
