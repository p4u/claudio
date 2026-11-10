# `claudio web`

`claudio web` serves the session manager to a browser. It is one more client of the claudio
daemons, like the TUI: the same sessions, the same hosts (local and over SSH), the same
`config.toml`. Sessions keep running when you close the tab. The wire contract between the
server and the page is [web-protocol.md](web-protocol.md), version 2; the design is in
[specs/2026-11-08-web-ui-design.md](specs/2026-11-08-web-ui-design.md). `claudio web` is the
web addon (docs/addons.md, "The web addon"): it reaches the other addons only through the
registry, and the latex and mermaid lenses it renders are its own share of every launch.

**Protocol 2 and the addons.** What an addon shows comes as data and is drawn by generic
components: a session's `segments` (the Vocdoni addon's route, credential and 5h/7d windows)
in the top bar and the session card, an addon's **panel** (pages of blocks, windows, actions;
the Vocdoni addon's "Proxy stats" on `Alt+s`) in a dialog, and an addon's **checklist** (a
plugin's setup) when a launch answers `review`: Save keeps the choices and spawns, Skip spawns
without that plugin this once, Cancel spawns nothing. The wizard has one select per launch
option group and a checkbox per labelled launch flag (`addons.list`). The ops are `addons.*`;
the version-1 `proxy.*` and `plugins.*` ops are gone, and a tab of an older page reloads once
through the version handshake. Addon text is rendered as text only.

## Usage

```bash
claudio web                      # the daemon serves on 127.0.0.1:7337; open the browser, exit
claudio web --listen 127.0.0.1:9000 --no-open
claudio web status               # whether it runs, where, and why not (exit 0 while it runs)
claudio web stop                 # stop it, and keep it off
claudio web --foreground         # serve from this terminal instead, until Ctrl+C
claudio web --print-url          # print the sign-in URL (stored settings and token) and exit
claudio web --rotate-token       # make a new token: every browser signs in again
claudio web --set-password       # set the password other machines log in with (asks twice)
claudio web --remote             # TLS on 0.0.0.0:7337 for other machines; needs the password
claudio web --remote [::]:8443   # IPv6 and IPv4, port 8443
claudio web --print-cert         # the certificate, to install as trusted on a laptop or phone
claudio web --remote --rotate-cert   # a new certificate (browsers warn once more)
```

It prints `claudio web: http://127.0.0.1:7337/#t=<token>` and exits; the server keeps running in
the background (next section). Open that URL once: the page posts the token to `/api/login`,
gets a cookie and removes the token from the address bar. Later visits need no token while the
sign-in lives: until it goes unused for 30 days (an open tab counts as use), across server and
daemon restarts and closed browsers. The palette's (`k`) **Sign out** ends this browser's
sign-in; `--rotate-token` (or a new password) ends every browser's. Sign out closes every tab
opened with that sign-in, and a page whose sign-in has ended shows the login. `claudio web --rotate-token` restarts the web service, which closes every tab.

Flags: `--listen ADDR` (loopback only: `127.0.0.0/8`, `::1`, `localhost`; default
`127.0.0.1:7337`), `--allow-host NAME[:PORT]` (repeatable), `--no-open`, `--print-url`,
`--rotate-token`, `--foreground`. `CLAUDIO_NO_BROWSER=1` also stops it from opening a browser.

### The web service

`claudio web` checks its flags in the terminal, saves them in `config.toml`, has this host's
daemon start the server, prints what a start prints (with `--remote`, the certificate banner on
stderr too), opens the browser and exits. The daemon runs `claudio web --service` from `[web]`:

```toml
[web]
enabled = true
listen = "127.0.0.1:7337"         # or: remote = "0.0.0.0:7337" (TLS, the password)
allow_hosts = ["claudio.example.org"]
```

- `--listen`, `--remote` and `--allow-host` replace the stored settings as one unit (what you do
  not give takes its default); with none of them, `claudio web` uses the stored ones. The one-shot
  flags are never stored. `[web]` is checked like the flags: a hand-edited
  `listen = "0.0.0.0:7337"` fails with the flag's message and never binds; a hand edit takes
  effect at the next `claudio web` or daemon start.
- While `enabled = true`, every start of the daemon starts the web, and the daemon starts it again
  when it exits (after 1 s, doubling to 60 s while it keeps failing). A new build takes over with
  the daemon (`claudio upgrade`, `make dev`, or an outdated idle daemon being replaced). After a
  reboot it comes back when the daemon next starts: the next `claudio` of any kind. No systemd
  unit or launchd agent is installed, so it survives a logout only where the daemon does.
- `claudio web stop` turns it off (`enabled = false`). `claudio web status` shows whether it
  runs (its pid), the URL without the token, with `--remote` the certificate's SHA-256
  fingerprint, or why it failed and its log, `<runtime>/web.log` (the server's stderr; 2 MiB at
  most with `web.log.1`; never the token). It exits 0 while the web runs, 3 when it does not,
  and 1 when a running daemon did not answer.
- `claudio web --rotate-token`, `--print-url --rotate-token` and `--set-password` restart a
  running service at once: every browser signs in again.
- The server runs with the daemon's environment, not the terminal's that ran `claudio web`: its
  SSH hosts use the daemon's `SSH_AUTH_SOCK` (stale or absent when the daemon was started over ssh
  or by `systemd-run`), and `CLAUDIO_*` set in the terminal do not reach it. Run
  `claudio daemon restart` from a shell with the environment you want, or use `--foreground`.
- A daemon older than this claudio (or a build without the web addon) has no web service:
  `claudio web` says so, keeps `[web]` saved, and exits 1. `claudio daemon restart` when its
  sessions are idle, or `claudio web --foreground`.
- **`--foreground`** keeps the old behaviour exactly: the server runs in this terminal until
  Ctrl+C and never reads or writes `[web]`. A script or unit that ran `claudio web` as a
  long-running process adds `--foreground`.

`--remote [ADDR][:PORT]` serves TLS for other machines (below). Its value is `ADDR`,
`ADDR:PORT`, `:PORT`, `[V6]` or `[V6]:PORT` (a bare IPv6 address needs brackets, `[::]:7337`);
without one it listens on `0.0.0.0:7337`, IPv4 only. It is the one listener: `--remote` with
`--listen` is refused, and an `--allow-host` port must be the `--remote` port. `--rotate-cert`
(with `--remote`) makes a new certificate. `--set-password` and `--print-cert` take no other
argument and exit.

### SSH hosts

A host is added by name or `user@host`, from the hosts panel or Alt+n's "Another host…" (the
TUI's Alt+n takes a typed host too). It needs no `~/.ssh/config` entry, but key-based ssh:
claudio runs ssh with `BatchMode`, so a password is never asked. The first connection records
the host's key (`StrictHostKeyChecking=accept-new`); a changed key is refused with ssh's message.
A host that cannot be reached shows ssh's own error (`ssh failed (exit N): …`, such as "Could
not resolve hostname").

### From another machine

Without `--remote` the server never listens on a non-loopback address. Three ways to reach it:

- **SSH tunnel** (the simple one): `ssh -L 7337:127.0.0.1:7337 host`, then open the URL on the
  laptop.
- **`--remote`**: claudio serves TLS itself, and other machines log in with a password (next
  section).
- **A TLS reverse proxy** on the same host (Caddy, nginx) that forwards to the loopback port.
  Tell the server the name the browser uses: `claudio web --allow-host claudio.example.org`. That
  name is then accepted in `Host` and `Origin`, and the cookie gets `Secure`. The proxy must pass
  WebSocket upgrades on `/ws` and keep the `Host` header. If the proxy is not on 443, name its
  port: `--allow-host claudio.example.org:8443`. Keep using plain `--listen` with it; a proxy
  in front of `--remote` is unsupported, and one that rewrites `Host` to a loopback name would
  make every proxied browser local (a token holder), so never configure that.

### `--remote`: TLS and a password

1. **Set the password**: `claudio web --set-password`. It asks twice and never echoes; 8 to
   1024 bytes. It restarts a running web service at once, which signs every browser out; a
   `--foreground` server keeps the old password until it restarts (that restart signs every
   browser out).
2. **Start the server**: `claudio web --remote`. Without a password it exits before it saves
   anything, makes a certificate or opens a port. On stderr it prints `TLS certificate SHA-256
   AB:CD:…` (at every start), the names the certificate covers, and the token link for this machine
   (`https://127.0.0.1:7337/#t=…`); on stdout, the URL for other machines (no token). If a
   firewall (ufw, firewalld, macOS's) blocks the port, open it. Another machine may not
   resolve the host name: use `<host>.local` or the machine's address. A name the certificate
   does not cover is named on stderr; `--rotate-cert` adds it (the fingerprint changes).
3. **On the other machine**, open `https://<host>.local:7337/`. The browser warns that the
   certificate is not trusted. **Check the fingerprint** before you accept it: Chromium and
   Brave: "Not secure" → Certificate details → SHA-256 fingerprint; Firefox: Advanced → View
   Certificate → SHA-256. It must equal the printed one. Then log in with the password.
4. **Or install the certificate** (no warning, and the install-as-app button appears):
   `claudio web --print-cert > claudio.pem`, copy it to the device, and add it to the trust
   store. *These steps are checked by hand in AC9(b); not yet checked on any platform.* The
   certificate is a self-signed server certificate, not a CA, and some trust stores may refuse
   it; then use the exception path (step 3).
   - Firefox: Settings → Privacy & Security → Certificates → View Certificates → Servers → Add
     Exception, or Authorities → Import. *Not yet checked.*
   - Chromium on Linux: Settings → Privacy and security → Security → Manage certificates →
     Import. *Not yet checked.*
   - macOS: Keychain Access → System → import, then Get Info → Trust → Always Trust. *Not yet
     checked.*
   - Android: Settings → Security → Encryption & credentials → Install a certificate → CA
     certificate. *Not yet checked.*
   - iOS: open the file, install the profile, then Settings → General → About → Certificate
     Trust Settings → turn it on. *Not yet checked.*
5. **With the exception alone** (AC9(a), *not yet checked*): Chromium keeps the exception 7
   days, then asks again (Firefox until you remove it); the install-as-app affordance is not
   expected; Capture all keys and copy still work, because the page is a secure context. On a
   phone the warning cannot show the fingerprint: install the certificate instead of clicking
   through.

**Installed window.** Chromium and Safari can install the page as an app (Install, or Add to
Dock). An app window has no tab strip, so `Ctrl+W`, `Ctrl+T`, `Ctrl+N` and friends stay with
the terminal in it, and the page's own keys are not fighting the browser's.

## Security model

- **Loopback only, unless `--remote`.** `--listen` refuses anything else. With `--remote` there
  is one TLS listener on the address given, and every request that is not local needs the
  password. Otherwise remote access goes through SSH or a TLS proxy you run.
- **Local requests.** A request is local when its TCP peer is a loopback address (from the
  connection, never a header) **and** its `Host` is a loopback form (`127.0.0.1`, `localhost`,
  `[::1]` at the port). Without `--remote` the token works for every request; with it, only for
  local ones, so a browser on another machine (or behind a proxy that keeps `Host`) never logs
  in with the token.
- **The password** (`--remote` only). An argon2id hash (m=19 MiB, t=2, p=1) in
  `~/.config/claudio/web-password`, mode 0600; 8 to 1024 bytes (the length counts bytes). It is
  never in argv, a log, a
  notice or the page's storage (the page clears the field before it posts it). Whoever has it
  can type into your sessions.
- **The certificate** (`--remote` only). A self-signed P-256 server certificate (10 years) and
  its key in `~/.config/claudio/web-tls/` (0600 files in a 0700 directory), made on the first
  start and kept. Its SHA-256 fingerprint is printed at every start. `--rotate-cert` replaces
  it; a missing half, a foreign mode, a symlink or a key that does not match is refused (naming
  `--rotate-cert`), never replaced silently. No `Strict-Transport-Security` header, on purpose:
  it would stop a browser from accepting the self-signed certificate later.
- **Limits.** Login attempts are reserved before any check: 5 per address (an IPv6 /64 counts
  as one) per minute, and 30 a minute for all password attempts and non-local token attempts
  together; past a limit the answer is `429`. Password checks run 4 at once; one that waits 2 s
  for a slot answers `503` and does not count. With `--remote`: a TLS handshake gets 10 s and
  64 run at once, 8 at once per address; 256 connections in all, 32 per address; the headers
  of a connection's first request must arrive within 30 s; an idle connection afterwards keeps
  its slot until the browser closes it.
- **Accepted risks.** The first fingerprint check is yours: a browser remembers the exception
  per host and certificate, and a changed certificate warns again; clicking through a changed
  one without checking can be attacked. An attacker who keeps the global window full locks
  every remote password login (yours included) while the attack lasts; loopback use is
  unaffected. Many addresses together can fill the 256 connection slots while an attack lasts.
- **The token is a key.** It lives in `~/.config/claudio/web-token` (mode 0600, 64 hex),
  is reused across runs and is compared in constant time. Treat it like an SSH private key:
  whoever has it can type into your sessions. `--rotate-token` replaces it. `claudio web` prints
  it on stdout, from the token file, and so does `--print-url`; nothing else does (never a log,
  the web service's included, a query string, a WebSocket frame or an HTTP body). With
  `--remote` it is printed on stderr instead, in the link for this machine (when the listen
  address is loopback or every interface), and by `--print-url`, and accepted only from local
  requests.
- **Cookie.** `claudio_web=<64 hex>; Max-Age=34560000; HttpOnly; SameSite=Strict; Path=/`, plus
  `Secure` with `--remote` or when `--allow-host` is set. 400 days is the longest a browser keeps
  a cookie; the server decides when a sign-in ends: 30 days unused (an open tab marks it used
  once an hour), a sign-out, the least recently used of more than 64, or a server start after
  the token, the password or the mode (loopback or `--remote`) changed. Sign-ins are kept in
  `~/.config/claudio/web-sessions.json` (0600) as the SHA-256 of each cookie and its last use,
  never a cookie: a copy of the file signs no one in. The file is bound to an epoch, the SHA-256
  of the mode, the token and the password file as read at the server's start, so a new token or
  password, or the other mode, empties it at the next start. A stolen cookie is worth more than
  before (up to 30 days of disuse); sign out of a browser you no longer trust (its open tabs close), or rotate the token: with the web service,
  `claudio web --rotate-token` restarts it and closes every tab. Two servers on one config dir
  (a `--foreground` one beside the service) share the file unlocked: one may forget a sign-in
  the other made, or bring back one the other signed out (it still holds it in memory); with
  two, a sign-out is sure only when made on each, or with `--rotate-token` and a restart of both.
- **Host and Origin.** Every request needs an accepted `Host` (the listen address and the
  loopback names with the port, plus `--allow-host` names), else `421`. With `--remote` the
  accepted `Host`s are also any IP literal at the port, this machine's host name and
  `<host>.local`, and the `Origin` must be `https://`. A `POST` or a WebSocket upgrade also needs an
  accepted `Origin`, else `403`. `/ws` and `/api/*` need the cookie, else `401`; `/api/me`'s
  `401` lists the login methods the request may use. This stops DNS rebinding and cross-site
  requests.
- **CSP.** `default-src 'self'; script-src 'self' 'sha256-<import map>'; …`, with
  `frame-ancestors 'none'`, `object-src 'none'` and `connect-src 'self'`, plus `nosniff` and
  `Referrer-Policy: no-referrer`, on every response. The only inline script is the import map
  in `index.html`; `build.rs` hashes its exact text into the CSP. No inline handlers, no `eval`,
  no CDN: the page runs from the files embedded in the binary. The e2e asserts the browser
  sees zero `securitypolicyviolation` events.
- **Secrets never reach the page.** Proxy tokens, plugin environment values, `Spawn.env`,
  hook tokens and socket paths stay on the server; they travel only inside spawn requests to the
  daemon. Browser DTOs are built field by field.
- **Untrusted text.** Everything model-written is rendered as text, except answers: markdown
  goes through DOMPurify, formulas through KaTeX (`trust: false`), diagrams through mermaid
  (`securityLevel: "strict"`) and DOMPurify's SVG profile.
- **What is logged** (INFO, to stderr): `session.shell`, `session.spawn`, `session.kill`,
  `session.approve` and `extensions.action`, with the op, the host and the session id; `web:
  login refused` with the status and whether the request was local. Never values, typed text,
  tokens or passwords.
- **State.** The web writes `web-state.json` (layout, order, each session's `launch`, still
  with its `proxy`/`first_party` for one release, remote hosts), `web-sessions.json` and
  `web-token` in the config dir (with `--set-password` and `--remote`, also `web-password` and
  `web-tls/`), and `claudio web` writes `[web]` in `config.toml`. It reads the TUI's
  `state.json` and never writes it.

## Keys

The page has two modes, shown by the badge in the top bar. **TERM**: focus is inside a
terminal and every key goes to the session; the app takes one key only. **APP**: focus is
anywhere else and single keys drive the app. Clicking a terminal (or Enter on a session) enters
TERM; `Ctrl+Shift+Space` leaves it.

| Key | Mode | Action |
|---|---|---|
| `Ctrl+Shift+Space` | TERM | Leave the terminal (the only key the app takes there) |
| `k` | APP | Command palette |
| `Ctrl+K` | APP, text fields | Command palette (not inside a terminal) |
| `a` | APP | Jump to the next session that needs you |
| `n` | APP | New claude session (wizard) |
| `t` | APP | New terminal tab |
| `/` | APP | Search conversations |
| `1` `2` `3` `4` | APP | Focus pane 1 to 4 |
| `↓` / `↑` | APP | Next / previous session in the rail |
| `r` | APP | Reader: the session's last answers |
| `g` | APP | Git changes |
| `?` | APP | Help (this list) |
| `Enter` | APP | Type into the focused terminal |
| `Esc` | APP | Close the open panel |

Everything else is a button, the palette (`k`) or a menu: rename, restart, kill, terminal tab,
history, addon panels (proxy stats), snippets, extensions, hosts, settings.

### The TUI's keys, anywhere

These are the manager's Alt keys. They work in a terminal too: the page takes them before
xterm and the browser do (matched on the physical key, so any layout works; AltGr is not Alt).
`Alt+q` and everything not listed goes to the session.

| Key | Action |
|---|---|
| `Alt+←` / `Alt+→` | Previous / next session in the focused pane (rail order) |
| `Alt+Shift+1`…`9`, `0` | Go to session 1 to 10 |
| `Alt+n` | New claude session |
| `Alt+c` | Terminal beside, in the session's folder |
| `Alt+r` / `Alt+x` / `Alt+e` | Rename / kill (asks) / restart the focused session |
| `Alt+a` | Next session that needs you |
| `Alt+g` | The board |
| `Alt+s` / `Alt+l` / `Alt+u` | Addon panel (proxy stats) / history / reader drawer |
| `Alt+i` | Extensions screen |
| `Alt+h` | Help |
| `Alt+z` | Command palette (lists the extension actions) |
| `Alt+o` | Compose: a pop-up editor for a long or multi-line prompt (web only) |
| `Alt+Shift+F` | Fill the window with the focused pane; again to go back (web only) |

`Shift+Enter` in a terminal inserts a newline in claude (it sends ESC CR, like `Alt+Enter`).

**Browser keys.** In a normal tab the browser keeps `Ctrl+T`, `Ctrl+W`, `Ctrl+N` and `Ctrl+Tab`.
Two ways round it: **Capture all keys** (Settings, Help, or the palette) goes fullscreen and
locks the keyboard where the browser supports it (Chromium, Brave, Edge; hold Esc to leave
fullscreen); or install claudio as an app (Brave or Chrome menu, "Install claudio...") and
capture there too. `Alt+←` / `Alt+→` are the browser's history keys, claudio takes them first.

**Key check.** Help has a Key check section: turn it on and each key press shows its `key`,
`code`, modifiers and what claudio does with it. Nothing is sent to a session meanwhile.

## TUI parity

One row per capability of the TUI. `Test` is the e2e test title (`addons/web/web/e2e/tests`) or the
Rust test in `addons/web/tests/web.rs`; rows without an automated test say how they were checked.

| Capability | TUI key | Web action | WS op(s) | Test |
|---|---|---|---|---|
| Sessions on many hosts | wizard host list | the top bar's Hosts menu (one row per host), host panel, Add host… | `hosts.list`, `hosts.candidates`, `hosts.connect`, `hosts.disconnect` | `web_hosts_report_daemon_features`; ssh hosts: manual check on a real host |
| Terminal panes (1, 2, 4) | `Alt+←/→` | pane grid buttons, `1`-`4`, drag a card | `session.attach`, `session.detach`, `session.resize`, `layout.save` | `a pane shows the terminal and typed keys echo`; `web_attach_snapshot_input_and_resize`; `web_two_tabs_share_one_attach` |
| Terminal tabs | `Alt+c` | `t`, "Terminal beside" in a pane | `session.shell` | `a terminal tab runs echo ok`; `web_session_lifecycle_shell_rename_kill` |
| New-session wizard: dirs | `Alt+n` | `n`: folder field, recent folders, browser | `fs.list`, `fs.projects`, `session.spawn` | `the wizard starts a session in a chosen dir` |
| Wizard: resume | `Alt+n` resume list | "Conversation" field | `claude.sessions`, `session.spawn` (`resume`) | `web_reader_and_search_read_a_fixture_transcript`; `search finds a fixture conversation` |
| Wizard: launch options (proxy profile) | wizard proxy choice | one select per launch option group ("Proxy") | `addons.list`, `session.spawn` (`launch`) | `web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser`; `the wizard starts a session in a chosen dir` |
| Wizard: launch flags (first-party) | wizard checkbox | one checkbox per labelled flag ("First-party session") | `session.spawn` (`launch.flags`) | `web_addons_ops_refuse_unknown_addons_and_bad_args` (the rule); the UI box: manual check |
| Opening prompt | wizard prompt | "Opening prompt" | `session.spawn` (`prompt`) | `web_opening_prompt_is_pasted_after_bracketed_paste_mode` |
| Rename | `Alt+r` | pencil in the pane header | `session.rename` | `rename, kill and restart a session` |
| Stop (kill) | `Alt+x` | the stop icon `⏻` in the pane header (asks) | `session.kill` | `rename, kill and restart a session` |
| Restart (resume / new) | `Alt+e` | Restart / New conversation (asks) | `session.respawn` | `rename, kill and restart a session`; `web_respawn_without_a_known_proxy_asks` |
| Approve / deny a prompt | (type in the session) | Enter / Esc in the pane header or the board row | `session.approve` | `a permission prompt shows its text and Approve resolves it`; `an elicitation row waits for you but offers no Approve`; `web_approve_is_guarded_by_state_kind_and_seq` |
| Prompt box | (type in claude's own input) | Compose editor (`Alt+O`, pane header, palette): Insert pastes, Send pastes and presses Enter; Settings > "Show a composer under each pane" brings back an inline one | `session.paste` | `Insert pastes without submitting, Send submits…`; `save and insert a snippet`; `web_paste_reaches_the_pty_bracketed` |
| Fill the window | (none) | pane header button, `Alt+Shift+F`, palette ("Fullscreen browser" also goes browser-fullscreen) | none (this browser tab only) | `Alt+Shift+F fills the window and again restores it…` |
| Reader with formulas and diagrams | `Alt+u` (source text only) | `r`: Answers, Formulas | `reader.answers`, `lens`/`latex` events, `extensions.render` | `the reader renders KaTeX and mermaid with no CSP violation` |
| Git history | `Alt+l` | `g`, History tab | `git.log`, `git.commit`, `git.diff` | manual check (history list and commit view, probed in a browser on a fixture repo) |
| Git working tree | `Alt+l` | `g`, Changes tab | `git.status`, `git.worktree_diff` | `Changes lists a modified file and shows its diff`; `web_git_status_and_worktree_diff` |
| Proxy stats (an addon panel) | `Alt+s` | `Alt+s` or the palette ("Proxy stats"): the panel dialog, pages and windows | `addons.list`, `addons.panel` | `proxy stats render the fixture window`; `web_addons_panel_renders_the_fixture_and_never_leaks_the_token` |
| Subscription switch (a panel action) | `Alt+s` → `n` | the panel's action button, or the palette once the panel was read | `addons.action` | `web_addons_action_switch_uses_server_state`; the fake proxy has no `/session` route: manual check on a real proxy |
| Search | `Alt+z` `s` | `/` | `search` | `search finds a fixture conversation`; `web_reader_and_search_read_a_fixture_transcript` |
| Snippets | `Alt+z` `p` / `P` | palette: Insert snippet, Save last prompt | `snippets.list`, `snippets.save`, `reader.last_prompt` | `save and insert a snippet`; `web_snippets_save_and_list` |
| Extensions screen | `Alt+i` | Settings → Manage… | `extensions.list`, `extensions.toggle`, `extensions.action` | `toggling an extension rewrites its config.toml line`; `web_extension_toggle_edits_config_and_is_reread`; `web_external_extension_action_runs_its_program` |
| Plugin setup (an addon checklist) | first spawn, extensions screen | the checklist a launch's `review` asks for (Save / Skip / Cancel), "Set up" on an addon row | `addons.checklist`, `addons.checklist.save`, `addons.rows`, `session.spawn` (`defer`) | `a plugin without choices asks for its checklist before spawn and Skip spawns without it`; `web_spawn_with_launch_needs_review_then_defer_spawns_without_the_plugin` |
| claude install / update | update prompt | Hosts menu → host row → panel → Update claude; toasts | `claude.update`, `claude.skip`, `claude` events | `claude update shows progress and completion` |
| Host stats | status bar cpu / mem | footer line per host | `host_stats` events | manual check (footer shows `local cpu N% · mem N%`) |
| Notifications | OSC 9 | desktop notification, tab title count | (client side, from `session` events) | `a hidden session needing approval notifies once` |
| Next attention | `Alt+a` | `Alt+a`, `a` | (client side) | manual check (`a` focuses the session that needs approval); approve path: `a permission prompt shows its text…` |
| Help | `Alt+h` | `Alt+h`, `?` | (client side) | `the help has the key check and describes keys without sending them` |
| Overview of all sessions | `Alt+g` | `Alt+g`, Board | (client side) | `the board lists sessions with their states` |
| Login and security | n/a | token URL, cookie; password form with `--remote`; palette: Sign out | `/api/login`, `/api/me`, `/api/logout` | `logs in from the token URL and clears the fragment`; `over --remote the page asks for the password and the session works`; `the palette signs this browser out, and its cookie stays refused`; `web_auth_gates_api_and_ws` and the other `web_*auth*` tests; `web_remote_login_rules`, `web_remote_serves_tls_with_a_kept_certificate`, `web_sign_ins_and_sign_outs_survive_a_killed_server` |
| Layout and style | n/a | dark only ([DESIGN.md](../DESIGN.md)); Settings → Text size | `layout.save` | `the page follows DESIGN.md`; `web_order_and_layout_persist_in_web_state` |
| Keys | `Alt+…` (see Keys) | the same Alt keys, `Ctrl+Shift+Space`, `k` | (client side) | `Ctrl+Shift+Space then k opens the palette`; `Alt+u in a terminal opens the reader and sends nothing to claude`; `Alt+a focuses the session that needs attention, Alt+Right switches session` |
| Newline in the prompt | `Shift+Enter` | `Shift+Enter` in a terminal | `session.input` | `Shift+Enter puts ESC CR into the input, Enter submits` |
| Browser-reserved keys | n/a | Capture all keys, installable app | (client side) | `the settings have Capture all keys and the manifest is served` |

## Developing the page

The page is `addons/web/web/`, the server `addons/web/src/` (the claudio-web crate, the `web`
feature of the binary):

```bash
node --test addons/web/web/tests/*.test.mjs   # pure logic, no browser (Node 22)
node addons/web/web/dev/mock-server.mjs       # the page against a mock server on :7338 (MOCK_MANY=1, MOCK_FOREIGN=1)
cargo test --test web --test web_manager -- --test-threads=1   # the server, with a fake claude
make test-web                                 # node tests, then Playwright against a debug build
```

A debug build run as `CLAUDIO_WEB_DIR=addons/web/web claudio web --foreground` serves the files
from disk, so edits show on reload (without `--foreground` the daemon's service runs, with the
daemon's environment); every build embeds them gzipped (`addons/web/build.rs`).
`make test-web` needs the Playwright browsers in `~/.cache/ms-playwright`
(`npx playwright install chromium` once, in `addons/web/web/e2e`).

A file added or removed directly under `addons/web/web/` is not noticed by cargo until another
embedded file changes or `build.rs` is touched (the directory itself is not watched because of
`addons/web/web/e2e/node_modules`).

## The list, state icons, hover info and text size

- **Rail**: a static list of sessions (host, then repo when it holds two or more). Every card has
  one fixed height: state icon, name, host chip (only with more than one host connected) and age.
  Hover and selection change colours only. There is no "Needs you" queue; `Alt+a`/`a` still jump
  to the next session that needs you, and Approve/Deny sit in the pane header and the board row.
- **State icon** (`stateClass` in `addons/web/web/app/queue.js`): working (a green `●` LED that pulses
  softly, still when the system asks for reduced motion), waiting (violet `?`: a permission
  prompt, a question, a plan to approve), stopped (steel `■`: finished or not running), error
  (coral `✗`). Shell sessions show `$`. Every icon has a text label or tooltip; the board shows
  the state as a badge with its name.
- **Hover info**: pointing at (or focusing) a rail item or board row for 800 ms shows its facts in
  a popover beside it; a key press or leaving hides it. Text only.
- **Pane header**: `✕` closes the pane (the session keeps running, stays in the rail); `⏻` stops
  the session (asks).
- **Text size** (Settings): Small, Normal (default), Large scale the interface through the
  `--fs-*` tokens (the rail keeps its own, smaller scale); the terminal font size (default 15 px) is a
  separate setting.
- **Style**: the page follows [DESIGN.md](../DESIGN.md), dark only: Void, Carbon and Graphite
  surfaces with 1px Slate hairlines, radius 1px, no shadows, weight 400, JetBrains Mono for
  labels, tabs, badges and buttons, Inter for prose, Phosphor Green only for active and status.
  Every colour, radius and font is a token in `addons/web/web/styles/tokens.css`, the terminal's ANSI
  colours included; no ligatures anywhere.
