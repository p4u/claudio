# Custom environment variables for Claude Code sessions: design

Status: approved (2026-11-15)   Class: architectural (config format, a new 0600 state file, web protocol ops,
both front-ends, a security rule exception)
Owner decisions so far (2026-11-15):
- A menu in the TUI and in the web UI to set custom env vars for the Claude Code instances
  claudio runs, at two scopes: **global** (every session, every host) and **per session**.
- Per-session values are stored on the client in a 0600 file, never on a daemon.
- Values are **always shown**, in the TUI and in the browser.
- A var that a launch addon also sets (the proxy profile's `ANTHROPIC_BASE_URL`, token, default
  models…) is overridden by the addon: the proxy wins.
- Claude sessions only. Terminal (shell) tabs are unchanged.

## Problem

A user who wants every Claude Code session to run with, say, `ANTHROPIC_DEFAULT_OPUS_MODEL`,
`MAX_THINKING_TOKENS`, `BASH_DEFAULT_TIMEOUT_MS` or `HTTPS_PROXY` has two options today:

- export it in the shell that started the daemon, which does not reach remote hosts and is lost
  when the daemon restarts from ssh;
- put it in `~/.claude/settings.json` `env` on every host by hand.

claudio itself passes env to a session only from launch addons (`LaunchPart.env`,
`crates/core/src/app/launch/compose.rs:32-51`). The core contributes none, and there is no
per-session env at all.

Claude Code 2.1.323 documents 438 environment variables (research catalogue:
`.delivery/2026-11-15-custom-env/claude-code-env-vars.md`, from code.claude.com's env-vars,
monitoring, network-config and settings pages, checked against the installed binary).

## Goals

1. **Global env.** The user sets, edits and deletes env vars that every Claude Code session
   claudio launches gets, on every host, from either front-end. This includes `--plain` sessions
   (global scope only: `--plain` has no menu).
2. **Per-session env.** The user sets, edits and deletes env vars for one open Claude session.
   They override global values of the same name for that session.
3. **When changes apply.** Changes apply to the next launch of the session: a new session, a
   restart (Alt+e / Restart), a respawn of a dead or dormant session, or a recovery after a daemon
   restart. A running claude process is never changed, and both menus say so.
4. **Catalogue.** Both menus offer the documented Claude Code variables as suggestions, each with
   its category and a one-line description, and show the description of a known name.
5. **Refused names.** Names that would break claudio's sessions are refused with a reason. The
   refused set is `SESSION_MARKERS` (`crates/core/src/claude/mod.rs:32`), plus `PATH`, `TERM`,
   `COLORTERM`, `CLAUDE_CODE_REMOTE`, `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_SIMPLE` and
   `CLAUDE_CODE_SAFE_MODE` (the last two disable the hooks claudio relies on).
6. **Proxy wins.** When a launch addon sets a var the user also set, the addon's value is used.
   The launch reports the overridden names in one notice, naming keys only.

## Non-goals

- Shell (terminal) tabs, the print mode (`-p`) and the API server (`--api`) are unchanged.
- Per-host env, env removal (unsetting a var the daemon has) and typed per-variable controls (a
  toggle per boolean) are out. The catalogue's descriptions say which flags are presence-only.
- No attempt to beat `~/.claude/settings.json`: an `env` entry there overrides the launch env in
  Claude Code. The docs say so, and nothing reads or writes `~/.claude`.
- No change to the daemon or its protocol: env still travels in `Spawn`/`Respawn` env.
- Env is not shared between client machines. Both scopes belong to the machine whose claudio
  (TUI or web server) launches the session: another machine that restarts or recovers the
  session applies its own global env and no per-session env. docs/web.md and the README say so.
  (The daemon keeps a session's last spawn env in memory for its own `--resume` retry,
  `daemon/admit.rs:34-39`, `lifecycle.rs:21-30`, so a retry never changes a running session's env.)
- The passthrough (`claudio <args>` without `-p`) execs claude with the caller's environment and
  applies nothing.

## Current state

- **Composer.** `compose(base, core: SessionExtensions, parts)` (`app/launch/compose.rs:32`)
  layers each addon part's `env` in registry order, with later keys replacing earlier ones (:38-51).
  It unions `env_remove` (:52), writes `Spawn`/`Respawn` env (:75-92) and checks `frame_fits`
  (`:95`, `:103`).
  - The TUI (`crates/tui/src/addon_runner.rs:466-530`), `--plain` and the web
    (`addons/web/src/spawn.rs:144`, `:253`, `:345`) all build a `LaunchSpec` and go through it.
  - That includes recovery (`LaunchOp::Recover`, `crates/tui/src/sessions.rs:691`) and respawn.
- **Daemon.** It applies `spec.env` last (`daemon/command.rs:81-98`). It validates `env_remove`
  names (`:141`) but not `env` names. Shells get no spec env (`admit.rs:99-103`). The journal
  never holds env; after a restart the client re-sends it (`docs/manager-design.md` §2.3).
- **Config.** `config.toml` (0600, already holding `[proxy]` tokens) has core sections in
  `CORE_SECTIONS` (`addon/registry.rs:10`). `app/config_edit.rs` is its only writer:
  - `edit_section` (:51) edits minimally and atomically under `lock` (:24);
  - it supports flat scalar keys in `[section]`.
- **TUI.** No Alt+letter is free (the per-binding comments, `keymap.rs:187-188`). Claude 2.1.323 binds no Alt+punctuation (no
  `"alt+,"`/`"meta+,"` in the binary), and readline leaves Alt+, unbound.
  - Closest templates: the extensions menu (`extensions_view.rs`, `ui/extensions.rs`), the
    `Input` widget (`leader.rs:36`) and the filterable `Picker` (`leader.rs:94`).
- **Web.**
  - The protocol is version 2 (`docs/web-protocol.md`), and its §4 (`:187-188`, and again under
    "Addons", `:280`) says no DTO carries an env value.
  - Ops route in `addons/web/src/ws.rs:434`.
  - The closest page templates are `ui/extensions.js` (a server-backed list in a `Dialog`) and
    `ui/hosts.js` (a per-host setting).
  - The e2e fake claude records `FAKE_ENV_KEYS` (`e2e/fixtures/server.mjs:51`).

## Design

### Storage

- **Global**: a new core section `[env]` in `config.toml`, one string key per variable:

  ```toml
  [env]
  ANTHROPIC_DEFAULT_OPUS_MODEL = "claude-opus-5-5"
  BASH_DEFAULT_TIMEOUT_MS = "300000"
  ```

  - `"env"` is added to `CORE_SECTIONS`.
  - Written only through `config_edit::edit_section`.
  - `config_edit` writes a string that holds a newline on one line, as a TOML basic string with
    `\n` escapes (a change to `line_value`; the existing parse-back check still guards it), so
    every value this menu writes can be edited and removed again. A value someone wrote by hand
    as a multi-line `"""` string is listed, and editing or deleting it is refused with
    `SaveError::UnsupportedSection` ("edit [env] by hand").
  - `Config` gets no field. `app::env` reads the `[env]` table itself, as a `toml::Table`, each
    time it is asked, so nothing caches it and a bad entry never fails the rest of
    `config.toml`. An entry that is not a string, or fails the validator, is skipped with one
    notice (see "Notices and names"). The menu lists valid entries only; setting a name replaces
    whatever the file holds for it.
- **Per session**: `session-env.json`, next to `config.toml` (`paths::config_dir()`), mode 0600,
  written with `paths::write_atomic` under a sidecar lock (`paths::FileLock::beside`, as
  `config_edit::lock` does). The TUI and a web server on the same machine both write it.

  ```json
  {"version":1,"sessions":{"<session uuid>":{"host":"<host>","env":{"NAME":"value"}}}}
  ```

  - Lookups are by session id (client-made v4 UUIDs, unique across hosts). `host` is kept for
    pruning only.
  - `<host>` is the client's name for the host, as in `state.json` and the Session DTO: the
    literal `local`, or the ssh alias. The TUI and a web server on the same machine use the same
    names, so they share entries.
  - An entry is removed when its session is removed: on the daemon's `SessionEvent::Removed` for
    it (a kill from any client, or a clean exit), in whichever front-end sees the event; the web
    also forgets it on `session.kill`, the TUI when it sends `Kill`.
  - Entries of sessions removed while no front-end ran are pruned at recovery: when a host answers
    `ListSessions`, the entries of that host whose session it no longer lists are removed.
  - The file is capped at 8 MiB. `set` refuses a write that would pass it.
  - A missing file reads as empty. A file that is unreadable, not valid JSON, of a version this
    build does not know, or over the cap reads as empty with one notice, and every `set`, `unset`
    and `forget_session` then fails without writing ("session-env.json is not readable: fix or
    delete it"); `forget_session`'s failure is not shown. It is never overwritten.
- One module in the shared layer, `crates/core/src/app/env.rs` (A5), owns both scopes:
  `list(scope)`, `set(scope, name, value)`, `unset(scope, name)`, `forget_session(host, sid)`,
  and `effective(host, sid) -> Vec<(String, Value)>` (global overlaid with session).
- Values travel in one type, `env::Value(String)`, whose `Debug` prints `<n bytes>`. Every struct
  that carries a value outside `app::env` uses it: the TUI's effects, events and modal rows, the
  web's `env.set` args and `env.list` reply, and the composer's input. A unit test formats each
  with `{:?}` and checks the value is absent.

### Validation (one function, both writers, and again at compose)

- **Name:** `[A-Za-z_][A-Za-z0-9_]{0,127}`, not in the refused set (goal 5). Lowercase is
  allowed (`https_proxy`, `no_proxy`).
- **Value:** at most 8192 UTF-8 bytes; no character for which Rust's `char::is_control` is true
  except `\n` and `\t` (so NUL, `\r`, ESC, DEL and U+0080–U+009F are refused;
  `ANTHROPIC_CUSTOM_HEADERS` is newline-separated). Empty is allowed. The web turns `\r\n` into
  `\n` before it sends.
- **Count and total:** at most 128 vars per scope, and at most 128 KiB per scope for the sum of
  the names' and values' bytes. Sizes are UTF-8 bytes. `set` refuses a write that would pass
  either cap ("too many variables" / "environment too large"), so the two scopes together stay
  under a quarter of `MAX_FRAME` before JSON escaping.
- A hand-edited file past a cap: entries are taken in name order until a cap is reached; the rest
  are skipped with one notice giving their count.
- **At compose:** an entry that fails is skipped and named in a notice. The composer's
  `frame_fits` still guards the total.

### Composition

`compose` gains the user env as an input. The composer reads it itself, in `app::launch`'s
`send` (the one place every `Spawn` and `Respawn` of a claude session passes, after any review),
with `spawn_blocking`: `env::effective(host, sid)` for every `LaunchOp` (a new session has no
entry, so it gets the global scope). No front-end passes env, so no launch path can leave it out.
The files' directory is the directory of the runtime's `config_path`
(`RuntimeOptions.config_path`), which is what the tests set.

- **Order:** user env first, then each addon part as today. A part's key replaces the user's.
- **Removals win too.** After the parts are layered, every user key that is in the union of the
  parts' `env_remove` and that no part set is dropped from the env: an addon that removes a
  variable removes the user's value as well (the proxy's `ANTHROPIC_API_KEY`).
- Every user key a part replaced or removed is collected, and `compose` adds one notice: `custom
  env overridden by <addon>: NAME, NAME`. Keys only, never values. The front-ends show it for
  `New`, `Resume` and `Respawn`, and drop it for `Recover`.
- Respawn and recovery recompose, so they pick up the current values. Recovery after a daemon
  restart works because the client recomposes (§2.3).
- `retry_spawn_fresh` (in-memory `session_env`) keeps working unchanged, because the env is in
  the spawn.

### Catalogue

- `crates/core/src/claude/env_catalogue.tsv` is embedded with `include_str!`. It holds the
  documented variables only, one per line: `NAME\tcategory\tdescription`, with descriptions of
  at most 160 characters. Presence-only flags say so in the description.
- It is generated once from the research catalogue and committed as data, with a header comment
  that names the Claude Code version and the source URL.
- `claude::env_catalogue() -> &'static [(name, category, description)]` parses it once.
- A unit test checks the parse, unique names, and that no refused name is listed as settable.

### TUI

- **Opening it.** `Alt+,` (`Action::Environment`, `Scope::Global`) opens a modal
  `Modal::Environment`.
- **Two tabs, switched with Tab.** **Global**, and **Session: <name>** for the focused Claude
  session. The session tab is disabled with a reason on a terminal tab or with no session.
- **Rows.** `NAME = value` (in the escaped form below), with the selected row's catalogue
  description under the list.
- **Keys.**
  - `a`: add. Opens an `Input` for the name, filtered by the catalogue in a `Picker` (any name
    can be typed), then an `Input` for the value.
  - `Enter`: edit the value.
  - `d`/`Delete`: delete, with a confirm.
  - `Esc`: close.
- The footer says "applies on next start or restart (Alt+e)".
- **Escapes in the TUI.** The TUI shows and edits a value in one escaped form, in the row list
  and in the `Input` alike: a newline is `\n`, a tab is `\t`, a backslash is `\\`. On save the
  text is decoded: `\n` → newline, `\t` → tab, `\\` → `\`; a backslash before any other
  character, or at the end, is an error ("unknown escape \x: write \\ for a backslash") and
  nothing is saved. Decoding the shown form always gives the stored value back. The value prompt
  says "\n newline, \t tab, \\ backslash". The web edits raw text in a `<textarea>` and has no
  escapes.
- **Purity.** Reads and writes are `Effect`s run in `lib.rs` with `spawn_blocking` (A6): the
  modal opens on the list an effect brings back, and is re-read after each write. A failed write
  comes back as a notice that names the key and the reason, never the value.
- The help popup (generated), README and docs/web.md key tables list Alt+,.

### Web

- **Protocol** (`docs/web-protocol.md`, changed first in its own commit; version stays 2,
  because the ops are additive and the page and server ship together). New ops:
  - `env.catalogue` → `{vars:[{name, category, description}], refused:[{name, reason}]}`: the
    catalogue and the names goal 5 refuses, so the page can say why before it asks.
  - `env.list {scope:"global"}` | `{scope:"session", sid}` → `{vars:[{name, value}], skipped:
    n}`: the valid entries in name order, and how many entries of the file were skipped. `sid` is
    required with `"session"` and refused with `"global"` (`bad request`).
  - No event: the dialog lists when it opens and after each of its own writes.
  - `env.set {scope, sid?, name, value}` → `{ok}`, or an error naming the reason.
  - `env.unset {scope, sid?, name}` → `{ok}`.
  - Args use `deny_unknown_fields`. The server maps a `sid` to its host. A `sid` that names a
    terminal or an unknown session is refused.
  - §4's rule "no DTO carries an env value" gains one exception: `env.list` returns the values
    the user typed in this menu (owner decision). Addon env (proxy tokens, plugin env) still
    never reaches the page.
- **Page.** An **Environment** dialog (`ui/env.js`, `Dialog`), opened by Alt+, (`code: 'Comma'`,
  added to `TUI_BINDINGS`), a palette entry and a row in Settings.
  - Tabs: **Global**, and **This session** when the focused pane holds a Claude session.
  - A table of name, value (a `<textarea>` that grows for multi-line values) and delete.
  - An add row: a name input with a native `<datalist>` of catalogue names, and the
    description shown once the name matches.
  - A note: "Applies on next start or restart."
  - testids: `env`, `env-tab-global`, `env-tab-session`, `env-row` (`data-name`),
    `env-value`, `env-delete`, `env-add-name`, `env-add-value`, `env-add`, `env-error`.
- `session.kill` and a `Removed` event also forget the session's per-session env.

### Docs

- README: the `[env]` block and the key.
- docs/web.md: the parity row and the key tables.
- docs/web-protocol.md: the ops, DTOs and the §4 exception.
- docs/manager-design.md §2.5 (claude-proxy integration): the precedence of custom env and
  addon env.
- docs/addons.md: the composer's user env and the precedence.
- CLAUDE.md:
  - the secrets bullet: `config.toml [env]` and `session-env.json` are 0600 and may hold
    secrets; custom env values are shown in both UIs by owner decision;
  - the composer paragraph;
  - the A3 services list gains `app::env` (web).
- ACCEPTANCE.md C1 gains the same one-line exception.

### Alternatives considered

| Option | Why not |
|---|---|
| Store per-session env on the daemon | Owner chose the client: secrets never land on remote disks. |
| `--settings` `env` instead of process env | It would beat `~/.claude/settings.json`, but it changes argv handling and mixes with the user's own `--settings`; spawn env is what the composer already does. |
| Typed controls per catalogue variable | 438 variables with presence-only, enable and disable semantics; descriptions carry that, and free text covers every case. |
| Per-session env in `state.json` / `web-state.json` | Those files must never hold secrets (CLAUDE.md). |

## Compatibility and rollout

- **No daemon or wire change.** Old daemons get the env in `Spawn`/`Respawn` as today. An old
  binary ignores `[env]`, which is preserved by `ConfigFile.extra`, and `session-env.json`.
- **The page and the server** ship together. A page that sends `env.*` to an older server gets an
  "unknown op" error, which the dialog shows.
- **Rollback:** the old binary stops applying the vars. The files stay and are inert.

## Failure modes and security

- **Values are secrets at rest.** Both files are 0600. They are never logged, never in `Debug`
  output (keys only, as `LaunchPart`), never in notices and never in argv. The browser gets values
  only through `env.list`, over the authenticated WebSocket.
- **Untrusted input.**
  - Names and values from the page, the TUI and hand-edited files pass one validator (C2:
    bounded, never panics).
  - A refused name in a file is skipped at compose with a notice.
- **Concurrency.** The TUI and the web server write `config.toml` through `config_edit`'s lock,
  and `session-env.json` under its own sidecar lock with read-modify-write inside it.
- **Notices and names.** A notice names a key only when the key passes the name rule (so it is
  at most 128 ASCII characters); entries whose key does not are reported by count ("2 entries of
  [env] skipped: bad name"). A hand-edited key may be a value typed on the wrong side.
- **The exception is one message.** Only the `env.list` reply carries values. `env.set` errors,
  notices, events, `Debug` and logs name keys at most; a value the validator refuses is never
  echoed back.
- **Trust.** A signed-in browser can already open a shell on every connected host, so reading
  and setting env gives it nothing new. A global var it sets stays in `config.toml` and applies to
  sessions launched later from the TUI; this is accepted. `env.set` and `env.unset` log `web op`
  at INFO with the op, the scope, the sid and the name, never the value.
- **Where values end up.** On a remote host the values are in the daemon's memory and in the
  claude process's environment, readable by that user and root, and inherited by every command
  claude runs; a command that prints its environment puts them in the transcript on that host.
  docs/web.md says so next to the menu's description.
- **settings.json precedence.** A user `env` in `~/.claude/settings.json` silently wins in Claude
  Code. This is documented, not detected.
- **A var that breaks claude** (a bad `ANTHROPIC_BASE_URL`) breaks only the sessions that get it.
  The menu can delete it, and a restart fixes the session.

## Acceptance criteria

- [ ] AC1: core unit tests cover the validator (good and bad names, the refused set, value
  limits including `\n`/`\t` allowed and NUL, `\r` and U+0085 refused, the count and total
  caps) and catalogue parsing (unique names, no refused names); the TUI escape codec:
  `decode(encode(v)) == v` for values with newlines, tabs and backslashes; `C:\new` is refused;
  `C:\\new` gives `C:\new`; every value-carrying type's `Debug` omits the value.
- [ ] AC2: `app::env` unit tests cover:
  - global set/unset round-trips through `config.toml`, preserving other sections and comments;
  - session set/unset/forget in `session-env.json`, created 0600;
  - `effective` (session overrides global);
  - a corrupt or oversize file reads as empty with a notice;
  - two writers under the lock lose no update;
  - a global value with a newline is set, changed and unset, and stays on one line in
    `config.toml`;
  - an `[env] X = 1` entry is skipped and the rest of `config.toml` still loads;
  - an entry of a host is pruned when the host's list no longer has its session; another host's
    entries stay; an unreadable `session-env.json` makes `set` fail and is left untouched.
- [ ] AC3: composer unit tests:
  - user env reaches `Spawn` and `Respawn` env;
  - an addon part's key replaces a user key, and one notice names it (keys only);
  - a user key named in a part's `env_remove` is absent from the `Spawn`/`Respawn` env and named
    in the notice;
  - an invalid user entry is skipped with a notice;
  - no change to the output for an empty user env.
- [ ] AC4: TUI integration test, using the env-recording fake claude:
  - Alt+, opens the menu, adds a global var and a session var for the focused session;
  - restart (Alt+e), and the fake claude records both, the session value winning over a global
    one of the same name;
  - a second session gets only the global var;
  - closing the first session removes its entry from `session-env.json`, and so does a clean
    exit of a session;
  - typing `ANTHROPIC_DEF` in the add prompt offers `ANTHROPIC_DEFAULT_OPUS_MODEL`, and its
    description shows under the list once added; adding `PATH` shows the refusal and saves
    nothing; the footer reads "applies on next start or restart"; on a terminal tab the session
    tab is disabled and a new terminal tab's shell does not have the global var;
  - a `--plain` session records the global var.
- [ ] AC5: TUI recovery test: a session with a per-session var keeps it after a daemon stop and
  a new TUI recovers it.
- [ ] AC6: web Rust test: `env.set`/`env.list`/`env.unset` for both scopes, and the refused-name
  and unknown-`sid` errors. A spawned session's recorded env has the vars. The proxy case is in
  `vocdoni_web` (`addons/vocdoni/tests/web.rs`): a proxy session's `ANTHROPIC_BASE_URL` stays the
  proxy's, and a proxy session launched with a global `ANTHROPIC_API_KEY` records no
  `ANTHROPIC_API_KEY`.
- [ ] AC7: web e2e (`env.spec.mjs`):
  - the dialog adds a global and a session var;
  - a restart records them (the spec file starts its server with `FAKE_ENV_KEYS` extended by the
    names it sets);
  - the catalogue datalist offers `ANTHROPIC_DEFAULT_OPUS_MODEL` with its description;
  - zero CSP violations.
- [ ] AC8: the help popup, README and docs/web.md list Alt+,. A keymap unit test checks Alt+,
  collides with nothing.
- [ ] AC9: `make check` and `make matrix` pass; `make test-manager`, the web suites and the node
  tests pass.

## Constraints

- ACCEPTANCE.md applies:
  - A2/A3: the core and the TUI never name an addon;
  - A5: one implementation in `app/` for both front-ends;
  - A6: `app.rs` stays pure;
  - B3: config edits are minimal, atomic and serialised;
  - C1 with the owner's exception, C2 and C4;
  - F1: docs in the same branch;
  - F2: conventional commits, one change each.
- docs/web-protocol.md changes first, in its own commit. DESIGN.md governs the dialog.
- Commits: `type(scope): lowercase summary`, with no AI attribution and no `Co-Authored-By`
  trailers.

## Open questions

None.
