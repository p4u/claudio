# `claudio web`

`claudio web` serves the session manager to a browser. It is one more client of the claudio
daemons, like the TUI: the same sessions, the same hosts (local and over SSH), the same
`config.toml`. Sessions keep running when you close the tab. The wire contract between the
server and the page is [web-protocol.md](web-protocol.md); the design is in
[specs/2026-11-08-web-ui-design.md](specs/2026-11-08-web-ui-design.md).

## Usage

```bash
claudio web                      # serve on 127.0.0.1:7337 and open the browser
claudio web --listen 127.0.0.1:9000 --no-open
claudio web --print-url          # print the sign-in URL for the stored token and exit
claudio web --rotate-token       # make a new token (every browser signs in again)
```

On start it prints `claudio web: http://127.0.0.1:7337/#t=<token>`. Open that URL once: the
page posts the token to `/api/login`, gets a cookie and removes the token from the address bar.
Later visits need no token while the cookie lives (24 h idle).

Flags: `--listen ADDR` (loopback only: `127.0.0.0/8`, `::1`, `localhost`; default
`127.0.0.1:7337`), `--allow-host NAME[:PORT]` (repeatable), `--no-open`, `--print-url`,
`--rotate-token`. `CLAUDIO_NO_BROWSER=1` also stops it from opening a browser.

### From another machine

The server never listens on a non-loopback address. Two ways to reach it:

- **SSH tunnel** (the simple one): `ssh -L 7337:127.0.0.1:7337 host`, then open the URL on the
  laptop.
- **A TLS reverse proxy** on the same host (Caddy, nginx) that forwards to the loopback port.
  Tell the server the name the browser uses: `claudio web --allow-host claudio.example.org`. That
  name is then accepted in `Host` and `Origin`, and the cookie gets `Secure`. The proxy must pass
  WebSocket upgrades on `/ws` and keep the `Host` header. If the proxy is not on 443, name its
  port: `--allow-host claudio.example.org:8443`.

**Installed window.** Chromium and Safari can install the page as an app (Install, or Add to
Dock). An app window has no tab strip, so `Ctrl+W`, `Ctrl+T`, `Ctrl+N` and friends stay with
the terminal in it, and the page's own keys are not fighting the browser's.

## Security model

- **Loopback only.** `--listen` refuses anything else. Remote access goes through SSH or a TLS
  proxy you run.
- **The token is a key.** It lives in `~/.config/claudio/web-token` (mode 0600, 64 hex),
  is reused across runs and is compared in constant time. Treat it like an SSH private key:
  whoever has it can type into your sessions. `--rotate-token` replaces it. It is printed on
  stdout at start and by `--print-url`, and nowhere else (never in a log, a query string, a
  WebSocket frame or an HTTP body).
- **Cookie.** `claudio_web=<64 hex>; HttpOnly; SameSite=Strict; Path=/`, plus `Secure` when
  `--allow-host` is set. Sessions are kept in the server's memory: 24 h idle expiry, at most 64.
  Five failed logins in 60 s lock the login for the rest of that minute (`429`).
- **Host and Origin.** Every request needs an accepted `Host` (the listen address and the
  loopback names with the port, plus `--allow-host` names), else `421`. A `POST` or a WebSocket upgrade also needs an
  accepted `Origin`, else `403`. `/ws` and `/api/*` need the cookie, else `401`. This stops DNS
  rebinding and cross-site requests.
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
  `session.approve` and `extensions.action`, with the op, the host and the session id. Never
  values, typed text or tokens.
- **State.** The web writes `web-state.json` (layout, order, proxy choice per session) and
  `web-token` in the config dir. It reads the TUI's `state.json` and never writes it.

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
history, stats, snippets, extensions, hosts, settings.

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
| `Alt+s` / `Alt+l` / `Alt+u` | Stats / history / reader drawer |
| `Alt+i` | Extensions screen |
| `Alt+h` | Help |
| `Alt+z` | Command palette (lists the extension actions) |

`Shift+Enter` in a terminal inserts a newline in claude (it sends ESC CR, like `Alt+Enter`).

**Browser keys.** In a normal tab the browser keeps `Ctrl+T`, `Ctrl+W`, `Ctrl+N` and `Ctrl+Tab`.
Two ways round it: **Capture all keys** (Settings, Help, or the palette) goes fullscreen and
locks the keyboard where the browser supports it (Chromium, Brave, Edge; hold Esc to leave
fullscreen); or install claudio as an app (Brave or Chrome menu, "Install claudio...") and
capture there too. `Alt+←` / `Alt+→` are the browser's history keys, claudio takes them first.

**Key check.** Help has a Key check section: turn it on and each key press shows its `key`,
`code`, modifiers and what claudio does with it. Nothing is sent to a session meanwhile.

## TUI parity

One row per capability of the TUI. `Test` is the e2e test title (`web/e2e/tests`) or the Rust
test in `tests/web.rs`; rows without an automated test say how they were checked.

| Capability | TUI key | Web action | WS op(s) | Test |
|---|---|---|---|---|
| Sessions on many hosts | wizard host list | the top bar's Hosts menu (one row per host), host panel, Add host… | `hosts.list`, `hosts.candidates`, `hosts.connect`, `hosts.disconnect` | `web_hosts_report_daemon_features`; ssh hosts: manual check on a real host |
| Terminal panes (1, 2, 4) | `Alt+←/→` | pane grid buttons, `1`-`4`, drag a card | `session.attach`, `session.detach`, `session.resize`, `layout.save` | `a pane shows the terminal and typed keys echo`; `web_attach_snapshot_input_and_resize`; `web_two_tabs_share_one_attach` |
| Terminal tabs | `Alt+c` | `t`, "Terminal beside" in a pane | `session.shell` | `a terminal tab runs echo ok`; `web_session_lifecycle_shell_rename_kill` |
| New-session wizard: dirs | `Alt+n` | `n`: folder field, recent folders, browser | `fs.list`, `fs.projects`, `session.spawn` | `the wizard starts a session in a chosen dir` |
| Wizard: resume | `Alt+n` resume list | "Conversation" field | `claude.sessions`, `session.spawn` (`resume`) | `web_reader_and_search_read_a_fixture_transcript`; `search finds a fixture conversation` |
| Wizard: proxy profile | wizard proxy choice | "Proxy" select | `session.spawn` (`proxy`) | `web_spawn_keeps_proxy_and_plugin_secrets_out_of_the_browser` |
| Wizard: first-party | wizard checkbox | "First-party session" | `session.spawn` (`first_party`) | `web_respawn_without_a_known_proxy_asks` (the rule); the UI box: manual check |
| Opening prompt | wizard prompt | "Opening prompt" | `session.spawn` (`prompt`) | `web_opening_prompt_is_pasted_after_bracketed_paste_mode` |
| Rename | `Alt+r` | pencil in the pane header | `session.rename` | `rename, kill and restart a session` |
| Stop (kill) | `Alt+x` | the stop icon `⏻` in the pane header (asks) | `session.kill` | `rename, kill and restart a session` |
| Restart (resume / new) | `Alt+e` | Restart / New conversation (asks) | `session.respawn` | `rename, kill and restart a session`; `web_respawn_without_a_known_proxy_asks` |
| Approve / deny a prompt | (type in the session) | Enter / Esc in the pane header or the board row | `session.approve` | `a permission prompt shows its text and Approve resolves it`; `an elicitation row waits for you but offers no Approve`; `web_approve_is_guarded_by_state_kind_and_seq` |
| Prompt box | (type in the session) | composer under the terminal | `session.paste` | `save and insert a snippet`; `web_paste_reaches_the_pty_bracketed` |
| Reader with formulas and diagrams | `Alt+u` | `r`: Answers, Formulas | `reader.answers`, `lens`/`latex` events, `extensions.render` | `the reader renders KaTeX and mermaid with no CSP violation` |
| Git history | `Alt+l` | `g`, History tab | `git.log`, `git.commit`, `git.diff` | manual check (history list and commit view, probed in a browser on a fixture repo) |
| Git working tree | `Alt+l` | `g`, Changes tab | `git.status`, `git.worktree_diff` | `Changes lists a modified file and shows its diff`; `web_git_status_and_worktree_diff` |
| Proxy stats | `Alt+s` | Stats tab | `proxy.stats` | `proxy stats render the fixture window`; `web_proxy_stats_render_the_fixture` |
| Subscription switch | `Alt+s` → `n` | "Move to next subscription" in Stats | `proxy.credential`, `proxy.switch` | not covered: the fake proxy has no `/session` route; manual check on a real proxy |
| Search | `Alt+z` `s` | `/` | `search` | `search finds a fixture conversation`; `web_reader_and_search_read_a_fixture_transcript` |
| Snippets | `Alt+z` `p` / `P` | palette: Insert snippet, Save last prompt | `snippets.list`, `snippets.save`, `reader.last_prompt` | `save and insert a snippet`; `web_snippets_save_and_list` |
| Extensions screen | `Alt+i` | Settings → Manage… | `extensions.list`, `extensions.toggle`, `extensions.action` | `toggling an extension rewrites its config.toml line`; `web_extension_toggle_edits_config_and_is_reread`; `web_external_extension_action_runs_its_program` |
| Plugin setup | first spawn, extensions screen | setup step before the spawn, Enter on a plugin row | `plugins.list`, `plugins.items`, `plugins.save`, `plugins.status` | `a plugin without choices opens setup before spawn and Skip spawns without it`; `web_plugin_without_choices_is_left_out_until_saved` |
| claude install / update | update prompt | Hosts menu → host row → panel → Update claude; toasts | `claude.update`, `claude.skip`, `claude` events | `claude update shows progress and completion` |
| Host stats | status bar cpu / mem | footer line per host | `host_stats` events | manual check (footer shows `local cpu N% · mem N%`) |
| Notifications | OSC 9 | desktop notification, tab title count | (client side, from `session` events) | `a hidden session needing approval notifies once` |
| Next attention | `Alt+a` | `Alt+a`, `a` | (client side) | manual check (`a` focuses the session that needs approval); approve path: `a permission prompt shows its text…` |
| Help | `Alt+h` | `Alt+h`, `?` | (client side) | `the help has the key check and describes keys without sending them` |
| Overview of all sessions | `Alt+g` | `Alt+g`, Board | (client side) | `the board lists sessions with their states` |
| Login and security | n/a | token URL, cookie | `/api/login`, `/api/me` | `logs in from the token URL and clears the fragment`; `web_auth_gates_api_and_ws` and the other `web_*auth*` tests |
| Layout and style | n/a | dark only ([DESIGN.md](../DESIGN.md)); Settings → Text size | `layout.save` | `the page follows DESIGN.md`; `web_order_and_layout_persist_in_web_state` |
| Keys | `Alt+…` (see Keys) | the same Alt keys, `Ctrl+Shift+Space`, `k` | (client side) | `Ctrl+Shift+Space then k opens the palette`; `Alt+u in a terminal opens the reader and sends nothing to claude`; `Alt+a focuses the session that needs attention, Alt+Right switches session` |
| Newline in the prompt | `Shift+Enter` | `Shift+Enter` in a terminal | `session.input` | `Shift+Enter puts ESC CR into the input, Enter submits` |
| Browser-reserved keys | n/a | Capture all keys, installable app | (client side) | `the settings have Capture all keys and the manifest is served` |

## Developing the page

```bash
node --test web/tests/*.test.mjs   # pure logic, no browser (Node 22)
node web/dev/mock-server.mjs       # the page against a mock server on :7338 (MOCK_MANY=1, MOCK_FOREIGN=1)
cargo test --test web -- --test-threads=1   # the server, with a fake claude
make test-web                      # node tests, then Playwright against a debug build
```

A debug build run with `CLAUDIO_WEB_DIR=web` serves the files from disk, so edits show on
reload; every build embeds them gzipped (`build.rs`). `make test-web` needs the Playwright browsers in `~/.cache/ms-playwright`
(`npx playwright install chromium` once, in `web/e2e`).

A file added or removed directly under `web/` is not noticed by cargo until another embedded
file changes or `build.rs` is touched (`web/` itself is not watched because of
`web/e2e/node_modules`).

## The list, state icons, hover info and text size

- **Rail**: a static list of sessions (host, then repo when it holds two or more). Every card has
  one fixed height: state icon, name, host chip (only with more than one host connected) and age.
  Hover and selection change colours only. There is no "Needs you" queue; `Alt+a`/`a` still jump
  to the next session that needs you, and Approve/Deny sit in the pane header and the board row.
- **State icon** (`stateClass` in `web/app/queue.js`): working (a green `●` LED that pulses
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
  Every colour, radius and font is a token in `web/styles/tokens.css`, the terminal's ANSI
  colours included; no ligatures anywhere.
