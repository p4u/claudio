# Custom environment variables for Claude Code sessions: design

Status: draft   Class: architectural (config format, a new 0600 state file, web protocol ops,
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
   claudio launches gets, on every host, from either front-end.
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

## Current state

- **Composer.** `compose(base, core: SessionExtensions, parts)` (`app/launch/compose.rs:32`)
  layers each addon part's `env` in registry order, with later keys replacing earlier ones (:38-51).
  It unions `env_remove` (:52), writes `Spawn`/`Respawn` env (:75-92) and checks `frame_fits`
  (:101).
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
- **TUI.** No Alt+letter is free (`keymap.rs:107`). Claude 2.1.323 binds no Alt+punctuation (no
  `"alt+,"`/`"meta+,"` in the binary), and readline leaves Alt+, unbound.
  - Closest templates: the extensions menu (`extensions_view.rs`, `ui/extensions.rs`), the
    `Input` widget (`leader.rs:36`) and the filterable `Picker` (`leader.rs:94`).
- **Web.**
  - The protocol is version 2 (`docs/web-protocol.md`), and its §7 says no DTO carries an env
    value.
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
  - Read by `Config` (a new `env: BTreeMap<String, String>` field; an unknown or invalid entry
    is skipped with one notice naming the key).
  - A hand-edited non-string value (`X = 1`) is refused by the menu's writer with the existing
    `SaveError::UnsupportedSection` path, and skipped when read.
- **Per session**: `session-env.json`, next to `config.toml` (`paths::config_dir()`), mode 0600,
  written with `paths::write_atomic` under a sidecar lock (`paths::FileLock::beside`, as
  `config_edit::lock` does). The TUI and a web server on the same machine both write it.

  ```json
  {"version":1,"sessions":{"<host>\u0000<session uuid>":{"NAME":"value"}}}
  ```

  - The local host's key is the same host name the connections layer uses for it.
  - An entry is removed when its session is closed (killed and removed) from either front-end.
  - Stale entries for sessions closed elsewhere are harmless and stay.
  - A file that is missing, unreadable, of the wrong version or over 1 MiB reads as empty, with
    one notice. It is never rewritten silently.
- One module in the shared layer, `crates/core/src/app/env.rs` (A5), owns both scopes:
  `list(scope)`, `set(scope, name, value)`, `unset(scope, name)`, `forget_session(host, sid)`,
  and `effective(host, sid) -> Vec<(String, String)>` (global overlaid with session).

### Validation (one function, both writers, and again at compose)

- **Name:** `[A-Za-z_][A-Za-z0-9_]{0,127}`, not in the refused set (goal 5). Lowercase is
  allowed (`https_proxy`, `no_proxy`).
- **Value:** at most 8 KiB, no NUL, and no control characters other than `\n` and `\t`
  (`ANTHROPIC_CUSTOM_HEADERS` is newline-separated). Empty is allowed.
- **Count:** at most 128 vars per scope.
- **At compose:** an entry that fails is skipped and named in a notice. The composer's
  `frame_fits` still guards the total.

### Composition

`compose` gains the user env as an input. `LaunchSpec` gets `user_env: Vec<(String, String)>`,
filled by the caller from `env::effective(host, sid)`, or global only for a new session.

- **Order:** user env first, then each addon part as today. A part's key replaces the user's.
- Every user key that a part replaced is collected, and `compose` adds one notice: `custom env
  overridden by <addon>: NAME, NAME`. Keys only, never values.
- Respawn and recovery recompose, so they pick up the current values.
- The respawn and recovery paths must pass the session's `effective` env.
- Recovery after a daemon restart works because the client recomposes (§2.3).
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
- **Rows.** `NAME = value`, with the selected row's catalogue description under the list.
  A multi-line value shows `⏎` for newlines.
- **Keys.**
  - `a`: add. Opens an `Input` for the name, filtered by the catalogue in a `Picker` (any name
    can be typed), then an `Input` for the value.
  - `Enter`: edit the value.
  - `d`/`Delete`: delete, with a confirm.
  - `Esc`: close.
- The footer says "applies on next start or restart (Alt+e)".
- **Multi-line values.** The TUI `Input` is single-line: in the TUI, `\n` typed in a value is
  stored as a newline, and a newline in a stored value is shown and edited as `\n`. A literal
  backslash-n is written as `\\n`.
- **Purity.** All writes are `Effect`s run in `lib.rs` with `spawn_blocking` (A6). Results come
  back as a notice.
- The help popup (generated), README and docs/web.md key tables list Alt+,.

### Web

- **Protocol** (`docs/web-protocol.md`, changed first in its own commit; version stays 2,
  because the ops are additive and the page and server ship together). New ops:
  - `env.catalogue` → `{vars:[{name, category, description}]}`.
  - `env.list {scope:"global"}` | `{scope:"session", sid}` → `{vars:[{name, value}],
    refused:[names], session_name?}`.
  - `env.set {scope, sid?, name, value}` → `{ok}`, or an error naming the reason.
  - `env.unset {scope, sid?, name}` → `{ok}`.
  - Args use `deny_unknown_fields`. The server maps a `sid` to its host. A `sid` that names a
    terminal or an unknown session is refused.
  - §7's rule "no DTO carries an env value" gains one exception: `env.list` returns the values
    the user typed in this menu (owner decision). Addon env (proxy tokens, plugin env) still
    never reaches the page.
- **Page.** An **Environment** dialog (`ui/env.js`, `Dialog`), opened by Alt+, (added to the
  shared key table), a palette entry and a row in Settings.
  - Tabs: **Global**, and **This session** when the focused pane holds a Claude session.
  - A table of name, value (a `<textarea>` that grows for multi-line values) and delete.
  - An add row: a name input with a native `<datalist>` of catalogue names, and the
    description shown once the name matches.
  - A note: "Applies on next start or restart."
  - testids: `env`, `env-tab-global`, `env-tab-session`, `env-row` (`data-name`),
    `env-value`, `env-delete`, `env-add-name`, `env-add-value`, `env-add`, `env-error`.
- `session.close` also forgets the session's per-session env.

### Docs

- README: the `[env]` block and the key.
- docs/web.md: the parity row and the key tables.
- docs/web-protocol.md: the ops, DTOs and the §7 exception.
- docs/manager-design.md §2.5: env sources.
- docs/addons.md: the composer's user env and the precedence.
- CLAUDE.md:
  - the secrets bullet: `config.toml [env]` and `session-env.json` are 0600 and may hold
    secrets; custom env values are shown in both UIs by owner decision;
  - the composer paragraph;
  - the A3 services list, if the web uses `app::env`.
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
- **settings.json precedence.** A user `env` in `~/.claude/settings.json` silently wins in Claude
  Code. This is documented, not detected.
- **A var that breaks claude** (a bad `ANTHROPIC_BASE_URL`) breaks only the sessions that get it.
  The menu can delete it, and a restart fixes the session.

## Acceptance criteria

- [ ] AC1: core unit tests cover the validator (good and bad names, the refused set, value
  limits including `\n`/`\t` allowed and NUL refused, the count cap) and catalogue parsing
  (unique names, no refused names).
- [ ] AC2: `app::env` unit tests cover:
  - global set/unset round-trips through `config.toml`, preserving other sections and comments;
  - session set/unset/forget in `session-env.json`, created 0600;
  - `effective` (session overrides global);
  - a corrupt or oversize file reads as empty with a notice;
  - two writers under the lock lose no update.
- [ ] AC3: composer unit tests:
  - user env reaches `Spawn` and `Respawn` env;
  - an addon part's key replaces a user key, and one notice names it (keys only);
  - an invalid user entry is skipped with a notice;
  - no change to the output for an empty user env.
- [ ] AC4: TUI integration test, using the env-recording fake claude:
  - Alt+, opens the menu, adds a global var and a session var for the focused session;
  - restart (Alt+e), and the fake claude records both, the session value winning over a global
    one of the same name;
  - a second session gets only the global var;
  - closing the first session removes its entry from `session-env.json`.
- [ ] AC5: TUI recovery test: a session with a per-session var keeps it after a daemon stop and
  a new TUI recovers it.
- [ ] AC6: web Rust test: `env.set`/`env.list`/`env.unset` for both scopes, and the refused-name
  and unknown-`sid` errors. A spawned session's recorded env has the vars, and a proxy session's
  `ANTHROPIC_BASE_URL` stays the proxy's.
- [ ] AC7: web e2e (`env.spec.mjs`):
  - the dialog adds a global and a session var;
  - a restart records them (via `FAKE_ENV_KEYS`);
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
