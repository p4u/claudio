# `claudio web` browser protocol, version 1

This is the contract between the server (`src/web/`) and the browser app (`web/app/`). Both
are built from it in parallel; neither side may add, rename or reshape a message without
changing this file first. Derived from `docs/specs/2026-11-08-web-ui-design.md`
(§Browser protocol, §Session model, §Spawn composition). Where the spec is silent, the
decision is marked **(decided here)**.

## 1. HTTP surface

| Method and path | Cookie | Origin check | Answer |
|---|---|---|---|
| `GET`/`HEAD` `/`, `/index.html`, any embedded asset path | no | no | the asset; `404` when unknown |
| `POST /api/login` body `{"token":"<64 hex>"}` (≤ 4 KiB) | no | yes | `204` + `Set-Cookie`; `401` wrong token; `429` locked; `400` bad body; `413` too large |
| `POST /api/logout` | yes | yes | `204` + an expiring `Set-Cookie` |
| `GET /api/me` **(decided here)** | yes | no | `204`: the cookie is live (the page asks this at load when the URL has no token) |
| `GET /ws` (WebSocket upgrade) | yes | yes | `101`; `503` past 16 tabs **(decided here)** |
| any other `/api/*` | yes | for non-GET | `404` once authenticated |

Checks run in this order on every request: `Host` not accepted → `421`; a method other than
`GET`/`HEAD`, or any WebSocket upgrade, without an accepted `Origin` → `403`; `/ws` or
`/api/*` (but `/api/login`) without a live session cookie → `401`.

- Accepted `Host` values: the `--listen` address itself (e.g. `127.0.0.2:<port>`),
  `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` (each also without the port when it
  is 80), every `--allow-host NAME` as `NAME` or `NAME:<port>`, and every
  `--allow-host NAME:PORT` as exactly that, compared case-insensitively.
- Accepted `Origin`: `http://` or `https://` followed by an accepted Host value, nothing after
  it. `null` is refused.
- Cookie: `claudio_web=<64 hex>; HttpOnly; SameSite=Strict; Path=/`, plus `; Secure` when the
  server runs with any `--allow-host`. Server-side sessions: 24 h idle expiry, at most 64
  (the least recently used goes).
- Login limit: when 5 failed logins happened in the last 60 s, every login answers `429`
  until the oldest of them is 60 s old. Only requests that passed the Origin check count.
- Headers on every response, errors included:
  `Content-Security-Policy: default-src 'self'; script-src 'self'[ 'sha256-<import map>']; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`.
  `Cache-Control: no-store` on `/`, `/index.html` and `/api/*`; `Cache-Control: no-cache` plus
  an `ETag` on every other asset.
- **(decided here)** The one inline script allowed is `index.html`'s
  `<script type="importmap">`: an import map cannot be loaded from a file, and the vendored ES
  modules import bare specifiers. `build.rs` puts the SHA-256 (base64) of the map's exact text
  in the CSP. No other inline script, no `eval`, no inline event handlers.
- The token never travels in a query string. The page reads it from the fragment
  (`#t=<token>`), posts it to `/api/login`, then removes the fragment with
  `history.replaceState`.

## 2. WebSocket framing

One socket per browser tab, `ws://<host>/ws` (or `wss://` behind a TLS proxy).

- Text frames: one JSON object each, UTF-8. Client → server text frames ≤ 128 KiB, so a
  request carrying a 64 KiB text (`session.paste`, `session.spawn`'s `prompt`, `snippets.save`)
  fits with its JSON envelope and escaping, and an oversized text gets its op's `"too large"` /
  `"bad request"` answer instead of a close.
- Binary frames: `kind (1 byte) ‖ session uuid (16 bytes, RFC 4122 byte order) ‖ payload`.
  - `0x01` server → client: PTY output of a session the tab is attached to. The first `0x01`
    frame after an `attached` event for that session is the screen snapshot (reset the
    terminal, then write it).
  - `0x02` client → server: keyboard/mouse input for a session, payload ≤ 64 KiB (frame ≤
    65 553 bytes). Input for an unknown or not-running session is dropped silently.
- Server → client frames are at most 1 MiB + 17 bytes; the client accepts up to 2 MiB.
- Close codes (**decided here**): `1009` a client frame over its limit; `1003` a text frame
  that is not a JSON object with `"t":"req"`, or a binary frame shorter than 17 bytes or of a
  kind other than `0x02`; `1013` the tab's outgoing queue passed 8 MiB (the client reconnects
  and gets a fresh snapshot); `1001` the server is shutting down.
- At most 64 requests in flight per tab; more answer `ok:false, error:"busy"` **(decided
  here)**.

## 3. Messages

### 3.1 On open (server → client)

```json
{"t":"hello","v":1,"version":"0.8.0","features":[]}
{"t":"snapshot","hosts":[Host],"sessions":[Session],"profiles":{"names":["work"],"default":"work"},"layout":Layout,"order":["<uuid>"]}
```

`features` is reserved for later minor additions (empty in v1); clients ignore unknown entries.
Right after the snapshot the server replays, in this order **(decided here)**: one
`host_stats` per host with stats, one `lens` and one `latex` per session with a non-empty item
list, one `claude` per host with an open offer (`offer_install`/`offer_update`) or a running
install/update.

If `hello.v` is not `1`, the page shows "server updated" and reloads once
(`sessionStorage["claudio.reloaded"] = "1"`); a second mismatch in a row shows the message
without reloading. The flag is cleared after a matching hello.

### 3.2 Requests and replies

```json
{"t":"req","id":7,"op":"session.kill","sid":"6f1c…"}
```

The shape: `{"t":"req","id":<u32>,"op":"<name>", …args}`. Args are top-level keys next to
`t`, `id` and `op`. An argument named `id` would collide with the request id, so **every
session id argument, and every session id in an event, is named `sid`** **(decided here; the
spec wrote `{id}`)**. The Session DTO itself keeps `id`.

Replies: `{"t":"res","id":<u32>,"ok":true,"data":<value>}` or
`{"t":"res","id":<u32>,"ok":false,"error":"<text>"}`. Replies may arrive in any order.

### 3.3 Events (server → client)

`{"t":"ev","ev":"<name>", …fields}`; see §6.

## 4. DTOs

All strings the server sends were sanitized by the daemon (`claude::latex::sanitize` or
`daemon::git` sanitizing) and are still untrusted: render as text unless §7 says otherwise.
Field order is not significant. No DTO ever carries a token, a plugin env value, `Spawn.env`,
a socket path or a hook token.

### Session

| Field | Type | Meaning |
|---|---|---|
| `id` | string (uuid) | manager session id |
| `host` | string | `"local"` or an ssh alias |
| `kind` | `"claude"` \| `"shell"` | |
| `cwd` | string | |
| `name` | string \| null | user-given name |
| `title` | string \| null | terminal title (claude sessions only) |
| `label` | string | the TUI's label rule: `name`, else `title` (claude, not empty, not `"Claude Code"`), else the cwd's last component (claude) or `"term"` (shell); ≤ 200 chars |
| `state` | `"starting"` \| `"working"` \| `"needs_approval"` \| `"needs_input"` \| `"idle"` \| `"error"` \| `"exited"` \| `"unknown"` | `unknown` while its host is disconnected |
| `state_since` | number | Unix seconds when the server saw `state` start **(decided here)** |
| `attention` | Attention \| null | what it waits for; null unless `state` is `needs_approval` or `needs_input` and its daemon has the `attention` feature |
| `claude_session_id` | string \| null | the conversation id |
| `model` | string \| null | short model name |
| `context_tokens` | number \| null | |
| `output_tokens` | number \| null | |
| `turns` | number \| null | |
| `branch` | string \| null | |
| `git` | GitStatus \| null | |
| `created_at` | number | Unix seconds |
| `pid` | number \| null | |
| `dormant` | bool | journaled, no process (`pid` null and `state` `exited`): offer *Resume*, never auto-respawn |
| `proxy` | string \| null | proxy profile name; null = direct or unknown (see `proxy_known`) |
| `proxy_known` | bool | whether `proxy`/`first_party` are known (web-state.json or the TUI's state.json) **(decided here)** |
| `first_party` | bool | |
| `ephemeral` | bool | always false in v1 (ephemeral sessions are not listed) |
| `prompt_pending` | bool | an opening prompt waits to be pasted **(decided here)** |

### Attention

`{"seq": number, "kind": string, "message": string, "tool": string|null, "detail": string|null}`.
`seq` ≥ 1, growing per emitted attention for the life of the session's process. `kind` is one
of `permission_prompt`, `worker_permission_prompt`, `elicitation_dialog`,
`elicitation_url_dialog`, `idle_prompt`, `agent_needs_input`, `session_start`, `ask_user_question` (state `needs_input`;
`message` is the question, `detail` its options joined with ` · `) or `plan_approval` (state
`needs_approval`; never approvable through `session.approve`) (≤ 64 bytes). A client that does not
know a kind shows its `message` as for any other.
`message` ≤ 512 bytes, `detail` ≤ 1024 bytes, all one line. `tool`/`detail` are best effort
(the most recent tool call; may differ from the prompting one in a parallel burst) and are set
only for the approval kinds and the two kinds above. Show `message` first.

### GitStatus

`{"head": string, "detached": bool, "upstream": bool, "ahead": n, "behind": n, "staged": n, "unstaged": n, "untracked": n, "conflicts": n}`
(exactly `proto::GitStatus`).

### Host

| Field | Type | Meaning |
|---|---|---|
| `name` | string | `"local"` or the ssh alias |
| `connected` | bool | |
| `connecting` | bool | an attempt (bootstrap, ssh, handshake) runs |
| `error` | string \| null | last failure: for ssh, the last 3 sanitized stderr lines; kept until the next attempt |
| `outdated` | bool | the daemon is older than this claudio and was left running because it was busy; the server reconnects once it is idle (checked every 120 s) |
| `hostname`, `os`, `arch` | string \| null | from the daemon's Welcome |
| `claude` | `{"path": string, "version": string}` \| null | null: claude is not installed there |
| `features` | [string] | the daemon's features: `lens`, `search`, `last_prompt`, `attention`, `worktree` |
| `version` | string \| null | the daemon's claudio version |

### Layout **(decided here)**

`{"grid": 1|2|4, "panes": [uuid|null …] (exactly grid entries), "focus": 0..grid-1}`.
Default `{"grid":1,"panes":[null],"focus":0}`. Stored in `web-state.json`, shared by all
browsers.

### Others

- DirEntry `{"name","dir","git": string|null,"claude_at": n|null,"symlink","hidden"}`;
  ProjectDir `{"path","modified","git","symlink","hidden"}`; ClaudeSession
  `{"id","title","last_prompt","modified","messages"}`; Answer `{"prompt","text"}`;
  GitLogPage, GitCommitInfo, GitPatch, GitWorktreeStatus: exactly the `proto.rs` structs.
- GitWorktreeStatus `{"root","branch": string|null,"files":[{"path","old_path": string|null,"index","worktree","added": n|null,"removed": n|null}],"truncated"}`.
  `index`/`worktree` are one status letter each: `.` unchanged, `M`, `T`, `A`, `D`, `R`, `C`,
  `U` (unmerged), `?` (untracked: both `?`). `added`/`removed` sum the staged and unstaged
  counts; null for binary or untracked files. Paths are relative to `root`.
- SearchHit `{"host","conversation","cwd","modified","title","snippet","matches"}`.
- Extension `{"name","description","enabled": bool,"source": "builtin"|"external"|"plugin","profile": string|null,"refused": string|null,"lenses":[{"name","kind":"tag"|"fence"}],"actions":[{"id","title","input": string|null}]}`.
- ActionResult `{"kind":"document","title","markdown"}` | `{"kind":"notice","text"}` |
  `{"kind":"paste","text"}` | `{"kind":"list","title","items":[{"label","detail": string|null,"result": ActionResult}]}` |
  `{"kind":"resume","host","cwd","conversation"}`.

## 5. Ops

`→` gives the `data` of a successful reply. `{}` is an empty object. Every op not listed
answers `ok:false, error:"unknown op"`. Args failing to parse answer `"bad request: <why>"`.
A `sid` naming no session the server knows answers `"no such session"`. A `host` naming no
known host answers `"no such host"`; a known but disconnected one `"not connected"`.

### Hosts

| Op | Args | → | Notes |
|---|---|---|---|
| `hosts.list` | — | `{"hosts":[Host]}` | |
| `hosts.candidates` | — | `{"hosts":[string]}` | MRU then `~/.ssh/config` aliases, without connected ones |
| `hosts.connect` | `host` | `{"host":Host}` once connected | ssh `BatchMode=yes`; failure: the last 3 sanitized ssh stderr lines (or the bootstrap error). `"local"` answers `"local is always connected"` |
| `hosts.disconnect` | `host` | `{}` | drops the host: `session_removed` for its sessions, then `host_removed`. `"local"` answers `"cannot disconnect local"` |

### Sessions

| Op | Args | → | Notes |
|---|---|---|---|
| `session.attach` | `sid`, `rows`, `cols` (1..=1000) | `{}` | then an `attached` event and the snapshot. First subscriber of the session: the server attaches at `rows`×`cols`; later ones: the server re-attaches at the session's current size (no resize), so every subscribed tab gets a fresh `attached` + snapshot |
| `session.detach` | `sid` | `{}` | the server detaches from the daemon when the last subscriber leaves |
| `session.resize` | `sid`, `rows`, `cols` | `{}` | send only from the focused pane, debounced 250 ms, never in reply to `attached` |
| `session.spawn` | `host`, `cwd`, `rows`, `cols`, `proxy` (see below), `first_party?` (bool), `resume?` (conversation id), `name?`, `prompt?` (≤ 64 KiB), `skip_plugins?` ([name]) | `{"sid":uuid}` | `proxy`: `{"kind":"default"}` (`[proxy] default`; direct when none), `{"kind":"direct"}`, `{"kind":"profile","name":"work"}`. `first_party` without a proxy answers `"first-party needs a proxy profile"`. With `prompt`, the session's `prompt_pending` is true until the server pasted it (or gave up after 60 s with a `notice`). Waits ≤ 20 s for pending plugin manifests |
| `session.shell` | `host`, `cwd`, `rows`, `cols`, `name?` | `{"sid":uuid}` | a terminal tab (login shell) |
| `session.respawn` | `sid`, `fresh` (bool), `rows`, `cols`, `proxy?`, `first_party?` | `{"sid":uuid}` | resume unless `fresh`. Without `proxy`, the server uses what it knows; when it knows nothing (and the session is claude): `"proxy unknown"`; the UI asks (default/direct/profile, pre-selecting `[proxy] default`) and sends again with `proxy` |
| `session.kill` | `sid` | `{}` | |
| `session.rename` | `sid`, `name` (string ≤ 200 chars, or null to clear) | `{}` | |
| `session.paste` | `sid`, `text` (≤ 64 KiB), `submit` (bool) | `{}` | wrapped in bracketed paste; `ESC[201~` and C0 controls other than `\t`/`\n` removed (CR/CRLF become `\n`); `\r` after it when `submit`. `"too large"` past 64 KiB |
| `session.approve` | `sid`, `decision` (`"approve"`\|`"deny"`), `attention_seq` (number) | `{}` | accepted only when the server's state is `needs_approval`, its attention kind is `permission_prompt`/`worker_permission_prompt` and its seq equals `attention_seq`: `approve` sends `\r` (Enter), `deny` sends `ESC`. Else `"stale"`, or `"not a permission prompt"` for another kind with the right seq. On a host without the `attention` feature: `attention_seq` must be `0` |
| `session.order` | `sids` ([uuid]) | `{}` | the board order; saved, and sent to the other tabs as `order` |

### Files and conversations

| Op | Args | → |
|---|---|---|
| `fs.list` | `host`, `path` | `{"path","entries":[DirEntry],"truncated"}` |
| `fs.projects` | `host`, `limit?` (default 50) | `{"dirs":[ProjectDir]}` |
| `claude.sessions` | `host`, `cwd` | `{"cwd","sessions":[ClaudeSession]}` (newest first) |
| `reader.answers` | `sid` | `{"answers":[Answer]}` newest first |
| `reader.last_prompt` | `sid` | `{"prompt": string\|null,"cut": bool}` |
| `search` | `query` | `{"hits":[SearchHit],"failed":[{"host","error"}],"truncated": bool}` |

`search` asks every connected host with the `search` feature, one request per host at a time
(queued; 10 s deadline counting the wait); a host answering `search busy` is asked once more
after 500 ms, then listed in `failed`. Hits are merged newest first.

### Git (read-only)

| Op | Args | → | Notes |
|---|---|---|---|
| `git.status` | `sid` | GitWorktreeStatus | needs the host's `worktree` feature, else `"unsupported by this host's daemon"` |
| `git.worktree_diff` | `sid`, `path`, `staged` | GitPatch | `path` exactly as `git.status` listed it; anything else: `"no such path"` |
| `git.log` | `sid`, `all?`, `skip?`, `limit?` (default 100, ≤ 500) | GitLogPage | |
| `git.commit` | `sid`, `oid` (full hash) | GitCommitInfo | |
| `git.diff` | `sid`, `oid`, `file?` (index into the commit's `files`) | GitPatch | whole commit without `file` |

### Proxy

| Op | Args | → |
|---|---|---|
| `proxy.stats` | `profile`, `window` (`"24h"`\|`"7d"`\|`"30d"`) | `{"profile","window","user_name","totals":{"requests","errors","input_tokens","output_tokens","cache_read","cache_creation"},"by_model":[{"model","requests","errors","input_tokens","output_tokens","cache_read","cache_creation"}],"limit":{"output_tokens","window_seconds","used_output_tokens","used_pct","blocked","blocked_until"}\|null,"pool":[{"name","status"}]\|null}` |
| `proxy.credential` | `sid` | `{"credential":{"name": string\|null,"plan": string\|null,"provider","five_hour_pct": n\|null,"seven_day_pct": n\|null,"switched_secs_ago": n\|null}\|null}` (null: the proxy does not know the conversation yet) |
| `proxy.switch` | `sid` | `{"from": string\|null,"to": string\|null,"state": string}` |

`proxy.credential`/`proxy.switch` need a session with a proxy profile and a
`claude_session_id`; else `"no proxy for this session"` / `"no conversation yet"`.

### Snippets, extensions, plugins

| Op | Args | → | Notes |
|---|---|---|---|
| `snippets.list` | — | `{"snippets":[{"name","text"}]}` | |
| `snippets.save` | `text` (≤ 64 KiB) | `{"name"}` | |
| `extensions.list` | — | `{"extensions":[Extension]}` | registry order |
| `extensions.toggle` | `name`, `on` | `{}` | writes `[extensions] name = on` in config.toml; applies to new sessions |
| `extensions.action` | `ext`, `action`, `input?`, `sid?` | `{"result":ActionResult}` | external extensions only; a built-in answers `"not an external extension"` (search and snippets are UI features) |
| `extensions.render` **(decided here)** | `lens`, `kind` (`"tag"`\|`"fence"`), `source` | `{"inline": string\|null,"block":[string]\|null}` | an external lens item rendered by its program, as the TUI's reader does; both null when it has no rendering |
| `plugins.list` | — | `{"plugins":[{"name","version","profile","enabled","refused": string\|null,"choices_saved": bool,"manifest": "fetching"\|"ready"\|"failed","manifest_error": string\|null,"description": string}]}` | |
| `plugins.items` | `name` | `{"name","version","items":[{"id","section","title","checked"}]}` | `section` is the manifest section's heading; `checked` = not in the saved `disabled` (all checked on first sight) |
| `plugins.save` | `name`, `disabled` ([item id]) | `{}` | saves `[plugins.<name>]` and asks every connected host again |
| `plugins.status` | — | `{"status":[PluginStatus]}` | one per (host, plugin) asked; same shape as the `plugin_status` event minus `ev` |

### Claude, layout

| Op | Args | → | Notes |
|---|---|---|---|
| `claude.update` | `host`, `install` (bool) | `{}` at once | progress and result come as `claude` events |
| `claude.skip` **(decided here)** | `host` | `{}` | closes that host's offer for this server run (state.json is never written) |
| `layout.save` | `layout` | `{}` | validated (§4); saved and sent to other tabs as `layout` |

## 6. Events

| `ev` | Fields | When |
|---|---|---|
| `host` | `host`: Host | any change of a host |
| `host_removed` | `name` | `hosts.disconnect` |
| `host_stats` | `host`, `cpu` (0–100), `mem_used`, `mem_total` | every ~2 s per connected host |
| `session` | `session`: Session | created or changed (the whole DTO) |
| `session_removed` | `sid` | killed, exited cleanly, or gone after a reconnect |
| `attached` | `sid`, `rows`, `cols` | to each subscribed tab, whenever the daemon (re)attaches; a pane sized differently shows the foreign size in its header and never answers with `session.resize` |
| `lens` | `sid`, `seq`, `items`: [{`tag`,`text`}] | the session's whole tag-lens list, oldest first (a host without the `lens` feature: its formulas as `latex` items) |
| `latex` | `sid`, `seq`, `formulas`: [string] | sent with every `lens`, the `latex` items' texts, same `seq` |
| `notice` | `text`, `level` (`"info"`\|`"warn"`\|`"error"`), `host`: string\|null, `sid`: uuid\|null | |
| `plugin_status` | `host`, `name`, `state` (`"pending"`\|`"ready"`\|`"failed"`\|`"too_old"`), `installed` [id], `present` [id], `skipped` [{`id`,`reason`,`retry_secs`: n\|null}], `dropped` [string], `error`: string\|null | an `Applied` answer changed |
| `claude` | `host`, `phase` (`"offer_install"`\|`"offer_update"`\|`"installing"`\|`"updating"`\|`"done"`\|`"failed"`), `have`: string\|null, `want`: string\|null, `version`: string\|null, `tail`: string\|null (≤ 2 KiB) | install/update offers and progress |
| `order` | `sids` | another tab reordered |
| `layout` | `layout` | another tab saved a layout |
| `profiles` | `names`, `default` | config.toml's proxy profiles changed |
| `extensions_changed` | — | the registry changed (a toggle, or config.toml re-read): call `extensions.list` |

Seq rules for `lens`/`latex`: keep the list with the higher `seq`; a lower one is dropped;
after an `attached` event or a `session` event whose session was just (re)created
(`state` `starting`), any `seq` is taken.

## 7. Rendering rules the client must follow

- The terminal is the truth: xterm.js owns its canvas; Preact never re-renders it. OSC 52
  clipboard writes are ignored (no clipboard addon); links open only after `confirm()`.
- Markdown (answers, `document` action results): `marked` output through DOMPurify
  (`<script>`, `<style>`, `<iframe>`, `<form>`, `<input>`, `style=` and `on*=` forbidden).
- `<latex>` tags: KaTeX with `trust:false`, `maxSize: 20`, `maxExpand: 1000`,
  `throwOnError: true` (an error renders the source as `<code>`), `strict: "ignore"`. A tag
  alone on its line is display math, else inline. Tags inside code are text.
- ```` ```mermaid ```` fences: mermaid loaded on first use, `securityLevel: "strict"`,
  `htmlLabels: false`, `startOnLoad: false`; the SVG goes through DOMPurify's SVG profile.
  A diagram whose type is not available renders as a code block with a notice.
- Code fences: highlight.js (common languages), a copy button per block.
- Everything else from the server (labels, attention text, git text, hosts): text nodes only.
