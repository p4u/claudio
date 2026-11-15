# Custom env vars for Claude Code sessions: implementation plan

Spec: docs/specs/2026-11-15-custom-env-design.md
Class: architectural
Repos and branches: /home/p4u/claudio → feat/custom-env (base 15c1fa1)

Implementers work in the main checkout `/home/p4u/claudio` on `feat/custom-env`, one task at a
time, in the order below. Every code block was applied and run in a scratch worktree of main
3615ad9 (v0.9.9; main's next commit, ff85012, touches only addons/api/src/print/hooks.rs); the "Expected" lines are that run's real output (cargo's own lines; an RTK hook
may summarize them, `rtk proxy <cmd>` shows them raw).

Where the spec is loose, this plan decides (the controller may rule otherwise before Task 2):
- AC1 (spec :319) calls `C:\new` refused, but by the escape rules (spec :212-216) `\n` is an
  escape: `C:\new` decodes to "C:", a newline, "ew". The tests refuse `C:\Users` instead.
- The override notice is dropped for `Recover` once, in `app::launch::send` (both front-ends get
  it from there, A5), not in each front-end (spec :181).
- The TUI menu opens at once on "reading…" and fills when its read effect returns (spec :220).
- `env.set` and `env.unset` answer `{}` like every other op (spec :234-235 write `{ok}`).
- `forget_session(host, sid)` and `effective(host, sid)` look an entry up by sid and count it
  only on its own host.
- A8 is kept without growing the capped functions: the TUI's env effects reuse `run_effect`'s
  `Effect::Addon` arm (routed on to `env_edge`), a refactor commit moves the reader view's glue
  out of `interaction.rs`, and `hub.rs::connected` folds its `forget_unlisted` block.

## Global Constraints

- Commit convention: conventional commits `type(scope): lowercase summary`; no AI attribution,
  no Co-Authored-By or similar trailers; one logical change per commit (ACCEPTANCE F2). The
  commits of each task are given; use them as written.
- Never push; commit with explicit paths only (`git commit -m "…" -- <paths>`, a new file
  `git add <path>` first); never `git add -A` or `git add .`; never `git stash`; never switch
  branches.
- Build output never in /tmp: cargo's default `target/` (and `make check`'s `target/check`) are in
  the repo. Run tests with `TMPDIR=$HOME/.cache/cl-tmp` (create it): a TMPDIR whose path holds
  "claudio" fails two claudio-api tests that already exist (`print::hooks::tests::relay_command_*`,
  `settings_with_relay_has_no_wrapper_name`: the relay path must not name the wrapper).
- Leave files you did not create alone: the main checkout may hold someone else's uncommitted
  change; never stage, revert or format it (`cargo fmt` only after checking `git status`).
- ACCEPTANCE.md applies; each task names the rules it must respect. In short: A2/A3 the core and
  the TUI never name an addon, an addon uses the core's public API only; A5 one implementation
  in `crates/core/src/app/` for both front-ends; A6 `app.rs` and every `impl App` are pure, I/O
  runs in effects; A8 size budgets (below); B2 the web-protocol doc lands before its code; B3
  config edits are minimal, atomic and serialised (only through `config_edit::edit_section`);
  C1 with the owner's one exception (the custom env values typed in the menu are shown in both
  UIs and carried by the `env.list` reply, nowhere else); C2 every outside input bounded, no
  `unwrap`/`expect`/indexing on it; C4 files 0600 written with `paths::write_atomic`; D1 `cargo
  fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean; F1 docs in
  the same branch.
- Env values never appear in logs, notices, errors, `Debug`, argv or test snapshots (except the
  `env.list` reply); a notice or error names a key only when the key passes the name rule. Every
  type that carries a value outside `app::env` holds it as `claude::env::Value` (`Debug` prints
  `<n bytes>`).
- A8 budgets the code must keep (`scripts/check_rules.py`, `scripts/size-baseline.txt`): these
  are at their baseline and may not grow: `crates/tui/src/interaction.rs` file (1024) and its
  `modal_key` (113), `on_event` (91), `on_reply` (111); `crates/tui/src/lib.rs::run_effect`
  (123); `addons/web/src/hub.rs::connected` (119); `addons/web/src/hub.rs` file (2065, one line
  of room). `crates/tui/src/lib.rs` has 8 lines of room (814). New files stay under 800
  production lines, functions under 80. Task 5's first commit makes the room the TUI needs.
- Tests: the PTY harness asserts on the screen model only; never ESC right before an Alt+key
  (wait for the screen to change after an ESC); manager suites single-threaded; fake claudes
  ignore `--help`/`--version`; every test config has `update_check`/`remote_check`/`install =
  "off"` (the harness writes it); record a secret-like var with the `present` (`KEY=<set>`)
  pattern, a test's own sentinel values only with distinctive text.
- DESIGN.md governs the web dialog: tokens only (`var(--…)`), no hard-coded colour, radius or
  font; existing classes first (`.seg`, `.seg-btn`, `.input`, `.btn`, `.wz-note`, `.ext-bar`).
- The catalogue `crates/core/src/claude/env_catalogue.tsv` is generated data: never edit it by
  hand; Task 2 generates it once and checks its hash.

## Review Focus

Line numbers are those of the finished branch (the scratch run).

- The composer's layering, `crates/core/src/app/launch/compose.rs`: `OVERRIDDEN` (:20),
  `struct Env` (:114), `Env::new` (:131: the user's entries checked again), `Env::add` (:155: a
  part's key replaces a user key; a user key in a part's `env_remove` and set by no part is
  overridden), `Env::finish` (:185: one keys-only notice; the dropped keys). In
  `crates/core/src/app/launch/mod.rs`: `send` (:371) and `user_env` (:402): read off the
  executor, beside `rt.config_path()`, for every op; the `OVERRIDDEN` notice dropped for
  `LaunchOp::Recover` only (:389).
- `config_edit`'s string rendering, `crates/core/src/app/config_edit.rs`: `line_value` (:148) and
  `basic_string` (:160): only a string holding `\n`/`\r` changes form; `only_section_differs`
  (:105) still guards every write; a hand-written `"""` value stays refused (`set_key`'s
  multi-line check, :266).
- `session-env.json`, `crates/core/src/app/env.rs`: `SessionFile` (:94, no `Debug`), `set`
  (:158: the caps checked inside the edit, under the lock), `forget_session` (:216) and `prune`
  (:228: never create the file), `edit_sessions` (:274: sidecar `FileLock` held across
  read-modify-write; an unreadable file fails the write and is never overwritten; the 8 MiB
  cap), `read_sessions` (:328: bounded read, version check), `valid` (:356: name order, caps,
  one notice naming well-formed keys only), `fits` (:411).
- Every place a value could reach `Debug`, a log or a notice: `Value`'s `Debug`
  (`crates/core/src/claude/env.rs:84`), `Listed`, `EnvError` (`app/env.rs`), the TUI's
  `EnvCall` (`crates/tui/src/env_view.rs:34`), `EnvDone` (:54), `LineInput`'s `Debug` (:77),
  `EnvView` (:155), the web's `SetArgs` (`addons/web/src/env.rs:36`) and `log` (:127).
- The web ops' sid→host mapping and refusals: `scope_of` (`addons/web/src/env.rs:93`: `sid`
  only with `session`, the host from `hub.session`, terminals refused), `deny_unknown_fields`
  on every args struct, the ops routed at `addons/web/src/ws.rs` (`op if
  op.starts_with("env.")`).
- Forget and prune triggers: TUI `App::remove` (`crates/tui/src/sessions.rs:300`, claude
  sessions only: Kill, a `Removed` event, a cancelled launch) and `recover_host_inner` (:720,
  after the merge, with the daemon's whole list: dormant journaled sessions are in it); web
  `on_event` Removed (`addons/web/src/hub.rs:1459`), `connected` (:1618), `session.kill`
  (`addons/web/src/ws.rs:478`).
- TUI modal purity: `crates/tui/src/env_view.rs` does no I/O (`on_env` :429 only updates the
  modal and notifies); `crates/tui/src/env_edge.rs` (`run` :17, `apply` :30) runs the effects in
  `spawn_blocking`, results come back as `HostEvent::Env` with the modal's `gen`.

## Task order and parallelism

| Task | Depends on | Repo/files | May run in parallel with |
|---|---|---|---|
| 1 docs(web-protocol): the env.* ops | — | docs/web-protocol.md | none (one checkout) |
| 2 core: claude::env and the catalogue | 1 | crates/core/src/claude/ | none |
| 3 core: config_edit strings, app::env store | 2 | crates/core/src/app/, crates/core/src/addon/registry.rs | none |
| 4 core: the composer applies the custom env | 3 | crates/core/src/app/launch/, crates/core/src/app/addons/mod.rs | none |
| 5 TUI: the Alt+, menu, forget and prune | 4 | crates/tui/ | none |
| 6 web server: the env.* ops | 4 | addons/web/src/, addons/web/tests/web.rs, addons/vocdoni/tests/web.rs | none |
| 7 web page: the Environment dialog | 6 | addons/web/web/ | none |
| 8 docs | 5, 6, 7 | README.md, docs/, CLAUDE.md, ACCEPTANCE.md | none |

## Tasks

### Task 1: docs(web-protocol): the env.* ops, their DTOs and the env.list exception
Model: haiku   Repo: /home/p4u/claudio   Files: docs/web-protocol.md
Goal: B2, the protocol doc lands first (spec §Web "Protocol"); serves AC6/AC7.

Step 1: write the failing test
No test: a documentation change. Check it is not there yet:
Run: `grep -c "### Custom env" docs/web-protocol.md`
Expected: `0`

Step 2: implement
Apply this diff (it adds the version note, the §4 exception, the three DTOs and the
"Custom env" ops table; version stays 2):
````diff
diff --git a/docs/web-protocol.md b/docs/web-protocol.md
index 1086fde..78bb914 100644
--- a/docs/web-protocol.md
+++ b/docs/web-protocol.md
@@ -16,6 +16,10 @@ version-1 ops and args (`proxy.*`, `plugins.*`; `proxy`, `first_party`, `skip_pl
 `first_party`), snapshot field and events (`profiles`, `plugin_status`) are gone. Version 1's text
 is in git history.
 
+The custom env ops (`env.*`, §5) were added within version 2: they are additive, and the page
+and the server ship together (`docs/specs/2026-11-15-custom-env-design.md`). An older server
+answers them `"unknown op"`, which the page shows.
+
 ## 1. HTTP surface
 
 | Method and path | Cookie | Origin check | Answer |
@@ -185,7 +189,9 @@ Replies: `{"t":"res","id":<u32>,"ok":true,"data":<value>}` or
 All strings the server sends were sanitized by the daemon (`text::sanitize` or
 `daemon::git` sanitizing) and are still untrusted: render as text unless §7 says otherwise.
 Field order is not significant. No DTO ever carries a token, a plugin env value, `Spawn.env`,
-a socket path or a hook token.
+a socket path or a hook token. The one exception (owner decision): the reply of `env.list`
+(§5) carries the custom env values the person typed in the Environment menu; an addon's env
+(proxy tokens, plugin env) never reaches the page.
 
 ### Session
 
@@ -277,7 +283,8 @@ browsers.
 Every string here comes from an addon. The server cleans it before it is sent (control and
 format characters removed, one line, at most 4096 bytes; a segment at most 64 characters, a
 checklist at most 256 items, a panel over 64 KiB encoded replaced by an error page) and the
-page renders it as text (§7). No addon DTO carries an env value or a token.
+page renders it as text (§7). No addon DTO carries an env value or a token (the custom env of
+`env.list` is the person's, not an addon's).
 
 - Segment `{"addon","id","text","tone": "ok"|"warn"|"hot"|"muted"|"accent","priority": 0-255}`.
 - LaunchOption `{"id","label","group","default": bool,"detail": string|null}`: one entry of a
@@ -304,6 +311,11 @@ page renders it as text (§7). No addon DTO carries an env value or a token.
 - ExtensionRow `{"addon","name","description","on": bool,"refused": string|null,"details":[string],"setup": string|null}`
   (`setup`: the id of the checklist that sets it up, of the same addon).
 - Notice `{"text","level": "info"|"warn"|"error"}`.
+- EnvVar `{"name","category","description"}`: a documented Claude Code variable (the embedded
+  catalogue; `category` one word such as `model`, `auth`, `network`; `description` ≤ 160
+  characters, saying when a flag is presence-only). EnvRefused `{"name","reason"}`: a name the
+  menu refuses, and why. EnvEntry `{"name","value"}`: one custom env variable as stored
+  (`value` the raw text, newlines and tabs included).
 
 ## 5. Ops
 
@@ -400,6 +412,30 @@ the page sends (an extra arg answers `"bad request: …"`).
 | `addons.rows` | — | `{"rows":[ExtensionRow]}` | registry order; `on`/`refused` as the registry has them (a name a core extension holds is refused) |
 | `addons.row_action` | `addon`, `name`, `action`: `"toggle"` \| `"refresh"`, `on?` (bool; `toggle` only, required) | `{}` | `toggle` writes `[extensions] <name> = on` in config.toml, then tells the addon (a refused row answers `"refused: <why>"`); `refresh` has the addon refetch its rows' data. 15 s; then `extensions_changed` |
 
+### Custom env
+
+The person's env vars for the claude sessions this server (and the TUI on this machine)
+launches (`app::env`): `scope` `"global"` (`config.toml [env]`, every session on every host) or
+`"session"` (`session-env.json`, one open claude session, overriding a global var of the same
+name). They apply at the session's next start, restart, respawn or recovery; a running claude is
+never changed. Args use `deny_unknown_fields`. `sid` is required with `"session"` and refused
+with `"global"` (`"bad request: sid: …"`); a `sid` naming no session answers `"no such
+session"`, one naming a terminal `"not a claude session"`. A name is `[A-Za-z_][A-Za-z0-9_]{0,127}`
+and not one of `env.catalogue`'s `refused`; a value is at most 8192 UTF-8 bytes with no control
+character but newline and tab (the page turns `\r\n` into `\n` before it sends); a scope holds at
+most 128 variables and 128 KiB of names and values. A refusal names the variable and the reason,
+never the value.
+
+| Op | Args | → | Notes |
+|---|---|---|---|
+| `env.catalogue` | — | `{"vars":[EnvVar],"refused":[EnvRefused]}` | the documented variables in name order, and the names the menu refuses with the reason |
+| `env.list` | `scope`, `sid?` | `{"vars":[EnvEntry],"skipped": n}` | the valid entries in name order; `skipped` counts the entries of the file left out (not a string, a bad or refused name, a bad value, past a cap). The only reply that carries env values (§4) |
+| `env.set` | `scope`, `sid?`, `name`, `value` | `{}` | sets or replaces `name`; errors: `"bad request: …"`, `"<name>: refused: <why>"`, `"<name>: value …"`, `"too many variables"`, `"environment too large"`, `"could not save: <why>"`. Logged at INFO as `web op` with the op, scope, sid and name, never the value |
+| `env.unset` | `scope`, `sid?`, `name` | `{}` | removes `name` (absent: `{}` too); logged like `env.set` |
+
+No event: the dialog lists when it opens and after each of its own writes. `session.kill` and a
+session's removal (`session_removed`) also drop its per-session env.
+
 ### Claude, layout
 
 | Op | Args | → | Notes |
````
Run: `grep -nE '### Custom env|the reply of .env.list' docs/web-protocol.md`
Expected:
```
192:a socket path or a hook token. The one exception (owner decision): the reply of `env.list`
415:### Custom env
```

Step 3: commit
`git commit -m "docs(web-protocol): the env.* ops and the env.list exception" -- docs/web-protocol.md`

Interfaces this task provides to later tasks: the op names `env.catalogue`, `env.list`,
`env.set`, `env.unset`; args `scope` (`"global"`|`"session"`), `sid?`, `name`, `value`; replies
`{"vars":[{name,category,description}],"refused":[{name,reason}]}`,
`{"vars":[{name,value}],"skipped":n}`, `{}`; errors `"bad request: sid: …"`, `"no such
session"`, `"not a claude session"`, `"<NAME>: refused: <why>"`, `"too many variables (at most
128)"`, `"environment too large (at most 128 KiB)"`, `"could not save: <why>"`.

### Task 2: core: claude::env, the validator, the escapes and the catalogue
Model: sonnet   Repo: /home/p4u/claudio   Files: crates/core/src/claude/env.rs (new),
crates/core/src/claude/env_catalogue.tsv (new, generated), crates/core/src/claude/mod.rs
Goal: AC1: one validator for names and values, the refused set, the caps, `Value` with a
redacted `Debug`, the TUI's escape codec, and the embedded catalogue of documented variables.
Rules: A1 (Claude Code's own names only), C1 (no error quotes a value), C2 (bounded, no panic on
any input), D6 (every `pub` item documented), D7 (no new crate).

Step 1: write the failing test
Add `pub mod env;` to `crates/core/src/claude/mod.rs` (the diff in Step 2) and create
`crates/core/src/claude/env.rs` holding only the `#[cfg(test)] mod tests { … }` block of the
listing in Step 2 (from `#[cfg(test)]` to the end).
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib claude::env`
Expected: ``error[E0425]: cannot find function `check_name` in this scope`` (and more of its
kind); the build fails.

Step 2: implement
1. Generate the catalogue. Save this script OUTSIDE the repo as
   `~/.cache/claudio/gen_env_catalogue.py` (it is not committed: the TSV is the data):
```python
#!/usr/bin/env python3
"""Generate crates/core/src/claude/env_catalogue.tsv from the research catalogue.

Usage: gen_env_catalogue.py <claude-code-env-vars.md> > env_catalogue.tsv

Section 1 only (the documented variables). One line per variable:
NAME<TAB>category<TAB>description, description at most 160 characters. The
refused names (SESSION_MARKERS and the spec's goal 5) are left out.
"""
import re
import sys

REFUSED = {
    # SESSION_MARKERS (crates/core/src/claude/mod.rs)
    "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_SSE_PORT", "CLAUDE_SESSION_ID", "CLAUDE_PID", "CLAUDE_EFFORT",
    # goal 5
    "PATH", "TERM", "COLORTERM", "CLAUDE_CODE_REMOTE", "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_SIMPLE", "CLAUDE_CODE_SAFE_MODE",
}
PRESENCE_ONLY = {
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "DISABLE_TELEMETRY", "DISABLE_ERROR_REPORTING",
    "CLAUDE_CODE_TMUX_TRUECOLOR", "IS_DEMO",
}
CATEGORY = {
    "other": "other",
    "feature toggles/disable-*": "features",
    "limits/timeouts/tokens": "limits",
    "telemetry/OTEL": "telemetry",
    "provider (Bedrock/Vertex/Foundry/gateway)": "provider",
    "model selection": "model",
    "MCP": "mcp",
    "bash/tools": "tools",
    "auth": "auth",
    "UI/terminal": "ui",
    "paths/config": "paths",
    "network/proxy/TLS": "network",
    "IDE": "ide",
    "debugging": "debug",
}
MAX = 160


def shorten(text):
    """At most MAX characters: whole sentences that fit, else cut at a word with an ellipsis."""
    text = re.sub(r"\s+", " ", text).strip()
    if text.endswith("..."):
        text = text[:-3].rstrip()
        cut = True
    else:
        cut = False
    if len(text) <= MAX and not cut:
        return text
    sentences = re.split(r"(?<=[.!?])\s+", text)
    out = ""
    for s in sentences:
        joined = (out + " " + s).strip()
        if len(joined) > MAX or (cut and s is sentences[-1]):
            break
        out = joined
    if out:
        return out
    words = text[: MAX - 1].rsplit(" ", 1)[0].rstrip(" ,;:")
    return words + "…"


def main():
    lines = open(sys.argv[1], encoding="utf-8").read().split("\n")
    start = next(i for i, l in enumerate(lines) if l.startswith("## 1."))
    end = next(i for i, l in enumerate(lines) if l.startswith("## 2."))
    rows = []
    for line in lines[start:end]:
        if not line.startswith("| `"):
            continue
        cells = [c.strip() for c in re.split(r"(?<!\\)\|", line)[1:-1]]
        name = cells[0].strip("`")
        if name in REFUSED:
            continue
        category = CATEGORY[cells[1]]
        meaning = re.sub(r"\\(.)", r"\1", cells[4]).replace("\t", " ")
        if name in PRESENCE_ONLY:
            meaning = "Presence-only: any non-empty value turns it on, even 0 or false. " + meaning
        rows.append((name, category, shorten(meaning)))
    names = [r[0] for r in rows]
    assert len(names) == len(set(names)), "duplicate names"
    print("# Claude Code 2.1.323's documented environment variables: NAME<TAB>category<TAB>description.")
    print("# Source: https://code.claude.com/docs/en/env-vars (and the monitoring-usage, network-config")
    print("# and settings pages). Generated once from the research catalogue and kept as data; the names")
    print("# claudio refuses (app::env) are left out. Descriptions are cut at 160 characters.")
    for name, category, description in sorted(rows):
        assert re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", name), name
        assert len(description) <= MAX and "\t" not in description, name
        print(f"{name}\t{category}\t{description}")


main()
```
   Run, from the repo root (the research catalogue is the local-only file of the delivery):
   `python3 -I ~/.cache/claudio/gen_env_catalogue.py .delivery/2026-11-15-custom-env/claude-code-env-vars.md > crates/core/src/claude/env_catalogue.tsv && sha256sum crates/core/src/claude/env_catalogue.tsv && wc -l crates/core/src/claude/env_catalogue.tsv`
   Expected:
   `d4c59800e67f183eb2ec1c76bd900a15e66ca782c3e6c718104de2141eae423b  crates/core/src/claude/env_catalogue.tsv`
   `431 crates/core/src/claude/env_catalogue.tsv`
   Its first line is `# Claude Code 2.1.323's documented environment variables: NAME<TAB>category<TAB>description.`,
   its fifth `ANTHROPIC_API_KEY<TAB>auth<TAB>API key sent as X-Api-Key header. …`, its last
   `VERTEX_REGION_CLAUDE_HAIKU_5_5<TAB>provider<TAB>Override region for Claude Haiku 5.5 when using Google Cloud’s Agent Platform. Added in v2.1.293`.
   (427 variables: the 438 documented ones less the 11 refused names the docs list.) If the hash
   differs, stop and report: the research file changed.
2. `crates/core/src/claude/mod.rs`:
````diff
diff --git a/crates/core/src/claude/mod.rs b/crates/core/src/claude/mod.rs
index e70542f..1bc3cb2 100644
--- a/crates/core/src/claude/mod.rs
+++ b/crates/core/src/claude/mod.rs
@@ -2,6 +2,8 @@
 //!
 //! Sub-modules:
 //!
+//! - [`env`] — the custom env's names, values, caps and the catalogue of
+//!   Claude Code's documented variables.
 //! - [`hooks`] — hook event constants, settings builder, per-spawn token
 //!   generator, and the short-lived relay process.
 //! - [`latex`] — the `<latex>` formulas of claude's answers, and the
@@ -13,6 +15,7 @@
 //! - [`state`] — pure state machine that maps hook events onto
 //!   [`proto::SessionEvent`]s.
 
+pub mod env;
 pub mod hooks;
 pub mod latex;
 pub mod projects;
@@ -22,6 +25,8 @@ pub mod update;
 
 use std::process::{Command, ExitCode};
 
+pub use env::catalogue as env_catalogue;
+
 /// Variables a running claude sets for its own children. A process started
 /// from inside a claude session inherits them, and a session spawned with them
 /// believes it is a child: it turns off transcript saving (breaking `--resume`)
````
3. `crates/core/src/claude/env.rs` (new, whole file):
```rust
//! Claude Code's environment variables as claudio's custom env sees them
//! (docs/specs/2026-11-15-custom-env-design.md): the names claudio refuses,
//! the one validator of a name and a value, the caps, the TUI's escaped form
//! of a value, and the embedded catalogue of the documented variables.
//!
//! A value is a secret: it travels in [`Value`], whose `Debug` prints its
//! length only, and no error here ever quotes one (C1).

use std::fmt;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::SESSION_MARKERS;

/// The longest name, in bytes (ASCII).
pub const MAX_NAME: usize = 128;
/// The longest value, in UTF-8 bytes.
pub const MAX_VALUE: usize = 8192;
/// The most variables in one scope.
pub const MAX_VARS: usize = 128;
/// The most bytes of names and values together in one scope.
pub const MAX_SCOPE_BYTES: usize = 128 * 1024;

/// Why a name is refused, besides [`SESSION_MARKERS`] (refused because claude
/// sets them for its own children).
const REFUSED: &[(&str, &str)] = &[
    (
        "PATH",
        "the daemon sets it (the login shell's), and claude must stay on it",
    ),
    ("TERM", "the session's terminal sets it"),
    ("COLORTERM", "the session's terminal sets it"),
    ("CLAUDE_CODE_REMOTE", "Claude Code sets it for its host"),
    (
        "CLAUDE_CODE_ACCOUNT_UUID",
        "Claude Code sets it for its host",
    ),
    (
        "CLAUDE_CODE_SIMPLE",
        "it turns off the hooks claudio relies on",
    ),
    (
        "CLAUDE_CODE_SAFE_MODE",
        "it turns off the hooks claudio relies on",
    ),
];

/// Why a [`SESSION_MARKERS`] name is refused.
const MARKER: &str = "claude sets it for its own children; a session with it believes it is one";

/// An env value: a secret. Its `Debug` prints `<n bytes>`, never the text.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Value(String);

impl Value {
    /// `text` as a value (not checked: see [`check_value`]).
    pub fn new(text: impl Into<String>) -> Value {
        Value(text.into())
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The text, owned.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Its length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

/// Why a name or a value is not taken. Its text never quotes a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not `[A-Za-z_][A-Za-z0-9_]{0,127}`.
    BadName,
    /// A name claudio refuses, and why.
    Refused(&'static str),
    /// Over [`MAX_VALUE`] bytes.
    TooLong,
    /// A control character other than newline and tab.
    Control,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::BadName => write!(
                f,
                "not a variable name (letters, digits and _, not starting with a digit, at most {MAX_NAME})"
            ),
            Refusal::Refused(why) => write!(f, "refused: {why}"),
            Refusal::TooLong => write!(f, "value over {MAX_VALUE} bytes"),
            Refusal::Control => write!(
                f,
                "value holds a control character (only newline and tab are allowed)"
            ),
        }
    }
}

/// Whether `name` is `[A-Za-z_][A-Za-z0-9_]{0,127}`.
pub fn name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why claudio refuses `name` (exact case), if it does.
pub fn refused(name: &str) -> Option<&'static str> {
    if SESSION_MARKERS.contains(&name) {
        return Some(MARKER);
    }
    REFUSED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, why)| *why)
}

/// Every refused name and why: the session markers, then the others.
pub fn refused_names() -> Vec<(&'static str, &'static str)> {
    let markers = SESSION_MARKERS.iter().map(|n| (*n, MARKER));
    markers.chain(REFUSED.iter().copied()).collect()
}

/// A name a user may set: well formed and not refused.
pub fn check_name(name: &str) -> Result<(), Refusal> {
    if !name_ok(name) {
        return Err(Refusal::BadName);
    }
    match refused(name) {
        Some(why) => Err(Refusal::Refused(why)),
        None => Ok(()),
    }
}

/// A value a user may set: at most [`MAX_VALUE`] bytes, and no character
/// for which `char::is_control` holds but `\n` and `\t` (so NUL, `\r`, ESC,
/// DEL and U+0080–U+009F are refused). Empty is allowed.
pub fn check_value(value: &str) -> Result<(), Refusal> {
    if value.len() > MAX_VALUE {
        return Err(Refusal::TooLong);
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Refusal::Control);
    }
    Ok(())
}

/// `name` and `value` both taken; the error names the variable when its
/// name is well formed (never the value).
pub fn check(name: &str, value: &str) -> Result<(), String> {
    check_name(name).map_err(|e| shown(name, &e))?;
    check_value(value).map_err(|e| shown(name, &e))
}

/// `why` about variable `name`: `NAME: why` when the name is well formed,
/// else `why` alone (a bad key may be a value typed on the wrong side).
pub fn shown(name: &str, why: &impl fmt::Display) -> String {
    if name_ok(name) {
        format!("{name}: {why}")
    } else {
        why.to_string()
    }
}

/// `value` as the TUI shows and edits it: a newline as `\n`, a tab as `\t`,
/// a backslash as `\\`.
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

/// The TUI's escaped text back to the value: `\n`, `\t` and `\\`; a
/// backslash before any other character, or at the end, is an error.
/// `unescape(&escape(v)) == Ok(v)` for every `v`.
pub fn unescape(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                let other = if other.is_control() { '?' } else { other };
                return Err(format!(
                    "unknown escape \\{other}: write \\\\ for a backslash"
                ));
            }
            None => return Err("a lone \\ at the end: write \\\\ for a backslash".to_owned()),
        }
    }
    Ok(out)
}

/// One documented Claude Code variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownVar {
    pub name: &'static str,
    /// One word: `model`, `auth`, `network`…
    pub category: &'static str,
    /// At most 160 characters; says when a flag is presence-only.
    pub description: &'static str,
}

/// The catalogue as embedded: `NAME\tcategory\tdescription` per line, `#`
/// comments.
const CATALOGUE: &str = include_str!("env_catalogue.tsv");

/// Claude Code's documented variables (the embedded catalogue), in name
/// order, parsed once. A malformed line is left out.
pub fn catalogue() -> &'static [KnownVar] {
    static PARSED: OnceLock<Vec<KnownVar>> = OnceLock::new();
    PARSED.get_or_init(|| parse_catalogue(CATALOGUE))
}

/// The catalogue entry of `name`, if it is documented.
pub fn known(name: &str) -> Option<&'static KnownVar> {
    let list = catalogue();
    list.binary_search_by(|v| v.name.cmp(name))
        .ok()
        .and_then(|i| list.get(i))
}

/// `text`'s well-formed lines, sorted by name.
fn parse_catalogue(text: &'static str) -> Vec<KnownVar> {
    let mut list: Vec<KnownVar> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut cells = l.split('\t');
            let (name, category, description) = (cells.next()?, cells.next()?, cells.next()?);
            let ok = cells.next().is_none() && name_ok(name) && !category.is_empty();
            ok.then_some(KnownVar {
                name,
                category,
                description,
            })
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(b.name));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_letters_digits_and_underscores_not_starting_with_a_digit() {
        for good in [
            "A",
            "_",
            "https_proxy",
            "no_proxy",
            "MAX_THINKING_TOKENS",
            "a1",
        ] {
            assert_eq!(check_name(good), Ok(()), "{good}");
        }
        let long = "A".repeat(MAX_NAME);
        assert_eq!(check_name(&long), Ok(()));
        for bad in [
            "",
            "1A",
            "A-B",
            "A B",
            "Ä",
            "A=B",
            "A.B",
            &"A".repeat(MAX_NAME + 1),
        ] {
            assert_eq!(check_name(bad), Err(Refusal::BadName), "{bad}");
        }
    }

    #[test]
    fn the_refused_set_is_the_session_markers_and_the_seven_names() {
        for name in SESSION_MARKERS {
            assert!(
                matches!(check_name(name), Err(Refusal::Refused(_))),
                "{name}"
            );
        }
        for name in [
            "PATH",
            "TERM",
            "COLORTERM",
            "CLAUDE_CODE_REMOTE",
            "CLAUDE_CODE_ACCOUNT_UUID",
            "CLAUDE_CODE_SIMPLE",
            "CLAUDE_CODE_SAFE_MODE",
        ] {
            assert!(
                matches!(check_name(name), Err(Refusal::Refused(_))),
                "{name}"
            );
        }
        assert_eq!(refused_names().len(), SESSION_MARKERS.len() + 7);
        // Exact case: `path` is another variable.
        assert_eq!(check_name("path"), Ok(()));
        let why = check("PATH", "x").unwrap_err();
        assert!(why.starts_with("PATH: refused: "), "{why}");
    }

    #[test]
    fn values_allow_newlines_and_tabs_and_refuse_other_controls_and_length() {
        for good in ["", "x", "a\nb", "a\tb", "Name: v\nOther: w", "ü € 😀"] {
            assert_eq!(check_value(good), Ok(()), "{good:?}");
        }
        assert_eq!(check_value(&"x".repeat(MAX_VALUE)), Ok(()));
        assert_eq!(
            check_value(&"x".repeat(MAX_VALUE + 1)),
            Err(Refusal::TooLong)
        );
        // 8192 bytes, not characters: 2731 × 'é' (2 bytes) is 5462 bytes, fine;
        // 4097 × 'é' is 8194.
        assert_eq!(check_value(&"é".repeat(4097)), Err(Refusal::TooLong));
        for bad in ["a\0b", "a\rb", "\x1b[31m", "a\x7fb", "a\u{85}b", "a\u{9f}b"] {
            assert_eq!(check_value(bad), Err(Refusal::Control), "{bad:?}");
        }
    }

    #[test]
    fn errors_name_a_well_formed_key_and_never_the_value() {
        let secret = "sk-SECRET\0";
        let why = check("TOKEN", secret).unwrap_err();
        assert!(why.starts_with("TOKEN: value holds a control"), "{why}");
        assert!(!why.contains("sk-SECRET"));
        // A bad key may be a value typed on the wrong side: not quoted.
        let why = check("sk-SECRET", "v").unwrap_err();
        assert!(!why.contains("sk-SECRET"), "{why}");
    }

    #[test]
    fn the_escaped_form_round_trips_and_refuses_unknown_escapes() {
        for v in [
            "",
            "plain",
            "a\nb",
            "\t",
            "C:\\new",
            "\\\\",
            "x\\",
            "\\n literal",
            "ü\n\t\\",
        ] {
            assert_eq!(unescape(&escape(v)).as_deref(), Ok(v), "{v:?}");
        }
        assert_eq!(escape("a\nb\tc\\d"), "a\\nb\\tc\\\\d");
        // `C:\\new` is the path; `C:\new` is "C:", a newline and "ew" (`\n`
        // is an escape), and `C:\Users` is refused: `\U` is none.
        assert_eq!(unescape("C:\\\\new").as_deref(), Ok("C:\\new"));
        assert_eq!(unescape("C:\\new").as_deref(), Ok("C:\new"));
        assert_eq!(
            unescape("C:\\Users").unwrap_err(),
            "unknown escape \\U: write \\\\ for a backslash"
        );
        assert!(unescape("end\\").unwrap_err().contains("at the end"));
    }

    #[test]
    fn value_debug_prints_the_length_only() {
        let v = Value::new("sk-SECRET-value");
        assert_eq!(format!("{v:?}"), "<15 bytes>");
        let pair = vec![("TOKEN".to_owned(), v)];
        let shown = format!("{pair:?} {:?}", Some(&pair));
        assert!(!shown.contains("SECRET"), "{shown}");
        assert_eq!(
            serde_json::to_string(&pair[0].1).unwrap(),
            "\"sk-SECRET-value\""
        );
    }

    #[test]
    fn the_catalogue_parses_with_unique_names_and_no_refused_one() {
        let list = catalogue();
        assert_eq!(
            list.len(),
            CATALOGUE.lines().filter(|l| !l.starts_with('#')).count()
        );
        assert!(list.len() > 400, "{}", list.len());
        for pair in list.windows(2) {
            assert!(
                pair[0].name < pair[1].name,
                "{} twice or unsorted",
                pair[1].name
            );
        }
        for v in list {
            assert_eq!(check_name(v.name), Ok(()), "{} is refused or bad", v.name);
            assert!(v.description.chars().count() <= 160, "{}", v.name);
            assert!(!v.category.contains(' '), "{}", v.name);
        }
        let opus = known("ANTHROPIC_DEFAULT_OPUS_MODEL").expect("documented");
        assert_eq!(opus.category, "model");
        assert!(
            known("DISABLE_TELEMETRY").is_some_and(|v| v.description.starts_with("Presence-only"))
        );
        assert!(known("PATH").is_none() && known("CLAUDECODE").is_none());
    }

    #[test]
    fn a_malformed_catalogue_line_is_left_out() {
        let list =
            parse_catalogue("# c\nA\tcat\tok\nbad line\n1X\tcat\tbad name\nB\tcat\tx\textra\n\n");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "A");
    }
}
```
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib claude::env`
Expected: `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 581 filtered out`
Run: `cargo fmt --check && cargo clippy -p claudio-core --all-targets -- -D warnings`
Expected: no output from fmt; clippy ends `Finished` with no warning.

Step 3: commit
`git add crates/core/src/claude/env.rs crates/core/src/claude/env_catalogue.tsv`
`git commit -m "feat(core): claude::env, the custom env's validator, caps and documented variables" -- crates/core/src/claude/env.rs crates/core/src/claude/env_catalogue.tsv crates/core/src/claude/mod.rs`

Interfaces this task provides to later tasks (all in `claudio_core::claude::env`):
`pub const MAX_NAME: usize = 128`, `MAX_VALUE = 8192`, `MAX_VARS = 128`,
`MAX_SCOPE_BYTES = 131072`; `pub struct Value(String)` (`new`, `as_str`, `into_string`, `len`,
`is_empty`; `Clone, PartialEq, Eq, Default, Serialize, Deserialize` transparent; `Debug` =
`<n bytes>`); `pub enum Refusal { BadName, Refused(&'static str), TooLong, Control }` (Display
never quotes a value); `pub fn name_ok(&str) -> bool`; `pub fn refused(&str) -> Option<&'static
str>`; `pub fn refused_names() -> Vec<(&'static str, &'static str)>`; `pub fn check_name(&str)
-> Result<(), Refusal>`; `pub fn check_value(&str) -> Result<(), Refusal>`; `pub fn check(name,
value) -> Result<(), String>` (`NAME: why`); `pub fn shown(name, &impl Display) -> String`;
`pub fn escape(&str) -> String`; `pub fn unescape(&str) -> Result<String, String>`;
`pub struct KnownVar { name, category, description: &'static str }`; `pub fn catalogue() ->
&'static [KnownVar]` (name order; re-exported as `claudio_core::claude::env_catalogue`);
`pub fn known(&str) -> Option<&'static KnownVar>`.

### Task 3: core: config_edit writes a newline-holding string on one line; app::env, the store
Model: opus   Repo: /home/p4u/claudio   Files: crates/core/src/app/config_edit.rs,
crates/core/src/app/env.rs (new), crates/core/src/app/env/tests.rs (new),
crates/core/src/app/mod.rs, crates/core/src/addon/registry.rs
Goal: AC2: the global scope in `config.toml [env]` (read as a `toml::Table` each time, written
only through `edit_section`), the per-session scope in `session-env.json` (0600, sidecar lock,
8 MiB cap, fail closed), `forget_session`, `prune`, `effective`; `"env"` a core section.
Rules: A5 (one store for both front-ends and the composer), B3 (minimal, atomic, serialised
edits; an old binary keeps `[env]` in `ConfigFile.extra`), C1, C2, C4, D3 (callers run it off
the executor), D5.

Two commits: the `config_edit` change first (its own test), then the store.

Step 1: write the failing test
1. In `crates/core/src/app/config_edit.rs`'s `mod tests`, add the test
   `edit_section_writes_a_string_with_a_newline_on_one_line` (the second hunk of the
   `config_edit.rs` diff in Step 2, alone).
   Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib config_edit::tests::edit_section_writes_a_string`
   Expected (abridged): `panicked at crates/core/src/app/config_edit.rs:…` with
   `left: "# mine\n[ui]\nnotify = true\n\n[env]\nHEADERS = '''\nX-One: 1\nX-Two: \"2\"\tend\\'''\n"`
   and `test result: FAILED. 0 passed; 1 failed`.
2. After the first commit: create `crates/core/src/app/env/tests.rs` (the listing in Step 2.3)
   and add `pub mod env;` to `crates/core/src/app/mod.rs` with a stub
   `crates/core/src/app/env.rs` holding only `#[cfg(test)] mod tests;`.
   Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib app::env`
   Expected: ``error[E0432]: unresolved import`` / ``cannot find type `Store` `` errors; the build fails.

Step 2: implement
1. `crates/core/src/app/config_edit.rs` (`line_value` renders a string holding `\n`/`\r` with
   the new `basic_string`; the test):
````diff
diff --git a/crates/core/src/app/config_edit.rs b/crates/core/src/app/config_edit.rs
index 3a0e22d..4d29ee5 100644
--- a/crates/core/src/app/config_edit.rs
+++ b/crates/core/src/app/config_edit.rs
@@ -142,16 +142,39 @@ pub fn edit_text(text: &str, section: &str, old: &Table, new: &Table) -> Option<
 }
 
 /// A value this editor writes on one line: a string, an integer, a boolean,
-/// or a flat array of those.
+/// or a flat array of those. A string holding a line break is written as a
+/// basic string with escapes ([`basic_string`]), never as a multi-line one,
+/// so the line edit can change or remove it again.
 fn line_value(value: &Value) -> Option<String> {
     let flat = |v: &Value| matches!(v, Value::String(_) | Value::Integer(_) | Value::Boolean(_));
     match value {
+        Value::String(s) if s.contains(['\n', '\r']) => Some(basic_string(s)),
         v if flat(v) => Some(v.to_string()),
         Value::Array(items) if items.iter().all(flat) => Some(value.to_string()),
         _ => None,
     }
 }
 
+/// `s` as a TOML basic string on one line: `"`, `\` and every control
+/// character escaped (`\n`, `\r`, `\t`, else `\uXXXX`).
+fn basic_string(s: &str) -> String {
+    let mut out = String::with_capacity(s.len() + 2);
+    out.push('"');
+    for c in s.chars() {
+        match c {
+            '"' => out.push_str("\\\""),
+            '\\' => out.push_str("\\\\"),
+            '\n' => out.push_str("\\n"),
+            '\r' => out.push_str("\\r"),
+            '\t' => out.push_str("\\t"),
+            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
+            c => out.push(c),
+        }
+    }
+    out.push('"');
+    out
+}
+
 /// `key` as a TOML key: bare when it can be, else a quoted string.
 fn render_key(key: &str) -> String {
     let bare = !key.is_empty()
@@ -817,6 +840,41 @@ mod tests {
         let _ = std::fs::remove_dir_all(&dir);
     }
 
+    /// A string holding a newline is written on one line, as a basic string
+    /// with escapes, so it can be changed and removed again; the rest of the
+    /// file stays.
+    #[test]
+    fn edit_section_writes_a_string_with_a_newline_on_one_line() {
+        let text = "# mine\n[ui]\nnotify = true\n";
+        let (dir, path) = scratch(Some(text));
+        let header = "X-One: 1\nX-Two: \"2\"\tend\\";
+        edit_section(&path, "env", |t| {
+            t.insert("HEADERS".into(), Value::String(header.into()));
+        })
+        .unwrap();
+        let out = std::fs::read_to_string(&path).unwrap();
+        assert_eq!(
+            out,
+            "# mine\n[ui]\nnotify = true\n\n[env]\n\
+             HEADERS = \"X-One: 1\\nX-Two: \\\"2\\\"\\tend\\\\\"\n"
+        );
+        let parsed: Table = out.parse().unwrap();
+        assert_eq!(parsed["env"]["HEADERS"].as_str(), Some(header));
+        edit_section(&path, "env", |t| {
+            t.insert("HEADERS".into(), Value::String("a\r\nb".into()));
+        })
+        .unwrap();
+        let out = std::fs::read_to_string(&path).unwrap();
+        assert!(out.ends_with("[env]\nHEADERS = \"a\\r\\nb\"\n"), "{out}");
+        edit_section(&path, "env", |t| {
+            t.remove("HEADERS");
+        })
+        .unwrap();
+        let out = std::fs::read_to_string(&path).unwrap();
+        assert_eq!(out, "# mine\n[ui]\nnotify = true\n\n[env]\n");
+        let _ = std::fs::remove_dir_all(&dir);
+    }
+
     #[test]
     fn edits_are_serialised_by_one_lock() {
         let (dir, path) = scratch(Some(""));
````
   Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib config_edit`
   Expected: `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 577 filtered out`
   Commit it now (Step 3, first commit).
2. `crates/core/src/app/mod.rs` and `crates/core/src/addon/registry.rs` (`"env"` joins
   `CORE_SECTIONS`, so no addon may claim it):
````diff
diff --git a/crates/core/src/addon/registry.rs b/crates/core/src/addon/registry.rs
index 4a4f569..36b35a6 100644
--- a/crates/core/src/addon/registry.rs
+++ b/crates/core/src/addon/registry.rs
@@ -7,7 +7,7 @@ use std::sync::Arc;
 use super::{feature, name_ok, Addon, FlagDecl, PanelDecl};
 
 /// The `config.toml` tables the core owns; no addon may claim them.
-pub const CORE_SECTIONS: &[&str] = &["ui", "keys", "update", "claude", "extensions", "ssh"];
+pub const CORE_SECTIONS: &[&str] = &["ui", "keys", "update", "claude", "extensions", "ssh", "env"];
 
 /// The core's command words; never an addon's name.
 pub const RESERVED_WORDS: &[&str] = &[
diff --git a/crates/core/src/app/mod.rs b/crates/core/src/app/mod.rs
index 1ed1366..1e6e80b 100644
--- a/crates/core/src/app/mod.rs
+++ b/crates/core/src/app/mod.rs
@@ -1,11 +1,13 @@
 //! The layer both front-ends (the console TUI and the web UI) share: client
-//! state, daemon connections, the extension registry and snippets. Nothing
-//! here draws or reads input; it imports neither ratatui, crossterm nor axum.
+//! state, daemon connections, the extension registry, snippets and the
+//! custom env. Nothing here draws or reads input; it imports neither
+//! ratatui, crossterm nor axum.
 
 pub mod addons;
 pub mod browser;
 pub mod config_edit;
 pub mod connections;
+pub mod env;
 pub mod extensions;
 pub mod label;
 pub mod launch;
````
3. `crates/core/src/app/env.rs` (new, whole file):
```rust
//! The custom env of claude sessions (docs/specs/2026-11-15-custom-env-design.md):
//! the global scope in `config.toml [env]`, the per-session scope in
//! `session-env.json` beside it. The one implementation the TUI, the web and
//! the launch composer use (A5).
//!
//! Every name and value passes `claude::env`'s validator when it is written
//! and again when it is read; an entry that fails is skipped with one notice.
//! Values are secrets at rest (both files 0600): nothing here logs or formats
//! one, and errors and notices name keys at most, only well-formed ones (C1).

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::addon::{line, TEXT_BYTES};
use crate::app::config_edit;
use crate::claude::env::{
    check_name, check_value, name_ok, refused, shown, Refusal, Value, MAX_SCOPE_BYTES, MAX_VARS,
};
use crate::config::SaveError;
use crate::paths;
use crate::proto::SessionId;

/// The `config.toml` table of the global scope.
pub const SECTION: &str = "env";
/// The per-session file, beside `config.toml`.
pub const SESSION_FILE: &str = "session-env.json";
/// The most bytes `session-env.json` may hold.
pub const MAX_SESSION_FILE: usize = 8 * 1024 * 1024;
/// The format of `session-env.json` this build reads and writes.
const VERSION: u32 = 1;
/// How many skipped names a notice lists.
const NAMES_SHOWN: usize = 8;
/// What a front-end shows when `session-env.json` cannot be read.
pub const UNREADABLE: &str = "session-env.json is not readable: fix or delete it";

/// Which env a read or a write is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every claude session, on every host: `config.toml [env]`.
    Global,
    /// One claude session; `host` as the front-ends name it (`local` or an
    /// ssh alias), kept for pruning.
    Session { host: String, sid: SessionId },
}

/// A scope as read: its valid entries in name order, how many entries of the
/// file were skipped, and the notices about them (key names only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub vars: Vec<(String, Value)>,
    pub skipped: usize,
    pub notices: Vec<String>,
}

/// Why a write did not happen. Its text names a key at most, never a value.
#[derive(Debug)]
pub enum EnvError {
    /// The name or the value is refused; the text says which and why.
    Refused(String),
    /// The scope would hold more than [`MAX_VARS`] variables.
    TooMany,
    /// The scope would hold more than [`MAX_SCOPE_BYTES`].
    TooLarge,
    /// `session-env.json` would pass [`MAX_SESSION_FILE`].
    Full,
    /// `session-env.json` cannot be read; it is never overwritten.
    Unreadable,
    /// The `config.toml` edit failed.
    Save(SaveError),
    /// `session-env.json` could not be locked or written.
    Io(io::Error),
}

impl fmt::Display for EnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvError::Refused(why) => write!(f, "{why}"),
            EnvError::TooMany => write!(f, "too many variables (at most {MAX_VARS})"),
            EnvError::TooLarge => write!(f, "environment too large (at most 128 KiB)"),
            EnvError::Full => write!(f, "session-env.json is full (8 MiB)"),
            EnvError::Unreadable => write!(f, "{UNREADABLE}"),
            EnvError::Save(e) => write!(f, "could not save: {e}"),
            EnvError::Io(e) => write!(f, "could not save: {e}"),
        }
    }
}

/// `session-env.json`. No `Debug`: it holds values.
#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    sessions: BTreeMap<SessionId, SessionEntry>,
}

/// One session's entry. A value that is not a string (a hand edit) is kept
/// and skipped on read.
#[derive(Serialize, Deserialize)]
struct SessionEntry {
    host: String,
    env: BTreeMap<String, serde_json::Value>,
}

impl Default for SessionFile {
    fn default() -> SessionFile {
        SessionFile {
            version: VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

/// The custom env's two files: a `config.toml` and `session-env.json` beside
/// it. Every call reads the files again: nothing is cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    config: PathBuf,
    sessions: PathBuf,
}

impl Store {
    /// The store of the config file at `config_path` (a runtime's
    /// `RuntimeOptions.config_path`).
    pub fn beside(config_path: &Path) -> Store {
        let dir = config_path.parent().unwrap_or(Path::new("."));
        Store {
            config: config_path.to_path_buf(),
            sessions: dir.join(SESSION_FILE),
        }
    }

    /// The valid entries of `scope`. A session's entry counts only on its
    /// host. An unreadable file reads as empty, with a notice.
    pub fn list(&self, scope: &Scope) -> Listed {
        match scope {
            Scope::Global => self.global(),
            Scope::Session { host, sid } => self.session(host, *sid),
        }
    }

    /// Global overlaid with the session's own (it wins), in name order, and
    /// the notices of both reads: what a launch of `sid` on `host` gets.
    pub fn effective(&self, host: &str, sid: SessionId) -> (Vec<(String, Value)>, Vec<String>) {
        let global = self.global();
        let session = self.session(host, sid);
        let mut vars: BTreeMap<String, Value> = global.vars.into_iter().collect();
        vars.extend(session.vars);
        let mut notices = global.notices;
        notices.extend(session.notices);
        (vars.into_iter().collect(), notices)
    }

    /// Set `name` to `value` in `scope`, replacing whatever the file holds
    /// for it; refused past the caps.
    pub fn set(&self, scope: &Scope, name: &str, value: &Value) -> Result<(), EnvError> {
        check_name(name).map_err(|e| EnvError::Refused(shown(name, &e)))?;
        check_value(value.as_str()).map_err(|e| EnvError::Refused(shown(name, &e)))?;
        match scope {
            Scope::Global => {
                let mut over = None;
                config_edit::edit_section(&self.config, SECTION, |table| {
                    match fits(&valid_vars(entries_of(table)), name, value) {
                        Ok(()) => {
                            let text = toml::Value::String(value.as_str().to_owned());
                            table.insert(name.to_owned(), text);
                        }
                        Err(e) => over = Some(e),
                    }
                })
                .map_err(EnvError::Save)?;
                over.map_or(Ok(()), Err)
            }
            Scope::Session { host, sid } => self.edit_sessions(|file| {
                let entry = file.sessions.entry(*sid).or_insert_with(|| SessionEntry {
                    host: host.clone(),
                    env: BTreeMap::new(),
                });
                let own = entry.env.iter().map(|(k, v)| (k.clone(), string(v)));
                fits(&valid_vars(own.collect()), name, value)?;
                entry.host = host.clone();
                let text = serde_json::Value::String(value.as_str().to_owned());
                entry.env.insert(name.to_owned(), text);
                Ok(true)
            }),
        }
    }

    /// Remove `name` from `scope` (a name it does not hold is not an error).
    pub fn unset(&self, scope: &Scope, name: &str) -> Result<(), EnvError> {
        if !name_ok(name) {
            return Err(EnvError::Refused(Refusal::BadName.to_string()));
        }
        match scope {
            Scope::Global => config_edit::edit_section(&self.config, SECTION, |table| {
                table.remove(name);
            })
            .map(|_| ())
            .map_err(EnvError::Save),
            Scope::Session { host, sid } => self.edit_sessions(|file| {
                let Some(entry) = file.sessions.get_mut(sid).filter(|e| e.host == *host) else {
                    return Ok(false);
                };
                let removed = entry.env.remove(name).is_some();
                if entry.env.is_empty() {
                    file.sessions.remove(sid);
                }
                Ok(removed)
            }),
        }
    }

    /// Drop session `sid`'s entry (its session was removed).
    pub fn forget_session(&self, host: &str, sid: SessionId) -> Result<(), EnvError> {
        if !self.sessions.exists() {
            return Ok(());
        }
        self.edit_sessions(|file| {
            let ours = file.sessions.get(&sid).is_some_and(|e| e.host == host);
            Ok(ours && file.sessions.remove(&sid).is_some())
        })
    }

    /// Drop the entries of `host`'s sessions that are not in `live` (its
    /// daemon's list): sessions removed while no front-end ran.
    pub fn prune(&self, host: &str, live: &[SessionId]) -> Result<(), EnvError> {
        if !self.sessions.exists() {
            return Ok(());
        }
        self.edit_sessions(|file| {
            let before = file.sessions.len();
            file.sessions
                .retain(|sid, e| e.host != host || live.contains(sid));
            Ok(file.sessions.len() != before)
        })
    }

    /// `[env]`'s valid entries.
    fn global(&self) -> Listed {
        match read_table(&self.config) {
            Ok(entries) => valid("[env]", entries),
            Err(notice) => Listed {
                notices: vec![notice],
                ..Listed::default()
            },
        }
    }

    /// Session `sid`'s valid entries, when its entry is `host`'s.
    fn session(&self, host: &str, sid: SessionId) -> Listed {
        let file = match read_sessions(&self.sessions) {
            Ok(file) => file,
            Err(()) => {
                return Listed {
                    notices: vec![UNREADABLE.to_owned()],
                    ..Listed::default()
                }
            }
        };
        match file.sessions.get(&sid).filter(|e| e.host == host) {
            Some(entry) => {
                let entries = entry.env.iter().map(|(k, v)| (k.clone(), string(v)));
                valid("session env", entries.collect())
            }
            None => Listed::default(),
        }
    }

    /// Read-modify-write `session-env.json` under its sidecar lock: `edit`
    /// says whether it changed anything. An unreadable file fails the write
    /// and stays as it is.
    fn edit_sessions(
        &self,
        edit: impl FnOnce(&mut SessionFile) -> Result<bool, EnvError>,
    ) -> Result<(), EnvError> {
        let _lock =
            paths::FileLock::beside(&self.sessions, paths::LOCK_WAIT).map_err(EnvError::Io)?;
        let mut file = read_sessions(&self.sessions).map_err(|()| EnvError::Unreadable)?;
        if !edit(&mut file)? {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&file).map_err(|e| EnvError::Io(io::Error::other(e)))?;
        if bytes.len() > MAX_SESSION_FILE {
            return Err(EnvError::Full);
        }
        paths::write_atomic(&self.sessions, &bytes).map_err(EnvError::Io)
    }
}

/// A JSON value as an entry's text (`None`: not a string).
fn string(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// `table`'s entries as names and texts (`None`: not a string).
fn entries_of(table: &toml::Table) -> Vec<(String, Option<String>)> {
    table
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_owned)))
        .collect()
}

/// `[env]` of the config file at `path` (a missing file or table: none),
/// or the notice why it cannot be read.
fn read_table(path: &Path) -> Result<Vec<(String, Option<String>)>, String> {
    let text = match std::fs::read(path) {
        Ok(bytes) => {
            String::from_utf8(bytes).map_err(|_| "config.toml is not UTF-8: [env] not applied")?
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("config.toml is not readable: [env] not applied".to_owned()),
    };
    let table: toml::Table = text
        .parse()
        .map_err(|_| "config.toml is not valid TOML: [env] not applied".to_owned())?;
    match table.get(SECTION) {
        None => Ok(Vec::new()),
        Some(toml::Value::Table(env)) => Ok(entries_of(env)),
        Some(_) => Err("config.toml: [env] is not a table: not applied".to_owned()),
    }
}

/// `session-env.json` at `path` (missing: empty); `Err` when it cannot be
/// read, is over [`MAX_SESSION_FILE`], is not this format or of another
/// version.
fn read_sessions(path: &Path) -> Result<SessionFile, ()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SessionFile::default()),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    let cap = MAX_SESSION_FILE as u64 + 1;
    file.take(cap).read_to_end(&mut bytes).map_err(|_| ())?;
    if bytes.len() > MAX_SESSION_FILE {
        return Err(());
    }
    let parsed: SessionFile = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if parsed.version != VERSION {
        return Err(());
    }
    Ok(parsed)
}

/// The valid entries of `entries`, in name order, within the caps.
fn valid_vars(entries: Vec<(String, Option<String>)>) -> Vec<(String, Value)> {
    valid("", entries).vars
}

/// `entries` checked in name order: a bad or refused name, a value that is
/// not a string or fails the validator is skipped; past a cap the rest are.
/// One notice, prefixed `label`, says what was skipped (names only when they
/// are well formed).
fn valid(label: &str, mut entries: Vec<(String, Option<String>)>) -> Listed {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let (mut vars, mut named, mut bad_names, mut over) = (Vec::new(), Vec::new(), 0, 0);
    let mut bytes = 0;
    for (name, text) in entries {
        if !name_ok(&name) {
            bad_names += 1;
            continue;
        }
        let why = match &text {
            _ if refused(&name).is_some() => Some("refused"),
            None => Some("not a string"),
            Some(text) if check_value(text).is_err() => Some("bad value"),
            Some(_) => None,
        };
        if let Some(why) = why {
            named.push(format!("{name} ({why})"));
            continue;
        }
        let text = text.unwrap_or_default();
        let size = name.len() + text.len();
        if over > 0 || vars.len() == MAX_VARS || bytes + size > MAX_SCOPE_BYTES {
            over += 1;
            continue;
        }
        bytes += size;
        vars.push((name, Value::new(text)));
    }
    let skipped = named.len() + bad_names + over;
    let mut parts: Vec<String> = named.iter().take(NAMES_SHOWN).cloned().collect();
    if named.len() > NAMES_SHOWN {
        parts.push(format!("{} more", named.len() - NAMES_SHOWN));
    }
    if bad_names > 0 {
        parts.push(format!("{bad_names} with a bad name"));
    }
    if over > 0 {
        parts.push(format!(
            "{over} past the caps ({MAX_VARS} variables, 128 KiB)"
        ));
    }
    let notices = if skipped == 0 || label.is_empty() {
        Vec::new()
    } else {
        let text = format!("{label}: {skipped} skipped: {}", parts.join(", "));
        vec![line(&text, TEXT_BYTES)]
    };
    Listed {
        vars,
        skipped,
        notices,
    }
}

/// Whether `vars` (without `name`) and `name = value` stay within the caps.
fn fits(vars: &[(String, Value)], name: &str, value: &Value) -> Result<(), EnvError> {
    let others = vars.iter().filter(|(k, _)| k != name);
    let (count, bytes) = others.fold((0, 0), |(n, b), (k, v)| (n + 1, b + k.len() + v.len()));
    if count + 1 > MAX_VARS {
        return Err(EnvError::TooMany);
    }
    if bytes + name.len() + value.len() > MAX_SCOPE_BYTES {
        return Err(EnvError::TooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
```
4. `crates/core/src/app/env/tests.rs` (new, whole file):
```rust
//! `app::env` against real files in a scratch dir.

use std::os::unix::fs::PermissionsExt;

use super::*;

/// A scratch dir with `config.toml` holding `text`, and its store.
fn scratch(text: &str) -> (PathBuf, Store) {
    let dir = std::env::temp_dir().join(format!("claudio-env-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
    let store = Store::beside(&dir.join("config.toml"));
    (dir, store)
}

fn sid(n: u128) -> SessionId {
    SessionId::from_u128(n)
}

fn session(host: &str, n: u128) -> Scope {
    Scope::Session {
        host: host.to_owned(),
        sid: sid(n),
    }
}

fn v(text: &str) -> Value {
    Value::new(text)
}

fn names(listed: &Listed) -> Vec<(&str, &str)> {
    listed
        .vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn global_set_and_unset_round_trip_keeping_other_sections_and_comments() {
    let text = "# my settings\n[ui]\nnotify = true # yes\n\n[env]\n# models\nA = \"1\"\n";
    let (dir, store) = scratch(text);
    store
        .set(&Scope::Global, "BASH_DEFAULT_TIMEOUT_MS", &v("300000"))
        .unwrap();
    store.set(&Scope::Global, "A", &v("2")).unwrap();
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(
        out,
        "# my settings\n[ui]\nnotify = true # yes\n\n[env]\n# models\nA = \"2\"\n\
         BASH_DEFAULT_TIMEOUT_MS = \"300000\"\n"
    );
    let listed = store.list(&Scope::Global);
    assert_eq!(
        names(&listed),
        [("A", "2"), ("BASH_DEFAULT_TIMEOUT_MS", "300000")]
    );
    assert_eq!((listed.skipped, listed.notices.len()), (0, 0));
    store.unset(&Scope::Global, "A").unwrap();
    store.unset(&Scope::Global, "NOT_THERE").unwrap();
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(
        out,
        "# my settings\n[ui]\nnotify = true # yes\n\n[env]\n# models\n\
         BASH_DEFAULT_TIMEOUT_MS = \"300000\"\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_global_value_with_a_newline_stays_on_one_line_through_set_change_and_unset() {
    let (dir, store) = scratch("[claude]\ninstall = \"off\"\n");
    let headers = "X-One: 1\nX-Two: 2";
    store
        .set(&Scope::Global, "ANTHROPIC_CUSTOM_HEADERS", &v(headers))
        .unwrap();
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(
        out.ends_with("ANTHROPIC_CUSTOM_HEADERS = \"X-One: 1\\nX-Two: 2\"\n"),
        "{out}"
    );
    assert_eq!(out.lines().count(), 5, "{out}");
    assert_eq!(
        names(&store.list(&Scope::Global)),
        [("ANTHROPIC_CUSTOM_HEADERS", headers)]
    );
    store
        .set(&Scope::Global, "ANTHROPIC_CUSTOM_HEADERS", &v("X: 3\n\tY"))
        .unwrap();
    assert_eq!(
        names(&store.list(&Scope::Global)),
        [("ANTHROPIC_CUSTOM_HEADERS", "X: 3\n\tY")]
    );
    store
        .unset(&Scope::Global, "ANTHROPIC_CUSTOM_HEADERS")
        .unwrap();
    assert!(store.list(&Scope::Global).vars.is_empty());
    let out = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert_eq!(out, "[claude]\ninstall = \"off\"\n\n[env]\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hand_written_multi_line_value_is_listed_and_its_edit_refused() {
    let text = "[env]\nH = \"\"\"\nline\n\"\"\"\n";
    let (dir, store) = scratch(text);
    assert_eq!(names(&store.list(&Scope::Global)), [("H", "line\n")]);
    let err = store.set(&Scope::Global, "H", &v("x")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "could not save: config.toml layout not understood; edit [env] by hand"
    );
    assert!(store.unset(&Scope::Global, "H").is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("config.toml")).unwrap(),
        text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_global_entry_is_skipped_with_one_notice_and_the_config_still_loads() {
    let text = "[ui]\nnotify = true\n[env]\nX = 1\nPATH = \"/bin\"\n\"1BAD\" = \"v\"\nCTL = \"a\\u0000b\"\nOK = \"yes\"\n";
    let (dir, store) = scratch(text);
    let listed = store.list(&Scope::Global);
    assert_eq!(names(&listed), [("OK", "yes")]);
    assert_eq!(listed.skipped, 4);
    assert_eq!(
        listed.notices,
        ["[env]: 4 skipped: CTL (bad value), PATH (refused), X (not a string), 1 with a bad name"]
    );
    assert!(
        !listed.notices[0].contains("1BAD"),
        "a bad key is never quoted"
    );
    let config = crate::config::load_from(&dir.join("config.toml"));
    assert!(config.ui.notify, "the rest of config.toml still loads");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_set_unset_and_forget_in_a_private_file() {
    let (dir, store) = scratch("");
    let file = dir.join(SESSION_FILE);
    store.set(&session("local", 1), "A", &v("1")).unwrap();
    store
        .set(&session("local", 1), "B", &v("two\nlines"))
        .unwrap();
    store.set(&session("box", 2), "A", &v("x")).unwrap();
    assert_eq!(mode(&file), 0o600);
    assert_eq!(
        names(&store.list(&session("local", 1))),
        [("A", "1"), ("B", "two\nlines")]
    );
    // Lookups are by session id, on its own host only.
    assert!(store.list(&session("box", 1)).vars.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["sessions"][sid(1).to_string()]["host"], "local");
    store.unset(&session("local", 1), "A").unwrap();
    assert_eq!(
        names(&store.list(&session("local", 1))),
        [("B", "two\nlines")]
    );
    store.forget_session("local", sid(1)).unwrap();
    assert!(store.list(&session("local", 1)).vars.is_empty());
    assert_eq!(names(&store.list(&session("box", 2))), [("A", "x")]);
    // The last var of a session takes its entry with it.
    store.unset(&session("box", 2), "A").unwrap();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(json["sessions"], serde_json::json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn effective_overlays_the_session_on_the_global_scope() {
    let (dir, store) = scratch("[env]\nA = \"global\"\nB = \"global\"\n");
    store.set(&session("local", 1), "B", &v("mine")).unwrap();
    store.set(&session("local", 1), "C", &v("mine")).unwrap();
    let (vars, notices) = store.effective("local", sid(1));
    let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(vars, [("A", "global"), ("B", "mine"), ("C", "mine")]);
    assert!(notices.is_empty());
    let (other, _) = store.effective("local", sid(2));
    assert_eq!(other.len(), 2, "a new session gets the global scope");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_or_oversize_session_file_reads_empty_with_a_notice_and_refuses_writes() {
    for bad in [
        b"not json".to_vec(),
        br#"{"version":2,"sessions":{}}"#.to_vec(),
        vec![b' '; MAX_SESSION_FILE + 1],
    ] {
        let (dir, store) = scratch("[env]\nA = \"1\"\n");
        let file = dir.join(SESSION_FILE);
        std::fs::write(&file, &bad).unwrap();
        let listed = store.list(&session("local", 1));
        assert!(listed.vars.is_empty());
        assert_eq!(listed.notices, [UNREADABLE]);
        let (vars, notices) = store.effective("local", sid(1));
        assert_eq!(vars.len(), 1, "the global scope still applies");
        assert_eq!(notices, [UNREADABLE]);
        let err = store.set(&session("local", 1), "B", &v("2")).unwrap_err();
        assert_eq!(err.to_string(), UNREADABLE);
        assert!(store.unset(&session("local", 1), "B").is_err());
        assert!(store.forget_session("local", sid(1)).is_err());
        assert!(store.prune("local", &[]).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bad, "never overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn an_unreadable_session_file_is_left_untouched() {
    let (dir, store) = scratch("");
    let file = dir.join(SESSION_FILE);
    std::fs::write(&file, br#"{"version":1,"sessions":{}}"#).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    // root reads anything: the check is meaningful for a normal user only.
    if std::fs::read(&file).is_err() {
        assert_eq!(store.list(&session("local", 1)).notices, [UNREADABLE]);
        assert!(store.set(&session("local", 1), "A", &v("1")).is_err());
        assert_eq!(mode(&file), 0o000);
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prune_drops_the_hosts_gone_sessions_and_keeps_other_hosts() {
    let (dir, store) = scratch("");
    store.set(&session("local", 1), "A", &v("1")).unwrap();
    store.set(&session("local", 2), "A", &v("2")).unwrap();
    store.set(&session("box", 3), "A", &v("3")).unwrap();
    store.prune("local", &[sid(2), sid(9)]).unwrap();
    assert!(store.list(&session("local", 1)).vars.is_empty());
    assert_eq!(store.list(&session("local", 2)).vars.len(), 1);
    assert_eq!(store.list(&session("box", 3)).vars.len(), 1);
    // A missing file is never created by a prune or a forget.
    let (dir2, store2) = scratch("");
    store2.prune("local", &[]).unwrap();
    store2.forget_session("local", sid(1)).unwrap();
    assert!(!dir2.join(SESSION_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn two_writers_under_the_lock_lose_no_update() {
    let (dir, store) = scratch("");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                for k in 0..5 {
                    let scope = session("local", 1 + (i % 2));
                    store.set(&scope, &format!("V{i}_{k}"), &v("x")).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let total =
        store.list(&session("local", 1)).vars.len() + store.list(&session("local", 2)).vars.len();
    assert_eq!(total, 40, "no write lost");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writes_are_checked_and_capped() {
    let (dir, store) = scratch("");
    for scope in [Scope::Global, session("local", 1)] {
        let err = store.set(&scope, "PATH", &v("/x")).unwrap_err();
        assert!(err.to_string().starts_with("PATH: refused: "), "{err}");
        let err = store.set(&scope, "TOKEN", &v("sk-SECRET\r")).unwrap_err();
        assert!(err.to_string().starts_with("TOKEN: value holds"), "{err}");
        assert!(!err.to_string().contains("SECRET"));
        let err = store.set(&scope, "sk-SECRET", &v("x")).unwrap_err();
        assert!(!err.to_string().contains("SECRET"), "{err}");
        for i in 0..MAX_VARS {
            store.set(&scope, &format!("V{i}"), &v("x")).unwrap();
        }
        // Replacing one is fine; a new one is past the cap.
        store.set(&scope, "V0", &v("y")).unwrap();
        let err = store.set(&scope, "NEW", &v("x")).unwrap_err();
        assert!(err.to_string().starts_with("too many variables"), "{err}");
        for i in 0..MAX_VARS {
            store.unset(&scope, &format!("V{i}")).unwrap();
        }
        let big = "x".repeat(8000);
        for i in 0..16 {
            store.set(&scope, &format!("BIG{i:02}"), &v(&big)).unwrap();
        }
        let err = store.set(&scope, "BIG16", &v(&big)).unwrap_err();
        assert!(
            err.to_string().starts_with("environment too large"),
            "{err}"
        );
        for i in 0..16 {
            store.unset(&scope, &format!("BIG{i:02}")).unwrap();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hand_edited_scope_past_a_cap_keeps_the_first_entries_in_name_order() {
    let mut text = String::from("[env]\n");
    for i in 0..(MAX_VARS + 3) {
        text.push_str(&format!("V{i:03} = \"x\"\n"));
    }
    let (dir, store) = scratch(&text);
    let listed = store.list(&Scope::Global);
    assert_eq!(listed.vars.len(), MAX_VARS);
    assert_eq!(listed.vars[0].0, "V000");
    assert_eq!(listed.skipped, 3);
    assert_eq!(
        listed.notices,
        ["[env]: 3 skipped: 3 past the caps (128 variables, 128 KiB)"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_value_reaches_debug() {
    let (dir, store) = scratch("[env]\nTOKEN = \"sk-SECRET-1\"\n");
    let listed = store.list(&Scope::Global);
    let shown = format!(
        "{listed:?} {store:?} {:?}",
        store.effective("local", sid(1))
    );
    assert!(!shown.contains("SECRET"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}
```
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib app::env`
Expected: `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 590 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib`
Expected: `test result: ok. 603 passed; 0 failed`
Run: `cargo fmt --check && cargo clippy -p claudio-core --all-targets -- -D warnings && python3 scripts/check_rules.py`
Expected: clean; `check_rules.py` prints no violation.

Step 3: commit
`git commit -m "fix(core): config_edit writes a string holding a newline on one line" -- crates/core/src/app/config_edit.rs`
`git add crates/core/src/app/env.rs crates/core/src/app/env/tests.rs`
`git commit -m "feat(core): app::env, the custom env's global and per-session store" -- crates/core/src/app/env.rs crates/core/src/app/env/tests.rs crates/core/src/app/mod.rs crates/core/src/addon/registry.rs`

Interfaces this task provides to later tasks (all in `claudio_core::app::env`):
`pub const SECTION = "env"`, `SESSION_FILE = "session-env.json"`, `MAX_SESSION_FILE`,
`UNREADABLE = "session-env.json is not readable: fix or delete it"`;
`pub enum Scope { Global, Session { host: String, sid: SessionId } }`;
`pub struct Listed { pub vars: Vec<(String, Value)>, pub skipped: usize, pub notices: Vec<String> }`;
`pub enum EnvError { Refused(String), TooMany, TooLarge, Full, Unreadable, Save(SaveError), Io(io::Error) }` (Display names a key at most);
`pub struct Store` (`Clone, Debug, PartialEq, Eq`) with `Store::beside(config_path: &Path)`,
`list(&Scope) -> Listed`, `effective(host, sid) -> (Vec<(String, Value)>, Vec<String>)`,
`set(&Scope, name, &Value) -> Result<(), EnvError>`, `unset(&Scope, name)`,
`forget_session(host, sid)`, `prune(host, live: &[SessionId])` (all blocking: call them off the
executor).

### Task 4: core: the launch composer applies the custom env, the addons winning
Model: opus   Repo: /home/p4u/claudio   Files: crates/core/src/app/launch/compose.rs,
crates/core/src/app/launch/mod.rs, crates/core/src/app/launch/tests.rs,
crates/core/src/app/addons/mod.rs
Goal: AC3: `send` reads `Store::effective(host, sid)` (off the executor, beside the runtime's
`config_path`) for every `LaunchOp`; `compose` layers it first, a part's key replaces it, a part's
`env_remove` drops it, one keys-only notice `custom env overridden by <addon>: NAME, …`, dropped
for `Recover`. No front-end passes env.
Rules: A3/A5 (the core composes for every front-end and names no addon), A10 (the frame check
still guards the total), C1 (no value in a notice or `Debug`), D3.

Step 1: write the failing test
Add the new tests of `crates/core/src/app/launch/tests.rs` (the added block from
`fn user(env: &[(&str, &str)])` to the end of
`the_composer_reads_the_custom_env_for_every_op_and_a_recovery_drops_the_override_notice`, in the
diff below) and the `Vec::new(),` argument the diff adds to the 8 existing `compose(…)` calls.
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib app::launch`
Expected: `error[E0061]: this function takes 3 arguments but 4 arguments were supplied`; the
build fails.

Step 2: implement
````diff
diff --git a/crates/core/src/app/addons/mod.rs b/crates/core/src/app/addons/mod.rs
index 26078f5..4d9ca0d 100644
--- a/crates/core/src/app/addons/mod.rs
+++ b/crates/core/src/app/addons/mod.rs
@@ -16,7 +16,7 @@ pub mod test_support;
 mod tests;
 
 use std::collections::{BTreeMap, HashMap};
-use std::path::PathBuf;
+use std::path::{Path, PathBuf};
 use std::sync::{Arc, Mutex, MutexGuard};
 use std::time::Duration;
 
@@ -197,6 +197,12 @@ impl AddonRuntime {
         &self.inner.addons
     }
 
+    /// The `config.toml` this runtime reads (`RuntimeOptions.config_path`);
+    /// the custom env (`app::env`) lives beside it.
+    pub fn config_path(&self) -> &Path {
+        &self.inner.opts.config_path
+    }
+
     /// Addon `addon`'s context (D2).
     pub fn ctx(&self, addon: &'static str) -> Arc<dyn AddonCtx> {
         Arc::new(ctx::Ctx::new(&self.inner, addon))
diff --git a/crates/core/src/app/launch/compose.rs b/crates/core/src/app/launch/compose.rs
index ef6753f..53e71cb 100644
--- a/crates/core/src/app/launch/compose.rs
+++ b/crates/core/src/app/launch/compose.rs
@@ -1,11 +1,12 @@
-//! Compose: the addons' parts of a launch into its `Spawn` or `Respawn`
-//! (docs/addons.md, "Launches", step 4). Pure; the one implementation for
-//! every front-end. The env it writes is a secret: nothing here logs or
-//! formats it.
+//! Compose: the user's custom env and the addons' parts of a launch into
+//! its `Spawn` or `Respawn` (docs/addons.md, "Launches", step 4). Pure; the
+//! one implementation for every front-end. The env it writes is a secret:
+//! nothing here logs or formats it, and its notices name keys only.
 
 use std::collections::BTreeSet;
 
 use crate::addon::{line, LaunchArg, LaunchPart, TEXT_BYTES};
+use crate::claude::env::{check, name_ok, Value};
 use crate::proto::{
     lens_name_ok, Envelope, Frame, LensSpec, Msg, SessionExtensions, MAX_FRAME, MAX_INSTRUCTION,
     MAX_INSTRUCTIONS, MAX_LENSES, MAX_PLUGIN_INSTRUCTIONS,
@@ -14,11 +15,20 @@ use crate::proto::{
 /// The most bytes of a lens name quoted in a notice.
 const NAME_BYTES: usize = 64;
 
+/// How the notice naming the custom env an addon overrode starts (a
+/// recovery drops it: `super::send`).
+pub(super) const OVERRIDDEN: &str = "custom env overridden by ";
+
 /// `base` (a `Spawn` or `Respawn` without env, env_remove, optional or
-/// extensions) with the `parts`, in registry order, and the core's lenses:
+/// extensions) with the user's custom env (`app::env::Store::effective`),
+/// the `parts`, in registry order, and the core's lenses:
 ///
-/// - env in order, a later addon's key replacing an earlier one's, no key
-///   twice; `env_remove` the sorted union, always `Some`;
+/// - env: the user's first (an entry the validator refuses is left out with
+///   a notice), then each part's in order, a later key replacing an earlier
+///   one, no key twice; a user key that a part's `env_remove` names and no
+///   part sets is dropped (an addon's removal wins too); one notice names
+///   the user keys a part replaced or removed; `env_remove` the sorted
+///   union, always `Some`;
 /// - a `Spawn`'s args appended in order (`Optional { flag, value }` as
 ///   `flag value`, with `flag` in `optional`); a `Respawn` gets none (its
 ///   journal has them);
@@ -32,24 +42,18 @@ const NAME_BYTES: usize = 64;
 pub fn compose(
     base: Msg,
     core: SessionExtensions,
+    user: Vec<(String, Value)>,
     parts: Vec<(&'static str, LaunchPart)>,
 ) -> Result<(Msg, Vec<String>), String> {
     let mut notices = Vec::new();
-    let mut env: Vec<(String, String)> = Vec::new();
-    let mut env_remove = BTreeSet::new();
+    let mut env = Env::new(user, &mut notices);
     let mut args = Vec::new();
     let mut optional: Vec<String> = Vec::new();
     let mut lenses = Lenses::new(core.lenses);
     let mut instructions = Instructions::default();
     instructions.add("core", &core.instructions);
     for (addon, part) in parts {
-        for (key, value) in part.env {
-            match env.iter_mut().find(|(k, _)| *k == key) {
-                Some(slot) => slot.1 = value,
-                None => env.push((key, value)),
-            }
-        }
-        env_remove.extend(part.env_remove);
+        env.add(addon, part.env, part.env_remove);
         for LaunchArg::Optional { flag, value } in part.args {
             if !optional.contains(&flag) {
                 optional.push(flag.clone());
@@ -70,6 +74,7 @@ pub fn compose(
     };
     ext.validate()
         .map_err(|e| format!("launch extensions: {e}"))?;
+    let (env, env_remove) = env.finish(&mut notices);
     let env_remove = Some(env_remove.into_iter().collect());
     let msg = match base {
         Msg::Spawn(mut spec) => {
@@ -105,6 +110,94 @@ fn frame_fits(msg: &Msg) -> bool {
     frame.encode().len().saturating_sub(4) <= MAX_FRAME
 }
 
+/// The env as it is layered: the user's custom env, then each part's.
+struct Env {
+    list: Vec<(String, String)>,
+    /// The user's keys.
+    user: BTreeSet<String>,
+    /// The keys a part set.
+    set: BTreeSet<String>,
+    /// The union of the parts' `env_remove`.
+    remove: BTreeSet<String>,
+    /// The addons that replaced or removed a user key, and those keys, in
+    /// order, each once.
+    by: Vec<&'static str>,
+    overridden: Vec<String>,
+}
+
+impl Env {
+    /// The user's entries the validator takes; a refused one is a notice
+    /// (naming a well-formed key only).
+    fn new(user: Vec<(String, Value)>, notices: &mut Vec<String>) -> Env {
+        let mut env = Env {
+            list: Vec::new(),
+            user: BTreeSet::new(),
+            set: BTreeSet::new(),
+            remove: BTreeSet::new(),
+            by: Vec::new(),
+            overridden: Vec::new(),
+        };
+        for (key, value) in user {
+            match check(&key, value.as_str()) {
+                Ok(()) if !env.user.contains(&key) => {
+                    env.user.insert(key.clone());
+                    env.list.push((key, value.into_string()));
+                }
+                Ok(()) => {}
+                Err(why) if name_ok(&key) => notices.push(format!("custom env skipped: {why}")),
+                Err(_) => notices.push("custom env: an entry with a bad name skipped".to_owned()),
+            }
+        }
+        env
+    }
+
+    /// `addon`'s env (its keys replace earlier ones) and removals.
+    fn add(&mut self, addon: &'static str, env: Vec<(String, String)>, remove: Vec<String>) {
+        for (key, value) in env {
+            if self.user.contains(&key) {
+                self.overrode(addon, &key);
+            }
+            self.set.insert(key.clone());
+            match self.list.iter_mut().find(|(k, _)| *k == key) {
+                Some(slot) => slot.1 = value,
+                None => self.list.push((key, value)),
+            }
+        }
+        for key in remove {
+            if self.user.contains(&key) && !self.set.contains(&key) {
+                self.overrode(addon, &key);
+            }
+            self.remove.insert(key);
+        }
+    }
+
+    fn overrode(&mut self, addon: &'static str, key: &str) {
+        if !self.by.contains(&addon) {
+            self.by.push(addon);
+        }
+        if !self.overridden.iter().any(|k| k == key) {
+            self.overridden.push(key.to_owned());
+        }
+    }
+
+    /// The env (a user key a part removed and none set dropped) and
+    /// `env_remove`; one notice names the user keys the parts overrode.
+    fn finish(self, notices: &mut Vec<String>) -> (Vec<(String, String)>, BTreeSet<String>) {
+        if !self.overridden.is_empty() {
+            let text = format!(
+                "{OVERRIDDEN}{}: {}",
+                self.by.join(", "),
+                self.overridden.join(", ")
+            );
+            notices.push(line(&text, TEXT_BYTES));
+        }
+        let (user, set, remove) = (&self.user, &self.set, &self.remove);
+        let dropped = |k: &String| user.contains(k) && remove.contains(k) && !set.contains(k);
+        let list = self.list.into_iter().filter(|(k, _)| !dropped(k)).collect();
+        (list, self.remove)
+    }
+}
+
 /// The session's lenses as they are merged.
 struct Lenses {
     list: Vec<LensSpec>,
diff --git a/crates/core/src/app/launch/mod.rs b/crates/core/src/app/launch/mod.rs
index e42070b..ec65f9a 100644
--- a/crates/core/src/app/launch/mod.rs
+++ b/crates/core/src/app/launch/mod.rs
@@ -23,6 +23,8 @@ use crate::addon::{
     LaunchRequest, LaunchSelect, LegacyChoice, Pick, Routing, TEXT_BYTES,
 };
 use crate::app::addons::AddonRuntime;
+use crate::app::env::Store;
+use crate::claude::env::Value;
 use crate::client::Client;
 use crate::proto::{Msg, SessionExtensions, SessionId};
 
@@ -333,7 +335,7 @@ pub(super) async fn proceed(rt: &AddonRuntime, mut launch: Parked, client: Clien
             };
         }
         if launch.todo.is_empty() {
-            return send(launch, client).await;
+            return send(rt, launch, client).await;
         }
     }
 }
@@ -364,8 +366,9 @@ async fn prepare_all(
     join_all(calls).await
 }
 
-/// Compose `launch`'s parts and send the message.
-async fn send(mut launch: Parked, client: Client) -> Step {
+/// Read the user's custom env, compose it and `launch`'s parts, and send
+/// the message. A recovery drops the notice of the env an addon overrode.
+async fn send(rt: &AddonRuntime, mut launch: Parked, client: Client) -> Step {
     launch.sort_parts();
     let Parked {
         spec,
@@ -374,11 +377,17 @@ async fn send(mut launch: Parked, client: Client) -> Step {
         mut notices,
         ..
     } = launch;
-    let (msg, more) = match compose(spec.base, spec.core_lenses, parts) {
+    let (user, read) = user_env(rt, &spec).await;
+    notices.extend(read);
+    let recover = spec.op == LaunchOp::Recover;
+    let (msg, more) = match compose(spec.base, spec.core_lenses, user, parts) {
         Ok(composed) => composed,
         Err(e) => return Step::Failed(e),
     };
-    notices.extend(more);
+    notices.extend(
+        more.into_iter()
+            .filter(|n| !(recover && n.starts_with(compose::OVERRIDDEN))),
+    );
     let reply = client.request(msg).await;
     Step::Sent {
         choice,
@@ -387,6 +396,17 @@ async fn send(mut launch: Parked, client: Client) -> Step {
     }
 }
 
+/// The custom env of `spec`'s session (`app::env`, beside the runtime's
+/// config.toml; a new session has none of its own), read off the executor,
+/// and the read's notices.
+async fn user_env(rt: &AddonRuntime, spec: &LaunchSpec) -> (Vec<(String, Value)>, Vec<String>) {
+    let store = Store::beside(rt.config_path());
+    let (host, sid) = (spec.host.clone(), spec.sid);
+    tokio::task::spawn_blocking(move || store.effective(&host, sid))
+        .await
+        .unwrap_or_else(|_| (Vec::new(), vec!["custom env not read".to_owned()]))
+}
+
 /// The request `addon` prepares for.
 fn request(spec: &LaunchSpec, choice: Option<LaunchChoice>, routing: Routing) -> LaunchRequest {
     LaunchRequest {
diff --git a/crates/core/src/app/launch/tests.rs b/crates/core/src/app/launch/tests.rs
index 01f9598..cf10a41 100644
--- a/crates/core/src/app/launch/tests.rs
+++ b/crates/core/src/app/launch/tests.rs
@@ -151,7 +151,7 @@ fn compose_layers_env_later_addon_wins_no_key_twice() {
         ("a", env_part(&[("K", "1"), ("A", "x"), ("K", "1b")])),
         ("b", env_part(&[("K", "2"), ("B", "y")])),
     ];
-    let (msg, notices) = compose(spawn_base(), core(), parts).unwrap();
+    let (msg, notices) = compose(spawn_base(), core(), Vec::new(), parts).unwrap();
     let spec = spawned(&msg);
     let pairs: Vec<(&str, &str)> = spec
         .env
@@ -164,14 +164,14 @@ fn compose_layers_env_later_addon_wins_no_key_twice() {
 
 #[test]
 fn compose_unions_env_remove_and_always_sends_some() {
-    let (msg, _) = compose(spawn_base(), core(), Vec::new()).unwrap();
+    let (msg, _) = compose(spawn_base(), core(), Vec::new(), Vec::new()).unwrap();
     assert_eq!(spawned(&msg).env_remove, Some(Vec::new()));
     let part = |names: &[&str]| LaunchPart {
         env_remove: names.iter().map(|n| n.to_string()).collect(),
         ..Default::default()
     };
     let parts = vec![("a", part(&["Z", "A"])), ("b", part(&["A", "M"]))];
-    let (msg, _) = compose(respawn_base(), core(), parts).unwrap();
+    let (msg, _) = compose(respawn_base(), core(), Vec::new(), parts).unwrap();
     let Msg::Respawn(spec) = msg else {
         panic!("not a respawn")
     };
@@ -190,7 +190,7 @@ fn compose_marks_optional_args_and_respawn_drops_args() {
         ..Default::default()
     };
     let parts = vec![("a", part("{1}")), ("b", part("{2}"))];
-    let (msg, _) = compose(spawn_base(), core(), parts.clone()).unwrap();
+    let (msg, _) = compose(spawn_base(), core(), Vec::new(), parts.clone()).unwrap();
     let spec = spawned(&msg);
     let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
     assert_eq!(
@@ -206,7 +206,7 @@ fn compose_marks_optional_args_and_respawn_drops_args() {
     );
     assert_eq!(spec.optional, ["--mcp-config"]);
     // A respawn: env and extensions, no args (the journal has them).
-    let (msg, _) = compose(respawn_base(), core(), parts).unwrap();
+    let (msg, _) = compose(respawn_base(), core(), Vec::new(), parts).unwrap();
     let Msg::Respawn(spec) = msg else {
         panic!("not a respawn")
     };
@@ -239,7 +239,7 @@ fn compose_merges_lenses_first_wins_within_caps_and_cuts_instructions_at_8158()
         ..Default::default()
     };
     let parts = vec![("a", a), ("b", b), ("c", c)];
-    let (msg, notices) = compose(spawn_base(), core(), parts).unwrap();
+    let (msg, notices) = compose(spawn_base(), core(), Vec::new(), parts).unwrap();
     let ext = spawned(&msg).extensions.clone().unwrap();
     assert_eq!(ext.lenses.len(), MAX_LENSES);
     assert_eq!(ext.lenses[0], lens("latex", "write <latex>"), "first wins");
@@ -264,6 +264,7 @@ fn compose_merges_lenses_first_wins_within_caps_and_cuts_instructions_at_8158()
     let (msg, _) = compose(
         spawn_base(),
         core(),
+        Vec::new(),
         vec![("a", small("A")), ("b", small("B"))],
     )
     .unwrap();
@@ -273,11 +274,165 @@ fn compose_merges_lenses_first_wins_within_caps_and_cuts_instructions_at_8158()
     );
 }
 
+fn user(env: &[(&str, &str)]) -> Vec<(String, crate::claude::env::Value)> {
+    env.iter()
+        .map(|(k, v)| (k.to_string(), crate::claude::env::Value::new(*v)))
+        .collect()
+}
+
+fn env_of(msg: &Msg) -> Vec<(&str, &str)> {
+    let env = match msg {
+        Msg::Spawn(spec) => &spec.env,
+        Msg::Respawn(spec) => &spec.env,
+        other => panic!("not a launch: {other:?}"),
+    };
+    env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
+}
+
+#[test]
+fn compose_puts_the_user_env_first_in_spawn_and_respawn() {
+    let mine = user(&[("A", "1"), ("MULTI", "x\ny")]);
+    let parts = vec![("a", env_part(&[("P", "p")]))];
+    for base in [spawn_base(), respawn_base()] {
+        let (msg, notices) = compose(base, core(), mine.clone(), parts.clone()).unwrap();
+        assert_eq!(env_of(&msg), [("A", "1"), ("MULTI", "x\ny"), ("P", "p")]);
+        assert!(notices.is_empty(), "{notices:?}");
+    }
+}
+
+#[test]
+fn an_addon_key_replaces_a_user_key_and_one_notice_names_it() {
+    let mine = user(&[
+        ("ANTHROPIC_BASE_URL", "https://mine.example"),
+        ("KEEP", "k"),
+    ]);
+    let parts = vec![
+        (
+            "route",
+            env_part(&[("ANTHROPIC_BASE_URL", "http://proxy"), ("T", "t")]),
+        ),
+        ("other", env_part(&[("KEEP", "theirs")])),
+    ];
+    let (msg, notices) = compose(spawn_base(), core(), mine, parts).unwrap();
+    assert_eq!(
+        env_of(&msg),
+        [
+            ("ANTHROPIC_BASE_URL", "http://proxy"),
+            ("KEEP", "theirs"),
+            ("T", "t")
+        ]
+    );
+    assert_eq!(
+        notices,
+        ["custom env overridden by route, other: ANTHROPIC_BASE_URL, KEEP"]
+    );
+    assert!(!notices[0].contains("mine.example"), "keys only");
+}
+
+#[test]
+fn a_user_key_in_a_parts_env_remove_is_dropped_and_named() {
+    let mine = user(&[("ANTHROPIC_API_KEY", "sk-user"), ("X", "1")]);
+    let part = LaunchPart {
+        env: vec![("ANTHROPIC_AUTH_TOKEN".into(), "tok".into())],
+        env_remove: vec!["ANTHROPIC_API_KEY".into()],
+        ..Default::default()
+    };
+    let (msg, notices) = compose(spawn_base(), core(), mine, vec![("route", part)]).unwrap();
+    assert_eq!(env_of(&msg), [("X", "1"), ("ANTHROPIC_AUTH_TOKEN", "tok")]);
+    assert_eq!(
+        spawned(&msg).env_remove.as_deref(),
+        Some(&["ANTHROPIC_API_KEY".to_owned()][..])
+    );
+    assert_eq!(
+        notices,
+        ["custom env overridden by route: ANTHROPIC_API_KEY"]
+    );
+    assert!(!format!("{notices:?}").contains("sk-user"));
+}
+
+#[test]
+fn an_invalid_user_entry_is_skipped_with_a_notice() {
+    let mine = user(&[
+        ("PATH", "/x"),
+        ("1BAD", "sk-SECRET"),
+        ("CTL", "a\rb"),
+        ("OK", "y"),
+    ]);
+    let (msg, notices) = compose(spawn_base(), core(), mine, Vec::new()).unwrap();
+    assert_eq!(env_of(&msg), [("OK", "y")]);
+    assert_eq!(notices.len(), 3, "{notices:?}");
+    assert!(notices[0].starts_with("custom env skipped: PATH: refused: "));
+    assert_eq!(notices[1], "custom env: an entry with a bad name skipped");
+    assert!(notices[2].starts_with("custom env skipped: CTL: value holds"));
+    assert!(!format!("{notices:?}").contains("SECRET"));
+}
+
+#[test]
+fn an_empty_user_env_changes_nothing() {
+    let parts = vec![
+        ("a", env_part(&[("K", "a"), ("A", "1")])),
+        ("b", env_part(&[("K", "b")])),
+    ];
+    let with = compose(spawn_base(), core(), Vec::new(), parts.clone()).unwrap();
+    assert_eq!(env_of(&with.0), [("K", "b"), ("A", "1")]);
+    assert!(with.1.is_empty());
+}
+
+/// A runtime reading `config.toml` (and `session-env.json`) in `dir`.
+fn runtime_in(dir: &std::path::Path, addons: Vec<Arc<dyn Addon>>) -> (AddonRuntime, RuntimeRx) {
+    let addons = Arc::new(Addons::new(addons).unwrap());
+    let opts = RuntimeOptions {
+        config_path: dir.join("config.toml"),
+        watch_config: false,
+    };
+    AddonRuntime::start(addons, opts)
+}
+
+#[tokio::test]
+async fn the_composer_reads_the_custom_env_for_every_op_and_a_recovery_drops_the_override_notice() {
+    let dir = std::env::temp_dir().join(format!("claudio-launch-env-{}", uuid::Uuid::new_v4()));
+    std::fs::create_dir_all(&dir).unwrap();
+    std::fs::write(
+        dir.join("config.toml"),
+        "[env]\nG = \"global\"\nT = \"mine\"\n",
+    )
+    .unwrap();
+    let store = crate::app::env::Store::beside(&dir.join("config.toml"));
+    let scope = crate::app::env::Scope::Session {
+        host: "local".into(),
+        sid: sid(),
+    };
+    store
+        .set(&scope, "G", &crate::claude::env::Value::new("session"))
+        .unwrap();
+    let addon = Arc::new(
+        TestAddon::new("route")
+            .routing(Ok(Routing::Optional))
+            .prepare(Script::Ok(env_part(&[("T", "addon")]))),
+    );
+    let (rt, _rx) = runtime_in(&dir, vec![addon]);
+    for op in [LaunchOp::New, LaunchOp::Respawn, LaunchOp::Recover] {
+        let (client, sent) = daemon().await;
+        let step = run(&rt, spec(op, LaunchSelect::default()), client).await;
+        let Step::Sent { notices, .. } = step else {
+            panic!("{op:?}: {step:?}");
+        };
+        let msg = sent.lock().unwrap()[0].clone();
+        assert_eq!(env_of(&msg), [("G", "session"), ("T", "addon")], "{op:?}");
+        let overridden = notices
+            .iter()
+            .any(|n| n.starts_with("custom env overridden"));
+        assert_eq!(overridden, op != LaunchOp::Recover, "{op:?}: {notices:?}");
+    }
+    rt.shutdown();
+    let _ = std::fs::remove_dir_all(&dir);
+}
+
 #[test]
 fn compose_refuses_a_frame_over_the_cap() {
     let big = "v".repeat(crate::proto::MAX_FRAME);
     let parts = vec![("a", env_part(&[("BIG", &big)]))];
-    let err = compose(spawn_base(), core(), parts).unwrap_err();
+    let err = compose(spawn_base(), core(), Vec::new(), parts).unwrap_err();
     assert_eq!(err, "launch too large");
     assert!(!err.contains("vvv"));
 }
````
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-core --lib app::launch`
Expected: `test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 586 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --locked --workspace --lib --bins`
Expected: every `test result: ok.` (claudio-core `609 passed`, claudio-tui `393 passed`).
Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && python3 scripts/check_rules.py`
Expected: clean.

Step 3: commit
`git commit -m "feat(core): the launch composer applies the custom env, addons winning" -- crates/core/src/app/launch/compose.rs crates/core/src/app/launch/mod.rs crates/core/src/app/launch/tests.rs crates/core/src/app/addons/mod.rs`

Interfaces this task provides to later tasks: `AddonRuntime::config_path(&self) -> &Path`;
`app::launch::compose(base, core, user: Vec<(String, Value)>, parts)`; the launch notice
`custom env overridden by <addons>: <keys>` (front-ends show it as any launch notice).

### Task 5: TUI: Alt+, opens the custom env menu; a session's env goes with it
Model: sonnet   Repo: /home/p4u/claudio   Files: crates/tui/src/interaction.rs,
crates/tui/src/reader/mod.rs, crates/tui/src/keymap.rs, crates/tui/src/app.rs,
crates/tui/src/lib.rs, crates/tui/src/addon_runner.rs, crates/tui/src/sessions.rs,
crates/tui/src/env_view.rs (new), crates/tui/src/env_view/tests.rs (new),
crates/tui/src/env_edge.rs (new), crates/tui/src/ui/env.rs (new), crates/tui/src/ui/mod.rs,
crates/tui/tests/common/mod.rs, crates/tui/tests/manager.rs
Goal: AC4, AC5, AC8 (TUI part): `Action::Environment` on Alt+, (`Scope::Global`, not in
`--plain`), `Modal::Environment` with Global/Session tabs, add (name with catalogue matches, then
value), edit, delete with a confirm, the escaped form, the footer; reads and writes as
`Effect::Env` run by `env_edge.rs`; a claude tab's removal (Kill, a `Removed` event, a cancelled
launch) forgets its entry; a host's session list prunes.
Rules: A2/A3 (no addon named), A5 (the store is `app::env`), A6 (`env_view.rs` pure; I/O in
`env_edge.rs` via the existing `Effect::Addon` arm of `run_effect`), A7 (chrome around the pane
only), A8 (see Global Constraints: `interaction.rs`, `modal_key`, `run_effect` may not grow; the
first commit makes room), E4 (screen model only).

Two commits: the A8 refactor (no behaviour change), then the menu.

Step 1: write the failing test
1. (Refactor commit: no new test; `cargo test -p claudio-tui --lib` stays `393 passed`.)
2. Menu commit: add the keymap test (`alt_comma_opens_the_environment_menu_and_collides_with_nothing`
   in the `keymap.rs` diff), and the PTY tests and helpers at the end of
   `crates/tui/tests/manager.rs` with `ALT_COMMA` in `tests/common/mod.rs` (their hunks in the
   diff of Step 2.2). The unit tests of the menu, `env_view/tests.rs`, come with the module.
   Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib keymap`
   Expected: ``error[E0599]: no variant or associated item named `Environment` found for enum `Action` ``.
   Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test manager test_env_menu -- --test-threads=1`
   Expected: `wait_for timeout: "applies on next start or restart" not found in Screen` and
   `test result: FAILED. 0 passed; 1 failed`.

Step 2: implement
1. Refactor (A8 room): the reader view's glue (`open_reader`, `reader_mut`,
   `on_answers_reply`, `on_answers_rendered`, `on_answers_failed`) moves from `interaction.rs`
   to `reader/mod.rs` unchanged but for visibility, and `modal_key`'s reader arm becomes one
   line calling the new `App::reader_key`:
````diff
diff --git a/crates/tui/src/interaction.rs b/crates/tui/src/interaction.rs
index 4f38e41..d4c3c39 100644
--- a/crates/tui/src/interaction.rs
+++ b/crates/tui/src/interaction.rs
@@ -23,7 +23,7 @@ use super::extensions_view::ExtensionsView;
 use super::git_view::GitView;
 use super::keymap::Action;
 use super::leader::{Input, Picker};
-use super::reader::{ReaderView, RenderedAnswer};
+use super::reader::ReaderView;
 use super::selection::{self, Pos, Selection, Unit};
 use super::sessions::SessionView;
 use super::ui;
@@ -233,67 +233,6 @@ impl App {
         self.redraw = true;
     }
 
-    /// The reader view of the active session (none without one): it asks
-    /// the session's daemon for the answers, and shows "Loading…" until
-    /// they are rendered.
-    pub(super) fn open_reader(&mut self) {
-        let Some((id, host)) = self.active_view().map(|v| (v.id, v.host.clone())) else {
-            return;
-        };
-        self.modal_gen += 1;
-        let gen = self.modal_gen;
-        self.modal = Some(Modal::Reader(Box::new(ReaderView::new(id, &host, gen))));
-        self.request(&host, Msg::Answers { id }, ReplyTo::Answers { gen, id });
-    }
-
-    /// The reader view opened under `gen` for session `id`, if it is the
-    /// one open: a reply for a closed view or another session is dropped.
-    fn reader_mut(&mut self, gen: u64, id: SessionId) -> Option<&mut ReaderView> {
-        match &mut self.modal {
-            Some(Modal::Reader(view)) if view.gen == gen && view.session == id => Some(view),
-            _ => None,
-        }
-    }
-
-    /// The daemon's answers for the reader view: parsed off the UI thread
-    /// (none to render: shown at once), or why there are none.
-    fn on_answers_reply(&mut self, gen: u64, id: SessionId, reply: io::Result<Msg>) {
-        let Some(view) = self.reader_mut(gen, id) else {
-            return;
-        };
-        let host = view.host.clone();
-        match reply {
-            Ok(Msg::AnswersReply { answers, .. }) if answers.is_empty() => {
-                view.set_answers(Vec::new())
-            }
-            Ok(Msg::AnswersReply { answers, .. }) => {
-                self.emit(Effect::RenderAnswers { gen, id, answers })
-            }
-            Ok(_) => view.set_failed("unexpected reply from the daemon"),
-            Err(e) => {
-                let why = App::older_daemon_notice(&host, &e).unwrap_or_else(|| e.to_string());
-                view.set_failed(why);
-            }
-        }
-    }
-
-    /// The answers of the reader view opened under `gen` for session `id`
-    /// were parsed (off the UI thread): shown, if that view is open.
-    pub fn on_answers_rendered(&mut self, gen: u64, id: SessionId, answers: Vec<RenderedAnswer>) {
-        if let Some(view) = self.reader_mut(gen, id) {
-            view.set_answers(answers);
-            self.redraw = true;
-        }
-    }
-
-    /// The answers of the reader view `gen` could not be rendered.
-    pub fn on_answers_failed(&mut self, gen: u64, id: SessionId, why: &str) {
-        if let Some(view) = self.reader_mut(gen, id) {
-            view.set_failed(why);
-            self.redraw = true;
-        }
-    }
-
     pub fn modal_key(&mut self, key: KeyEvent) {
         if key.kind == KeyEventKind::Release {
             return;
@@ -395,11 +334,7 @@ impl App {
                 let outcome = view.on_key(&key, rows);
                 self.git_outcome(outcome);
             }
-            Modal::Reader(view) => {
-                if view.on_key(&key) {
-                    self.modal = None;
-                }
-            }
+            Modal::Reader(_) => self.reader_key(&key),
             Modal::Leader => self.leader_key(&key),
             Modal::ActionInput(_) => self.action_input_key(&key),
             Modal::Picker(_) => self.picker_key(&key),
diff --git a/crates/tui/src/reader/mod.rs b/crates/tui/src/reader/mod.rs
index 84f53c9..e619b8f 100644
--- a/crates/tui/src/reader/mod.rs
+++ b/crates/tui/src/reader/mod.rs
@@ -12,15 +12,19 @@ pub mod layout;
 
 use std::cell::{Cell, RefCell};
 use std::collections::HashMap;
+use std::io;
 use std::rc::Rc;
 
 use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
 use ratatui::style::{Modifier, Style};
 use unicode_width::UnicodeWidthStr;
 
-use claudio_core::proto::SessionId;
+use claudio_core::proto::{Msg, SessionId};
 use layout::{Line, Seg};
 
+use crate::app::{App, Effect, ReplyTo};
+use crate::interaction::Modal;
+
 /// How far PageUp/PageDown (and Space) scroll, and the mouse wheel.
 const PAGE: u16 = 10;
 const WHEEL: u16 = 3;
@@ -260,6 +264,80 @@ fn fit(text: &str, width: usize) -> String {
     out
 }
 
+// ── App glue: opening the view, the daemon's reply, the parsed answers ──────
+
+impl App {
+    /// The reader view of the active session (none without one): it asks
+    /// the session's daemon for the answers, and shows "Loading…" until
+    /// they are rendered.
+    pub(crate) fn open_reader(&mut self) {
+        let Some((id, host)) = self.active_view().map(|v| (v.id, v.host.clone())) else {
+            return;
+        };
+        self.modal_gen += 1;
+        let gen = self.modal_gen;
+        self.modal = Some(Modal::Reader(Box::new(ReaderView::new(id, &host, gen))));
+        self.request(&host, Msg::Answers { id }, ReplyTo::Answers { gen, id });
+    }
+
+    /// The reader view opened under `gen` for session `id`, if it is the
+    /// one open: a reply for a closed view or another session is dropped.
+    fn reader_mut(&mut self, gen: u64, id: SessionId) -> Option<&mut ReaderView> {
+        match &mut self.modal {
+            Some(Modal::Reader(view)) if view.gen == gen && view.session == id => Some(view),
+            _ => None,
+        }
+    }
+
+    /// The daemon's answers for the reader view: parsed off the UI thread
+    /// (none to render: shown at once), or why there are none.
+    pub(crate) fn on_answers_reply(&mut self, gen: u64, id: SessionId, reply: io::Result<Msg>) {
+        let Some(view) = self.reader_mut(gen, id) else {
+            return;
+        };
+        let host = view.host.clone();
+        match reply {
+            Ok(Msg::AnswersReply { answers, .. }) if answers.is_empty() => {
+                view.set_answers(Vec::new())
+            }
+            Ok(Msg::AnswersReply { answers, .. }) => {
+                self.emit(Effect::RenderAnswers { gen, id, answers })
+            }
+            Ok(_) => view.set_failed("unexpected reply from the daemon"),
+            Err(e) => {
+                let why = App::older_daemon_notice(&host, &e).unwrap_or_else(|| e.to_string());
+                view.set_failed(why);
+            }
+        }
+    }
+
+    /// The answers of the reader view opened under `gen` for session `id`
+    /// were parsed (off the UI thread): shown, if that view is open.
+    pub fn on_answers_rendered(&mut self, gen: u64, id: SessionId, answers: Vec<RenderedAnswer>) {
+        if let Some(view) = self.reader_mut(gen, id) {
+            view.set_answers(answers);
+            self.redraw = true;
+        }
+    }
+
+    /// The answers of the reader view `gen` could not be rendered.
+    pub fn on_answers_failed(&mut self, gen: u64, id: SessionId, why: &str) {
+        if let Some(view) = self.reader_mut(gen, id) {
+            view.set_failed(why);
+            self.redraw = true;
+        }
+    }
+
+    /// A key in the reader view: Esc (or q) closes it.
+    pub(crate) fn reader_key(&mut self, key: &KeyEvent) {
+        if let Some(Modal::Reader(view)) = &mut self.modal {
+            if view.on_key(key) {
+                self.modal = None;
+            }
+        }
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
````
   Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib && python3 scripts/check_rules.py`
   Expected: `test result: ok. 393 passed; 0 failed`; no violation (`interaction.rs` 959
   production lines, `modal_key` 109). Commit it (Step 3, first commit).
2. The menu. Modified files:
````diff
diff --git a/crates/tui/src/addon_runner.rs b/crates/tui/src/addon_runner.rs
index 2a17b04..7db14af 100644
--- a/crates/tui/src/addon_runner.rs
+++ b/crates/tui/src/addon_runner.rs
@@ -298,6 +298,8 @@ pub(super) fn run(effect: Effect, app: &mut App, edge: &Edge, tx: &mpsc::Sender<
             toggle_row(edge, (addon, name, on), tx)
         }
         Effect::Addon(AddonCall::RefreshRows) => refresh_rows(edge, tx),
+        // The custom env lives beside the runtime's config.toml.
+        Effect::Env(call) => super::env_edge::run(call, edge.config_path(), tx),
         _ => {}
     }
 }
diff --git a/crates/tui/src/app.rs b/crates/tui/src/app.rs
index 0982c9f..3f81044 100644
--- a/crates/tui/src/app.rs
+++ b/crates/tui/src/app.rs
@@ -13,6 +13,7 @@
 //! - `confirm`       – the generic multiple-choice prompt
 //! - `checklist`     – addon checklists (a plugin's setup, a review)
 //! - `addon_calls`   – what `App` asks of the addons, and their answers
+//! - `env_view`      – the custom env menu (Alt+,)
 //! - `claude_update` – keeping claude up to date on every host
 //! - `panel_view`    – addon panels (the proxy stats, Alt+s)
 //! - `git_app`       – the history viewer's glue (its state is `git_view`)
@@ -139,6 +140,9 @@ pub enum Effect {
     },
     /// A call into an addon (`addon_runner::run`).
     Addon(AddonCall),
+    /// A read or a write of the custom env, off the UI thread
+    /// (`env_edge::run`); a read's result goes to [`App::on_env`].
+    Env(super::env_view::EnvCall),
 }
 
 /// A launch for the event loop's runner (`addon_runner::launch`). It holds
diff --git a/crates/tui/src/interaction.rs b/crates/tui/src/interaction.rs
index d4c3c39..9878f23 100644
--- a/crates/tui/src/interaction.rs
+++ b/crates/tui/src/interaction.rs
@@ -69,6 +69,9 @@ pub enum Modal {
     /// An addon's checklist (see `checklist.rs`): swallows every key,
     /// like `Confirm`.
     Checklist(Box<super::checklist::ChecklistView>),
+    /// The custom env menu (Alt+,, see `env_view.rs`): it types, so it
+    /// swallows every key.
+    Environment(Box<super::env_view::EnvView>),
 }
 
 impl Modal {
@@ -214,6 +217,7 @@ impl App {
             Action::Formulas => self.open_reader(),
             Action::Extensions => self.open_leader(),
             Action::ExtensionsMenu => self.open_extensions_menu(),
+            Action::Environment => self.open_environment(),
             _ => {}
         }
         self.redraw = true;
@@ -340,6 +344,7 @@ impl App {
             Modal::Picker(_) => self.picker_key(&key),
             Modal::ExtensionsMenu(_) => self.extensions_menu_key(&key),
             Modal::Checklist(_) => self.checklist_key(&key),
+            Modal::Environment(_) => self.env_key(&key),
         }
     }
 
@@ -401,6 +406,10 @@ impl App {
                 view.on_paste(text);
                 self.redraw = true;
             }
+            Some(Modal::Environment(view)) => {
+                view.on_paste(text);
+                self.redraw = true;
+            }
             Some(
                 Modal::Panel(_)
                 | Modal::Overview { .. }
diff --git a/crates/tui/src/keymap.rs b/crates/tui/src/keymap.rs
index c10535a..28747bf 100644
--- a/crates/tui/src/keymap.rs
+++ b/crates/tui/src/keymap.rs
@@ -46,6 +46,9 @@ pub enum Action {
     /// Open the extensions screen: each extension's description, keys and
     /// lenses, and turn it on or off.
     ExtensionsMenu,
+    /// Open the custom env menu: the env vars of claude sessions, global and
+    /// the focused session's.
+    Environment,
     /// Show/hide dot-directories in the new-session wizard (wizard scope).
     ToggleHidden,
     /// Switch ssh agent forwarding for the wizard's highlighted remote host
@@ -76,6 +79,7 @@ impl Action {
             Action::Formulas => "formulas",
             Action::Extensions => "extensions",
             Action::ExtensionsMenu => "extensions_menu",
+            Action::Environment => "environment",
             Action::ToggleHidden => "toggle_hidden",
             Action::ForwardAgent => "forward_agent",
             Action::GotoSession(_) => "goto_session",
@@ -243,6 +247,16 @@ pub const DEFAULT_BINDINGS: &[Binding] = &[
         action: Action::ExtensionsMenu,
         scope: Scope::Global,
     },
+    // Alt+,: the custom env menu. No Alt+letter is free (see above); Claude
+    // Code 2.1.323 binds no Alt+punctuation (no "alt+,"/"meta+," in its
+    // bundle), and readline leaves Alt+, unbound.
+    Binding {
+        code: KeyCode::Char(','),
+        mods: KeyModifiers::ALT,
+        label: "custom env vars of claude sessions",
+        action: Action::Environment,
+        scope: Scope::Global,
+    },
     // Quit is last so it's always visible in the status bar even when truncated.
     Binding {
         code: KeyCode::Char('q'),
@@ -757,6 +771,32 @@ mod tests {
         }
     }
 
+    /// Alt+, opens the custom env menu, in the manager only, and no other
+    /// binding of any scope has it.
+    #[test]
+    fn alt_comma_opens_the_environment_menu_and_collides_with_nothing() {
+        let km = Keymap::default();
+        let alt_comma = key(KeyCode::Char(','), KeyModifiers::ALT);
+        assert_eq!(km.lookup(&alt_comma), Some(Action::Environment));
+        assert_eq!(km.key_for(Action::Environment).as_deref(), Some("Alt+,"));
+        assert_eq!(Action::Environment.name(), "environment");
+        let taken = DEFAULT_BINDINGS
+            .iter()
+            .filter(|b| b.code == KeyCode::Char(',') && b.mods == KeyModifiers::ALT)
+            .count();
+        assert_eq!(taken, 1);
+        assert_eq!(
+            parse_key_spec("alt+,"),
+            Ok((KeyCode::Char(','), KeyModifiers::ALT))
+        );
+        let help = km.help_entries();
+        assert!(help
+            .iter()
+            .any(|(k, label)| k == "Alt+," && label.contains("env")));
+        let plain = Keymap::build_plain(&Default::default(), &mut Vec::new(), &[]);
+        assert_eq!(plain.lookup(&alt_comma), None, "--plain has no menu");
+    }
+
     #[test]
     fn modifiers_must_match_exactly() {
         let km = Keymap::default();
diff --git a/crates/tui/src/lib.rs b/crates/tui/src/lib.rs
index 7b45e84..4c5c597 100644
--- a/crates/tui/src/lib.rs
+++ b/crates/tui/src/lib.rs
@@ -11,6 +11,8 @@ mod builtins;
 mod checklist;
 mod claude_update;
 mod confirm;
+mod env_edge;
+mod env_view;
 mod extensions_view;
 mod fmt;
 mod git_app;
@@ -435,6 +437,8 @@ enum HostEvent {
     Launched(app::Launched),
     /// What an addon call's task found (`addon_runner`).
     Addon(app::AddonResult),
+    /// What a read or a write of the custom env found (`env_edge`).
+    Env(env_view::EnvDone),
 }
 
 // ── Event loop ────────────────────────────────────────────────────────────────
@@ -669,6 +673,7 @@ async fn event_loop(
                 Some(HostEvent::Notice(text)) => app.notify(text),
                 Some(HostEvent::Launched(done)) => app.on_launched(done),
                 Some(HostEvent::Addon(result)) => app.on_addon(result),
+                Some(HostEvent::Env(done)) => app.on_env(done),
                 None => return Ok(Default::default()),
             },
             Some((to, reply)) = reply_rx.recv() => addon_runner::on_reply(&mut app, &runtime, to, reply),
@@ -782,7 +787,7 @@ fn run_effect(
             // Stopping a program waits up to its grace: never here.
             tokio::spawn(ext_host.set_enabled(&name, on));
         }
-        e @ Effect::Addon(_) => addon_runner::run(e, app, runtime, ev_tx),
+        e @ (Effect::Addon(_) | Effect::Env(_)) => addon_runner::run(e, app, runtime, ev_tx),
         Effect::SaveExtension { name, on } => addon_runner::save_extension(name, on, ev_tx),
         Effect::RenderAnswers { gen, id, answers } => {
             // Markdown from the transcript is parsed off the UI thread.
diff --git a/crates/tui/src/sessions.rs b/crates/tui/src/sessions.rs
index 6510a35..73b15ec 100644
--- a/crates/tui/src/sessions.rs
+++ b/crates/tui/src/sessions.rs
@@ -296,6 +296,9 @@ impl App {
         }
         let removed = self.sessions.remove(i);
         self.segments.remove(&removed.id);
+        if removed.kind == SessionKind::Claude {
+            self.forget_env(&removed.host, removed.id);
+        }
         match self.active {
             Some(a) if a == i => {
                 self.active = None;
@@ -713,6 +716,8 @@ impl App {
         sessions.extend(recovered);
         self.sessions = sessions;
         self.active = active_id.and_then(|id| self.index_of(id));
+        // Sessions removed while no front-end ran: their custom env goes.
+        self.prune_env(host, live.iter().map(|info| info.id).collect());
     }
 
     /// Re-run recovery after the local daemon reconnects.
diff --git a/crates/tui/src/ui/mod.rs b/crates/tui/src/ui/mod.rs
index e43e693..2272aa3 100644
--- a/crates/tui/src/ui/mod.rs
+++ b/crates/tui/src/ui/mod.rs
@@ -13,6 +13,7 @@ use claudio_core::proto::{SessionKind, SessionState};
 
 mod checklist;
 mod confirm;
+mod env;
 mod extensions;
 mod git;
 mod leader;
@@ -103,6 +104,7 @@ fn draw_modal(frame: &mut Frame, app: &App, pane: Rect) {
         Some(Modal::Picker(picker)) => leader::draw_picker(frame, picker),
         Some(Modal::ExtensionsMenu(view)) => extensions::draw_extensions(frame, app, view),
         Some(Modal::Checklist(view)) => checklist::draw_checklist(frame, view),
+        Some(Modal::Environment(view)) => env::draw_env(frame, app, view),
         None => {}
     }
 }
diff --git a/crates/tui/tests/common/mod.rs b/crates/tui/tests/common/mod.rs
index 7a131db..dd98f35 100644
--- a/crates/tui/tests/common/mod.rs
+++ b/crates/tui/tests/common/mod.rs
@@ -1496,3 +1496,6 @@ pub fn scopeguard<F: FnOnce()>(f: F) -> ScopeGuard<F> {
 pub const ALT_Z: &[u8] = b"\x1bz";
 
 pub const ALT_I: &[u8] = b"\x1bi";
+
+/// Alt+, (the custom env menu).
+pub const ALT_COMMA: &[u8] = b"\x1b,";
diff --git a/crates/tui/tests/manager.rs b/crates/tui/tests/manager.rs
index 7af349a..1556932 100644
--- a/crates/tui/tests/manager.rs
+++ b/crates/tui/tests/manager.rs
@@ -27,10 +27,10 @@ use std::{fs, thread};
 
 use common::{
     current_nonce, encode_cwd, extract_nonce, run_git, scopeguard, wizard_pick_dir,
-    ClaudeProjectGuard, FakeSsh, ManagerHarness, Region, TuiProcess, ALT_C, ALT_E, ALT_F, ALT_G,
-    ALT_H, ALT_L, ALT_LEFT, ALT_N, ALT_Q, ALT_R, ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2, ALT_U, ALT_X,
-    ALT_Z, BINARY, CTRL_U, DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, LEFT_ARROW, RECONNECT_WAIT,
-    RIGHT_ARROW, UP_ARROW, WAIT,
+    ClaudeProjectGuard, FakeSsh, ManagerHarness, Region, TuiProcess, ALT_C, ALT_COMMA, ALT_E,
+    ALT_F, ALT_G, ALT_H, ALT_L, ALT_LEFT, ALT_N, ALT_Q, ALT_R, ALT_RIGHT, ALT_SHIFT_1, ALT_SHIFT_2,
+    ALT_U, ALT_X, ALT_Z, BINARY, CTRL_U, DAEMON_WAIT, DOWN_ARROW, ENTER, ESC, LEFT_ARROW,
+    RECONNECT_WAIT, RIGHT_ARROW, UP_ARROW, WAIT,
 };
 use portable_pty::CommandBuilder;
 
@@ -2282,3 +2282,249 @@ fn test_missing_claude_is_installed_at_start() {
     wizard_pick_dir(&mut tui, &h.dirs[0]);
     tui.quit(WAIT);
 }
+
+// ── Custom env (Alt+,) ────────────────────────────────────────────────────────
+
+/// Where the env-recording fake claude of these tests writes: its own cwd,
+/// so each session's last launch has its own record.
+const ENV_RECORD: &str = "$PWD/claude.env";
+
+/// A harness whose fake claude records `MY_VAR` and
+/// `ANTHROPIC_DEFAULT_OPUS_MODEL` at each launch into its cwd.
+fn env_harness() -> ManagerHarness {
+    let harness = ManagerHarness::new();
+    harness.write_env_recording_fake_claude(
+        std::path::Path::new(ENV_RECORD),
+        &["MY_VAR", "ANTHROPIC_DEFAULT_OPUS_MODEL"],
+        &[],
+    );
+    harness
+}
+
+/// What the fake claude in `dir` recorded at its last launch (waits for it).
+fn recorded_env(dir: &std::path::Path) -> String {
+    let file = dir.join("claude.env");
+    let deadline = Instant::now() + WAIT;
+    loop {
+        if let Ok(env) = fs::read_to_string(&file) {
+            return env;
+        }
+        assert!(Instant::now() < deadline, "no launch recorded in {dir:?}");
+        thread::sleep(Duration::from_millis(50));
+    }
+}
+
+/// session-env.json, parsed (`null` while it does not exist).
+fn session_env(harness: &ManagerHarness) -> serde_json::Value {
+    let path = harness.config_home.join("claudio/session-env.json");
+    fs::read_to_string(path)
+        .ok()
+        .and_then(|s| serde_json::from_str(&s).ok())
+        .unwrap_or(serde_json::Value::Null)
+}
+
+/// The id state.json gives the session in `dir`.
+fn sid_in(harness: &ManagerHarness, dir: &std::path::Path) -> String {
+    let dir = dir.to_str().unwrap();
+    let state = harness.read_state();
+    let sessions = state["sessions"].as_array().cloned().unwrap_or_default();
+    let session = sessions.iter().find(|s| s["cwd"] == dir);
+    session.expect("the session is saved")["id"]
+        .as_str()
+        .unwrap()
+        .to_owned()
+}
+
+/// Wait until session-env.json has (`want`) or has not an entry for `sid`.
+fn wait_session_env(harness: &ManagerHarness, sid: &str, want: bool) {
+    let deadline = Instant::now() + WAIT;
+    while session_env(harness)["sessions"].get(sid).is_some() != want {
+        assert!(Instant::now() < deadline, "session-env.json: {sid} {want}");
+        thread::sleep(Duration::from_millis(50));
+    }
+}
+
+/// In the open menu, on its open tab: `a`, the name, the value, saved.
+fn add_var(tui: &mut TuiProcess, name: &str, value: &str) {
+    tui.send_keys(b"a");
+    tui.wait_for("name:", Region::Screen, WAIT);
+    tui.send_keys(format!("{name}\r").as_bytes());
+    tui.wait_for("\\n newline", Region::Screen, WAIT);
+    tui.send_keys(format!("{value}\r").as_bytes());
+    let row = format!("{name} = {value}");
+    tui.wait_until(
+        |s| s.contains(&row, Region::Screen) && !s.contains("\\n newline", Region::Screen),
+        WAIT,
+    );
+}
+
+/// Close the menu (Esc, alone: nothing follows it until it is gone).
+fn close_env_menu(tui: &mut TuiProcess) {
+    tui.send_keys(ESC);
+    tui.wait_until(
+        |s| !s.contains("applies on next start", Region::Screen),
+        WAIT,
+    );
+}
+
+/// AC4: Alt+, sets a global and a session var; a restart of the session gets
+/// both (its own value wins), a second session only the global one; the
+/// catalogue offers names and describes them; PATH is refused; closing the
+/// session drops its entry; a terminal tab has no session tab and its shell
+/// none of the vars.
+#[test]
+fn test_env_menu_sets_global_and_session_vars_for_the_next_start() {
+    let harness = env_harness();
+    let mut tui = harness.start_tui();
+    wizard_pick_dir(&mut tui, &harness.dirs[0]);
+    let first = recorded_env(&harness.dirs[0]);
+    assert!(!first.contains("MY_VAR="), "{first}");
+
+    tui.send_keys(ALT_COMMA);
+    tui.wait_for("applies on next start or restart", Region::Screen, WAIT);
+    // Typing part of a documented name offers it; added, it is described.
+    tui.send_keys(b"a");
+    tui.wait_for("name:", Region::Screen, WAIT);
+    tui.send_keys(b"ANTHROPIC_DEF");
+    tui.wait_for("ANTHROPIC_DEFAULT_OPUS_MODEL", Region::Screen, WAIT);
+    tui.send_keys(b"AULT_OPUS_MODEL\r");
+    tui.wait_for("\\n newline", Region::Screen, WAIT);
+    tui.send_keys(b"opus-global\r");
+    tui.wait_until(
+        |s| {
+            s.contains("ANTHROPIC_DEFAULT_OPUS_MODEL = opus-global", Region::Screen)
+                && s.contains("Model ID that the opus alias resolves to", Region::Screen)
+        },
+        WAIT,
+    );
+    // PATH is refused and nothing is saved.
+    tui.send_keys(b"aPATH\r");
+    tui.wait_for("PATH: refused", Region::Screen, WAIT);
+    tui.send_keys(CTRL_U);
+    tui.send_keys(b"MY_VAR\r");
+    tui.wait_for("\\n newline", Region::Screen, WAIT);
+    tui.send_keys(b"global\r");
+    tui.wait_until(
+        |s| {
+            s.contains("MY_VAR = global", Region::Screen)
+                && !s.contains("\\n newline", Region::Screen)
+        },
+        WAIT,
+    );
+    // The session tab: this session's own value.
+    tui.send_keys(b"\t");
+    tui.wait_for("none yet", Region::Screen, WAIT);
+    add_var(&mut tui, "MY_VAR", "session");
+    close_env_menu(&mut tui);
+
+    let config = fs::read_to_string(harness.config_home.join("claudio/config.toml")).unwrap();
+    assert!(
+        config.contains("ANTHROPIC_DEFAULT_OPUS_MODEL = \"opus-global\""),
+        "{config}"
+    );
+    assert!(config.contains("MY_VAR = \"global\""), "{config}");
+    assert!(!config.contains("PATH"), "{config}");
+    let sid = sid_in(&harness, &harness.dirs[0]);
+    let entry = &session_env(&harness)["sessions"][&sid];
+    assert_eq!(entry["host"], "local");
+    assert_eq!(entry["env"]["MY_VAR"], "session");
+
+    // A restart gets both, the session's value winning.
+    fs::remove_file(harness.dirs[0].join("claude.env")).unwrap();
+    tui.send_keys(ALT_E);
+    tui.wait_for("restart & resume", Region::Screen, WAIT);
+    tui.send_keys(b"r");
+    let env = recorded_env(&harness.dirs[0]);
+    assert!(env.lines().any(|l| l == "MY_VAR=session"), "{env}");
+    assert!(
+        env.lines()
+            .any(|l| l == "ANTHROPIC_DEFAULT_OPUS_MODEL=opus-global"),
+        "{env}"
+    );
+
+    // A second session gets the global vars only.
+    tui.send_keys(ALT_N);
+    wizard_pick_dir(&mut tui, &harness.dirs[1]);
+    let env = recorded_env(&harness.dirs[1]);
+    assert!(env.lines().any(|l| l == "MY_VAR=global"), "{env}");
+
+    // Closing the first session drops its entry.
+    tui.send_keys(ALT_LEFT);
+    tui.send_keys(ALT_X);
+    tui.wait_for("Kill session", Region::Screen, WAIT);
+    tui.send_keys(b"y");
+    wait_session_env(&harness, &sid, false);
+
+    // A terminal tab: no session tab, and its shell has none of the vars.
+    tui.send_keys(ALT_C);
+    tui.wait_for("$ term@local", Region::TabBar, WAIT);
+    tui.wait_for("$", Region::Pane, WAIT);
+    tui.send_keys(ALT_COMMA);
+    tui.wait_for("a terminal tab runs no claude", Region::Screen, WAIT);
+    close_env_menu(&mut tui);
+    tui.send_keys(b"echo \"[${MY_VAR:-unset}]\"\r");
+    tui.wait_for("[unset]", Region::Pane, WAIT);
+    tui.quit(WAIT);
+}
+
+/// AC4: a clean exit of a session (Ctrl+D) drops its custom env.
+#[test]
+fn test_env_of_a_session_goes_with_its_clean_exit() {
+    let harness = env_harness();
+    let mut tui = harness.start_tui();
+    wizard_pick_dir(&mut tui, &harness.dirs[0]);
+    tui.send_keys(ALT_COMMA);
+    tui.wait_for("applies on next start or restart", Region::Screen, WAIT);
+    tui.send_keys(b"\t");
+    tui.wait_for("none yet", Region::Screen, WAIT);
+    add_var(&mut tui, "MY_VAR", "mine");
+    close_env_menu(&mut tui);
+    let sid = sid_in(&harness, &harness.dirs[0]);
+    wait_session_env(&harness, &sid, true);
+
+    tui.send_keys(b"\x04");
+    tui.wait_for("session ended", Region::Screen, WAIT);
+    wait_session_env(&harness, &sid, false);
+    tui.send_keys(ESC);
+    tui.wait_until(|s| !s.contains("New session", Region::Screen), WAIT);
+    tui.quit(WAIT);
+}
+
+/// AC5: a session's own var survives a daemon stop: a new TUI recovers the
+/// session (through the composer) with it.
+#[test]
+fn test_env_of_a_session_survives_a_daemon_stop_and_recovery() {
+    let harness = env_harness();
+    let mut tui = harness.start_tui();
+    wizard_pick_dir(&mut tui, &harness.dirs[0]);
+    tui.send_keys(ALT_COMMA);
+    tui.wait_for("applies on next start or restart", Region::Screen, WAIT);
+    tui.send_keys(b"\t");
+    tui.wait_for("none yet", Region::Screen, WAIT);
+    add_var(&mut tui, "MY_VAR", "kept");
+    close_env_menu(&mut tui);
+    tui.quit(WAIT);
+
+    harness.kill_daemon();
+    fs::remove_file(harness.dirs[0].join("claude.env")).unwrap();
+    let tui = harness.start_tui();
+    let env = recorded_env(&harness.dirs[0]);
+    assert!(env.lines().any(|l| l == "MY_VAR=kept"), "{env}");
+    let sid = sid_in(&harness, &harness.dirs[0]);
+    wait_session_env(&harness, &sid, true);
+    drop(tui);
+}
+
+/// AC4: a `--plain` session gets the global vars (it has no menu).
+#[test]
+fn test_plain_session_gets_the_global_env() {
+    let harness = env_harness();
+    let config = harness.config_home.join("claudio/config.toml");
+    let mut text = fs::read_to_string(&config).unwrap();
+    text.push_str("\n[env]\nMY_VAR = \"plain\"\n");
+    fs::write(&config, text).unwrap();
+    let tui = harness.start_plain_with(&[], |_| {});
+    let env = recorded_env(&harness.dirs[0]);
+    assert!(env.lines().any(|l| l == "MY_VAR=plain"), "{env}");
+    drop(tui);
+}
````
3. `crates/tui/src/env_view.rs` (new, whole file):
```rust
//! The custom env menu (Alt+,): the env vars every claude session gets
//! (Global, `config.toml [env]`) and the focused claude session's own
//! (`session-env.json`), applied at the session's next start or restart.
//! Values are shown and edited in the escaped form of
//! `claude::env::escape` (`\n`, `\t`, `\\`).
//!
//! Pure, like `app.rs`: every read and write is an [`Effect::Env`] the event
//! loop runs off the UI thread (`env_edge.rs`); what it found comes back
//! through [`App::on_env`], and the menu shows it. A session's entry is
//! forgotten when its tab goes and pruned when its host lists its sessions.
//! Drawn by `ui/env.rs`.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Effect};
use super::interaction::Modal;
use claudio_core::app::env::{Listed, Scope};
use claudio_core::claude::env::{
    catalogue, check_name, check_value, escape, shown, unescape, KnownVar, Value, MAX_VALUE,
};
use claudio_core::proto::{SessionId, SessionKind};

/// The longest text an input holds, in bytes: a value's escaped form.
const INPUT_MAX: usize = 2 * MAX_VALUE;
/// How far PageUp/PageDown move.
const PAGE: usize = 10;
/// The value prompt's reminder of the escapes.
pub const ESCAPES: &str = "\\n newline, \\t tab, \\\\ backslash";

/// A read or a write of the custom env, for the event loop (`env_edge`).
#[derive(Debug, Clone, PartialEq)]
pub enum EnvCall {
    /// Read both scopes for the menu opened under `gen` (the session scope
    /// of `session`, when it has one).
    Read { gen: u64, session: Option<Scope> },
    /// Set `name` to `value` (`None`: unset) in `scope`, then read again.
    Write {
        gen: u64,
        scope: Scope,
        name: String,
        value: Option<Value>,
        session: Option<Scope>,
    },
    /// Forget the entry of a session whose tab went.
    Forget { host: String, sid: SessionId },
    /// Forget `host`'s entries whose session is not in `live`.
    Prune { host: String, live: Vec<SessionId> },
}

/// What a read or a write found, for the menu opened under `gen`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvDone {
    pub gen: u64,
    pub global: Listed,
    pub session: Option<Listed>,
    /// Why a write failed (a key at most, never a value).
    pub failed: Option<String>,
}

/// The menu's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Global,
    Session,
}

/// One line of input. Its `Debug` hides the text (a value is a secret).
#[derive(Clone, PartialEq, Default)]
pub struct LineInput {
    pub text: String,
    /// A byte offset into `text`, on a char boundary.
    pub cursor: usize,
}

impl fmt::Debug for LineInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LineInput(<{} bytes>)", self.text.len())
    }
}

impl LineInput {
    fn with(text: String) -> LineInput {
        let cursor = text.len();
        LineInput { text, cursor }
    }

    /// Insert `text` at the cursor, up to [`INPUT_MAX`] bytes; control
    /// characters are left out.
    fn insert(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.text.len() + c.len_utf8() > INPUT_MAX {
                break;
            }
            self.text.insert(self.cursor, c);
            self.cursor += c.len_utf8();
        }
    }

    /// An editing key; `false` when it is not one.
    fn on_key(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => *self = LineInput::default(),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(&c.to_string())
            }
            KeyCode::Backspace => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.text.remove(self.cursor);
                }
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            KeyCode::Left => {
                if let Some(c) = self.text[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => return false,
        }
        true
    }
}

/// What the menu is doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The rows of the tab.
    List,
    /// Typing a new variable's name; `pick` is the highlighted catalogue
    /// match (`None`: the typed text).
    Name {
        input: LineInput,
        pick: Option<usize>,
    },
    /// Typing `name`'s value, escaped.
    Value { name: String, input: LineInput },
    /// Asking whether to delete `name`.
    Delete(String),
}

/// The menu's state.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvView {
    /// The generation it was opened under: results of another are dropped.
    pub gen: u64,
    pub tab: Tab,
    /// The focused claude session's scope and label, or why the session
    /// tab is off.
    pub session: Result<(Scope, String), &'static str>,
    /// The global rows, once read.
    pub global: Option<Vec<(String, Value)>>,
    /// The session's rows, once read.
    pub own: Option<Vec<(String, Value)>>,
    /// An index into the tab's rows.
    pub selected: usize,
    pub step: Step,
    /// The last refusal or failed write, until the next key (a key at most,
    /// never a value).
    pub error: Option<String>,
}

/// What a key asks of `App`.
enum Outcome {
    Stay,
    Close,
    Write {
        scope: Scope,
        name: String,
        value: Option<Value>,
    },
}

impl EnvView {
    fn new(gen: u64, session: Result<(Scope, String), &'static str>) -> EnvView {
        EnvView {
            gen,
            tab: Tab::Global,
            session,
            global: None,
            own: None,
            selected: 0,
            step: Step::List,
            error: None,
        }
    }

    /// The session tab's scope, when there is one.
    pub fn session_scope(&self) -> Option<Scope> {
        self.session.as_ref().ok().map(|(scope, _)| scope.clone())
    }

    /// The open tab's scope.
    fn scope(&self) -> Option<Scope> {
        match self.tab {
            Tab::Global => Some(Scope::Global),
            Tab::Session => self.session_scope(),
        }
    }

    /// The open tab's rows (none until read).
    pub fn rows(&self) -> &[(String, Value)] {
        let rows = match self.tab {
            Tab::Global => &self.global,
            Tab::Session => &self.own,
        };
        rows.as_deref().unwrap_or(&[])
    }

    /// The name of the selected row.
    pub fn selected_name(&self) -> Option<&str> {
        self.rows().get(self.selected).map(|(k, _)| k.as_str())
    }

    /// The catalogue entries whose name holds `typed` (any case), those
    /// starting with it first.
    pub fn matches(typed: &str) -> Vec<&'static KnownVar> {
        let typed = typed.trim().to_ascii_uppercase();
        let (mut first, mut then) = (Vec::new(), Vec::new());
        for var in catalogue() {
            let name = var.name.to_ascii_uppercase();
            if name.starts_with(&typed) {
                first.push(var);
            } else if name.contains(&typed) {
                then.push(var);
            }
        }
        first.extend(then);
        first
    }

    fn on_key(&mut self, key: &KeyEvent) -> Outcome {
        self.error = None;
        match std::mem::replace(&mut self.step, Step::List) {
            Step::List => self.list_key(key),
            Step::Name { input, pick } => self.name_key(key, input, pick),
            Step::Value { name, input } => self.value_key(key, name, input),
            Step::Delete(name) => self.delete_key(key, name),
        }
    }

    fn list_key(&mut self, key: &KeyEvent) -> Outcome {
        let last = self.rows().len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Tab | KeyCode::BackTab => match (&self.session, self.tab) {
                (Ok(_), Tab::Global) => self.tab = Tab::Session,
                (_, Tab::Session) => self.tab = Tab::Global,
                (Err(why), Tab::Global) => self.error = Some(format!("no session tab: {why}")),
            },
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            KeyCode::PageDown => self.selected = (self.selected + PAGE).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Char('a') => {
                self.step = Step::Name {
                    input: LineInput::default(),
                    pick: None,
                };
            }
            KeyCode::Enter => {
                if let Some((name, value)) = self.rows().get(self.selected).cloned() {
                    let input = LineInput::with(escape(value.as_str()));
                    self.step = Step::Value { name, input };
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(name) = self.selected_name().map(str::to_owned) {
                    self.step = Step::Delete(name);
                }
            }
            _ => {}
        }
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        Outcome::Stay
    }

    fn name_key(&mut self, key: &KeyEvent, mut input: LineInput, pick: Option<usize>) -> Outcome {
        let count = EnvView::matches(&input.text).len();
        let pick = match key.code {
            KeyCode::Esc => return Outcome::Stay,
            KeyCode::Down if count > 0 => Some(pick.map_or(0, |p| (p + 1).min(count - 1))),
            KeyCode::Up => pick.and_then(|p| p.checked_sub(1)),
            KeyCode::Enter => {
                let name = match pick.and_then(|p| EnvView::matches(&input.text).get(p).copied()) {
                    Some(var) => var.name.to_owned(),
                    None => input.text.trim().to_owned(),
                };
                if let Err(why) = check_name(&name) {
                    self.error = Some(shown(&name, &why));
                    self.step = Step::Name { input, pick };
                    return Outcome::Stay;
                }
                let old = self.rows().iter().find(|(k, _)| *k == name);
                let text = old.map(|(_, v)| escape(v.as_str())).unwrap_or_default();
                let input = LineInput::with(text);
                self.step = Step::Value { name, input };
                return Outcome::Stay;
            }
            _ if input.on_key(key) => None,
            _ => pick,
        };
        self.step = Step::Name { input, pick };
        Outcome::Stay
    }

    fn value_key(&mut self, key: &KeyEvent, name: String, mut input: LineInput) -> Outcome {
        match key.code {
            KeyCode::Esc => return Outcome::Stay,
            KeyCode::Enter => {
                let checked = unescape(&input.text)
                    .map_err(|why| format!("{name}: {why}"))
                    .and_then(|v| check_value(&v).map(|()| v).map_err(|e| shown(&name, &e)));
                match (checked, self.scope()) {
                    (Ok(value), Some(scope)) => {
                        return Outcome::Write {
                            scope,
                            name,
                            value: Some(Value::new(value)),
                        }
                    }
                    (Ok(_), None) => {}
                    (Err(why), _) => self.error = Some(why),
                }
            }
            _ => {
                input.on_key(key);
            }
        }
        self.step = Step::Value { name, input };
        Outcome::Stay
    }

    fn delete_key(&mut self, key: &KeyEvent, name: String) -> Outcome {
        match (key.code, self.scope()) {
            (KeyCode::Char('y') | KeyCode::Enter, Some(scope)) => Outcome::Write {
                scope,
                name,
                value: None,
            },
            (KeyCode::Char('n') | KeyCode::Esc, _) => Outcome::Stay,
            _ => {
                self.step = Step::Delete(name);
                Outcome::Stay
            }
        }
    }

    /// A paste: the name's first line, or the value's text escaped (`\r\n`
    /// as a newline).
    pub fn on_paste(&mut self, text: &str) {
        match &mut self.step {
            Step::Name { input, pick } => {
                input.insert(text.lines().next().unwrap_or("").trim());
                *pick = None;
            }
            Step::Value { input, .. } => input.insert(&escape(&text.replace("\r\n", "\n"))),
            Step::List | Step::Delete(_) => {}
        }
    }
}

impl App {
    /// Alt+,: the custom env menu, on the global tab; the session tab is
    /// the focused claude session's. Its rows come with [`App::on_env`].
    pub(super) fn open_environment(&mut self) {
        self.modal_gen += 1;
        let gen = self.modal_gen;
        let session = match self.active_view() {
            Some(v) if v.kind == SessionKind::Claude => {
                let scope = Scope::Session {
                    host: v.host.clone(),
                    sid: v.id,
                };
                Ok((scope, v.label()))
            }
            Some(_) => Err("a terminal tab runs no claude"),
            None => Err("no session is open"),
        };
        let view = EnvView::new(gen, session);
        let read = EnvCall::Read {
            gen,
            session: view.session_scope(),
        };
        self.modal = Some(Modal::Environment(Box::new(view)));
        self.emit(Effect::Env(read));
        self.redraw = true;
    }

    /// A key in the custom env menu.
    pub(super) fn env_key(&mut self, key: &KeyEvent) {
        let Some(Modal::Environment(view)) = &mut self.modal else {
            return;
        };
        match view.on_key(key) {
            Outcome::Stay => {}
            Outcome::Close => self.modal = None,
            Outcome::Write { scope, name, value } => {
                let (gen, session) = (view.gen, view.session_scope());
                let call = EnvCall::Write {
                    gen,
                    scope,
                    name,
                    value,
                    session,
                };
                self.emit(Effect::Env(call));
            }
        }
        self.redraw = true;
    }

    /// What a read or a write of the custom env found: the menu opened
    /// under `done.gen` shows it; a failed write and the reads' notices
    /// (key names only) become notices.
    pub fn on_env(&mut self, done: EnvDone) {
        if let Some(why) = &done.failed {
            self.notify(why.clone());
        }
        let Some(Modal::Environment(view)) = &mut self.modal else {
            return;
        };
        if view.gen != done.gen {
            return;
        }
        let mut notices = done.global.notices;
        view.global = Some(done.global.vars);
        if let Some(own) = done.session {
            notices.extend(own.notices);
            view.own = Some(own.vars);
        }
        view.error = done.failed.or(view.error.take());
        view.selected = view.selected.min(view.rows().len().saturating_sub(1));
        for notice in notices {
            self.notify(notice);
        }
        self.redraw = true;
    }

    /// The tab of session `sid` on `host` went: its custom env goes too.
    pub(super) fn forget_env(&mut self, host: &str, sid: SessionId) {
        let host = host.to_owned();
        self.emit(Effect::Env(EnvCall::Forget { host, sid }));
    }

    /// `host` listed its sessions (`live`): the custom env of its others
    /// goes.
    pub(super) fn prune_env(&mut self, host: &str, live: Vec<SessionId>) {
        let host = host.to_owned();
        self.emit(Effect::Env(EnvCall::Prune { host, live }));
    }

    /// The menu's footer: when its changes apply.
    pub fn env_footer(&self) -> String {
        let restart = self.keymap.key_for(super::keymap::Action::Reset);
        let restart = restart.unwrap_or_else(|| "Alt+e".to_owned());
        format!("applies on next start or restart ({restart}); a running claude keeps its env")
    }
}

#[cfg(test)]
mod tests;
```
4. `crates/tui/src/env_view/tests.rs` (new, whole file):
```rust
//! The custom env menu: keys, effects and drawing, without I/O.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};

use super::*;
use crate::app::Effect;
use crate::interaction::Modal;
use crate::test_support::{alt, app_with, info, key, press, shell_info};
use claudio_core::app::env::Listed;
use claudio_core::proto::{SessionEvent, SessionInfo};

fn screen(app: &App) -> String {
    let mut term = Terminal::new(TestBackend::new(app.width, app.height)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn view(app: &App) -> &EnvView {
    match &app.modal {
        Some(Modal::Environment(view)) => view,
        _ => panic!("the env menu is not open"),
    }
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_terminal(press(KeyCode::Char(c)));
    }
}

fn env_calls(effects: Vec<Effect>) -> Vec<EnvCall> {
    effects
        .into_iter()
        .filter_map(|e| match e {
            Effect::Env(call) => Some(call),
            _ => None,
        })
        .collect()
}

fn listed(vars: &[(&str, &str)]) -> Listed {
    Listed {
        vars: vars
            .iter()
            .map(|(k, v)| (k.to_string(), Value::new(*v)))
            .collect(),
        ..Listed::default()
    }
}

/// The menu open on `live[0]`, its first read answered with `global` and
/// `own`.
fn opened(live: &[SessionInfo], global: &[(&str, &str)], own: &[(&str, &str)]) -> App {
    let mut app = app_with(live);
    app.on_terminal(alt(','));
    let gen = view(&app).gen;
    app.take_effects();
    let session = view(&app).session.is_ok().then(|| listed(own));
    app.on_env(EnvDone {
        gen,
        global: listed(global),
        session,
        failed: None,
    });
    app
}

#[test]
fn alt_comma_opens_the_menu_on_the_global_tab_and_reads_both_scopes() {
    let live = [info(Some(1), None)];
    let mut app = app_with(&live);
    app.on_terminal(alt(','));
    let calls = env_calls(app.take_effects());
    let want = Scope::Session {
        host: "local".into(),
        sid: live[0].id,
    };
    assert_eq!(
        calls,
        [EnvCall::Read {
            gen: view(&app).gen,
            session: Some(want)
        }]
    );
    assert_eq!(view(&app).tab, Tab::Global);
    assert!(screen(&app).contains("reading…"));
}

#[test]
fn rows_show_escaped_with_the_description_and_the_footer() {
    let live = [info(Some(1), None)];
    let global = [("ANTHROPIC_DEFAULT_OPUS_MODEL", "opus-x"), ("H", "a\nb\\c")];
    let app = opened(&live, &global, &[]);
    let text = screen(&app);
    assert!(
        text.contains("ANTHROPIC_DEFAULT_OPUS_MODEL = opus-x"),
        "{text}"
    );
    assert!(text.contains("H = a\\nb\\\\c"), "{text}");
    assert!(
        text.contains("Model ID that the opus alias resolves to"),
        "{text}"
    );
    assert!(
        text.contains("applies on next start or restart (Alt+e)"),
        "{text}"
    );
}

#[test]
fn adding_a_var_decodes_the_escapes_and_writes_the_open_tab() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[], &[]);
    app.on_terminal(press(KeyCode::Tab));
    assert_eq!(view(&app).tab, Tab::Session);
    app.on_terminal(press(KeyCode::Char('a')));
    typed(&mut app, "ANTHROPIC_DEF");
    assert!(screen(&app).contains("ANTHROPIC_DEFAULT_OPUS_MODEL"));
    typed(&mut app, "AULT_OPUS_MODEL");
    app.on_terminal(press(KeyCode::Enter));
    assert!(screen(&app).contains("\\n newline, \\t tab, \\\\ backslash"));
    typed(&mut app, "C:\\\\new\\tx");
    app.on_terminal(press(KeyCode::Enter));
    let calls = env_calls(app.take_effects());
    let session = Scope::Session {
        host: "local".into(),
        sid: live[0].id,
    };
    assert_eq!(
        calls,
        [EnvCall::Write {
            gen: view(&app).gen,
            scope: session.clone(),
            name: "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
            value: Some(Value::new("C:\\new\tx")),
            session: Some(session),
        }]
    );
    assert_eq!(view(&app).step, Step::List);
}

#[test]
fn a_bad_escape_or_a_refused_name_writes_nothing_and_says_why() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[], &[]);
    app.on_terminal(press(KeyCode::Char('a')));
    typed(&mut app, "PATH");
    app.on_terminal(press(KeyCode::Enter));
    assert!(matches!(view(&app).step, Step::Name { .. }));
    assert!(screen(&app).contains("PATH: refused: "));
    app.on_terminal(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
    typed(&mut app, "WIN_DIR");
    app.on_terminal(press(KeyCode::Enter));
    typed(&mut app, "C:\\Users");
    app.on_terminal(press(KeyCode::Enter));
    assert!(screen(&app).contains("WIN_DIR: unknown escape \\U: write \\\\ for a backslash"));
    assert!(env_calls(app.take_effects()).is_empty());
    app.on_terminal(press(KeyCode::Esc));
    assert_eq!(view(&app).step, Step::List);
}

#[test]
fn enter_edits_and_d_deletes_after_a_confirm() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[("A", "1"), ("B", "x\ny")], &[]);
    app.on_terminal(press(KeyCode::Down));
    app.on_terminal(press(KeyCode::Enter));
    match &view(&app).step {
        Step::Value { name, input } => {
            assert_eq!((name.as_str(), input.text.as_str()), ("B", "x\\ny"))
        }
        other => panic!("{other:?}"),
    }
    app.on_terminal(press(KeyCode::Esc));
    app.on_terminal(press(KeyCode::Char('d')));
    assert!(screen(&app).contains("delete B? (y/n)"));
    app.on_terminal(press(KeyCode::Char('y')));
    let calls = env_calls(app.take_effects());
    assert!(matches!(
        calls.as_slice(),
        [EnvCall::Write { scope: Scope::Global, name, value: None, .. }] if name == "B"
    ));
}

#[test]
fn a_terminal_tab_has_no_session_tab() {
    let live = [shell_info(Some(1))];
    let mut app = opened(&live, &[], &[]);
    assert!(screen(&app).contains("Session: — (a terminal tab runs no claude)"));
    app.on_terminal(press(KeyCode::Tab));
    assert_eq!(view(&app).tab, Tab::Global);
    assert!(screen(&app).contains("no session tab: a terminal tab runs no claude"));
}

#[test]
fn a_stale_read_is_dropped_and_a_failed_write_is_a_notice() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[("A", "1")], &[]);
    let gen = view(&app).gen;
    app.on_env(EnvDone {
        gen: gen + 1,
        global: listed(&[("STALE", "x")]),
        session: None,
        failed: None,
    });
    assert_eq!(view(&app).rows().len(), 1);
    app.on_env(EnvDone {
        gen,
        global: listed(&[("A", "1")]),
        session: Some(listed(&[])),
        failed: Some("A: too many variables (at most 128)".into()),
    });
    assert_eq!(
        app.notice.as_ref().map(|n| n.text.as_str()),
        Some("A: too many variables (at most 128)")
    );
}

#[test]
fn a_closed_tab_and_a_removed_session_forget_their_env_and_a_recovery_prunes() {
    let live = [info(Some(1), None), shell_info(Some(2))];
    let mut app = app_with(&live);
    app.kill_session(live[0].id);
    let calls = env_calls(app.take_effects());
    assert_eq!(
        calls,
        [EnvCall::Forget {
            host: "local".into(),
            sid: live[0].id
        }]
    );
    // A terminal has no custom env.
    app.on_incoming_from(
        "local",
        claudio_core::client::Incoming::Event {
            id: live[1].id,
            event: SessionEvent::Removed,
        },
    );
    assert!(env_calls(app.take_effects()).is_empty());
    let other = info(Some(3), None);
    app.recover_host("local", std::slice::from_ref(&other));
    let calls = env_calls(app.take_effects());
    assert!(calls.contains(&EnvCall::Prune {
        host: "local".into(),
        live: vec![other.id]
    }));
}

#[test]
fn no_value_reaches_debug() {
    let live = [info(Some(1), None)];
    let mut app = opened(&live, &[("TOKEN", "sk-SECRET-1")], &[]);
    app.on_terminal(press(KeyCode::Enter));
    typed(&mut app, "-MORE-SECRET");
    let shown = format!("{:?}", view(&app));
    assert!(!shown.contains("SECRET"), "{shown}");
    app.on_terminal(press(KeyCode::Enter));
    let shown = format!("{:?}", app.take_effects());
    assert!(!shown.contains("SECRET"), "{shown}");
}
```
5. `crates/tui/src/env_edge.rs` (new, whole file):
```rust
//! The TUI's edge of the custom env (`app::env`): it runs `App`'s
//! [`EnvCall`]s off the UI thread, on the files beside the addon runtime's
//! config.toml, and sends what a read or a write found back as
//! [`HostEvent::Env`]. A forget's or a prune's failure is not shown.

use std::path::Path;

use tokio::sync::mpsc;

use super::env_view::{EnvCall, EnvDone};
use super::HostEvent;
use claudio_core::app::env::{EnvError, Scope, Store};
use claudio_core::claude::env::shown;

/// Run `call` on the store beside `config_path` (each call one blocking
/// task, bounded by the files' lock wait).
pub(super) fn run(call: EnvCall, config_path: &Path, tx: &mpsc::Sender<HostEvent>) {
    let store = Store::beside(config_path);
    let tx = tx.clone();
    tokio::spawn(async move {
        let done = tokio::task::spawn_blocking(move || apply(&store, call)).await;
        if let Ok(Some(done)) = done {
            let _ = tx.send(HostEvent::Env(done)).await;
        }
    });
}

/// `call` on `store`: what a read or a write found (a forget or a prune
/// reports nothing).
fn apply(store: &Store, call: EnvCall) -> Option<EnvDone> {
    match call {
        EnvCall::Read { gen, session } => Some(read(store, gen, session.as_ref(), None)),
        EnvCall::Write {
            gen,
            scope,
            name,
            value,
            session,
        } => {
            let written = match &value {
                Some(value) => store.set(&scope, &name, value),
                None => store.unset(&scope, &name),
            };
            let failed = written.err().map(|e| match e {
                EnvError::Refused(why) => why,
                e => shown(&name, &e),
            });
            Some(read(store, gen, session.as_ref(), failed))
        }
        EnvCall::Forget { host, sid } => {
            let _ = store.forget_session(&host, sid);
            None
        }
        EnvCall::Prune { host, live } => {
            let _ = store.prune(&host, &live);
            None
        }
    }
}

/// Both scopes as they are now.
fn read(store: &Store, gen: u64, session: Option<&Scope>, failed: Option<String>) -> EnvDone {
    EnvDone {
        gen,
        global: store.list(&Scope::Global),
        session: session.map(|scope| store.list(scope)),
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claudio_core::claude::env::Value;
    use claudio_core::proto::SessionId;

    fn scratch() -> (std::path::PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!("claudio-env-edge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::beside(&dir.join("config.toml"));
        (dir, store)
    }

    #[test]
    fn a_write_reads_both_scopes_again_and_a_refusal_names_the_key_only() {
        let (dir, store) = scratch();
        let session = Scope::Session {
            host: "local".into(),
            sid: SessionId::from_u128(1),
        };
        let set = |scope: Scope, name: &str, value: &str| EnvCall::Write {
            gen: 3,
            scope,
            name: name.into(),
            value: Some(Value::new(value)),
            session: Some(session.clone()),
        };
        let done = apply(&store, set(Scope::Global, "G", "1")).unwrap();
        assert_eq!(done.gen, 3);
        assert_eq!(done.global.vars, [("G".to_owned(), Value::new("1"))]);
        assert_eq!(done.session.map(|l| l.vars.len()), Some(0));
        let done = apply(&store, set(session.clone(), "S", "2")).unwrap();
        assert_eq!(done.session.unwrap().vars.len(), 1);
        let done = apply(&store, set(Scope::Global, "PATH", "sk-SECRET")).unwrap();
        let failed = done.failed.unwrap();
        assert!(failed.starts_with("PATH: refused: "), "{failed}");
        assert!(!failed.contains("SECRET"));
        let forget = EnvCall::Forget {
            host: "local".into(),
            sid: SessionId::from_u128(1),
        };
        assert!(apply(&store, forget).is_none());
        assert!(store.list(&session).vars.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```
6. `crates/tui/src/ui/env.rs` (new, whole file):
```rust
//! Drawing the custom env menu (`tui/env_view.rs` has its state and keys):
//! the tabs, the rows (`NAME = value`, escaped) or the step's input with
//! the catalogue's matches, the selected name's description, the last
//! refusal and the footer. Every line is made safe and cut to the width.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{centered, draw_input, popup};
use crate::app::App;
use crate::env_view::{EnvView, Step, Tab, ESCAPES};
use crate::fmt::truncate;
use crate::reader::layout::safe;
use claudio_core::claude::env::{escape, known};

/// The menu's title.
pub const TITLE: &str = "Environment (Tab scope · a add · Enter edit · d delete · Esc close)";

/// The menu over the whole terminal.
pub fn draw_env(frame: &mut Frame, app: &App, view: &EnvView) {
    let area = frame.area();
    let rect = centered(
        area,
        100.min(area.width.saturating_sub(2)),
        area.height.saturating_sub(4).max(3),
    );
    let inner = popup(frame, rect, TITLE);
    if inner.height < 3 || inner.width == 0 {
        return;
    }
    let width = inner.width as usize;
    let fit = |text: &str| truncate(&safe(text), width);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let at = |y: usize, h: usize| Rect {
        y: inner.y + y as u16,
        height: h as u16,
        ..inner
    };
    let height = inner.height as usize;
    frame.render_widget(Paragraph::new(tabs(view)), at(0, 1));
    // The footer and the error take the last lines, the description the one
    // above them; the step's body what is left.
    let footer = app.env_footer();
    frame.render_widget(
        Paragraph::new(Line::styled(fit(&footer), dim)),
        at(height - 1, 1),
    );
    let mut bottom = height - 1;
    if let Some(error) = &view.error {
        bottom -= 1;
        let style = Style::default().add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::styled(fit(error), style)),
            at(bottom, 1),
        );
    }
    let described = match &view.step {
        Step::List => view.selected_name().map(str::to_owned),
        Step::Name { input, pick } => EnvView::matches(&input.text)
            .get(pick.unwrap_or(0))
            .map(|v| v.name.to_owned()),
        Step::Value { name, .. } | Step::Delete(name) => Some(name.clone()),
    };
    if let Some(var) = described.as_deref().and_then(known) {
        bottom = bottom.saturating_sub(1);
        let text = format!("{} · {}: {}", var.name, var.category, var.description);
        frame.render_widget(Paragraph::new(Line::styled(fit(&text), dim)), at(bottom, 1));
    }
    let body = at(2, bottom.saturating_sub(3));
    match &view.step {
        Step::List => draw_rows(frame, view, body),
        Step::Name { input, pick } => {
            draw_input(frame, at(2, 1), "name: ", &input.text);
            draw_matches(frame, &input.text, *pick, at(3, bottom.saturating_sub(4)));
        }
        Step::Value { name, input } => {
            draw_input(frame, at(2, 1), &format!("{name} = "), &input.text);
            let hint = Line::styled(fit(&format!("  {ESCAPES}")), dim);
            frame.render_widget(Paragraph::new(hint), at(3, 1));
        }
        Step::Delete(name) => {
            let ask = Line::raw(fit(&format!("delete {name}? (y/n)")));
            frame.render_widget(Paragraph::new(ask), at(2, 1));
        }
    }
}

/// `Global` and `Session: <label>` (or why there is none), the open one
/// reversed.
fn tabs(view: &EnvView) -> Line<'static> {
    let on = Style::default().add_modifier(Modifier::REVERSED);
    let off = Style::default();
    let dim = Style::default().add_modifier(Modifier::DIM);
    let (global, session) = match view.tab {
        Tab::Global => (on, off),
        Tab::Session => (off, on),
    };
    let label = match &view.session {
        Ok((_, label)) => Span::styled(format!(" Session: {} ", safe(label)), session),
        Err(why) => Span::styled(format!(" Session: — ({why}) "), dim),
    };
    Line::from(vec![
        Span::styled(" Global ", global),
        Span::raw("  "),
        label,
    ])
}

/// The tab's rows, `NAME = value` (escaped), the selected one reversed.
fn draw_rows(frame: &mut Frame, view: &EnvView, area: Rect) {
    let width = area.width as usize;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let loaded = match view.tab {
        Tab::Global => view.global.is_some(),
        Tab::Session => view.own.is_some(),
    };
    let rows = view.rows();
    let lines: Vec<Line> = if !loaded {
        vec![Line::styled("reading…", dim)]
    } else if rows.is_empty() {
        vec![Line::styled("none yet: a adds one", dim)]
    } else {
        let visible = (area.height as usize).max(1);
        let offset = view.selected.saturating_sub(visible - 1);
        rows.iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .map(|(i, (name, value))| {
                let text = truncate(
                    &safe(&format!("{name} = {}", escape(value.as_str()))),
                    width,
                );
                let style = if i == view.selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Line::styled(text, style)
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), area);
}

/// The catalogue's matches of `typed`, `pick` highlighted and scrolled into
/// view, each with its category.
fn draw_matches(frame: &mut Frame, typed: &str, pick: Option<usize>, area: Rect) {
    let visible = area.height as usize;
    if visible == 0 {
        return;
    }
    let width = area.width as usize;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let matches = EnvView::matches(typed);
    let offset = pick.map_or(0, |p| p.saturating_sub(visible - 1));
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, var)| {
            let style = if Some(i) == pick {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let name = truncate(&format!("  {}", var.name), width);
            Line::from(vec![
                Span::styled(name, style),
                Span::styled(format!("  {}", var.category), dim),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
```
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test -p claudio-tui --lib`
Expected: `test result: ok. 404 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test manager env -- --test-threads=1`
Expected: `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 43 filtered out`
(`test_env_menu_sets_global_and_session_vars_for_the_next_start`,
`test_env_of_a_session_goes_with_its_clean_exit`,
`test_env_of_a_session_survives_a_daemon_stop_and_recovery`,
`test_plain_session_gets_the_global_env`, and the existing
`test_env_recording_fakes_keep_inherited_secrets_out_of_fixtures`).
Run: `TMPDIR=$HOME/.cache/cl-tmp make test-manager` and
`TMPDIR=$HOME/.cache/cl-tmp cargo test --locked --no-default-features --test manager -- --test-threads=1`
Expected: each `test result: ok. 48 passed; 0 failed` (about 2.5 min each).
Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && python3 scripts/check_rules.py`
Expected: clean (`interaction.rs` 968, `modal_key` 110, `lib.rs` 811, `run_effect` 123).

Step 3: commit
`git commit -m "refactor(tui): the reader view's glue moves to reader/" -- crates/tui/src/interaction.rs crates/tui/src/reader/mod.rs`
`git add crates/tui/src/env_view.rs crates/tui/src/env_view/tests.rs crates/tui/src/env_edge.rs crates/tui/src/ui/env.rs`
`git commit -m "feat(tui): Alt+, opens the custom env menu" -- crates/tui/src/keymap.rs crates/tui/src/app.rs crates/tui/src/interaction.rs crates/tui/src/lib.rs crates/tui/src/addon_runner.rs crates/tui/src/sessions.rs crates/tui/src/env_view.rs crates/tui/src/env_view/tests.rs crates/tui/src/env_edge.rs crates/tui/src/ui/env.rs crates/tui/src/ui/mod.rs crates/tui/tests/common/mod.rs crates/tui/tests/manager.rs`

Interfaces this task provides to later tasks: `Action::Environment` (`[keys] environment`,
default `alt+,`, label "custom env vars of claude sessions" in the help popup); the TUI footer
text "applies on next start or restart (Alt+e); a running claude keeps its env" (docs quote it).

### Task 6: web server: the env.* ops
Model: sonnet   Repo: /home/p4u/claudio   Files: addons/web/src/env.rs (new),
addons/web/src/lib.rs, addons/web/src/ws.rs, addons/web/src/hub.rs, addons/web/tests/web.rs,
addons/vocdoni/tests/web.rs
Goal: AC6: `env.catalogue`, `env.list`, `env.set`, `env.unset` as docs/web-protocol.md §5
"Custom env" says (Task 1); `session.kill` and a `Removed` event forget the session's entry;
`connected` prunes the host's.
Rules: A3 (the web uses `app::env`, `claude::env` and `AddonRuntime::config_path`, public
services), B2 (the doc is in), C1 with the one exception (`env.list`), C2 (`deny_unknown_fields`,
the store's caps), D3 (`spawn_blocking`), A8 (`hub.rs` and `connected` may not grow: the diff
shrinks `connected`'s `forget_unlisted` block to make room).

Step 1: write the failing test
Add `web_env_ops_set_list_unset_and_reach_the_launch` and `write_recording_fake_claude` to
`addons/web/tests/web.rs`, and `web_proxy_session_overrides_the_custom_env` (with
`ANTHROPIC_BASE_URL` added to the recorded keys) to `addons/vocdoni/tests/web.rs` (the test
hunks of the diff below).
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test web web_env_ops -- --test-threads=1`
Expected: `panicked at addons/web/tests/web.rs:…: catalogue: "unknown op"`; `1 failed`.

Step 2: implement
1. Modified files:
````diff
diff --git a/addons/vocdoni/tests/web.rs b/addons/vocdoni/tests/web.rs
index 38dcd54..3233dfb 100644
--- a/addons/vocdoni/tests/web.rs
+++ b/addons/vocdoni/tests/web.rs
@@ -108,6 +108,7 @@ fn write_env_fake_claude(h: &ManagerHarness, env_file: &std::path::Path) {
         &[
             "ANTHROPIC_AUTH_TOKEN",
             "ANTHROPIC_API_KEY",
+            "ANTHROPIC_BASE_URL",
             "VOCDONI_TEST_TOKEN",
         ],
         &["CLAUDECODE"],
@@ -311,6 +312,68 @@ async fn web_spawn_uses_the_composer_env_remove() {
     assert_eq!(err, "bad request: launch: flags need an addon's option");
 }
 
+/// AC6: the custom env gives way to a proxy session's: a global
+/// `ANTHROPIC_BASE_URL` stays the proxy's, a global `ANTHROPIC_API_KEY` is
+/// removed (the addon's `env_remove`), and one notice names both keys,
+/// never a value; a direct session gets the user's values.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_proxy_session_overrides_the_custom_env() {
+    let h = ManagerHarness::new();
+    let env_file = h.root.join("claude-env.txt");
+    write_env_fake_claude(&h, &env_file);
+    let dirs = h.dirs.clone();
+    let proxy = fake_proxy();
+    let web = web_with_proxy(h, &proxy);
+    let cookie = web.login();
+    let mut tab = Tab::open(&web, &cookie).await;
+    tab.connected("local").await;
+    for (name, value) in [
+        ("ANTHROPIC_BASE_URL", "https://user-VALUE.example"),
+        ("ANTHROPIC_API_KEY", "sk-user-VALUE"),
+    ] {
+        let set = json!({"scope": "global", "name": name, "value": value});
+        tab.req("env.set", set).await.expect("set");
+    }
+    let spawn = |dir: &std::path::PathBuf, pick: &str| {
+        json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80,
+        "launch": {"pick": pick}, "defer": [{"addon": "vocdoni", "id": PLUGIN}]})
+    };
+    tab.req("session.spawn", spawn(&dirs[0], "default"))
+        .await
+        .expect("spawn through the proxy");
+    let env = launched_env(&env_file).await;
+    assert!(env.contains(&format!("ANTHROPIC_AUTH_TOKEN={PROXY_TOKEN}")));
+    assert!(
+        env.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:"),
+        "{env}"
+    );
+    assert!(!env.contains("ANTHROPIC_API_KEY="), "{env}");
+    assert!(!env.contains("VALUE"), "{env}");
+    let notice = tab
+        .event("notice", |v| {
+            v["text"]
+                .as_str()
+                .is_some_and(|t| t.starts_with("custom env overridden by"))
+        })
+        .await;
+    assert_eq!(
+        notice["text"],
+        "custom env overridden by vocdoni: ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY"
+    );
+    fs::remove_file(&env_file).unwrap();
+    tab.req("session.spawn", spawn(&dirs[1], "direct"))
+        .await
+        .expect("spawn direct");
+    let env = launched_env(&env_file).await;
+    assert!(
+        env.contains("ANTHROPIC_API_KEY=sk-user-VALUE"),
+        "{}",
+        env.len()
+    );
+    assert!(env.contains("ANTHROPIC_BASE_URL=https://user-VALUE.example"));
+    assert!(!env.contains("ANTHROPIC_AUTH_TOKEN="));
+}
+
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn web_plugin_without_choices_is_left_out_until_saved() {
     let h = ManagerHarness::new();
diff --git a/addons/web/src/hub.rs b/addons/web/src/hub.rs
index a2e4cbd..7cc29b6 100644
--- a/addons/web/src/hub.rs
+++ b/addons/web/src/hub.rs
@@ -1456,6 +1456,7 @@ impl Hub {
             }
         }
         if applied.removed {
+            super::env::forget(&self.runtime, host, sid);
             self.remove_view(sid);
             return;
         }
@@ -1609,13 +1610,12 @@ impl Hub {
             .filter(|s| !s.ephemeral)
             .map(|s| (s.id, s))
             .collect();
-        // Sessions that ended while no web was running had no view to remove.
-        if self
+        // Sessions that ended while no web was running had no view to
+        // remove; their record and custom env go.
+        self.dirty |= self
             .web
-            .forget_unlisted(host, |sid| listed.contains_key(sid))
-        {
-            self.dirty = true;
-        }
+            .forget_unlisted(host, |sid| listed.contains_key(sid));
+        super::env::prune(&self.runtime, host, listed.keys().copied().collect());
         let gone: Vec<SessionId> = self
             .sessions
             .values()
diff --git a/addons/web/src/lib.rs b/addons/web/src/lib.rs
index e0a223b..69c3f4f 100644
--- a/addons/web/src/lib.rs
+++ b/addons/web/src/lib.rs
@@ -11,6 +11,7 @@ mod atomic;
 pub(crate) mod auth;
 mod cli;
 mod dto;
+mod env;
 mod events;
 mod ext;
 mod git_ops;
diff --git a/addons/web/src/ws.rs b/addons/web/src/ws.rs
index b263664..bed5b96 100644
--- a/addons/web/src/ws.rs
+++ b/addons/web/src/ws.rs
@@ -475,6 +475,7 @@ pub(crate) async fn dispatch(
                 .await
                 .map_err(daemon_error)?;
             tracing::info!(op = "session.kill", host = %s.host, sid = %a.sid, "web op");
+            super::env::forget(&state.addons, &s.host, a.sid);
             Ok(json!({}))
         }
         "session.rename" => {
@@ -636,6 +637,7 @@ pub(crate) async fn dispatch(
             state.ctx.ext.render(&a.lens, &a.kind, &a.source).await
         }
         op if op.starts_with("addons.") => addons::dispatch(state, op, args).await,
+        op if op.starts_with("env.") => super::env::dispatch(state, op, args).await,
         _ => Err("unknown op".to_owned()),
     }
 }
diff --git a/addons/web/tests/web.rs b/addons/web/tests/web.rs
index ec2dc9d..49654a8 100644
--- a/addons/web/tests/web.rs
+++ b/addons/web/tests/web.rs
@@ -1924,11 +1924,13 @@ async fn web_order_and_layout_persist_in_web_state() {
 /// [`common::record_env_sh`]), lists `--agents`/`--mcp-config` in `--help`,
 /// then prints the banner and echoes lines.
 fn write_env_fake_claude(h: &ManagerHarness, env_file: &std::path::Path) {
-    let record = common::record_env_sh(
-        &env_file.display().to_string(),
-        &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
-        &["CLAUDECODE"],
-    );
+    write_recording_fake_claude(h, env_file, &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]);
+}
+
+/// [`write_env_fake_claude`] recording `values` (and whether `CLAUDECODE`
+/// is set).
+fn write_recording_fake_claude(h: &ManagerHarness, env_file: &std::path::Path, values: &[&str]) {
+    let record = common::record_env_sh(&env_file.display().to_string(), values, &["CLAUDECODE"]);
     let script = format!(
         r#"#!/bin/sh
 [ "$1" = "--version" ] && echo 'claude 0.0.0-fake' && exit 0
@@ -2035,6 +2037,151 @@ async fn web_direct_launches_keep_the_inherited_api_key() {
     assert!(!env.contains("CLAUDECODE="), "a parent claude's marker");
 }
 
+/// AC6: `env.*` sets, lists and unsets both scopes; refuses a refused
+/// name (never echoing a value), extra args, a sid with the global scope or
+/// none with the session one, an unknown sid and a terminal's; a launch gets
+/// the vars, the session's own winning; a kill drops the session's entry;
+/// the log names keys, never values.
+#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
+async fn web_env_ops_set_list_unset_and_reach_the_launch() {
+    let h = ManagerHarness::new();
+    let env_file = h.root.join("claude-env.txt");
+    write_recording_fake_claude(&h, &env_file, &["MY_VAR", "MY_GLOBAL"]);
+    let dir = h.dirs[0].clone();
+    let session_env = h.config_home.join("claudio/session-env.json");
+    let web = Web::start_with(h, &[], |_| {});
+    let cookie = web.login();
+    let mut tab = Tab::open(&web, &cookie).await;
+    tab.connected("local").await;
+
+    let cat = tab
+        .req("env.catalogue", json!({}))
+        .await
+        .expect("catalogue");
+    let vars = cat["vars"].as_array().unwrap();
+    let opus = vars
+        .iter()
+        .find(|v| v["name"] == "ANTHROPIC_DEFAULT_OPUS_MODEL");
+    assert_eq!(opus.expect("documented")["category"], "model");
+    let refused = cat["refused"].as_array().unwrap();
+    assert!(refused
+        .iter()
+        .any(|r| r["name"] == "PATH" && r["reason"].is_string()));
+
+    let set = |scope: Value, name: &str, value: &str| {
+        let mut args = scope;
+        args["name"] = name.into();
+        args["value"] = value.into();
+        args
+    };
+    let global = json!({"scope": "global"});
+    for (name, value) in [
+        ("MY_GLOBAL", "g-VALUE-1"),
+        ("MY_VAR", "global-VALUE-2\nline"),
+    ] {
+        tab.req("env.set", set(global.clone(), name, value))
+            .await
+            .expect("set");
+    }
+    assert_eq!(
+        tab.req("env.list", global.clone()).await.unwrap(),
+        json!({"vars": [{"name": "MY_GLOBAL", "value": "g-VALUE-1"}, {"name": "MY_VAR", "value": "global-VALUE-2\nline"}], "skipped": 0})
+    );
+    let err = tab
+        .req("env.set", set(global.clone(), "PATH", "/x"))
+        .await
+        .unwrap_err();
+    assert!(err.starts_with("PATH: refused: "), "{err}");
+    let err = tab
+        .req("env.set", set(global.clone(), "TOKEN", "sk-SECRET\r"))
+        .await
+        .unwrap_err();
+    assert!(err.starts_with("TOKEN: value holds a control"), "{err}");
+    let mut extra = set(global.clone(), "A", "1");
+    extra["more"] = 1.into();
+    let bad = |e: Result<Value, String>| e.unwrap_err().starts_with("bad request: ");
+    assert!(bad(tab.req("env.set", extra).await));
+    let stray = uuid::Uuid::new_v4().to_string();
+    assert!(bad(tab
+        .req("env.list", json!({"scope": "global", "sid": stray}))
+        .await));
+    assert!(bad(tab.req("env.list", json!({"scope": "session"})).await));
+    assert_eq!(
+        tab.req("env.list", json!({"scope": "session", "sid": stray}))
+            .await,
+        Err("no such session".to_owned())
+    );
+    let shell = json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80});
+    let term = tab.req("session.shell", shell).await.expect("shell")["sid"].clone();
+    assert_eq!(
+        tab.req(
+            "env.set",
+            set(json!({"scope": "session", "sid": term}), "A", "1")
+        )
+        .await,
+        Err("not a claude session".to_owned())
+    );
+
+    // A launch gets the global vars; a respawn its own value too.
+    let spawn =
+        json!({"host": "local", "cwd": dir, "rows": 24, "cols": 80, "launch": {"pick": "direct"}});
+    let sid = tab.req("session.spawn", spawn).await.expect("spawn")["sid"].clone();
+    let env = read_env_file(&env_file).await;
+    assert!(
+        env.contains("MY_GLOBAL=g-VALUE-1\n") && env.contains("MY_VAR=global-VALUE-2\nline\n"),
+        "{env}"
+    );
+    let own = json!({"scope": "session", "sid": sid});
+    tab.req("env.set", set(own.clone(), "MY_VAR", "own-VALUE-3"))
+        .await
+        .expect("set own");
+    assert_eq!(
+        tab.req("env.list", own.clone()).await.unwrap(),
+        json!({"vars": [{"name": "MY_VAR", "value": "own-VALUE-3"}], "skipped": 0})
+    );
+    fs::remove_file(&env_file).unwrap();
+    let again =
+        json!({"sid": sid, "fresh": true, "rows": 24, "cols": 80, "launch": {"pick": "direct"}});
+    tab.req("session.respawn", again).await.expect("respawn");
+    let env = read_env_file(&env_file).await;
+    assert!(
+        env.contains("MY_VAR=own-VALUE-3\n") && env.contains("MY_GLOBAL=g-VALUE-1\n"),
+        "{env}"
+    );
+    tab.req("env.unset", json!({"scope": "global", "name": "MY_GLOBAL"}))
+        .await
+        .expect("unset");
+    let list = tab.req("env.list", global).await.unwrap();
+    assert_eq!(list["vars"].as_array().unwrap().len(), 1);
+
+    // A kill drops the session's entry.
+    let key = sid.as_str().unwrap().to_owned();
+    let has = || -> bool {
+        fs::read_to_string(&session_env)
+            .ok()
+            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
+            .is_some_and(|v| v["sessions"].get(&key).is_some())
+    };
+    assert!(has());
+    tab.req("session.kill", json!({"sid": sid}))
+        .await
+        .expect("kill");
+    let deadline = std::time::Instant::now() + WAIT;
+    while has() {
+        assert!(std::time::Instant::now() < deadline, "the entry stayed");
+        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
+    }
+    let log = fs::read_to_string(web.h().root.join("web.log")).unwrap();
+    assert!(
+        log.contains("env.set") && log.contains("MY_GLOBAL"),
+        "{log}"
+    );
+    assert!(
+        !log.contains("VALUE-") && !log.contains("sk-SECRET"),
+        "a value in the log"
+    );
+}
+
 #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
 async fn web_extension_toggle_edits_config_and_is_reread() {
     let web = Web::start();
````
2. `addons/web/src/env.rs` (new, whole file):
```rust
//! The custom env ops (`env.*`, docs/web-protocol.md §5 "Custom env") on
//! `app::env`, whose files are beside the addon runtime's config.toml (the
//! TUI's on this machine). `env.list` is the one reply that carries env
//! values (§4's exception, an owner decision); errors and the `web op` log
//! name keys only. The hub drops a session's entry when it is removed, and
//! prunes a host's entries when the host lists its sessions.

use serde::Deserialize;
use serde_json::{json, Value as Json};

use super::ws::{parse, NoArgs};
use super::AppState;
use claudio_core::app::addons::AddonRuntime;
use claudio_core::app::env::{Listed, Scope, Store};
use claudio_core::claude::env::{catalogue, refused_names, Value};
use claudio_core::proto::{SessionId, SessionKind};

/// The `scope` arg.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ScopeArg {
    Global,
    Session,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    scope: ScopeArg,
    #[serde(default)]
    sid: Option<SessionId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    scope: ScopeArg,
    #[serde(default)]
    sid: Option<SessionId>,
    name: String,
    value: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsetArgs {
    scope: ScopeArg,
    #[serde(default)]
    sid: Option<SessionId>,
    name: String,
}

/// One `env.*` op.
pub(crate) async fn dispatch(state: &AppState, op: &str, args: Json) -> Result<Json, String> {
    let store = Store::beside(state.addons.config_path());
    match op {
        "env.catalogue" => {
            parse::<NoArgs>(args)?;
            Ok(catalogue_json())
        }
        "env.list" => {
            let a: ListArgs = parse(args)?;
            let scope = scope_of(state, a.scope, a.sid).await?;
            let listed = blocking(move || store.list(&scope)).await?;
            Ok(list_json(listed))
        }
        "env.set" => {
            let a: SetArgs = parse(args)?;
            let scope = scope_of(state, a.scope, a.sid).await?;
            let (kept, name) = (scope.clone(), a.name.clone());
            blocking(move || store.set(&kept, &name, &a.value))
                .await?
                .map_err(|e| e.to_string())?;
            log("env.set", &scope, &a.name);
            Ok(json!({}))
        }
        "env.unset" => {
            let a: UnsetArgs = parse(args)?;
            let scope = scope_of(state, a.scope, a.sid).await?;
            let (kept, name) = (scope.clone(), a.name.clone());
            blocking(move || store.unset(&kept, &name))
                .await?
                .map_err(|e| e.to_string())?;
            log("env.unset", &scope, &a.name);
            Ok(json!({}))
        }
        _ => Err("unknown op".to_owned()),
    }
}

/// `scope` and `sid` as a scope: `sid` only with `session`, naming a claude
/// session the hub knows (its host is the hub's).
async fn scope_of(
    state: &AppState,
    scope: ScopeArg,
    sid: Option<SessionId>,
) -> Result<Scope, String> {
    match (scope, sid) {
        (ScopeArg::Global, None) => Ok(Scope::Global),
        (ScopeArg::Global, Some(_)) => Err("bad request: sid: not with scope global".to_owned()),
        (ScopeArg::Session, None) => {
            Err("bad request: sid: required with scope session".to_owned())
        }
        (ScopeArg::Session, Some(sid)) => {
            let session = state.hub.session(sid).await?;
            if session.info.kind != SessionKind::Claude {
                return Err("not a claude session".to_owned());
            }
            Ok(Scope::Session {
                host: session.host,
                sid,
            })
        }
    }
}

/// `work` off the executor (the files' lock waits up to 5 s).
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| "the env store stopped".to_owned())
}

/// A write, at INFO: the op, the scope, the sid and the name, never the value.
fn log(op: &str, scope: &Scope, name: &str) {
    match scope {
        Scope::Global => tracing::info!(op, scope = "global", name, "web op"),
        Scope::Session { host, sid } => {
            tracing::info!(op, scope = "session", %host, %sid, name, "web op")
        }
    }
}

/// `{"vars":[EnvVar],"refused":[EnvRefused]}`.
fn catalogue_json() -> Json {
    let vars: Vec<Json> = catalogue()
        .iter()
        .map(|v| json!({"name": v.name, "category": v.category, "description": v.description}))
        .collect();
    let refused: Vec<Json> = refused_names()
        .into_iter()
        .map(|(name, reason)| json!({"name": name, "reason": reason}))
        .collect();
    json!({ "vars": vars, "refused": refused })
}

/// `{"vars":[EnvEntry],"skipped": n}`: the values, by owner decision.
fn list_json(listed: Listed) -> Json {
    let vars: Vec<Json> = listed
        .vars
        .into_iter()
        .map(|(name, value)| json!({"name": name, "value": value.into_string()}))
        .collect();
    json!({ "vars": vars, "skipped": listed.skipped })
}

/// Drop session `sid`'s custom env (killed or removed), off the executor; a
/// failure is not shown.
pub(crate) fn forget(runtime: &AddonRuntime, host: &str, sid: SessionId) {
    let (store, host) = (Store::beside(runtime.config_path()), host.to_owned());
    tokio::task::spawn_blocking(move || {
        let _ = store.forget_session(&host, sid);
    });
}

/// Drop the custom env of `host`'s sessions not in `live` (its daemon's
/// list), off the executor; a failure is not shown.
pub(crate) fn prune(runtime: &AddonRuntime, host: &str, live: Vec<SessionId>) {
    let (store, host) = (Store::beside(runtime.config_path()), host.to_owned());
    tokio::task::spawn_blocking(move || {
        let _ = store.prune(&host, &live);
    });
}
```
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test web web_env_ops -- --test-threads=1`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 65 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test vocdoni_web web_proxy_session_overrides -- --test-threads=1`
Expected: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out`
Run: `TMPDIR=$HOME/.cache/cl-tmp cargo test --test web --test web_manager -- --test-threads=1` and
`TMPDIR=$HOME/.cache/cl-tmp cargo test --test vocdoni_web -- --test-threads=1`
Expected: `test result: ok. 66 passed`, `test result: ok. 4 passed`, `test result: ok. 14 passed`.
Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && python3 scripts/check_rules.py`
Expected: clean (`hub.rs` 2064 production lines, `connected` 118).

Step 3: commit
`git add addons/web/src/env.rs`
`git commit -m "feat(web): the env.* ops on the custom env store" -- addons/web/src/env.rs addons/web/src/lib.rs addons/web/src/ws.rs addons/web/src/hub.rs addons/web/tests/web.rs addons/vocdoni/tests/web.rs`

Interfaces this task provides to later tasks: the ops as documented in Task 1; refusal texts
`"<NAME>: refused: <why>"`, `"not a claude session"`, `"no such session"`.

### Task 7: web page: the Environment dialog (Alt+,)
Model: sonnet   Repo: /home/p4u/claudio   Files: addons/web/web/app/env.js (new),
addons/web/web/app/ui/env.js (new), addons/web/web/app/keymap.js,
addons/web/web/app/ui/app.js, addons/web/web/app/ui/settings.js,
addons/web/web/styles/components.css, addons/web/web/tests/env.test.mjs (new),
addons/web/web/tests/keymap.test.mjs, addons/web/web/e2e/fixtures/server.mjs,
addons/web/web/e2e/tests/env.spec.mjs (new)
Goal: AC7, AC8 (web part): the dialog (`Dialog`, testid `env`) with the Global and This
session tabs, the rows (name, a growing `<textarea>`, delete), the add row with a `<datalist>`
of the catalogue and the matching description, the note "Applies on next start or restart.";
Alt+, (`code: 'Comma'`, in `TUI_BINDINGS`), a palette entry and a Settings row; the dialog
re-reads once the WebSocket is open.
Rules: F3/DESIGN.md (tokens only; the existing `.seg`/`.btn`/`.input` classes), C5 (no inline
script or handler: Preact props only; the import map is untouched), pure modules import no bare
specifier (`app/env.js` is node-tested), the testids of the spec are the e2e's interface.

Step 1: write the failing test
Add `addons/web/web/tests/env.test.mjs`, the `Comma: 'environment'` line of
`tests/keymap.test.mjs`, the `envKeys` option of `e2e/fixtures/server.mjs` and
`e2e/tests/env.spec.mjs`.
Run: `node --test addons/web/web/tests/*.test.mjs`
Expected: `not ok … - tests/env.test.mjs` (it cannot import `app/env.js`), `not ok … - every
TUI binding maps from ev.code` (no `Comma`), `# fail 2`.

Step 2: implement
1. Modified files:
````diff
diff --git a/addons/web/web/app/keymap.js b/addons/web/web/app/keymap.js
index a8a6f33..d50882b 100644
--- a/addons/web/web/app/keymap.js
+++ b/addons/web/web/app/keymap.js
@@ -70,6 +70,7 @@ export const TUI_BINDINGS = [
   A('Alt+l', 'KeyL', 'history', 'history drawer'),
   A('Alt+u', 'KeyU', 'reader', 'reader drawer'),
   A('Alt+i', 'KeyI', 'extensions', 'extensions screen'),
+  A('Alt+,', 'Comma', 'environment', 'custom env vars of claude sessions', { key: ',' }),
   A('Alt+h', 'KeyH', 'help', 'this help'),
   A('Alt+z', 'KeyZ', 'palette', 'command palette (the extension actions)'),
   // Web-only. Claude Code uses Alt+P, Alt+T, Alt+M and readline's Alt+B/F/D/Enter, so Alt+O and
diff --git a/addons/web/web/app/ui/app.js b/addons/web/web/app/ui/app.js
index cd182ab..243c63a 100644
--- a/addons/web/web/app/ui/app.js
+++ b/addons/web/web/app/ui/app.js
@@ -35,6 +35,7 @@ import './changes.js';
 import './history.js';
 import './session-tab.js';
 import { ExtensionsScreen, installExtensions } from './extensions.js';
+import { EnvDialog, installEnv } from './env.js';
 import { PanelHost, panelOpen, closePanel, installAddons } from './panel.js';
 import { ChecklistHost, checklistOpen, cancelChecklist } from './checklist.js';
 import { ClaudeToastList } from './claude.js';
@@ -99,7 +100,8 @@ export function App() {
     ${o === 'palette' ? html`<${Palette} />` : o === 'help' ? html`<${Help} />` : o === 'settings' ? html`<${Settings} />`
       : o === 'wizard' ? html`<${Wizard} />` : o === 'search' ? html`<${Search} />` : o === 'snippets' ? html`<${Snippets} />` : o === 'compose' ? html`<${ComposeDialog} />`
       : o === 'document' ? html`<${DocumentView} />` : o === 'list' ? html`<${ActionList} />`
-      : o === 'extensions' ? html`<${ExtensionsScreen} />` : o === 'hosts' ? html`<${HostPanel} />` : null}
+      : o === 'extensions' ? html`<${ExtensionsScreen} />` : o === 'hosts' ? html`<${HostPanel} />`
+      : o === 'env' ? html`<${EnvDialog} />` : null}
     <${PanelHost} /><${ChecklistHost} /><${ProxyChooserHost} /><${Modal} />
     <${InfoPopover} />
     <div class="toasts" aria-live="polite"><${ClaudeToastList} /><${ToastList} /></div>`;
@@ -154,6 +156,7 @@ export function installFocusModel() {
   defineAction('restart-focused', withFocused((x) => restartSession(x, false)));
   defineAction('board', () => { showBoard.value = !boardShown(model.value.layout); });
   defineAction('extensions', () => { overlay.value = 'extensions'; });
+  defineAction('environment', () => { overlay.value = 'env'; });
   for (let n = 1; n <= 4; n++) {
     defineAction(`focus-pane-${n}`, () => {
       if (narrow.value || n > model.value.layout.grid) return;
@@ -174,6 +177,7 @@ export function installFocusModel() {
   installMaximize();
   installActions();
   installExtensions();
+  installEnv();
   installHosts();
   installAddons();
   onServerEvent(trackRemoved);
diff --git a/addons/web/web/app/ui/settings.js b/addons/web/web/app/ui/settings.js
index b1bd0cf..9adef46 100644
--- a/addons/web/web/app/ui/settings.js
+++ b/addons/web/web/app/ui/settings.js
@@ -30,6 +30,8 @@ export function Settings() {
       <input class="toggle" type="checkbox" checked=${s.sound} onChange=${(e) => updateSettings({ sound: e.target.checked })} /></label>
     <div class="set-row"><span>Extensions<small>lenses, actions and addon rows</small></span>
       <button type="button" class="btn btn-sm" data-testid="open-extensions" onClick=${() => { overlay.value = 'extensions'; }}>Manage…</button></div>
+    <div class="set-row"><span>Environment<small>env vars of claude sessions, global and per session (Alt+,)</small></span>
+      <button type="button" class="btn btn-sm" data-testid="open-env" onClick=${() => { overlay.value = 'env'; }}>Manage…</button></div>
     <div class="set-row"><span>Capture all keys<small>fullscreen, so Ctrl+T, Ctrl+W, Ctrl+N and Ctrl+Tab reach the terminal</small></span>
       <button type="button" class="btn btn-sm" data-testid="capture-keys" aria-pressed=${captured.value} onClick=${captureKeys}>${captured.value ? 'Release' : 'Capture'}</button></div>
     <label class="set-row"><span>Notifications<small>when a background session needs you</small></span>
diff --git a/addons/web/web/e2e/fixtures/server.mjs b/addons/web/web/e2e/fixtures/server.mjs
index 68c44e6..1a016a4 100644
--- a/addons/web/web/e2e/fixtures/server.mjs
+++ b/addons/web/web/e2e/fixtures/server.mjs
@@ -116,9 +116,10 @@ async function connectApi(port, body, remote) {
 
 /**
  * `remote`: `claudio web --remote` (TLS, the fixture password in the isolated config) instead of
- * `--listen`; `allowHosts` are its `--allow-host` names.
+ * `--listen`; `allowHosts` are its `--allow-host` names; `envKeys` more env vars the fake claude
+ * records (a spec asserting on its own custom env).
  */
-export async function startServer({ proxy = false, hosts = [], remote = false, allowHosts = [] } = {}) {
+export async function startServer({ proxy = false, hosts = [], remote = false, allowHosts = [], envKeys = [] } = {}) {
   const home = os.homedir();
   const base = path.join(home, '.cache/claudio/web-e2e');
   const root = path.join(base, crypto.randomBytes(4).toString('hex'));
@@ -152,6 +153,7 @@ export async function startServer({ proxy = false, hosts = [], remote = false, a
 
   const fakeProxy = proxy ? await startFakeProxy() : null;
   const extra = fakeProxy ? { CLAUDIO_PROXY_URL: `${PROXY_TOKEN}@${fakeProxy.addr}` } : {};
+  if (envKeys.length) extra.FAKE_ENV_KEYS = ['ANTHROPIC_AUTH_TOKEN', 'VOCDONI_TEST_TOKEN', ...envKeys].join(' ');
   const env = isolatedEnv(root, extra);
   // The isolation is the point: never let a test reach the user's own daemon.
   if (!env.XDG_RUNTIME_DIR.startsWith(base)) throw new Error('runtime dir is not isolated');
diff --git a/addons/web/web/styles/components.css b/addons/web/web/styles/components.css
index e05bdfe..dd907a9 100644
--- a/addons/web/web/styles/components.css
+++ b/addons/web/web/styles/components.css
@@ -558,6 +558,20 @@ input[type="radio"] { accent-color: var(--accent); }
 .ext-details { list-style: none; margin: 6px 0 0; padding: 0; display: grid; gap: 2px; font-family: var(--font-mono); font-size: var(--fs-sm); color: var(--text-muted); }
 .ext-tags { display: flex; gap: 4px; flex-wrap: wrap; margin-top: 8px; }
 .setup-bar { display: flex; align-items: center; justify-content: space-between; margin-bottom: var(--space-2); font-size: var(--fs-sm); }
+
+/* ---- custom env (Alt+,): a name, its raw value (mono, grows with its lines), delete ---- */
+.env-table { width: 100%; border-collapse: collapse; margin-top: var(--space-2); border-top: var(--hair) solid var(--border); }
+.env-table td { padding: var(--space-2) var(--space-1); border-bottom: var(--hair) solid var(--border); vertical-align: top; }
+.env-name { width: 34%; padding-top: 10px; font-family: var(--font-mono); color: var(--text-strong); word-break: break-all; }
+.env-value {
+  width: 100%; min-height: 34px; padding: 6px 10px; resize: vertical;
+  background: var(--bg); color: var(--text-strong); border: var(--hair) solid var(--border); border-radius: var(--radius);
+  font-family: var(--font-mono);
+}
+.env-value:hover { border-color: var(--border-strong); }
+.env-value::placeholder { color: var(--text-faint); }
+.env-add { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 2fr) auto; gap: var(--space-2); align-items: start; margin-top: var(--space-3); }
+.env-desc { margin: var(--space-2) 0 0; color: var(--text-muted); font-size: var(--fs-sm); line-height: 1.5; }
 .setup-bulk { display: inline-flex; gap: 6px; }
 .setup-items { max-height: 46vh; overflow-y: auto; border: var(--hair) solid var(--border); border-radius: var(--radius); background: var(--bg); }
 .setup-sec h3 { margin: 0; padding: var(--space-2) var(--space-3) 4px; font-family: var(--font-mono); font-size: var(--fs-xs); letter-spacing: var(--track-mono-wide); text-transform: uppercase; color: var(--text-muted); display: flex; gap: var(--space-2); position: sticky; top: 0; background: var(--bg); border-bottom: var(--hair) solid var(--border); }
diff --git a/addons/web/web/tests/keymap.test.mjs b/addons/web/web/tests/keymap.test.mjs
index 5d50e39..b9ad1af 100644
--- a/addons/web/web/tests/keymap.test.mjs
+++ b/addons/web/web/tests/keymap.test.mjs
@@ -62,6 +62,7 @@ test('every TUI binding maps from ev.code', () => {
     ArrowLeft: 'session-prev', ArrowRight: 'session-next', KeyN: 'new-session', KeyC: 'terminal-beside',
     KeyR: 'rename-focused', KeyX: 'kill-focused', KeyE: 'restart-focused', KeyA: 'next-attention',
     KeyG: 'board', KeyS: 'stats', KeyL: 'history', KeyU: 'reader', KeyI: 'extensions', KeyH: 'help', KeyZ: 'palette',
+    Comma: 'environment',
   };
   for (const [code, action] of Object.entries(want)) assert.equal(tuiBinding(alt(code)), action, code);
   // layout independent: the key value is whatever the layout makes of it
````
2. `addons/web/web/app/env.js` (new, whole file):
```javascript
// The custom env dialog's pure part (ui/env.js draws it). Imports nothing.

const NAME = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

/** Whether `name` is a variable name the server takes (before the refused set). */
export function nameOk(name) {
  return NAME.test(name);
}

/** A value as the server gets it: `\r\n` (and a lone `\r`) as `\n`. */
export function normalizeValue(text) {
  return String(text).replace(/\r\n?/g, '\n');
}

/** The rows a value's textarea shows: one per line, 1 to 8. */
export function valueRows(text) {
  return Math.min(8, Math.max(1, String(text).split('\n').length));
}

/**
 * What the add row says about `name` (trimmed): `{refused}` with the reason, `{bad}` for a
 * malformed one, `{known}` with the catalogue entry, or `{}` (any other name is fine).
 */
export function nameNote(catalogue, name) {
  const n = String(name).trim();
  if (!n) return {};
  if (!nameOk(n)) return { bad: true };
  const refused = (catalogue.refused ?? []).find((r) => r.name === n);
  if (refused) return { refused: refused.reason };
  const known = (catalogue.vars ?? []).find((v) => v.name === n);
  return known ? { known } : {};
}

/** The `env.*` scope args of a tab: `global`, or `session` with the focused claude session. */
export function scopeArgs(tab, sid) {
  return tab === 'session' && sid ? { scope: 'session', sid } : { scope: 'global' };
}
```
3. `addons/web/web/app/ui/env.js` (new, whole file):
```javascript
// The Environment dialog (Alt+,): the env vars every claude session gets (Global) and the
// focused claude session's own (This session), applied at the session's next start or restart.
// Values are raw text (a newline is a newline). The server keeps them (`env.*`,
// docs/web-protocol.md §5); the dialog lists when it opens (or its connection does) and after
// each of its writes.
import { html } from 'htm/preact';
import { useEffect, useState } from 'preact/hooks';
import { model, focused, conn, reqQuiet } from '../store.js';
import { overlay, register } from '../palette.js';
import { Dialog } from './modal.js';
import { nameNote, normalizeValue, scopeArgs, valueRows } from '../env.js';

export function installEnv() {
  register({ id: 'environment', group: 'App', title: 'Environment variables', keys: 'Alt+,', run: () => { overlay.value = 'env'; } });
}

function Row({ v, onSave, onDelete }) {
  const [text, setText] = useState(v.value);
  useEffect(() => setText(v.value), [v.value]);
  return html`<tr data-testid="env-row" data-name=${v.name}>
    <td class="env-name">${v.name}</td>
    <td><textarea class="env-value" data-testid="env-value" aria-label=${`${v.name} value`} rows=${valueRows(text)} value=${text}
      onInput=${(e) => setText(e.target.value)} onChange=${(e) => onSave(v.name, normalizeValue(e.target.value))}></textarea></td>
    <td><button type="button" class="btn btn-sm" data-testid="env-delete" aria-label=${`delete ${v.name}`} onClick=${() => onDelete(v.name)}>Delete</button></td>
  </tr>`;
}

export function EnvDialog() {
  const s = model.value.sessions[focused.value];
  const claude = s && s.kind === 'claude' ? s : null;
  const [tab, setTab] = useState('global');
  const [cat, setCat] = useState({ vars: [], refused: [] });
  const [list, setList] = useState({ loading: true, vars: [], skipped: 0 });
  const [error, setError] = useState(null);
  const [draft, setDraft] = useState({ name: '', value: '' });
  const shown = claude ? tab : 'global';
  const args = scopeArgs(shown, claude?.id);
  const open = conn.value === 'open';

  const load = async () => {
    try {
      const r = await reqQuiet('env.list', args);
      setList({ loading: false, vars: r.vars ?? [], skipped: r.skipped ?? 0 });
    } catch (e) {
      setList({ loading: false, vars: [], skipped: 0 });
      setError(e.message);
    }
  };
  useEffect(() => {
    if (open) reqQuiet('env.catalogue').then(setCat).catch((e) => setError(e.message));
  }, [open]);
  useEffect(() => {
    if (!open) return;
    setError(null);
    setList((l) => ({ ...l, loading: true }));
    load();
  }, [open, shown, claude?.id]);

  // A write, then the list read again; the server's refusal is shown, never a value.
  const write = async (op, more) => {
    setError(null);
    try {
      await reqQuiet(op, { ...args, ...more });
    } catch (e) {
      setError(e.message);
      return false;
    }
    await load();
    return true;
  };
  const save = (name, value) => write('env.set', { name, value });
  const remove = (name) => write('env.unset', { name });
  const note = nameNote(cat, draft.name);
  const add = async () => {
    const name = draft.name.trim();
    if (note.bad) { setError(`${name}: not a variable name (letters, digits and _, not starting with a digit)`); return; }
    if (note.refused) { setError(`${name}: refused: ${note.refused}`); return; }
    if (await save(name, normalizeValue(draft.value))) setDraft({ name: '', value: '' });
  };

  return html`<${Dialog} title="Environment" testid="env" wide onClose=${() => { overlay.value = null; }}>
    <div class="ext-bar">
      <div class="seg" role="tablist">
        <button type="button" class="seg-btn" role="tab" data-testid="env-tab-global" aria-selected=${shown === 'global'} onClick=${() => setTab('global')}>Global</button>
        ${claude ? html`<button type="button" class="seg-btn" role="tab" data-testid="env-tab-session" aria-selected=${shown === 'session'} onClick=${() => setTab('session')}>This session: ${claude.label}</button>` : null}
      </div>
      <span class="muted">Applies on next start or restart.</span>
    </div>
    <p class="wz-note">${shown === 'global'
      ? 'Every claude session this machine launches, on every host. A var of the same name in a session wins; a proxy\'s wins over both.'
      : 'This session only, over the global vars.'} Values are stored here (0600) and reach claude's environment on the session's host.</p>
    ${error ? html`<p class="panel-empty panel-error" role="alert" data-testid="env-error">${error}</p>` : null}
    ${list.loading ? html`<p class="panel-empty">Reading…</p>` : null}
    ${list.skipped ? html`<p class="wz-note">${list.skipped} entr${list.skipped === 1 ? 'y' : 'ies'} of the file left out (not valid here).</p>` : null}
    <table class="env-table"><tbody>
      ${list.vars.map((v) => html`<${Row} key=${v.name} v=${v} onSave=${save} onDelete=${remove} />`)}
    </tbody></table>
    <div class="env-add">
      <input class="input" data-testid="env-add-name" list="env-names" placeholder="NAME" aria-label="name" autocomplete="off" spellcheck="false" data-autofocus
        value=${draft.name} onInput=${(e) => setDraft({ ...draft, name: e.target.value })} />
      <textarea class="env-value" data-testid="env-add-value" placeholder="value" aria-label="value" rows=${valueRows(draft.value)}
        value=${draft.value} onInput=${(e) => setDraft({ ...draft, value: e.target.value })}></textarea>
      <button type="button" class="btn btn-sm" data-testid="env-add" onClick=${add}>Add</button>
    </div>
    ${note.known ? html`<p class="env-desc" data-testid="env-add-desc"><span class="chip">${note.known.category}</span> ${note.known.description}</p>` : null}
    ${note.refused ? html`<p class="ext-refused">refused: ${note.refused}</p>` : null}
    <datalist id="env-names">${cat.vars.map((v) => html`<option key=${v.name} value=${v.name}>${v.category}: ${v.description}</option>`)}</datalist>
  <//>`;
}
```
4. `addons/web/web/tests/env.test.mjs` (new, whole file):
```javascript
import test from 'node:test';
import assert from 'node:assert/strict';
import { nameOk, normalizeValue, valueRows, nameNote, scopeArgs } from '../app/env.js';

const catalogue = {
  vars: [{ name: 'ANTHROPIC_DEFAULT_OPUS_MODEL', category: 'model', description: 'Model ID that the opus alias resolves to.' }],
  refused: [{ name: 'PATH', reason: 'the daemon sets it' }],
};

test('names are letters, digits and underscores, not starting with a digit', () => {
  for (const good of ['A', '_', 'https_proxy', 'MAX_THINKING_TOKENS', 'A'.repeat(128)]) assert.ok(nameOk(good), good);
  for (const bad of ['', '1A', 'A-B', 'A B', 'Ä', 'A'.repeat(129)]) assert.ok(!nameOk(bad), bad);
});

test('a value keeps newlines and tabs; CRLF and CR become LF', () => {
  assert.equal(normalizeValue('a\r\nb\rc\nd\te'), 'a\nb\nc\nd\te');
  assert.equal(valueRows('one'), 1);
  assert.equal(valueRows('a\nb\nc'), 3);
  assert.equal(valueRows('\n'.repeat(20)), 8);
});

test('the add row describes a documented name and says why one is refused', () => {
  assert.deepEqual(nameNote(catalogue, ''), {});
  assert.deepEqual(nameNote(catalogue, ' PATH '), { refused: 'the daemon sets it' });
  assert.deepEqual(nameNote(catalogue, '1X'), { bad: true });
  assert.equal(nameNote(catalogue, 'ANTHROPIC_DEFAULT_OPUS_MODEL').known.category, 'model');
  assert.deepEqual(nameNote(catalogue, 'MY_OWN'), {});
});

test('a tab is the global scope, or the focused session', () => {
  assert.deepEqual(scopeArgs('global', 'abc'), { scope: 'global' });
  assert.deepEqual(scopeArgs('session', 'abc'), { scope: 'session', sid: 'abc' });
  assert.deepEqual(scopeArgs('session', null), { scope: 'global' });
});
```
5. `addons/web/web/e2e/tests/env.spec.mjs` (new, whole file):
```javascript
import { test, expect } from '@playwright/test';
import { startServer } from '../fixtures/server.mjs';

let srv;
test.beforeAll(async () => { srv = await startServer({ envKeys: ['MY_GLOBAL', 'MY_SESSION', 'ANTHROPIC_DEFAULT_OPUS_MODEL'] }); });
test.afterAll(async () => { await srv?.stop(); });
test.afterEach(async () => {
  for (const name of ['MY_GLOBAL', 'ANTHROPIC_DEFAULT_OPUS_MODEL']) await srv.api('env.unset', { scope: 'global', name }).catch(() => {});
  await srv.reset();
});

const csp = (page) => page.evaluate(() => window.__claudioCsp ?? []);

test('the dialog adds a global and a session var, and a restart records them', async ({ page }) => {
  const sid = await srv.spawn(srv.dirs.a);
  await page.goto(srv.url);
  await page.locator(`[data-testid="session-card"][data-sid="${sid}"]`).click();
  await expect(page.getByTestId('terminal')).toContainText('FAKE_CLAUDE_BANNER');
  const first = await srv.waitFor(() => srv.launchedEnv(srv.dirs.a), 'the first launch');
  expect(first).not.toContain('MY_GLOBAL');

  await page.keyboard.press('Alt+Comma');
  const dialog = page.getByTestId('env');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('Applies on next start or restart.');
  await expect(page.getByTestId('env-tab-global')).toHaveAttribute('aria-selected', 'true');
  await page.getByTestId('env-add-name').fill('MY_GLOBAL');
  await page.getByTestId('env-add-value').fill('one\ntwo');
  await page.getByTestId('env-add').click();
  const row = page.locator('[data-testid="env-row"][data-name="MY_GLOBAL"]');
  await expect(row.getByTestId('env-value')).toHaveValue('one\ntwo');
  await expect(page.getByTestId('env-add-name')).toHaveValue('');

  await page.getByTestId('env-tab-session').click();
  await expect(page.getByTestId('env-tab-session')).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('env-row')).toHaveCount(0);
  await page.getByTestId('env-add-name').fill('MY_SESSION');
  await page.getByTestId('env-add-value').fill('mine');
  await page.getByTestId('env-add').click();
  await expect(page.locator('[data-testid="env-row"][data-name="MY_SESSION"]')).toHaveCount(1);
  // A refused name says why and writes nothing.
  await page.getByTestId('env-add-name').fill('PATH');
  await page.getByTestId('env-add-value').fill('/x');
  await page.getByTestId('env-add').click();
  await expect(page.getByTestId('env-error')).toContainText('PATH: refused');
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  await page.getByTestId('terminal').click();
  const before = srv.launchedEnv(srv.dirs.a);
  await page.keyboard.press('Alt+e');
  await page.getByTestId('confirm-yes').click();
  await srv.waitFor(() => { const e = srv.launchedEnv(srv.dirs.a); return e && e !== before && e; }, 'the restart');
  const env = srv.launchedEnv(srv.dirs.a);
  expect(env).toContain('MY_GLOBAL=one\ntwo');
  expect(env).toContain('MY_SESSION=mine');
  expect(await csp(page)).toEqual([]);
});

test('the catalogue datalist offers documented names with their description', async ({ page }) => {
  await page.goto(srv.url);
  await expect(page.getByTestId('rail')).toBeVisible();
  await page.keyboard.press('Alt+Comma');
  await expect(page.getByTestId('env')).toBeVisible();
  await expect(page.getByTestId('env-tab-session')).toHaveCount(0);
  const option = page.locator('#env-names option[value="ANTHROPIC_DEFAULT_OPUS_MODEL"]');
  await expect(option).toHaveCount(1);
  await expect(option).toContainText('Model ID that the opus alias resolves to');
  await page.getByTestId('env-add-name').fill('ANTHROPIC_DEFAULT_OPUS_MODEL');
  await expect(page.getByTestId('env-add-desc')).toContainText('Model ID that the opus alias resolves to');
  await page.getByTestId('env-add-value').fill('claude-opus-5-5');
  await page.getByTestId('env-add').click();
  const row = page.locator('[data-testid="env-row"][data-name="ANTHROPIC_DEFAULT_OPUS_MODEL"]');
  await row.getByTestId('env-value').fill('claude-opus-5-6');
  await row.getByTestId('env-value').blur();
  await expect.poll(async () => (await srv.api('env.list', { scope: 'global' })).vars).toEqual([{ name: 'ANTHROPIC_DEFAULT_OPUS_MODEL', value: 'claude-opus-5-6' }]);
  await row.getByTestId('env-delete').click();
  await expect(page.getByTestId('env-row')).toHaveCount(0);
  expect(await csp(page)).toEqual([]);
});
```
Run: `node --test addons/web/web/tests/*.test.mjs`
Expected: `# tests 160` and `# fail 0`.
Run: `cargo build --locked && cd addons/web/web/e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund && npx playwright test env`
Expected: `2 passed`.
Run: `TMPDIR=$HOME/.cache/cl-tmp make test-web`
Expected: the node tests, then `65 passed` and no failure.

Step 3: commit
`git add addons/web/web/app/env.js addons/web/web/app/ui/env.js addons/web/web/tests/env.test.mjs addons/web/web/e2e/tests/env.spec.mjs`
`git commit -m "feat(web): the Environment dialog (Alt+,)" -- addons/web/web/app/env.js addons/web/web/app/ui/env.js addons/web/web/app/keymap.js addons/web/web/app/ui/app.js addons/web/web/app/ui/settings.js addons/web/web/styles/components.css addons/web/web/tests/env.test.mjs addons/web/web/tests/keymap.test.mjs addons/web/web/e2e/fixtures/server.mjs addons/web/web/e2e/tests/env.spec.mjs`

Interfaces this task provides to later tasks: the overlay `env`, the action `environment`,
the palette entry "Environment variables", Settings → Environment (testid `open-env`).

### Task 8: docs: the custom env in the README, the web, manager and addon docs, CLAUDE.md, ACCEPTANCE.md
Model: haiku   Repo: /home/p4u/claudio   Files: README.md, docs/web.md,
docs/manager-design.md, docs/addons.md, CLAUDE.md, ACCEPTANCE.md
Goal: F1 and AC8 (README and docs/web.md list Alt+,): the `[env]` block and the key, the parity
row, the key tables, the per-machine note and where values end up, the precedence of custom and
addon env, the composer's user env, CLAUDE.md's secrets bullet, composer paragraph and A3
services, ACCEPTANCE.md C1's exception (its own commit, as ACCEPTANCE.md requires).

Step 1: write the failing test
No test: documentation. Check:
Run: `grep -c "Alt+," README.md docs/web.md`
Expected: `README.md:0` and `docs/web.md:0`.

Step 2: implement
````diff
diff --git a/ACCEPTANCE.md b/ACCEPTANCE.md
index 9b751a8..a79a3bf 100644
--- a/ACCEPTANCE.md
+++ b/ACCEPTANCE.md
@@ -90,7 +90,9 @@ writing what the previous release reads for one release, so reverting the merge
 redacted in-memory launch data, and reach a managed session's claude only through
 `Spawn`/`Respawn` env over the daemon socket (a print/API backend's child env directly). Never in
 argv, logs, notices, public DTOs, ordinary session state, `Debug` output or test snapshots;
-messages name keys, never values.
+messages name keys, never values. One exception (owner decision, 2026-11-15): the custom env
+values a person types in the Environment menu are shown in both UIs, and the web's `env.list`
+reply carries them; they live 0600 in `config.toml [env]` and `session-env.json`.
 
 **C2. Outside input is bounded and never panics.** Data from files, sockets, ssh, transcripts,
 extension programs, the network or a browser has a size cap and is parsed without `unwrap`,
diff --git a/CLAUDE.md b/CLAUDE.md
index 5b5c63f..c20eb55 100644
--- a/CLAUDE.md
+++ b/CLAUDE.md
@@ -183,8 +183,8 @@ declares. The services addons use today: `paths` (all); `term::{probe, strip_esc
 (api); `proto`, `text`, `client`, `remote::bootstrap`, `term::secret` and
 `app::{addons, launch, browser}` (web and vocdoni); `claude::SESSION_MARKERS` and
 `app::config_edit` (vocdoni, web; and `daemon::test_support` in the web's tests); `config`, `freshness`, `claude::{hooks, update}`,
-`remote::{hosts, validate_host}`, `sys::hostname` and `app::label` (web). The web addon (the former in-tree web UI, a second front-end) also uses
-`app::{state, connections, extensions, snippets}`: the layer both front-ends share (A5).
+`remote::{hosts, validate_host}`, `sys::hostname`, `app::label`, `app::env` and `claude::env` (web). The web addon (the former in-tree web UI, a second front-end) also uses
+`app::{state, connections, extensions, snippets, env}`: the layer both front-ends share (A5).
 
 ### Session manager (bare `claudio`)
 
@@ -242,18 +242,22 @@ flowchart LR
   `launch/` (the launch composer), `config_edit.rs` (every `config.toml` writer: a minimal,
   atomic section edit under one process-wide lock), `state.rs` (`state.json`),
   `connections.rs`, `extensions/` (the registry, external manifests in
-  `~/.config/claudio/extensions/`, the extension host), `snippets.rs`.
+  `~/.config/claudio/extensions/`, the extension host), `snippets.rs`, `env.rs` (the custom env:
+  `config.toml [env]` and `session-env.json`, both front-ends' and the composer's).
 - **Launch composer** (`app/launch/`): the one path of a claude session's `Spawn`/`Respawn`,
   from the TUI, `--plain` and the web. Resolve (every `Launch` addon's `Routing`; a `Required`
   addon that fails fails the launch, never direct) → prepare (25 s budget) → review
-  (`NeedsReview` parks the launch) → `compose.rs` (env layered, `env_remove` unioned, optional
-  args, lenses merged, instructions cut at 8158 bytes). A saved route this build cannot make
-  is kept and fails its launches, never falls back to direct.
+  (`NeedsReview` parks the launch) → `compose.rs` (the person's custom env first, read by `send`
+  with `app::env` beside the runtime's `config_path`, then the addons' env layered: an addon's
+  key or `env_remove` wins, one keys-only notice, dropped for a recovery; `env_remove` unioned,
+  optional args, lenses merged, instructions cut at 8158 bytes). A saved route this build cannot
+  make is kept and fails its launches, never falls back to direct.
 - **TUI** (`crates/tui/src/`): `lib.rs` is the I/O edge (terminal, event loop, every `Effect`);
   `app.rs` is the pure coordinator; `interaction.rs` keys, mouse and modals; `sessions.rs`
   builds every spawn (`send_spawn`; a claude session is an `Effect::Launch`);
   `addon_runner.rs` is the TUI's edge of the addon runtime and the launch composer, and keeps
-  parked launches out of `App`; `keymap.rs`'s `DEFAULT_BINDINGS` is the single source of truth
+  parked launches out of `App`; `env_view.rs` is the custom env menu (Alt+,; its I/O is
+  `env_edge.rs`); `keymap.rs`'s `DEFAULT_BINDINGS` is the single source of truth
   for keys (`Alt+Shift+<digit>` is fixed, outside it); `plain.rs` is `--plain`; `ui/` draws.
 
 ### Web UI (`claudio web`, `addons/web/src/`, `addons/web/web/`)
@@ -331,7 +335,10 @@ security model and the TUI parity table: `docs/web.md`.
   `LaunchPart.env`, whose `Debug` shows keys only); an `AddonOp` body
   (`vocdoni/apply`) carries secret names, never values, and is never logged. The journal and `state.json`
   never hold secrets (profile names only); `config.toml` and
-  `plugins.json` do, both mode 0600. Logs and notices name keys, never
+  `plugins.json` do, both mode 0600, and so may `config.toml [env]` and
+  `session-env.json` (the custom env, 0600). Custom env values are shown
+  in both UIs by owner decision: the web's `env.list` reply is the one DTO
+  that carries them. Logs and notices name keys, never
   values. The `--mcp-config` JSON on argv holds `${NAME}` placeholders
   that claude expands from the env; `--agents`/`--mcp-config` are added by
   claudio's spawn path only, never parsed from the user's args.
diff --git a/README.md b/README.md
index 1eb2171..10df0d6 100644
--- a/README.md
+++ b/README.md
@@ -96,6 +96,7 @@ Only `Alt` keys are claudio's; everything else goes to Claude Code. `Alt+h` list
 | `Alt+r` / `Alt+x` | Rename / close (asks first) |
 | `Alt+z` | Extension actions: `s` search, `p` insert a snippet, `P` save the last prompt, and your own |
 | `Alt+i` | Extensions screen: turn each one on or off |
+| `Alt+,` | Environment: env vars for claude sessions, global or this session's (next start or restart) |
 | `Alt+s` | The addon panel (the Vocdoni addon's proxy stats) |
 | `Alt+h` | Help |
 | `Alt+q` | Quit the UI; sessions keep running |
@@ -227,8 +228,22 @@ diagram = false                 # e.g. no Mermaid instruction or drawing (Alt+i
 new_session = "alt+n"
 git_log     = "alt+l"
 formulas    = "alt+u"           # the reader
+
+[env]                           # env vars every claude session gets, on every host (Alt+, writes this table)
+ANTHROPIC_DEFAULT_OPUS_MODEL = "claude-opus-5-5"
+BASH_DEFAULT_TIMEOUT_MS = "300000"
 ```
 
+`Alt+,` (and Environment in the web UI) sets env vars for the claude sessions claudio launches:
+**global** ones in `[env]`, and **per-session** ones, which win over a global var of the same
+name, in `session-env.json` beside `config.toml` (0600; an entry goes with its session). They
+apply at the session's next start, restart (`Alt+e`), respawn or recovery; a running claude
+keeps its env. The values are shown in both UIs. Both scopes belong to the machine that runs the
+UI: another machine that restarts the session applies its own. A launch addon's vars win (a
+proxy session keeps the proxy's `ANTHROPIC_BASE_URL` and drops an `ANTHROPIC_API_KEY`), and so
+does an `env` entry in `~/.claude/settings.json`. Names claudio relies on (`PATH`, `TERM`, the
+session markers, `CLAUDE_CODE_SIMPLE`…) are refused. Terminal tabs, `-p` and `--api` get none.
+
 The `[proxy]` and `[plugins.*]` tables belong to the Vocdoni addon. Snippets are files in `snippets/`; your own extensions are directories in `extensions/` ([docs/extensions.md](docs/extensions.md), example in [`examples/extensions/keys/`](examples/extensions/keys/)). `CLAUDIO_NO_UPDATE_CHECK=1` turns off the claudio and claude release checks, and `CLAUDIO_NO_BROWSER=1` keeps claudio from opening a browser.
 
 ## Building from source
diff --git a/docs/addons.md b/docs/addons.md
index 22ecb40..e5a618a 100644
--- a/docs/addons.md
+++ b/docs/addons.md
@@ -213,9 +213,15 @@ queues it until it is idle; the web ignores offers.
 
 **4. Compose.** The core merges the parts, the one implementation for every front-end:
 
-- `env`: in registry order; a later addon's key wins; no key twice. `LaunchPart.env` is a secret:
-  it goes only into `SpawnSpec.env` over the daemon socket, never into `App`, a DTO, a log, a
-  notice or `Debug` (`LaunchPart`'s `Debug` prints env keys only; C1).
+- `env`: the person's custom env first (`app::env::Store::effective` for the session, read by
+  the composer itself beside the runtime's `config_path`: global `[env]` overlaid with the
+  session's own; every entry checked again), then each part in registry order; a later key wins;
+  no key twice. An addon's key replaces the person's, and a person's key named in a part's
+  `env_remove` (and set by no part) is dropped: the addon wins. One notice names the person's
+  keys an addon overrode (`custom env overridden by <addon>: NAME, …`), keys only; a recovery
+  drops it. `LaunchPart.env` and the custom env are secrets: they go only into `SpawnSpec.env`
+  over the daemon socket, never into `App`, a DTO, a log, a notice or `Debug` (`LaunchPart`'s
+  `Debug` prints env keys only, `claude::env::Value`'s its length; C1).
 - `env_remove`: names the daemon removes from the session's inherited env, besides claude's own
   session markers.
 - `args`: appended in order. `LaunchArg::Optional { flag, value }` is dropped by the daemon when
@@ -516,8 +522,8 @@ changes it: the edit gets the section as a `toml::Table`, and the core writes ba
 keys that changed, atomically, keeping every other line and comment, through one process-wide
 lock shared with every other writer of `config.toml` (B3).
 
-The core owns these sections: `ui`, `keys`, `update`, `claude`, `extensions`. No addon may
-claim them.
+The core owns these sections: `ui`, `keys`, `update`, `claude`, `extensions`, `ssh`, `env`. No
+addon may claim them.
 
 The core's command words are reserved (no addon may be named after one, and the core runs them
 before any addon's `dispatch`): `__probe`, `--slave`, `__hook`, `__context`, `--daemon`,
diff --git a/docs/manager-design.md b/docs/manager-design.md
index 29e855f..288c5f5 100644
--- a/docs/manager-design.md
+++ b/docs/manager-design.md
@@ -386,6 +386,15 @@ the upstream model.
 `ANTHROPIC_API_KEY` is unset. Note that `/proc/<pid>/environ` is readable by
 the same uid and root.
 
+#### Custom env and the proxy's
+A person's custom env (`app::env`: `config.toml [env]` and `session-env.json`, set with
+`Alt+,` or the web's Environment dialog) is composed first, then every addon's part. The
+proxy's env wins: a key it sets replaces the person's (`ANTHROPIC_BASE_URL`, the token, the
+default models), and a key in its `env_remove` (`ANTHROPIC_API_KEY`) is dropped from the
+person's env too. One launch notice names the keys the proxy overrode (`custom env overridden
+by vocdoni: ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY`), never a value; a recovery does not repeat
+it.
+
 **[later]** Supply the token through claude's `apiKeyHelper` (`claudio __token`,
 which asks the daemon), so it never sits in the environment.
 
diff --git a/docs/web.md b/docs/web.md
index 17ba9da..f2a5834 100644
--- a/docs/web.md
+++ b/docs/web.md
@@ -104,6 +104,26 @@ without one it listens on `0.0.0.0:7337`, IPv4 only. It is the one listener: `--
 (with `--remote`) makes a new certificate. `--set-password` and `--print-cert` take no other
 argument and exit.
 
+### Custom env vars
+
+`Alt+,` (or Environment in the palette and in Settings) opens the env vars of the claude
+sessions claudio launches. **Global** ones go to every session on every host (`config.toml
+[env]`); **This session** shows when the focused pane holds a claude session, and its vars win
+over a global one of the same name (`session-env.json` beside `config.toml`, 0600). The name field
+offers Claude Code's documented variables (a datalist) and shows the description of a known one;
+names claudio relies on are refused with the reason. A value is raw text: a newline is a newline.
+Changes apply at the session's next start, restart, respawn or recovery; a running claude keeps
+its env. A launch addon's vars win (a proxy session keeps its `ANTHROPIC_BASE_URL` and drops an
+`ANTHROPIC_API_KEY`, with a notice naming the keys), and an `env` entry in the host's
+`~/.claude/settings.json` wins over both.
+
+Both scopes belong to this machine: the TUI here and this server share them, and another
+machine's claudio that restarts the session applies its own global vars and none of this
+session's. The values are shown here and in the TUI (an owner decision). Where they end up: on the
+session's host they are in the daemon's memory and in claude's environment, readable by that
+user and root and inherited by every command claude runs, so a command that prints its
+environment puts them in that host's transcript.
+
 ### SSH hosts
 
 A host is added by name or `user@host`, from the hosts panel or Alt+n's "Another host…" (the
@@ -429,6 +449,7 @@ xterm and the browser do (matched on the physical key, so any layout works; AltG
 | `Alt+g` | The board |
 | `Alt+s` / `Alt+l` / `Alt+u` | Addon panel (proxy stats) / history / reader drawer |
 | `Alt+i` | Extensions screen |
+| `Alt+,` | Environment: custom env vars of claude sessions |
 | `Alt+h` | Help |
 | `Alt+z` | Command palette (lists the extension actions) |
 | `Alt+o` | Compose: a pop-up editor for a long or multi-line prompt (web only) |
@@ -475,6 +496,7 @@ Rust test in `addons/web/tests/web.rs`; rows without an automated test say how t
 | Search | `Alt+z` `s` | `/` | `search` | `search finds a fixture conversation`; `web_reader_and_search_read_a_fixture_transcript` |
 | Snippets | `Alt+z` `p` / `P` | palette: Insert snippet, Save last prompt | `snippets.list`, `snippets.save`, `reader.last_prompt` | `save and insert a snippet`; `web_snippets_save_and_list` |
 | Extensions screen | `Alt+i` | Settings → Manage… | `extensions.list`, `extensions.toggle`, `extensions.action` | `toggling an extension rewrites its config.toml line`; `web_extension_toggle_edits_config_and_is_reread`; `web_external_extension_action_runs_its_program` |
+| Custom env vars | `Alt+,` | `Alt+,`, the palette ("Environment variables"), Settings → Environment | `env.catalogue`, `env.list`, `env.set`, `env.unset` | `the dialog adds a global and a session var, and a restart records them`; `the catalogue datalist offers documented names with their description`; `web_env_ops_set_list_unset_and_reach_the_launch`; `web_proxy_session_overrides_the_custom_env` |
 | Plugin setup (an addon checklist) | first spawn, extensions screen | the checklist a launch's `review` asks for (Save / Skip / Cancel), "Set up" on an addon row | `addons.checklist`, `addons.checklist.save`, `addons.rows`, `session.spawn` (`defer`) | `a plugin without choices asks for its checklist before spawn and Skip spawns without it`; `web_spawn_with_launch_needs_review_then_defer_spawns_without_the_plugin` |
 | claude install / update | update prompt | Hosts menu → host row → panel → Update claude; toasts | `claude.update`, `claude.skip`, `claude` events | `claude update shows progress and completion` |
 | Host stats | status bar cpu / mem | footer: the host of the session in view (every host when none is) | `host_stats` events | node (`footerStats`) |
````
Run: `grep -c "Alt+," README.md docs/web.md`
Expected: `README.md:3` and `docs/web.md:3`.

Step 3: commit
`git commit -m "docs: custom env vars in the README, the web, manager and addon docs and CLAUDE.md" -- README.md docs/web.md docs/manager-design.md docs/addons.md CLAUDE.md`
`git commit -m "docs(acceptance): C1 gains the custom env exception, an owner decision" -- ACCEPTANCE.md`

Interfaces this task provides to later tasks: none.

## Merge and release

1. Full suites on `feat/custom-env`, with `TMPDIR=$HOME/.cache/cl-tmp`:
   - `make check` → fmt, clippy, `check_rules.py`, the core-only build, then every
     `test result: ok.` (claudio-core 609, claudio-tui 404, claudio-api 106, claudio-web 188,
     claudio-vocdoni 150, …) and each crate alone.
   - `make matrix` → the five cells pass (the core-only cell runs the manager suite: 48 passed).
   - `make test-manager` → `48 passed`.
   - `cargo test --test web --test web_manager -- --test-threads=1` → `66 passed`, `4 passed`;
     `cargo test --test vocdoni_web -- --test-threads=1` → `14 passed`.
   - `node --test addons/web/web/tests/*.test.mjs` → `# tests 160`, `# fail 0`.
   - `make test-web` → the node tests, then Playwright `65 passed`.
2. Merge order: `feat/custom-env` → `main` with `git merge --no-ff feat/custom-env` (controller).
3. Release, per the repo's procedure: bump `version` in the root `Cargo.toml` `[workspace.package]`,
   `cargo update -w`, commit `chore(release): vX.Y.Z` (Cargo.toml, Cargo.lock), annotated tag
   `vX.Y.Z`; release notes through `gh` once the tag's workflow ran. v0.10.0 is reserved, and
   main already has v0.9.8 and v0.9.9 (released after the spec), so this is **v0.9.10** unless
   the owner says otherwise.
4. **Owner OK required before:** pushing `main`; pushing the tag (it triggers the release
   workflow); publishing the release notes.
5. Post-release checks: CI green on Linux and macOS for `main` and for the tag; the release has
   its assets (the linux and macOS binaries the workflow uploads); `claudio --version` of a
   downloaded asset prints the new version; on a dev install, Alt+, opens the menu and the web
   dialog lists the catalogue.
