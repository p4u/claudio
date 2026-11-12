# Addons

An addon adds a feature to claudio without the core knowing it by name: a command, a way to
route a session (a proxy), status-bar segments, a panel, a checklist, extension rows, or work
inside each host's daemon. The core stays generic (ACCEPTANCE.md A1, A3); everything else is an
addon. This page is the addon API, `claudio_core::addon` (`crates/core/src/addon/`). It describes
the contract the core keeps with an addon and the limits it enforces.

- [Crates](#crates)
- [Capabilities](#capabilities)
- [The registry](#the-registry)
- [Execution model](#execution-model)
- [Launches](#launches)
- [Data, not widgets](#data-not-widgets)
- [Daemon ops](#daemon-ops)
- [The API addon](#the-api-addon)
- [The web addon](#the-web-addon)
- [The Vocdoni addon](#the-vocdoni-addon)
- [Config sections and reserved words](#config-sections-and-reserved-words)
- [Limits](#limits)

## Crates

claudio is one binary built from a Cargo workspace. Each addon is its own crate and a cargo
feature of the `claudio` package (all on by default):

| Crate | Path | What |
|---|---|---|
| `claudio-core` | `crates/core` | the addon API, the daemon and its protocol, the client, SSH, the shared app layer |
| `claudio-tui` | `crates/tui` | the console session manager and `--plain` |
| `claudio-api` | `addons/api` | feature `api`: `-p`/`--print` and `--api` |
| `claudio-web` | `addons/web` | feature `web`: `claudio web`, its server and page |
| `claudio-vocdoni` | `addons/vocdoni` | feature `vocdoni`: the claude-proxy client and the team plugins |
| `claudio` | `.` (`src/main.rs`) | the binary: wiring only, `addons()` registers the enabled addons |

**The dependency rule (ACCEPTANCE.md A2).** An addon crate depends on `claudio-core` only, never
on the TUI or another addon; `claudio-tui` depends on `claudio-core` only, and `claudio-core` on
none of these. An addon's dev-dependencies may add the TUI or another addon for its tests.
`scripts/check_rules.py` (in `make check`) enforces it from `cargo metadata`.

**`test-support`.** `claudio-core`'s fixtures for other crates' tests (`TestAddon`, the daemon
harness, the fake host behind a real `Client`) are compiled under its `test-support` feature,
which only dev-dependencies enable: a release binary never links them.

**Builds without addons.** `cargo build --no-default-features` is the core alone (A4);
`--no-default-features --features web,vocdoni` picks some. `make matrix` builds and tests the
core alone, each addon alone and all of them. A build without the `api` addon refuses
`-p`/`--print`/`--api` (before a bare `--`) with exit 2, because claudio never invokes
`claude -p`: `claudio: print mode and the API server need the api addon; this build has none`.
A build without the `web` addon asks claude for no lenses.

## Capabilities

An addon is a value that implements `Addon`. It exposes optional capabilities and implements
only those it has. A command-only addon has no UI methods.

```rust
pub trait Addon: Send + Sync + 'static {
    fn name(&self) -> &'static str;                         // [a-z][a-z0-9-]{0,31}
    fn config_sections(&self) -> &'static [&'static str];   // top-level config.toml tables
    fn commands(&self) -> Option<&dyn Commands>;
    fn launch(&self) -> Option<&dyn Launch>;
    fn ui(&self) -> Option<&dyn Ui>;
    fn daemon(&self) -> Option<&dyn DaemonOps>;
    fn on_event(&self, event: &AddonEvent, ctx: &Arc<dyn AddonCtx>);
}
```

| Capability | What it adds | Who calls it |
|---|---|---|
| `Commands` | `claudio <word> …` subcommands, hook relays, a `--help` appendix | `main` |
| `Launch` | launch flags, wizard options, routing, a share of every launch | the launch composer (TUI, `--plain`, web) |
| `Ui` | status segments, panels and their actions, checklists, extension rows | the TUI and the web page, through the runtime |
| `DaemonOps` | ops run inside each host's daemon (`AddonOp`) | the daemon |

`name()` is the addon's registry key, its CLI word when it has commands, and its daemon
feature (`addon:<name>`).

**Commands.** `src/main.rs` is wiring: it routes argv (`argv[0]` included) in this order.
1. `Addons::relay`: the first addon whose `is_relay(argv)` is true runs it through its own
   `dispatch`, so a hook relay (a copy of the binary that claude runs as a hook) is answered
   before anything else parses the command line.
2. The core's words: `__probe`, `--slave`, the diag words (`--features diag`), the manager's
   `__hook <Event> <socket> <token>`, `__context`, `--daemon`, `upgrade`, `daemon`,
   `sessions`, `--plain`, no argument (the manager) and the addons' launch flags (the manager,
   routed as they ask).
3. `Addons::dispatch`: each addon's `dispatch(argv, addons)`, in registry order; the first
   `Some(code)` wins, `None` means "not mine". In a build without the `api` addon, `-p`,
   `--print` or `--api` before a bare `--` then exits 2 (see [Crates](#crates)).
4. `-h`/`--help`: claude's help, then every addon's `help()`, in registry order.
5. Anything else: the real `claude`, unchanged.

**Launch**, **Ui** and **DaemonOps** are described below.

Nothing downcasts an addon: the core reaches it only through these capabilities.

## The registry

`Addons::new(list)` builds the immutable set this binary runs with; `main` builds it once
(`addons()` in `src/main.rs`) and hands an `Arc<Addons>` to every entry point. Registration is refused,
and claudio exits with the reason, when:

- a name is not `[a-z][a-z0-9-]{0,31}`, is repeated, or is a reserved word;
- a config section is a core section, or two addons (or one addon twice) claim it;
- a launch flag is not `--[a-z][a-z0-9-]*`, is declared twice, or is a core flag;
- a panel id is not `[a-z][a-z0-9-]{0,31}` or is repeated within the addon, or two panels bind
  the same keymap action.

`daemon_features()` lists `addon:<name>` for each addon with `daemon()`; the daemon adds them to
`Welcome.features`. `launch_flags()` lists every `(addon, FlagDecl)`.

## Execution model

An addon is a **pure state machine** (A6: no files, network, processes or sockets; unit tested)
wrapped by its capability impls, which are its **runner**. The runner does the I/O through
`AddonCtx`, which the core implements once (`crates/core/src/app/addons/`) for both front-ends: the TUI
runner and the web hub. Each addon gets its own context, bound to its name, so every
request, config read and edit is attributed to it.

| `AddonCtx` | |
|---|---|
| `spawn(task)` | a task on the front-end's runtime: at most 64 live per addon, each under 300 s, aborted at shutdown |
| `request(host, op, body)` | an `AddonOp` to that host's daemon; `Missing` unless its features list `addon:<name>`; 170 s |
| `host_has(host)` | that host is connected and lists `addon:<name>` |
| `sessions()` | a snapshot of the sessions (`SessionSnap`) |
| `config(section)` | a declared section of `config.toml` (empty when undeclared) |
| `edit_config(section, edit)` | an atomic, minimal edit of a declared section (below) |
| `extension_on(name)` | `[extensions] <name>` as `config.toml` has it, or `None`; for `latex` without one, `[claude] latex` (its documented alias) |
| `core_extensions()` | the names the core's own extensions hold (built-in and external): an addon row of such a name is refused, so the addon must not use it either |
| `notify(text)` | a notice in the front-end |
| `changed()` | segments, rows, options or an open panel changed: front-ends re-read them |
| `offer_checklist(id)` | ask front-ends to show a checklist when idle |

**Events.** `on_event` is called on the front-end's runtime and must not block; it hands work to
`ctx.spawn`. The order: `Started` first, once the snapshots are filled; then, as they happen,
`HostUp { host, features }`, `HostDown { host }`, `SessionAdded { sid, conversation }`,
`SessionRemoved { sid }`, `SessionChanged { sid, conversation }` (a new conversation id is how
`/clear` is seen; focus moves too), `ClaudeInstalled { host }`, `ConfigChanged`, and `Tick` every
second. The tasks an addon spawns while it handles `Started` are its start:
it reads there what `options()` and `resolve()` answer from (its own files, off the executor).
The runtime has started once they all ended (`AddonRuntime::started`), and a launch waits for
that, at most 10 s, before it resolves, so a launch at a front-end's start (a `--plain` session,
the web's first spawn) never resolves on an addon that has not read its state yet.

**Rules for a runner.** `segments()`, `extension_rows()`, `options()`, `checklist()` and
`resolve()` return at once without I/O: they read a snapshot under a `std::sync::Mutex` that is
never held across an `await` or a call into the context. Every task the runner starts has a
bound, a timeout, a stale-result check (a request generation) and ends at shutdown (A10).

**Bounds** the core enforces:

| What | Bound |
|---|---|
| runtime `Tick` | every 1 s |
| `ctx.spawn` | ≤ 64 live tasks per addon, each 300 s |
| runtime → front-end queue | 256 messages; a full queue drops a `changed` (re-sent from a dirty flag) and the oldest notice or offer, with a warning |
| `ctx.request` | 170 s; bodies ≤ 512 KiB encoded both ways |
| daemon `DaemonOps::handle` | 150 s; ≤ 8 in flight per addon |
| `Ui::panel` | 10 s, then an error page |
| `Ui::action`, `Ui::row_action` | 15 s |
| launch `prepare` | 25 s for all addons together (the review clock stops while a checklist is open) |
| web config poll | 5 s (`ConfigChanged`) |

## Launches

A launch is a new session, a resume, a respawn (Alt+e, reset) or a recovery after a daemon
restart (`LaunchOp::{New, Resume, Respawn, Recover}`), from the TUI, `--plain` or the web
(`Frontend`). A terminal tab skips the addons. The core runs four steps, the same for every
front-end.

**1. Resolve.** The member's choice is a `LaunchSelect { pick, flags }`: `pick` is `Default`,
`Direct` or `Option { addon, option }`, from the command-line flags (each addon's `select`
parses its own `FlagDecl`s), the wizard, or a session's persisted `LaunchChoice { addon, option,
flags }` (`LaunchSelect::of`). Each addon's `resolve` answers:

- `Routing::Required(choice)`: this addon's option is in effect, chosen or as its configured
  default. At most one addon may answer `Required`; two is a launch error. Its choice is the
  launch's effective choice and is persisted with the session.
- `Routing::Optional`: the addon contributes if it can.
- `Routing::Off`: the addon sits this launch out.
- `Err(message)`: an unknown or unusable choice of this addon (a profile that no longer
  exists). The launch fails with the message; it never becomes a direct session.

A wizard never turns a pick its options lack into direct. A chosen option they do not list (a
`--proxy NAME` whose profile is gone, or one that vanished while the wizard was open) is shown
as unavailable and refused until it comes back or the member picks direct; `Default` without an
option marked default stays `Default`, for `resolve` to decide.

A front-end knows a session's routing as direct, a choice, or unknown. A session it knows only
from its daemon (another front-end started it, and its own state file has no entry) is unknown:
a respawn or recovery of it refuses with a notice and is never launched direct, and the state
file keeps it unknown (`state.json`'s `route_unknown`).

**2. Prepare.** Each addon that is not `Off` gets a `LaunchRequest { op, sid, host, cwd, choice,
routing, frontend, deferred }` and returns a `LaunchPart`. All addons share one 25 s budget. A
`Required` addon's error or timeout fails the launch and no `Spawn` is sent; an `Optional` one's
becomes a notice and the launch goes on without it. Inside its share, the addon itself separates
what it requires (the proxy env) from what is an enhancement (a plugin whose host has not
answered).

**3. Review.** `prepare` may return `LaunchError::NeedsReview { checklist, revision }`. The
front-end shows that checklist (queued like a confirm prompt) and the 25 s clock stops while it
is open. Enter saves it (`save_checklist(id, Save { revision, off })`) and prepares that addon
again; "skip" prepares it again with the checklist id in `deferred`, for this launch only; Esc
cancels the launch. Skip and Esc never reach the addon. A revision change while the checklist is
open reopens it: the front-end shows the new items, and a save of the old revision that the
addon refuses prepares the addon again, which asks for the review again (the refusal is a
notice). A respawn or a recovery never waits for a review. The web protocol v1 has no
checklists: there a review is skipped for that launch, with a notice.

Outside a launch, an addon asks for a first-sight review with `ctx.offer_checklist(id)`: the TUI
queues it until it is idle; the web ignores offers.

**4. Compose.** The core merges the parts, the one implementation for every front-end:

- `env`: in registry order; a later addon's key wins; no key twice. `LaunchPart.env` is a secret:
  it goes only into `SpawnSpec.env` over the daemon socket, never into `App`, a DTO, a log, a
  notice or `Debug` (`LaunchPart`'s `Debug` prints env keys only; C1).
- `env_remove`: names the daemon removes from the session's inherited env, besides claude's own
  session markers.
- `args`: appended in order. `LaunchArg::Optional { flag, value }` is dropped by the daemon when
  the host's claude does not list `flag` in `--help`.
- `lenses`: merged by name, first wins, into `SessionExtensions.lenses`.
- `instructions`: joined by a blank line, cut at 8158 bytes.
- `notices`: shown once the session starts.
- The whole `Spawn` is checked against the frame cap before it is sent.

A respawn gets env, `env_remove`, lenses and instructions but not args (the daemon's journal
keeps those).

**Flags.** `FlagDecl { name, value, help, label }` declares `--name` (with a `NAME` placeholder
when it takes a value). The core parses addon flags on `claudio …` and `claudio --plain …`. A
switch with a `label` is also a wizard checkbox.

**Persistence.** `state.json` (each `SavedSession`) and `web-state.json` (each `SessionMeta`)
keep a session's `launch`, its `LaunchChoice` (omitted for a direct one). Sessions saved before
addons carry only `proxy` and `first_party` (`LegacyChoice`). On load, `launch` wins when an
addon of this build has it; without `launch`, the core offers the legacy fields to each addon's
`adopt_legacy` (no `proxy`: direct). A choice this build cannot make (its addon is not
registered, or no addon adopts the legacy fields) is kept as it was saved and written back
unchanged, and the session's launches fail with "this session's launch option is not available
in this build": never direct (AC8). For one release (compat(v0.10)) the core also writes the
legacy fields an addon derives with `legacy(choice)` beside `launch`, so a rolled-back build
keeps every session's routing. A session known only from its daemon (started by another
front-end) stays unknown (`route_unknown`) whatever these say.

## Data, not widgets

An addon hands the front-ends data; the TUI and the page render it with one generic view each.
Blocks are plain text: the web renders them as text nodes, never markup; the TUI as spans.

| Type | Fields |
|---|---|
| `Segment` | `id`, `text`, `tone`, `priority` |
| `Tone` | `ok`, `warn`, `hot`, `muted`, `accent` |
| `PanelDecl` | `id`, `title`, `key: Option<PanelKey { action, default }>`, `windows` |
| `PanelRequest` | `id`, `page`, `window`, `sid` (view-local state: the TUI's popup and each browser tab keep their own) |
| `Panel` | `title`, `pages: [Page { title, blocks }]`, `actions: [PanelAction]`, `updated` |
| `Block` | `text { text, tone? }`, `key_values { items }`, `table { header, rows }`, `gauge { label, used, limit, resets }`, `bars { label, series }` |
| `PanelAction` | `id`, `label`, `key`, `confirm` (a confirm prompt's text, if any) |
| `Checklist` | `id`, `revision`, `title`, `intro`, `items: [ChecklistItem { id, label, detail, section, on }]` |
| `ChecklistAnswer` | `Save { revision, off }` or `Defer` |
| `ExtensionRow` | `name`, `description`, `default_on`, `refused`, `details`, `setup` (the checklist Enter opens) |
| `RowAction` | `Toggled(bool)` or `Refresh` |
| `Notice` | `text`, `level` (`info`, `warn`) |

**Segments** follow each session's status. A lower number comes first. The TUI draws
priorities 0-99 on status line 1, after the core's own, left to right, and 100-255 on line 2,
among the core's gauges (cpu and context 100, git 101, memory 102, conversation totals 140);
when the terminal is narrow, line 2 drops whole segments, the highest number (the least
important) first. Tones map to the bar's colours (ok green, warn yellow, hot red, muted dim,
accent green). The page shows them in the session's header in the same order.

**Panels** are bound to a key with `PanelDecl.key` (a keymap action name and its default key
spec, checked for collisions by the keymap; `[keys] <action>` overrides it). A panel's
`windows` are the time windows a request may name, the first one the default.

**Checklist items** with a `section` print a heading when the section changes.

**Extension rows** appear on the extensions screen beside the core's, after them, in registry
order. A row whose name is not `[a-z][a-z0-9-]{0,31}`, or is held by a core extension or an
earlier row, is listed refused and off (`core_extensions()` tells the addon which names the
core holds). A row is on when `[extensions] <row>` says so, else when `default_on`, and never
while refused. The core writes a toggle to `[extensions] <row> = true|false`, has the runtime
re-read `config.toml`, then tells the addon with `row_action(row, Toggled(on))`; the addon reads
it back with `ctx.extension_on(row)` and never writes `[extensions]` itself. The screen's refresh
key sends `row_action(row, Refresh)` once to every addon with rows (its first row). Enter on a
row with `setup` opens that checklist.

**Sanitising (C2).** Every string an addon hands a front-end (segment, panel, checklist,
extension row, launch option, notice) goes through `crate::addon::clean` in the runtime, never
in the addon: `text::sanitize`, terminal controls and newlines removed, each string cut at
4096 bytes on a character boundary, segments at 64 characters, a checklist at 256 items, and a
panel over 64 KiB encoded replaced by one error page.

## Daemon ops

An addon with `DaemonOps` runs code inside each host's daemon: `handle(op, body, ctx)` answers
`Msg::AddonOp { addon, op, body }` with `Msg::AddonReply { body }`, or an `Error`. The daemon
runs it on a task, off its request loop, at most 8 in flight per addon (the ninth is refused
`addon <name> busy`), under a 150 s timeout. Bodies are at most 512 KiB encoded, checked on both
sides. A body never carries a secret value; it names secrets, and the session gets their values
through its spawn env. The daemon logs an op as addon, op and body length, never the body.

`DaemonCtx` gives the op `claude_bin()`, the daemon's login-shell `path()`, `claude_flags()`
(the flags this claude's `--help` lists, probed once), `cache_dir()` (`<cache>/<addon>`, which
the addon creates 0700), `updating()`, the lock held while `claude update` or a `claude
plugin …` runs, so `~/.claude` has one writer, and `config(section)`: one of the addon's
declared `config.toml` sections on this host. The daemon reads the file (off the runtime, at
most 1 MiB) when it makes the context, once per op, per `on_usage_limit` and per
`on_claude_installed`, so an edit applies at the next call without a daemon restart; an
undeclared section, or a file that is missing, over 1 MiB or not TOML, reads as empty.
`on_claude_installed` is called after the daemon installs claude.

**Usage limits.** When a claude session's turn ends on a usage limit (claude's `StopFailure`
hook with `error` `rate_limit` and a message that starts with one of claude's own usage-limit
prefixes, `claude::hooks::USAGE_LIMIT_PREFIXES`), the session's daemon calls
`on_usage_limit(hit, ctx)` of each daemon-side addon in registry order, on a task off the
session, and takes the first answer that is not `LimitOutcome::NotMine` (the default).

```rust
fn on_usage_limit<'a>(&'a self, hit: UsageLimit<'a>, ctx: Arc<dyn DaemonCtx>)
    -> BoxFuture<'a, LimitOutcome>;

pub struct UsageLimit<'a> {
    pub conversation: &'a str,                              // claude's session id
    pub message: &'a str,                                   // sanitised, one line, ≤ 256 bytes
    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),   // the session's spawn env, by name
}
pub enum LimitOutcome {
    NotMine,                                 // not this addon's session, or switched off here
    Switched { from: String, to: String },   // the next message goes to another subscription
    Failed(String),                          // tried; why it did not work
}
```

`env` reads the session's spawn env, secrets included: an addon never logs a value, and
`UsageLimit` has no `Debug`. The whole round has 20 s (`daemon::Config::limit_attempt_timeout`);
past it the outcome is `Failed("timed out")`, and a future that panics is `Failed("addon
panicked")`. The core cleans the outcome's strings (one safe line, at most 120 bytes) and shows
them as session notices:
- `Failed(why)`: `usage limit: could not switch subscription: <why>`.
- `Switched { from, to }`: after a 1 s settle the daemon types `continue`, and 150 ms later
  Enter as a separate write, then shows `usage limit: switched <from> → <to>, continuing (n/3)`.
  It types only while the session still sits in the error of that hit, no newer hit came, and
  no client typed since its last submitted prompt (a front-end's focus and mouse reports do
  not count). Otherwise it types nothing more and shows `usage limit: switch to <to>
  scheduled; not continuing automatically`.
- `NotMine` from every addon: nothing.

One attempt runs per session at a time; a hit meanwhile starts none. After 3 automatic
continues in a row the next hit shows `usage limit: 3 automatic switches in a row; continue by
hand` and asks no addon; a turn that ends normally (`Stop`) starts the count over. The daemon
acts with no client attached, once per hit whatever the number of clients.

**Services.** `DaemonOps::service(config)` maps the addon's own config sections (`config` hands it
only those) to the argv of one long-running child the daemon keeps running: `Ok(Some(argv))`, the
words after the claudio binary; `Ok(None)` for none (the default); or `Err(msg)` when the
sections are invalid, `msg` naming the key. It is pure, no I/O. The daemon asks at its start and
whenever the addon calls `DaemonCtx::service_reload(force)`, which converges: stop, start, or
restart when the argv changed or `force`; a `Failed` service starts at once, its backoff reset,
even with the same argv. It resolves once the child is ready, has failed its first attempt, or is
off (≤ 25 s: a 3 s stop and a 20 s start). Reloads of one addon run one at a time; those queued
behind a running one run as one. `DaemonCtx::service_state()` reads it: `ServiceState::Off |
Starting | Running { pid, since, argv } | Failed { exit, signal, tail, retry_in }` (`since` in Unix
seconds; `tail` the child's last ≤ 4 KiB of stderr, sanitised, or why it did not start;
`retry_in` until the next start, `None` when only a reload starts it). The child's contract (ready
line, stdin, exit, logs, stop, mixed versions) is in docs/daemon-protocol.md, "Addon services". A
daemon playing the previous release runs no service.

**Mixed versions (B1).** A client sends an `AddonOp` only to a host whose `Welcome.features`
lists `addon:<name>`; `ctx.request` returns `AddonError::Missing` otherwise, and the addon goes on
without that host's share and says so once (the Vocdoni addon: "daemon on <host> is older:
plugins after `claudio daemon restart`"). An older daemon answers an unknown op with `unsupported
op`. A core-only daemon lists no addon features. The daemon protocol, its features and the mixed-version table are in
`docs/daemon-protocol.md`.

## The API addon

`addons/api/` (claudio-api): `-p`/`--print` emulation and the OpenAI-compatible `--api` server
(docs/print-and-api.md), with `Commands` only. `is_relay` takes the print path's hook relays
(`<Event> <port>`, and `__hook <Event>` unless it is the core's five-word manager relay);
`dispatch` runs `cli::parse` on whatever argv is left (so `--` keeps its meaning) and takes it
when it asks for `--api` or `-p`; `help()` is the wrapper's flag and env reference. It is
registered last, after every addon with words of its own. Its PTY sessions are outside launch
composition (tools disabled, no addon's share).

## The web addon

`addons/web/` (claudio-web; the server is `src/`, the page `web/`, embedded by `build.rs`):
`Commands` for `claudio web …` (docs/web.md, protocol docs/web-protocol.md). Its `Launch` is `Optional` for
every launch, from every front-end: `prepare` gives the `latex` tag lens (with the
`<latex>` instruction) unless `extension_on("latex")` is false, and the `mermaid` fence lens
(with the diagram instruction) unless `extension_on("diagram")` is false, so the daemon
extracts what the page renders. `[claude] latex` is an alias of `[extensions] latex` (the runtime
maps it; an explicit `[extensions] latex` wins). Its `Ui` lists the `latex` and `diagram` extension
rows (on by default); their toggles are the core's to write. A build without the web addon asks
claude for no tags.

Its `DaemonOps` make the web a daemon service (docs/web.md, "The web service"). It claims the
`[web]` config section, which only `claudio web` writes (through `app::config_edit`): `enabled`,
and one unit of `listen` (loopback) or `remote` (TLS) with `allow_hosts`. While `enabled = true`,
`service()` answers `web --service`, then `--listen=<addr>` or `--remote=<addr>`, then one
`--allow-host=<name>` per host (each value in its flag's word, so none can be read as another
flag), after checking the section as the flags are checked; else nothing, or an error naming the
key. Its ops:

| Op | Body | Does |
|---|---|---|
| `apply` | `{"force": bool}` | `service_reload(force)`: re-reads `[web]` and converges |
| `status` | `{}` | `service_state()` |

Both answer one object: `{"state":"off"}`, `{"state":"starting"}`,
`{"state":"running","pid":<u32>,"since":<unix seconds>,"argv":[…]}` or
`{"state":"failed","exit":<code|null>,"signal":<number|null>,"tail":"…","retry_in":<seconds|null>}`.
Unknown fields are ignored, and a `state` the CLI does not know means the daemon is newer than it.
Bodies carry no secret. The CLI sends an op only to a daemon whose `Welcome.features` list
`addon:web`, and prints the URL of the reply's `argv`, so it describes what runs.

## The Vocdoni addon

`addons/vocdoni/` (claudio-vocdoni): the claude-proxy client and the team plugins. It owns the `[proxy]` and
`[plugins]` sections and lists `addon:vocdoni` in a daemon's features.

**Commands.** `claudio vocdoni login [URL] [--name NAME] [--token] [--unverified] | status |
use NAME|none | logout [NAME]`, and `claudio proxy …` as an alias with the same subcommands.
`login` without a URL signs in again to the default profile's proxy, else to
`https://claude.vocdoni.net` (`DEFAULT_URL`); `--token` reads a pasted token (stdin when it is
not a terminal), never an argument. Under `--features diag`, `__remote-manifest` drives
`vocdoni/apply` on an ssh host for the remote tests.

**Daemon op `apply`.** Apply one plugin manifest on the host: what was the `Manifest` op of the
releases before addons, unchanged in behaviour.

```json
{"op": "addon_op", "addon": "vocdoni", "name": "apply",
 "body": {"name": "kb", "url": "https://…/kb.toml", "sha256": "<64 hex>",
          "secrets": ["KB_TOKEN"], "disabled": ["mcp:jina"]}}
```

`name` is a plugin name (`[a-z0-9-]{1,40}`), `url` https, `sha256` 64 hex, `secrets` at most 64
env names (names only, never values), `disabled` at most 256 item ids; an unknown field refuses
the request. The reply body is an `Applied`: `mcp_config`, `agents`, `instructions`, `env`,
`installed`, `present`, `skipped` (`{id, reason}`; a reason ending `(retrying in N s)` says when
an install is tried again) and `dropped`. A refusal is `Error "manifest <name>: …"`.

The manifest is downloaded once per hash into `<cache>/vocdoni/` (the addon's `cache_dir`,
0700) as `<name>.toml` with its `<name>.sha256`; a manifest kept by an older release under
`<cache>/plugins/` is downloaded again once. Installs run after the reply, under the
`updating()` lock, as `claude plugin … --json`; `on_claude_installed` forgets their failures.

**Usage limits.** Its `on_usage_limit` answers `NotMine` when the host's `config.toml` has
`[proxy] auto_switch = false` (default `true`, and any value but `true` counts as off; read
through `DaemonCtx::config`), and when the session's env lacks `ANTHROPIC_BASE_URL` or
`ANTHROPIC_AUTH_TOKEN` (a direct session).
Otherwise it asks that proxy to move the conversation, `POST /v1/claudio/session/switch`
within 15 s: 202 is `Switched` with the proxy's names (`current subscription` and `next
available` when it sends none), 409 is `Failed` with the proxy's reason (no other eligible
subscription, or a session bound to its account), 404 is `Failed("the proxy does not know this
session")`, and anything else `Failed` with the client's error. The token goes only to the
session's own proxy URL, never to a log or an outcome.

**Launch.** Flags `--proxy NAME`, `--no-proxy` and `--first-party` (the wizard checkbox
"First-party session"); one option per proxy profile (group "Proxy", the configured default
marked). The default profile, or a chosen one, is `Routing::Required`: a profile that does not
exist fails the launch, never direct, and so does a `config.toml` whose `[proxy]` cannot be read
or parsed (the launch says why); `--no-proxy` and an explicit direct pick still start. `prepare` builds the profile's env from its proxy config
(cached 5 min, else fetched within 5 s; unreachable means the built-in model defaults, still
through the proxy), removes `ANTHROPIC_API_KEY` (and `CLAUDE_CODE_USE_GATEWAY` for a first-party
session) from the inherited env, and adds the plugins' share: for a new session it asks the host
for the manifests it has not answered (≤ 20 s), then each plugin's `--agents`/`--mcp-config` as
optional args, its `[env]` and secrets (under the profile's env, which wins) and its
instructions. A respawn or recovery takes what is answered now.

**Review.** A plugin's setup is the checklist whose id is the plugin's name: one item per
manifest item (`plugin:<name>@<market>`, `mcp:<name>`, `agent:<name>`, `instructions`, `env`),
under its section's heading, on unless `[plugins.<name>] disabled` lists it (all on at first
sight); its revision is the manifest's sha256, so saving a revision since replaced is refused
`stale`. Saving writes `[plugins.<name>]` and asks every host again. A new session or a resume
whose profile has an enabled plugin without saved choices waits (within the budget) for the
addon's start, the profile's list and that manifest, then answers `NeedsReview`; one skipped
for this launch is left out, its secrets too, and so is one whose manifest failed or did not
arrive, with a notice. When a manifest arrives for a plugin never set up, the addon offers its
checklist once per run; putting the offer off (`Defer`) asks no host for it in this run, and a
launch still asks for its review. A host whose daemon lacks
`addon:vocdoni` gets the proxy env without plugins and a notice. Legacy `proxy`/`first_party`
fields are adopted as `{addon: "vocdoni", option: <profile>, flags: ["first-party"]?}`.

**Ui.** A session it routes gets the segments `⇄ proxy:<profile>` (10), its credential as the
proxy names it for the conversation, `work-max (max)` (20), `⇆ switched 3m ago` for ten
minutes after a switch (21), and the credential's windows `5h 37%` (110) and `7d 12%` (150),
toned by use (below 60 % ok, below 85 % warn, then hot). A focused proxied session's credential
is asked of the proxy every 30 s, and 5 s after its conversation changes. The panel `proxy`
("Proxy stats", key action `proxy_stats`, default `alt+s`, windows `24h`, `7d`, `30d`) shows
the session's profile, else the default, else the first: pages Overview, Models, Trends, Pool &
limits and Sessions, fetched as the page needs them (stats younger than 10 s are reused, the
catalogue once); without a profile, how to connect one. Its action `switch` (key `n`, asked
first) moves the session's conversation to another subscription; the profile and conversation
are the addon's own snapshot of the session, never the request's.

**Client side.** `state/` is the addon's pure plugin state machine and proxy config cache (A5,
A6): each profile's list (`plugins.json`, then the proxy every 15 min), each plugin's manifest,
what each host answered, and the member's `[plugins.<name>]` choices. `runner.rs` drives it from
the runtime's events and does its tasks through the `AddonCtx` (fetches, `vocdoni/apply` asks,
`edit_config("plugins", …)`). `rows.rs` lists one extension row per plugin of every profile's
list (its version and profile, its manifest, setup and per-host state as details; `setup` is its
checklist); a toggle is the plugin's on/off for new sessions, `Refresh` refetches every list and
manifest and asks every host again. The runner starts on the runtime's first event; the profiles
its sessions use (and a `--proxy` given on the command line) have their lists fetched.

## Config sections and reserved words

An addon reads and writes only the top-level `config.toml` tables it declares in
`config_sections()` (A3). `ctx.config(section)` reads one; `ctx.edit_config(section, edit)`
changes it: the edit gets the section as a `toml::Table`, and the core writes back only the
keys that changed, atomically, keeping every other line and comment, through one process-wide
lock shared with every other writer of `config.toml` (B3).

The core owns these sections: `ui`, `keys`, `update`, `claude`, `extensions`. No addon may
claim them.

The core's command words are reserved (no addon may be named after one, and the core runs them
before any addon's `dispatch`): `__probe`, `--slave`, `__hook`, `__context`, `--daemon`,
`upgrade`, `daemon`, `sessions`, `--plain`. Only a relay (`is_relay`) is answered before them. So are
its flags: `--plain`, `--daemon`, `--slave`, `-p`, `--print`, `--api`, `-h`, `--help`.

## Limits

| What | Limit |
|---|---|
| addon name, panel id | `[a-z][a-z0-9-]{0,31}` |
| launch flag | `--[a-z][a-z0-9-]*` |
| segment text | 64 characters |
| any one string of a panel, checklist, row, option or notice | 4096 bytes |
| panel | 64 KiB encoded |
| checklist | 256 items |
| `AddonOp` body | 512 KiB encoded, each way |
| `LimitOutcome` string | 120 bytes, one line |
| `on_usage_limit` round, every addon's answer included | 20 s |
| composed instructions | 8158 bytes |

Errors an addon sees from `ctx.request` (`AddonError`): `NotConnected(host)`, `Missing(host)` (the
daemon is older), `TooLarge`, `Timeout`, `Daemon(message)`.
