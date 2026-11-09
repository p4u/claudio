# Addons

An addon adds a feature to claudio without the core knowing it by name: a command, a way to
route a session (a proxy), status-bar segments, a panel, a checklist, extension rows, or work
inside each host's daemon. The core stays generic (ACCEPTANCE.md A1, A3); everything else is an
addon. This page is the addon API, `crate::addon` (`src/addon/`). It describes the contract the
core keeps with an addon and the limits it enforces.

- [Capabilities](#capabilities)
- [The registry](#the-registry)
- [Execution model](#execution-model)
- [Launches](#launches)
- [Data, not widgets](#data-not-widgets)
- [Daemon ops](#daemon-ops)
- [Config sections and reserved words](#config-sections-and-reserved-words)
- [Limits](#limits)

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

**Commands.** `is_relay(argv)` is checked before anything else, so a hook relay (a copy of the
binary that claude runs as a hook) is answered without parsing the command line. `dispatch(argv,
addons)` runs after the core's own words, in registry order, before passthrough to `claude`;
`None` means "not mine". `help()` is appended to `claudio --help`.

**Launch**, **Ui** and **DaemonOps** are described below.

## The registry

`Addons::new(list)` builds the immutable set this binary runs with; `main` builds it once
(`crate::addons::all()`) and hands an `Arc<Addons>` to every entry point. Registration is refused,
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
`AddonCtx`, which the core implements once (`src/app/addons/`) for both front-ends: the TUI
runner and the web hub. Each addon gets its own context (`ctx.addon()` is its name), so every
request, config read and edit is attributed to it.

| `AddonCtx` | |
|---|---|
| `spawn(task)` | a task on the front-end's runtime: at most 64 live per addon, each under 300 s, aborted at shutdown |
| `http()` | the one shared HTTP client |
| `request(host, op, body)` | an `AddonOp` to that host's daemon; `Missing` unless its features list `addon:<name>`; 170 s |
| `host_has(host)` | that host is connected and lists `addon:<name>` |
| `sessions()`, `hosts()` | snapshots (`SessionSnap`, `HostSnap`) |
| `config(section)` | a declared section of `config.toml` (empty when undeclared) |
| `edit_config(section, edit)` | an atomic, minimal edit of a declared section (below) |
| `extension_on(name)` | `[extensions] <name>` as `config.toml` has it, or `None` |
| `notify(text)` | a notice in the front-end |
| `changed()` | segments, rows, options or an open panel changed: front-ends re-read them |
| `offer_checklist(id)` | ask front-ends to show a checklist when idle |

**Events.** `on_event` is called on the front-end's runtime and must not block; it hands work to
`ctx.spawn`. The order: `Started` first, once the snapshots are filled; then, as they happen,
`HostUp { host, features }`, `HostDown { host }`, `SessionAdded { sid, conversation }`,
`SessionRemoved { sid }`, `SessionChanged { sid, conversation }` (a new conversation id is how
`/clear` is seen; focus moves too), `ClaudeInstalled { host }`, `ConfigChanged`, and `Tick` every
second.

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
open reopens it. A respawn or a recovery never waits for a review.

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

**Legacy persistence.** Sessions saved before addons carry `proxy` and `first_party`
(`LegacyChoice`). The core offers them to each addon's `adopt_legacy`; one that no addon adopts
fails the launch, never direct. For one release the core also writes the legacy fields an addon
derives with `legacy(choice)`, so a rolled-back build keeps every session's routing.

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
| `Notice` | `text`, `level` (`info`, `warn`, `error`) |

**Segments** follow each session's status. The TUI draws priorities 0-99 on status line 1, left
to right, and 100-255 on line 2, where the lowest priorities are dropped first when the
terminal is narrow. The page shows them in the session's header in the same order.

**Panels** are bound to a key with `PanelDecl.key` (a keymap action name and its default key
spec, checked for collisions by the keymap; `[keys] <action>` overrides it). A panel's
`windows` are the time windows a request may name, the first one the default.

**Checklist items** with a `section` print a heading when the section changes.

**Extension rows** appear on the extensions screen beside the core's. The core writes a toggle
to `[extensions] <row> = true|false` and tells the addon with `row_action(row, Toggled(on))`; the
addon reads it back with `ctx.extension_on(row)` and never writes `[extensions]` itself.

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
the addon creates 0700), and `updating()`, the lock held while `claude update` or a `claude
plugin …` runs, so `~/.claude` has one writer. `on_claude_installed` is called after the daemon
installs claude.

**Mixed versions (B1).** A client sends an `AddonOp` only to a host whose `Welcome.features`
lists `addon:<name>`; `ctx.request` returns `AddonError::Missing` otherwise, and the addon goes on
without that host's share and says so once ("daemon too old: run `claudio daemon restart`"). An
older daemon answers an unknown op with `unsupported op`. A core-only daemon lists no addon
features. The daemon protocol, its features and the mixed-version table are in
`docs/daemon-protocol.md`.

## Config sections and reserved words

An addon reads and writes only the top-level `config.toml` tables it declares in
`config_sections()` (A3). `ctx.config(section)` reads one; `ctx.edit_config(section, edit)`
changes it: the edit gets the section as a `toml::Table`, and the core writes back only the
keys that changed, atomically, keeping every other line and comment, through one process-wide
lock shared with every other writer of `config.toml` (B3).

The core owns these sections: `ui`, `keys`, `update`, `claude`, `extensions`. No addon may
claim them.

The core's command words are reserved and are never dispatched to an addon: `__probe`,
`--slave`, `__hook`, `__context`, `--daemon`, `upgrade`, `daemon`, `sessions`, `--plain`. So are
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
| composed instructions | 8158 bytes |

Errors an addon sees from `ctx.request` (`AddonError`): `NotConnected(host)`, `Missing(host)` (the
daemon is older), `TooLarge`, `Timeout`, `Daemon(message)`.
