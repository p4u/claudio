# Web UI (`claudio web`): design

Status: audited (APPROVE WITH CHANGES, all findings folded in; approval delegated by the owner)   Class: architectural
Owner decisions so far (2026-11-08): build a modern, dynamic web UI with every capability of
the terminal manager plus what a browser enables; style "a bit retro-arcade, not too much";
run the delivery autonomously to the end (the owner delegated spec and plan approval).
Input: a Fable 5.1 persona review ("AI-first developer running 5–15 sessions"), kept at
`~/.cache/claudio/web-ui/persona-round1.md`; its ranked must-haves drive the scope below.

## Problem

The manager is a terminal UI. With many sessions it is blind: the tab bar shows a glyph per
session, so the user switches tab to learn *what* a session waits for (`Alt+a` cycles
attention). It shows one pane at a time, cannot render tables, KaTeX or mermaid properly (the
reader is a cell-grid approximation, `src/tui/reader/`, `src/tui/typeset/`,
`src/tui/diagram/`), and its git view is history only (`src/daemon/git.rs` has log/commit/diff
of commits; `git_status.rs` gives only counts for the status bar, no file list or working-tree
patch). Notifications rely on OSC 9, which many terminals
ignore.

## Goals

1. `claudio web` starts a local web server that is one more client of the claudio daemons
   (local over the Unix socket, remote over the existing SSH bridge), exactly like the TUI.
2. A browser app with parity for every manager capability: sessions on many hosts, terminal
   panes, terminal tabs, the new-session wizard (dirs, resume, proxy profile, first-party),
   rename/kill/restart, the reader with formulas and diagrams, the git viewer, proxy stats and
   subscription switch, search, snippets, extensions and the plugin setup screen, claude
   install/update, host stats, notifications.
3. Browser-native improvements: a live session board, an attention queue that shows what each
   session waits for with inline approve/deny, split panes (1, 2 or 4), a prompt composer, a
   working-tree diff viewer, a command palette, keyboard-first operation with an explicit
   focus model, browser notifications, themes (arcade and plain).
4. Secure by default: loopback only, a secret token exchanged for a cookie, Host and Origin
   checks, a strict CSP, secrets never reach the page.
5. One binary: the UI ships inside `claudio` (no node at build or run time, CI unchanged).

## Non-goals

- No public/LAN listening in this release: non-loopback addresses are refused. Remote use goes
  through `ssh -L` or a reverse proxy on the same host (`--allow-host`).
- No hosted relay, accounts, telemetry, or OIDC (later, possibly via oauth.vocdoni.org).
- No editing of files, no git writes (claudio stays read-only on git), no `-p`/`--api` from
  the UI, no chat-style re-rendering of the terminal (the terminal stays the truth).
- No change to the TUI's behaviour. The TUI keeps working unchanged against new daemons.
- The web server does not write the TUI's `state.json`.

## Current state

- Daemon client API without TUI dependencies: `client::connect`/`connect_ssh`/`request`/
  `take_incoming`/`send_input` (`src/client.rs:148,168,401,385,453`), `Incoming`
  (`src/client.rs:79`). Reconnect bookkeeping `tui::connections::Connections`
  (`src/tui/connections.rs:19-165`, no App dependency). Remote bootstrap
  `remote::bootstrap::ensure_remote` (`src/remote/bootstrap.rs:73`), SSH candidates
  `remote::hosts::candidates` (`src/remote/hosts.rs:76`), local daemon with freshness
  `client::ensure_local_daemon` (`src/client.rs:707`).
- Wire format (`src/proto.rs`): JSON `Msg` tagged by `op`, terminal `D` frames, `Unknown`
  catch-alls for `Msg` (`:316`) and `SessionEvent` (`:857`), `Welcome.features`
  (`DAEMON_FEATURES`, `:354`).
- Session state comes from hooks in `claude::state::Tracker` (`src/claude/state.rs:39`):
  Notification `permission_prompt` etc. → `NeedsApproval`; `idle_prompt` → `NeedsInput`.
  The hook payload's `message`, and the preceding `PreToolUse`'s `tool_name`/`tool_input`, are
  dropped today.
- Spawn composition lives in `App` methods: `send_spawn`/`send_with_proxy`
  (`src/tui/sessions.rs:402,465`), `plugin_parts` (`src/tui/plugins.rs:543`), the
  `SpawnWithProxy` effect (`src/tui/mod.rs:~1560-1625`). Pure pieces are reusable:
  `tui::plugins::compose`/`with_plugins` (`:175,:229`), `proxy::resolve::proxy_env_for`
  (`src/proxy/resolve.rs:111`), `Msg::merge_env`/`with_instructions` (`src/proto.rs:621-664`),
  `extensions::Registry::session_extensions` (`src/tui/extensions/mod.rs:282`).
- Proxy API calls (`src/proxy/api.rs`: config, stats, pool health, models, session
  credential, switch) and plugin list/manifest fetch (`src/proxy/plugins.rs:297,318`) are free
  async functions. Config writers `config::save_extension`/`save_plugin_choices`
  (`src/config.rs:473,591`). Snippets `tui::snippets::{load,save}` (`src/tui/snippets.rs`).
- axum 0.8 is already a dependency (the `--api` server).

## Design

### Overview

```mermaid
flowchart LR
  B[Browser app] -- loopback WS + REST --> W[claudio web]
  W -- Unix socket --> L[local daemon]
  W -- ssh claudio --slave --> R[remote daemons]
  W -- https --> P[claude-proxy]
```

`claudio web` is a long-running process (foreground; Ctrl+C stops it). It holds one
`client::Client` per connected host, a model of every host's sessions, and serves `/` and
static assets, `POST /api/login`, `POST /api/logout`, `GET /ws` (one WebSocket per browser
tab). All app traffic after login goes over the WebSocket.

### Source layout

- `src/web/mod.rs`: `claudio web` entry: args, token, listener, router, shutdown.
- `src/web/auth.rs`: token file, login, cookie sessions, Host/Origin checks, login rate limit.
- `src/web/assets.rs`: embedded assets (generated table), content types, gzip, ETag, CSP.
- `src/web/ws.rs`: the browser protocol: framing, request dispatch, per-tab subscriptions.
- `src/web/hub.rs`: hosts and sessions model, daemon connections, reconnect, attach fan-out,
  event application, broadcasting changes to tabs.
- `src/web/spawn.rs`: spawn/respawn composition: proxy env, plugins (lists, manifests,
  per-host `Applied`, compose), extensions, opening prompt.
- `src/web/proxy.rs`: stats, credentials, subscription switch, DTOs (`Serialize`).
- `src/web/state.rs`: `web-state.json` (session → proxy profile and first-party, board order,
  pane layout), atomic writes, 0600.
- `build.rs`: walks `web/` (excluding `web/tests`, `web/e2e`, `node_modules`), gzips every
  file (build-dependency `flate2`), writes `$OUT_DIR/web_assets.rs`.
- `web/`: the browser app: `index.html`, `app/` (ES modules), `styles/`, `vendor/` (pinned
  third-party builds with their licenses), `fonts/`, `tests/` (node unit tests), `e2e/`
  (Playwright, dev only).

TUI modules the web needs (`connections`, `plugins`' pure fns, `extensions` registry and host,
`snippets`, `state`'s read side) get `pub(crate)` visibility; no TUI logic changes. Logic that
lives in `App` methods (reconnect glue, plugin ask/re-ask, spawn composition) is re-expressed
as async code in `src/web/` rather than extracted, so the TUI is untouched.

### CLI

```
claudio web [--listen 127.0.0.1:7337] [--allow-host NAME]... [--no-open] [--print-url]
            [--rotate-token]
```

- `--listen` must be a loopback address (`127.0.0.0/8`, `::1`, or `localhost`); anything else
  exits 2 with a message pointing to `ssh -L 7337:127.0.0.1:7337 host` and `--allow-host`.
- `--allow-host NAME` adds a Host header value accepted besides `127.0.0.1:<port>`,
  `localhost:<port>`, `[::1]:<port>` (for a reverse proxy such as `tailscale serve`).
- On start it prints `claudio web: http://127.0.0.1:7337/#t=<token>` and opens it with
  `xdg-open`/`open` unless `--no-open` or `CLAUDIO_NO_BROWSER=1` (same rule as proxy login).
- `--print-url` prints the URL of the saved token and exits. `--rotate-token` writes a new
  token and continues.
- `cli::HELP_APPENDIX` documents the subcommand; `CLAUDIO_WEB_DIR` (debug builds only) serves
  `web/` from disk for development.

### Authentication and request checks

- Token: 32 bytes from `/dev/urandom`, hex, in `paths::config_dir()/web-token`, mode 0600,
  created on first start, reused afterwards so bookmarks keep working. `web-token` and
  `web-state.json` live in `paths::config_dir()` (beside `config.toml` and `state.json`),
  written with `paths::write_atomic`, mode 0600; `state.json` is read with
  `tui::state::ClientState::load` (made `pub(crate)`), never written.
- The token travels in the URL **fragment** (`#t=`), never in a query string, so it is not in
  server logs or `Referer`. `index.html` loads `app/boot.js` (no inline script), which posts it to `POST /api/login {"token": …}`,
  then removes the fragment with `history.replaceState`.
- `/api/login` compares in constant time. Success sets `claudio_web=<32 random bytes hex>;
  HttpOnly; SameSite=Strict; Path=/` and records the session (in memory; idle expiry 24 h; at
  most 64, oldest dropped). Failure answers 401; more than 5 failures in 60 s answers 429 for
  the rest of the minute. The Origin check runs before the body is read; only requests that
  passed it count towards the limit. The limit is global (every loopback client shares
  `127.0.0.1`).
- Every request: the `Host` header must be in the accepted set, else 421. Every request other
  than `GET`/`HEAD`, and every WebSocket upgrade, must carry an `Origin` whose scheme is
  `http` or `https` and whose host[:port] is in the accepted set (the three loopback forms
  plus every `--allow-host NAME`, with or without the listen port); else 403. With any
  `--allow-host`, the cookie also gets `Secure` and the docs tell the proxy to terminate TLS.
  `--allow-host` values are compared case-insensitively. `/api/*` (except login) and `/ws`
  without a valid cookie answer 401. Static assets need no cookie (they hold no data).
- Headers on every response: `Content-Security-Policy: default-src 'self'; script-src 'self';
  style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src
  'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store`
  on `/api/*`.
- Threat model: the attacker is a web page in the same browser (CSRF, cross-site WebSocket
  hijacking, DNS rebinding) or another local user. They control page content, DNS answers and
  model output shown in the UI. Must hold: no request without the token-derived cookie acts on
  sessions; no secret (proxy tokens, plugin env values, `Spawn.env`) is ever sent to the
  browser; model output rendered as HTML is sanitized; the token file and `web-state.json`
  are 0600. Holding the cookie equals holding the member's shell on every connected host,
  remote hosts included; the token file is as sensitive as an SSH key. `claudio web` logs
  every `session.shell`, `session.spawn`, `session.kill`, `session.approve` and
  `extensions.action` with its op, host and session id (never its text).

### Browser protocol (WebSocket, version 1)

Text frames are JSON; binary frames carry terminal bytes.

- Server → client on open: `{"t":"hello","v":1,"version":"<claudio>","features":[…]}` then
  `{"t":"snapshot","hosts":[Host…],"sessions":[Session…],"profiles":{"names":[…],
  "default":…},"layout":…,"order":[…]}`.
- Request: `{"t":"req","id":<u32>,"op":"<name>", …args}`. Reply:
  `{"t":"res","id":…,"ok":true,"data":…}` or `{"t":"res","id":…,"ok":false,"error":"…"}`.
  An unknown op answers `ok:false, error:"unknown op"`.
- Events: `{"t":"ev","ev":"<name>", …}` with `host` (Host DTO), `session` (Session DTO,
  created or changed), `session_removed {id}`, `attached {id,rows,cols}`, `latex {id,seq,
  formulas}`, `lens {id,seq,items}`, `host_stats {host,cpu,mem_used,mem_total}`, `notice
  {text, level}`, `plugin_status {host,name,…}`, `claude {host,…}` (install/update progress).
- Binary server → client: `0x01 ‖ session uuid (16 bytes) ‖ bytes` (PTY output; the first
  frame after `attached` is the snapshot). Binary client → server: `0x02 ‖ uuid ‖ bytes`
  (input, ≤ 64 KiB per frame).
- Limits: client→server text frame ≤ 64 KiB, client→server binary ≤ 64 KiB + 17 (input);
  larger frames close the socket with 1009. Server→client frames are bounded by the daemon's
  `MAX_FRAME` (1 MiB) plus 17 bytes; the client accepts up to 2 MiB.
  A tab whose outgoing queue exceeds 8 MiB is disconnected (it reconnects and re-snapshots).
- DTOs carry no secrets: `Session {id, host, kind, cwd, name, title, label, state, attention,
  claude_session_id, model, context_tokens, output_tokens, turns, git, created_at, pid,
  dormant, proxy (profile name or null), first_party, ephemeral}`; `Host {name, connected,
  connecting, error, outdated, hostname, os, arch, claude {path, version} | null, features,
  version}`.
- Ops (args → data):
  - `hosts.list`, `hosts.candidates` (ssh config + MRU), `hosts.connect {host}`,
    `hosts.disconnect {host}`. ssh runs with `BatchMode=yes` (no prompts): `hosts.connect`
    answers `ok:false` with the last 3 sanitized stderr lines of ssh (host keys, agent,
    permissions), and the Host DTO's `error` carries the same text until the next attempt.
  - `session.attach {id,rows,cols}`, `session.detach {id}`, `session.resize {id,rows,cols}`,
    `session.spawn {host,cwd,resume?,proxy:{kind:"default"|"direct"|"profile",name?},
    first_party?,name?,prompt?}` → `{id}`, `session.shell {host,cwd,name?}` → `{id}`,
    `session.respawn {id,fresh,proxy?}`, `session.kill {id}`, `session.rename {id,name|null}`,
    `session.paste {id,text,submit}`, `session.approve {id,decision:"approve"|"deny",
    attention_seq}`, `session.order {ids}`.
  - `fs.list {host,path}`, `fs.projects {host}`, `claude.sessions {host,cwd}`.
  - `reader.answers {id}`, `reader.last_prompt {id}`.
  - `search {query}` fans out to every connected host that has the `search` feature, one
    request per host at a time per hub (queued, 10 s deadline), merged newest first, each hit
    tagged with its host; a host that answers `search busy` is retried once after 500 ms,
    then reported in `data.failed: [{host, error}]`.
  - `git.status {id}`, `git.worktree_diff {id,path,staged}`, `git.log {id,all,skip}`,
    `git.commit {id,oid}`, `git.diff {id,oid,file}`.
  - `proxy.stats {profile,window:"24h"|"7d"|"30d"}`, `proxy.credential {id}`,
    `proxy.switch {id}`.
  - `snippets.list`, `snippets.save {text}`, `extensions.list`, `extensions.toggle {name,on}`,
    `extensions.action {ext,action,input?,session?}`.
  - `plugins.list`, `plugins.items {name}` (manifest items with checked state),
    `plugins.save {name,disabled}`, `plugins.status` (per host and plugin: installed, present,
    skipped with reasons and retry countdowns).
  - `claude.update {host,install}`, `layout.save {layout}`.

### Session model, attach and input

- The hub keeps, per host, the `Sessions` list and applies each `SessionEvent` exactly as
  `App::on_event` does for the fields the DTO carries (Meta fields `None` = no news; `Created`
  adds unless `ephemeral`; `Removed` deletes; a host disconnect marks its sessions `unknown`).
  Each change is broadcast to every tab as a `session` event. `Latex` is taken as the
  `latex` lens list only from a host without the `lens` feature; `Lens` only from a host with
  it; a list with a lower `seq` than the one held is dropped, and `seq` resets to 0 on
  `attached`.
- Journaled sessions without a pid are `dormant`; the UI offers *Resume* (respawn with
  `--resume`), never auto-respawns them (the TUI's recovery may run concurrently; spawn is
  idempotent by id anyway).
- Attach is reference-counted per session across tabs and panes: the first subscriber sends
  `Attach`, the last unsubscribe sends `Detach`. Output is fanned out to subscribed tabs. Only
  the focused pane sends `session.resize`; every pane follows the size in `attached` events
  (the daemon re-sends `Attached` when another client resizes). Last writer wins, as for a
  second TUI. A pane never sends `session.resize` in response to an `attached` event (no
  echo) and debounces its own fit by 250 ms; a pane sized by another client shows the
  foreign size in its header.
- `session.paste` wraps the text in bracketed paste (`ESC[200~ … ESC[201~`, with any
  `ESC[201~` inside removed and C0 controls other than `\t`, `\n` removed), appends `\r` when
  `submit`, ≤ 64 KiB.
- `session.approve` is accepted only when the hub's state for the session is
  `needs_approval`, its `attention.kind` is `permission_prompt` or
  `worker_permission_prompt`, and `attention.seq` equals the request's `attention_seq`;
  otherwise `ok:false, error:"stale"` (or `"not a permission prompt"`). `approve` sends `\r`,
  which selects the option claude highlights by default ("Yes"); `deny` sends `ESC`, which
  rejects the tool and interrupts the turn. Because another client may have moved the
  highlight, the buttons are labelled with the key they send ("Enter" and "Esc") and the card
  has *Open* next to them. Elicitation dialogs get only *Open* (no Approve/Deny). On a host
  without the `attention` feature the hub reports `attention: null` and `attention_seq` must be
  `0`; `seq` on hosts with the feature starts at 1, so `0` never matches a real attention.
  There the kind is unknown, so the card offers Approve/Deny with the warning "permission or
  elicitation: check the terminal".
- `session.spawn` with `prompt`: the hub attaches to the new session itself (its own
  subscriber, at the spawn's `rows`/`cols` so the PTY size does not change), waits for
  `needs_input`, then for the output to contain `ESC[?2004h` and to be quiet for 500 ms, then
  pastes the prompt with `submit`; it gives up after 60 s with a `notice`. While the pending
  prompt exists the pane shows "opening prompt pending". The hub's subscriber is released
  after the paste (the pane's own subscription is independent). `session.spawn` takes
  `rows`/`cols` (the pane's size).

### Spawn composition (parity with the TUI)

- Proxy choice resolves like the wizard: `default` = `[proxy] default`, `direct` = no proxy,
  `profile` = named profile; `first_party` only with a proxy. Env from
  `proxy::resolve::proxy_env_for` using a 300 s cache of `fetch_config`.
- Plugins: per profile, the plugin list (`proxy::plugins::fetch`, refreshed every 15 min, the
  shared `plugins.json` cache written exactly as the TUI does, dropped on 401/403), manifests
  fetched and hash-checked in memory, `Msg::Manifest` sent to each connected host for every
  plugin whose choices are saved (`[plugins.<name>]`), `Applied` kept per (host, plugin) and
  re-asked every 30 s while it lists `installing` or a retry is due. A spawn waits ≤ 20 s for
  pending manifests of its host, then `compose` + `with_plugins` + `merge_env`, as the TUI.
  A plugin without saved choices makes the UI open its setup screen before the spawn
  (checkboxes per item, all on by default); *Skip* spawns without it for this time.
- Extensions: the session's lenses from `Registry::session_extensions()` built from the same
  config as the TUI.
- `config.toml` has two writers (TUI and web). The web re-reads `config.toml` after every
  save and on a 5 s mtime poll (`spawn_blocking`), and applies the file's
  `[extensions]`/`[plugins.*]` to its registry; a lost edit is thus visible, not silent.
  (Cross-process locking is out of scope.)
- `web-state.json` records the proxy profile and first-party flag of each session the web
  started (needed for `session.respawn`); for a session it did not start it reads the TUI's
  `state.json` (read only). When neither knows, `session.respawn` answers `ok:false,
  error:"proxy unknown"` and the UI asks (default/direct/profile, pre-selecting `[proxy]
  default`) before sending again with an explicit `proxy` argument.

### Daemon additions (backward compatible)

1. Attention detail. `claude::state::Tracker` keeps the last `PreToolUse` `tool_name` and a
   one-line summary of `tool_input` (`command` for Bash, `file_path` for Edit/Write/Read,
   `url` for WebFetch, `pattern` for Grep/Glob, else compact JSON), and emits
   `SessionEvent::Attention {seq, kind, message, tool, detail}` for **every** Notification
   whose type maps to `NeedsApproval` or `NeedsInput` (and for the `SessionStart` that sets
   `NeedsInput`, with kind `session_start`), after any `State` event of the same hook and
   whether or not the state changed; `seq` increments per emitted `Attention`, starting at 1.
   `kind` is the hook's `notification_type` (`permission_prompt`,
   `worker_permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `idle_prompt`,
   `agent_needs_input`), sanitized, ≤ 64 bytes; `message` is the Notification's `message`.
   `tool`/`detail` are best-effort (they name the most recent `PreToolUse`, which may differ
   from the prompting tool in a parallel burst) and the UI shows `message` first. A
   `PreToolUse`, `UserPromptSubmit`, `Stop` or `StopFailure` clears the current attention
   (`SessionInfo.attention = None`; clients clear theirs on any `State` event that leaves
   `needs_approval`/`needs_input`). All strings pass `claude::latex::sanitize`, `message` ≤ 512 bytes, `detail` ≤
   1024 bytes (cut on a char boundary). `SessionInfo` gains `#[serde(default)] attention:
   Option<Attention>` (the current one while the state wants attention, else `None`).
   `DAEMON_FEATURES` gains `"attention"`.
2. Working tree. `Msg::GitWorktree {cwd}` → `Msg::GitWorktreeStatus {root, branch,
   files:[{path, old_path, index, worktree, added, removed}], truncated}` (porcelain v2 plus
   `--numstat` for staged and unstaged; ≤ 2000 files). `Msg::GitWorktreeDiff {cwd, path,
   staged}` → `Msg::GitPatch` (same caps and sanitizing as `GitDiff`; untracked files diffed
   against `/dev/null`). Both run through the existing git runner (deadline, scrubbed env, no
   textconv/ext-diff, per-client concurrency limit; `server.rs` routes `GitWorktree` and
   `GitWorktreeDiff` through `spawn_git` and `git::handle`, like the three existing ops).
   `path` must be relative, contain no `..` component or NUL, and name an entry of the status
   the daemon itself produced (the daemon re-runs `git status --porcelain=v2 -z` and requires
   an exact match); an untracked entry is diffed with `git diff --no-index -- /dev/null
   <path>` after `lstat` shows a regular file (symlinks are refused). Anything else answers
   `Error "no such path"`. `DAEMON_FEATURES` gains `"worktree"`.

Compatibility matrix:

| Writer → reader | Result |
|---|---|
| new daemon → old TUI/web | `Attention` deserializes to `SessionEvent::Unknown`; extra `SessionInfo.attention` ignored by serde |
| old daemon → new web | no `attention` feature: board shows state without detail; approve works on state only (seq 0) |
| new web → old daemon, `GitWorktree*` | `Error "unsupported op…"`; the UI checks the feature first and hides the Changes tab |
| browser protocol v1 ↔ other versions | `hello.v` mismatch → the page shows "server updated" and reloads once; a second mismatch in a row shows the message without reloading (`Cache-Control: no-store` on `/` and `index.html`) |

Rollout: one release (daemons are replaced by newer builds through freshness). Rollback: the
previous release; the additions are inert to old clients.

### The browser app

- No build step: ES modules loaded by `index.html`; Preact + htm + @preact/signals for UI,
  xterm.js (+ fit, unicode11, web-links, search addons) for terminals, KaTeX for `<latex>`,
  mermaid (`securityLevel: 'strict'`, loaded on first use) for diagrams, marked + DOMPurify for
  markdown, highlight.js (common languages) for code. All vendored, pinned, with licenses in
  `web/vendor/LICENSES.md`. Fonts (OFL): a pixel display face for the logo, headers and badges
  only, JetBrains Mono for UI and terminals, Inter for reader prose.
- Vendored files (exact list in `web/vendor/MANIFEST.txt` with version, URL and SHA-256):
  mermaid's ESM build (`mermaid.esm.min.mjs` + its `chunks/mermaid.esm.min/*.mjs`);
  `katex.min.js`, `katex.min.css` and `fonts/*.woff2` only (editing the CSS is forbidden, so
  the ttf/woff `url()`s stay and 404 harmlessly); xterm.js + the 4 addons; highlight.js
  `common` (ESM); preact, htm, signals, marked, dompurify. Fonts: two weights each of
  JetBrains Mono and Inter (woff2), one pixel face. Assets are stored gzipped (level 9) in the
  binary and served with `Content-Encoding: gzip` (decompressed on the fly for a client
  without gzip), so the budget counts gzipped bytes. The plan measures the gzipped set before
  any Rust work; if it exceeds 3.5 MiB, mermaid is reduced to the diagram chunks for
  flowchart, sequence, class, state, er, gantt and pie (the others render as a code block with
  a notice).
- Layout: top bar (logo, host chips with cpu/mem, 5h/7d gauges of the active profile, palette
  button, settings); left rail (attention queue on top, then sessions grouped by host and by
  repo, collapsible, drag to reorder); centre pane grid (1, 2 side by side, or 2×2; drag a
  session onto a pane; each pane has a header with state, name, branch and actions, an xterm
  terminal, a composer for claude sessions and a status strip: model, ctx gauge, git counts,
  output tokens, turns); right drawer with tabs Reader, Changes (working tree), History (git
  log/commit/patch), Stats, Session (details and actions).
- Attention queue: sorted needs-approval > needs-input > error > just-idle (≤ 10 min), each
  card with the session label, host, the attention message, tool and detail, and buttons Open,
  Approve (sends Enter), Deny (sends Esc). On narrow screens (< 760 px) the app shows only the
  board and queue; approvals there ask for a second confirmation.
- Focus model: keys go to the focused terminal. `Ctrl+Shift+Space` (or clicking outside the
  terminal) leaves it for app mode, shown by a mode badge; in app mode single keys act (`k`
  or `Ctrl+K` palette, `a` next attention, `n` new session, `t` terminal tab, `1`–`4` focus
  pane, `j`/`k`-like arrows move in the rail, `r` reader, `g` changes, `?` help, `Enter` back
  to the terminal). The app binds no `Alt+letter`, no `Ctrl+W/T/N/Tab`, and never intercepts
  a key while a terminal has focus except `Ctrl+Shift+Space`.
- Command palette: fuzzy list of every action and every session (go to, kill, restart,
  rename, reader, changes, stats, switch subscription, search, snippets, extension actions,
  settings, theme).
- Wizard (modal): host (local, connected, ssh candidates), directory (recent and claude
  projects with branch and age, browser with typed path and fuzzy filter), resume (list of the
  dir's conversations or "new"), proxy (default/direct/profiles, first-party), name, opening
  prompt, kind (claude or terminal).
- Reader: the session's answers (newest first, each with its prompt), markdown with tables,
  KaTeX for `<latex>` (display and inline), mermaid for ` ```mermaid ` fences, highlighted code
  with copy buttons, copy answer, a formulas list, lens items. Model text is untrusted: marked
  output goes through DOMPurify; mermaid SVG goes through DOMPurify's SVG profile; KaTeX runs
  with `trust:false`, `maxSize`, `maxExpand`, and per-formula errors render as code.
  Built-in lenses render natively (`latex` → KaTeX, `diagram` → mermaid). An external lens's
  item is rendered through the extension host exactly as the TUI's reader does and shown as
  its `block` rows (or `inline`) in a monospace `<pre>`, text only; a `document` action result
  is markdown through marked + DOMPurify; `paste` goes through `session.paste`; `list` opens
  the palette-style picker.
- Changes: working-tree file list (staged/unstaged/untracked/conflicts) refreshed when the
  session's `git` meta changes, unified and side-by-side diff with line numbers. History: log
  pages, commit details, per-file patches.
- Stats: the active profile's windows (24h/7d/30d) with SVG charts, per-model table, pool
  health, the session's credential and 5h/7d gauges, and *Move to next subscription* with a
  confirm.
- Notifications: the Notification API (asked once, from a user gesture) for sessions that
  newly want attention and are not visible, debounced per (session, state) like
  `tui/notifications.rs`; title badge with the count; optional sound (off by default).
- Toasts for notices, claude install/update offers (non-modal: Install, Skip), daemon outdated
  (with the TUI's 120 s retry), reconnecting hosts.
- Themes: `arcade` (default: near-black blue-green background, state colours identical to the
  TUI's glyph colours, 2 px borders, hard offset shadows on popovers, pixel face for headers
  and badges only, an optional 3 % scanline overlay on the board, never over terminals or the
  reader) and `plain` (no pixel face, no scanlines); dark first, a light variant of each.
  `prefers-reduced-motion` stops spinners and blinks. Every state has a glyph as well as a
  colour. Settings (theme, scanlines, font size, sound, notifications) live in
  `localStorage`; board order and layout in `web-state.json` (shared across browsers).
- xterm.js: OSC 52 clipboard writes ignored, links open only after a confirm, the terminal is
  never re-rendered by Preact (a stable DOM node owned by the pane component).

## Alternatives considered

| Option | Why not |
|---|---|
| Serve the UI from each daemon | Widens the daemon's attack surface; proxy profiles and plugin secrets live on the client machine, not the daemon host. |
| Drive the TUI's `App` headless | `App` is welded to crossterm events, the `Screen` mirror and ratatui modals. |
| Extract the TUI's reconnect/plugin/spawn glue into shared code | Real reuse, but a large refactor of the TUI's event loop in the same release; re-expressing it in async code in `src/web/` leaves the TUI untouched. Revisit once both exist. |
| A bundler (Vite + TS) with committed `dist/` | Needs node to change the UI and puts minified bundles in diffs; vendored ES modules are reviewable as written. |
| Allow LAN listening with TLS now | More surface for v1; `ssh -L` and a same-host reverse proxy cover the need. |

## Failure modes and security

- Untrusted input: model output (answers, attention text, titles, git text) rendered by the
  browser: sanitized by the daemon as today, and in the browser through DOMPurify and
  text-only rendering where markup is not needed. Browser messages: JSON parsed into typed
  structs with size caps; unknown ops rejected; ids must name a known session.
- Partial failure: a host disconnect marks its sessions `unknown`, reconnects with the TUI's
  backoff (1 s doubling to 30 s with jitter), re-subscribes host stats and re-attaches panes.
  A tab's socket drop reconnects with backoff and re-snapshots.
- Concurrency: the TUI and the web may act on the same session; the daemon already serves
  several clients. Approve is guarded by state + attention seq.
- Restarts: a `claudio web` restart loses cookies (re-login through the saved token URL) and
  nothing else; sessions live in the daemons.
- Resource limits: frame caps above; at most 16 tabs; search and git requests ride the
  daemon's existing limits; per-tab outgoing queue cap.
- Secrets: proxy tokens and plugin env stay in the server process; logs name keys, never
  values; DTOs are built field by field (no serializing `Profile` or `SpawnSpec`).

## Acceptance criteria

- [ ] AC1: `claudio web --listen 0.0.0.0:7337` exits 2 with the ssh -L hint; the default
      listens on 127.0.0.1:7337 (unit test of address validation; manual start).
- [ ] AC2: auth (Rust integration test against a running `claudio web`): no cookie → 401 on
      `/api/x` and `/ws`; wrong Host → 421; WS upgrade with foreign Origin → 403; wrong token
      → 401, 6th failure within 60 s → 429; right token → cookie and a working WS; CSP header
      present on `/`.
- [ ] AC3: secrets: with a proxy profile whose token is a known marker and a plugin env value
      marker, spawning through the web, no WS frame and no HTTP response contains either
      marker, and the fake claude's env does contain them (integration test).
- [ ] AC4: terminal round trip: attach → `attached` + snapshot binary frame; input frame
      reaches the fake claude; resize forwarded (integration test).
- [ ] AC5: attention: `Tracker` unit tests (PreToolUse Bash + permission Notification →
      `State` then `Attention{kind:"permission_prompt",tool:"Bash",detail:<command>}`; caps;
      sanitize; `idle_prompt` after `SessionStart` emits `Attention` with no `State`;
      `permission_prompt` then `elicitation_dialog` emits two `Attention`s with increasing
      `seq`); daemon test that `SessionInfo.attention` is set while waiting and cleared after.
- [ ] AC6: worktree ops: daemon tests on a temp repo with staged, unstaged, untracked and
      renamed files; caps; old-daemon compatibility (unknown op → Error); `GitWorktreeDiff`
      with `../x`, an absolute path, a symlink and a path not in the status → `Error`; a stuck
      `git` (fake binary that sleeps) does not block a concurrent `Sessions` request.
- [ ] AC7: approve guard: stale `attention_seq` → `stale`; matching → `\r` reaches the PTY;
      `elicitation_dialog` → `not a permission prompt`.
- [ ] AC8: browser e2e (Playwright, `make test-web`, fake claude): login from the token URL;
      the board lists sessions with states; a session in needs-approval shows its attention
      text in the queue and Approve resolves it (an `elicitation_dialog` card has no Approve); a pane shows the terminal and typed keys
      echo; the wizard starts a session in a chosen dir; the reader renders a KaTeX formula and
      a mermaid diagram from a fixture transcript; the Changes tab lists a modified file and
      shows its diff; the palette opens with Ctrl+Shift+Space then `k`; theme switch. The page
      registers a `securitypolicyviolation` listener; the e2e asserts zero violations after
      rendering KaTeX, mermaid, highlighted code and a terminal. With `Notification` permission
      granted (`context.grantPermissions(['notifications'])`), a fixture transition to
      needs-approval in a hidden session calls `new Notification` once (stubbed on `window`)
      and a second identical transition within the debounce window does not.
- [ ] AC9: JS unit tests (`node --test web/tests`) for the protocol client, the attention
      queue ordering, the keymap/focus model, the markdown/latex/mermaid preprocessing and the
      notification debounce.
- [ ] AC10: existing suites unchanged and green: `make test-unit`, `make test-manager`.
- [ ] AC11: release binary grows by at most 4.5 MiB (measured and reported; amended from 4 MiB by ruling R22 after measuring: +4.39 MiB with mermaid's optional elk and label-math chunks dropped).
- [ ] AC13: parity table in `docs/web.md`: one row per Goal-2 capability with the TUI key,
      the web action, the WS op(s) and the test (e2e name or Rust test). Every row has a test
      or a one-line manual check recorded in `.delivery/2026-11-08-web-ui/parity.md`.
- [ ] AC14: e2e (fake claude, fake proxy as in `tests/manager.rs`): open a terminal tab and
      run `echo ok`; rename, kill, restart a session; `search` finds a fixture conversation;
      save and insert a snippet; toggle an extension off and on (config.toml line changes); a
      plugin without choices opens the setup screen before spawn, Skip spawns without it;
      `proxy.stats` renders the fixture window; `claude.update` with a fake installer shows
      progress and completion.
- [ ] AC12: docs: README section, `docs/web.md` (usage, security model, keys), CLAUDE.md
      architecture entries for `src/web/` and `web/`.

## Constraints

- Conventional commits, no AI attribution or Co-Authored-By trailers.
- No node at build time; `cargo build --release` alone produces the full binary. Node/npx are
  only for the dev-time e2e and unit tests of `web/`.
- Unix only; Linux and macOS must compile (CI).
- Never log or send secrets; never pass secrets in argv; the web never writes `state.json`.
- New daemon ops and events follow the compatibility rules in CLAUDE.md (unknown op → Error,
  `Unknown` event variants, `#[serde(default)]` fields).
- No blocking work on the async executor (file I/O for config/snippets via `spawn_blocking`).
- Vendored files are pinned releases, unmodified, with their licenses.

## Open questions

None (owner delegated decisions for this run).
