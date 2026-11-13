# claudio in its own window: app window, launcher, sign-in tickets: design

Status: draft   Class: architectural (changes how a browser signs in: a security path)
Owner decisions so far (2026-11-13):
- A desktop experience, released as one artefact per platform: the `claudio` binary itself, no
  separate `claudio-desktop` download (Fable consult, `.delivery/2026-11-13-desktop/consult-desktop.md`).
- Linux and macOS only for now. Windows (WSL2, remote client or a native port) is decided later.
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

1. `claudio web --app` opens the manager in a chromeless app window of the installed
   Chromium-family browser.
   - It first makes sure the web service runs, as a bare `claudio web` does, saving `[web]` and
     having the daemon apply it.
   - It works when started without a terminal, from a desktop launcher.
2. `claudio web --install-launcher` puts a "claudio" entry in the desktop menu (Linux) or in
   `~/Applications` (macOS) that runs `claudio web --app`. `--remove-launcher` takes it away.
3. No long-lived web token ever goes in an argv that claudio builds. A browser that claudio opens
   signs in with a single-use sign-in ticket valid for 30 s.
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
- Changing the token login, the password login, cookies or sign-in sessions beyond adding the
  ticket.
- Trusting the `--remote` self-signed certificate on the user's behalf.

## Current state

- Flags: `WebArgs` and `parse_args` (addons/web/src/lib.rs:68-180). `--no-open`, `--print-url`,
  `--rotate-token` and the other one-shot flags are mutually constrained there (lib.rs:174).
- Service start: `cli::start` (addons/web/src/cli.rs:124-173) saves `[web]`, asks the daemon to
  `apply`, then calls `announce` (cli.rs:194-219). `announce` prints the URL and opens the
  browser when `browser_wanted` (browser.rs:7-11: a display, a TTY and no
  `CLAUDIO_NO_BROWSER`) and not `--no-open`. The foreground path opens it at lib.rs:567.
- Token login: the page reads `#t=<token>` (boot.js:105-122) and POSTs `{token}` to
  `/api/login`.
  - `login::login` (login.rs:29-72) calls `token_login` (login.rs:77-89).
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
2. Create the file `<runtime>/web-tickets/<sha256-hex of the ticket>`: empty, 0600, inside a
   directory made with `ensure_private_dir`.
3. The URL it opens is the service's base URL plus `#k=<ticket>`. It is never
   `#t=<token>`.

**Redeeming** (the page and the server):
- The page, on a `#k=<64 hex>` fragment, POSTs `{"ticket": "<ticket>"}` to `/api/login`, then
  clears the fragment, as it does for `#t=`.
- The server accepts a ticket only from a caller that may use the token (`token_allowed`: a
  loopback peer and Host when `--remote`; any allowed caller otherwise).
  - The attempt goes through the same rate limiter slot as a token attempt.
  - It computes the SHA-256 of the presented ticket and removes
    `<runtime>/web-tickets/<that hash>`.
  - The removal must succeed, which makes the ticket single-use when two attempts race. The
    file must also have been a regular file whose mtime is at most 30 s old.
  - A refused ticket counts as a wrong token.
  - Expired ticket files are removed whenever the directory is read, and at service start.
- A ticket is checked by file lookup, not by string comparison, so there is no timing leak to
  guard against.
- At most 16 live ticket files: minting beyond that removes the oldest.

**Errors on the page:** an expired or unknown ticket shows "This sign-in link has expired. Run
`claudio web --app` again." A browser that already has a valid cookie is signed in either way:
the page tries `/api/me` first when the ticket is refused.

### 2. `claudio web --app`

- **Flags.** `--app` is accepted with the service flags (`--listen`, `--remote`,
  `--allow-host`) and with `--foreground`. It is refused with `--no-open`, `--print-url`,
  `--set-password`, `--print-cert`, `--service`, and the launcher flags.
- **Start or find the service.** As `cli::start` does today. Then, instead of `open_browser`,
  it mints a ticket and opens the app window on `<base URL>#k=<ticket>`.
  - It does this whether or not a TTY or `DISPLAY` is present. A launcher has neither TTY nor
    always `DISPLAY` in its env check, and the window manager starts it.
  - `CLAUDIO_NO_BROWSER=1` still means no window at all.
  - With `--foreground`, the same happens once the server listens.
- **Finding the browser** is a pure function over the env and a file-exists check, unit-tested.
  It tries, in order:
  1. `$CLAUDIO_BROWSER` when set: an executable path or a name looked up in `Config::path`.
  2. On Linux, the first of `google-chrome`, `google-chrome-stable`, `chromium`,
     `chromium-browser`, `brave-browser`, `microsoft-edge`, `microsoft-edge-stable` found on the
     login-shell `PATH`.
  3. On macOS, the executable inside the first of `Google Chrome.app`, `Chromium.app`,
     `Brave Browser.app`, `Microsoft Edge.app` under `/Applications` or `~/Applications`.
- **Spawning.** argv is `--app=<url>`, plus `--class=claudio` on Linux. stdin, stdout and stderr
  are null, and the child is not waited for, as `open_browser` does. It uses the browser's
  default profile, so the existing cookie, notification permission and settings apply.
- **No such browser.** It falls back to `open_browser` on the ticket URL and prints one line:
  "no Chromium-family browser found: opened the default browser (an app window needs Chrome,
  Chromium, Brave or Edge)".
- **Remote.** A stored `[web]` with `--remote` gives the `https://127.0.0.1:<port>/` base URL.
  The browser handles the self-signed certificate as it does today.

### 3. `claudio web --install-launcher` / `--remove-launcher`

These are one-shot commands, refused with any other flag. They point at `std::env::current_exe()`
canonicalized. Files are written with `paths::write_atomic`; every path is under the user's home
or `XDG_DATA_HOME`. Running it again replaces the files, and removing what is absent succeeds.

- **Linux:**
  - `$XDG_DATA_HOME/applications/claudio.desktop` (default `~/.local/share`) with
    `Type=Application`, `Name=claudio`, `Exec="<exe>" web --app`, `Icon=claudio`,
    `StartupWMClass=claudio`, `Categories=Development;` and `Terminal=false`;
  - icons in `$XDG_DATA_HOME/icons/hicolor/{scalable,192x192,512x512}/apps/claudio.{svg,png}`.

  The exe path is quoted as the Desktop Entry spec requires. Characters the spec reserves are
  escaped.
- **macOS:** `~/Applications/claudio.app` containing:
  - `Contents/Info.plist` (`CFBundleName` claudio, `CFBundleIdentifier` `io.vocdoni.claudio`,
    `CFBundleExecutable` claudio, `CFBundleIconFile` claudio);
  - `Contents/MacOS/claudio`, a 0755 POSIX shell script that runs `exec "<exe>" web --app`;
  - `Contents/Resources/claudio.icns`, built from the embedded PNGs (an icns made of PNG
    entries).

  A bundle created locally has no quarantine attribute, so Gatekeeper does not block it. This is
  unverified on current macOS: the plan includes a manual check by the owner.
- They print what they wrote or removed.

### 4. PWA polish

- Commit `icons/claudio-192.png`, `icons/claudio-512.png` and `icons/claudio-maskable-512.png`.
  They are rendered once from the SVG, whose background becomes `#0b0e12`. The plan names the
  tool.
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

## Compatibility and rollout

- **Page and server** ship in one binary. An old page never sees a `#k=` URL, because only a new
  `claudio` mints them.
- **New `claudio web`, old running service** (a daemon not yet replaced): the old server refuses
  the `ticket` body (400). The page then tries `/api/me` and shows its login, or is already
  signed in through the cookie. Docs say `claudio daemon restart` if it persists. No crash, and
  no token leak.
- **The web protocol** stays version 2. The login body gains an optional alternative, which is
  additive.
- **Rollback:** an older claudio ignores `<runtime>/web-tickets/`. Launchers keep pointing at the
  exe path, so a downgraded binary still runs from them.

## Failure modes and security

- **A ticket in argv** is readable by other local users. It is worth a session only if they
  redeem it before the browser does, within 30 s and only once. They must also pass
  `token_allowed`, so with `--remote` they must be local. That is the residual risk, against a
  long-lived token today. Docs state it.
- **The ticket directory** is private (0700, owned, not a symlink, via `ensure_private_dir`).
  Ticket files are 0600 and empty. Their names are hashes, so listing the directory reveals no
  usable ticket.
- **Bounded:** 16 live tickets, a 30 s life, the existing login rate limiter and body cap.
- **Launchers** are written only under the user's own directories and never follow a symlink
  out (they use `write_atomic`). An exe path with spaces or quotes is quoted or escaped
  correctly, and a test covers it.
- **The browser** is spawned without a shell, so nothing in the URL is interpreted. The URL is
  built only from the service's own address and the hex ticket.
- **C1:** the token never appears in argv, logs or notices. The ticket appears only in the
  browser's argv, never in logs.

## Acceptance criteria

- [ ] AC1: unit tests of the browser search: `CLAUDIO_BROWSER` wins; the Linux order over a fake
  PATH; macOS bundle paths; none found gives `None`.
- [ ] AC2: unit tests of the argv built for `--app` (`--app=<url>`, `--class=claudio` on Linux),
  and that it never contains `#t=` or the token.
- [ ] AC3: unit tests of minting and redeeming tickets:
  - a ticket redeems once and a second redeem fails;
  - an expired file (mtime over 30 s) fails and is removed;
  - an unknown ticket fails;
  - at most 16 live tickets;
  - the directory is 0700 and files are 0600.
- [ ] AC4: web Rust test (addons/web/tests/web.rs):
  - a minted ticket POSTed as `{"ticket": …}` gives 204 and a cookie, and replaying it gives
    401;
  - with `--remote` and a non-local peer (`CLAUDIO_TEST_REMOTE_PEER`), a valid ticket gives 401
    and counts in the limiter;
  - a body with both `token` and `ticket` gives 400.
- [ ] AC5: web suite test: `claudio web --app` with a fake browser on `PATH` (a script that
  records its argv) starts it once with `--app=http://127.0.0.1:<port>/#k=<64 hex>`, and the
  recorded argv holds no token. The same holds with no TTY.
- [ ] AC6: the existing browser-opening path (`claudio web` with a fake `xdg-open` on `PATH`)
  opens a `#k=` URL and never `#t=`.
- [ ] AC7: launcher tests over a temp `HOME`/`XDG_DATA_HOME`:
  - Linux `.desktop` content, with an exe path containing a space and a quote, and the icon
    files;
  - macOS bundle layout, `Info.plist` keys, script mode 0755 and a valid icns header;
  - removing works and running twice is idempotent.
- [ ] AC8: Playwright: opening `/#k=<valid ticket>` signs in and clears the fragment. A replayed
  or expired ticket shows the expiry message. Zero CSP violations. The manifest lists the PNG
  icons and they are served with `image/png`.
- [ ] AC9: manual checks by the owner, recorded in the ledger. They are not CI gates:
  - (a) the key check in a Chrome `--app` window, run before the docs paragraph is written;
  - (b) `--install-launcher` on the Linux desktop starts the app window from the menu;
  - (c) on a Mac, the `~/Applications/claudio.app` stub opens from Finder.
- [ ] AC10: docs updated as §5. `make check`, `make matrix`, the web suites and `make test-web`
  pass.

## Constraints

- ACCEPTANCE.md throughout:
  - A2/A3: the core names no addon. Browser discovery lives in the web addon, or as a generic
    service in `app::browser`.
  - A8: size baselines. `lib.rs`, `cli.rs`, `ws.rs::dispatch` and `hub.rs` are near their
    limits, so new code goes in new modules.
  - A10: bounded.
  - B2: the protocol doc comes first.
  - C1: no token in argv or logs.
  - C4: secret files are 0600, in private directories.
  - E2: no weakened tests.
- DESIGN.md tokens. The import map, and thus the CSP hash, is unchanged.
- Tests are hermetic. Fake browsers are scripts on a test `PATH`. Never launch the owner's real
  browser, and never touch their daemon, web service or desktop menus.
- Linux and macOS.
- Commits: `type(scope): lowercase summary`, no AI attribution or Co-Authored-By trailers.

## Open questions

- The key check result (AC9a) shapes only the docs paragraph. It does not block the build.
