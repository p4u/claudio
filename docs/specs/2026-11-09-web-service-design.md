# `claudio web` as a daemon service: design

Status: draft   Class: architectural
Owner decisions so far:
- 2026-11-09: `claudio web` and `claudio web --remote` save their settings in `config.toml` and
  hand the server to the daemon; the terminal is not kept open.
- 2026-11-09: after a reboot the web comes back when the daemon next starts (the next `claudio`
  use of any kind); no systemd unit or launchd agent is installed.

## Problem

`claudio web` serves in the foreground (`addons/web/src/lib.rs` `run` → `serve`): it lives as
long as the terminal that started it. To keep the web up, a user has to leave a terminal (or a
tmux pane) open, and must run it again after every logout, crash or upgrade. The daemon
(`crates/core/src/daemon/`) already outlives terminals and is restarted onto new builds
(`freshness.rs`), but it knows nothing about the web, and A3 forbids the core to branch on an
addon's name.

## Goals

1. `claudio web [--listen … | --remote …] [--allow-host …]` validates its settings in the
   terminal, stores them in a `[web]` section of `config.toml`, has the local daemon start the
   server, prints the URL (and opens the browser as today), and exits.
2. While `[web] enabled = true`, every start of that host's daemon starts the web server, and the
   daemon restarts it if it crashes.
3. `claudio web stop` turns it off (stops it, `enabled = false`); `claudio web status` says
   whether it runs, where, and why not.
4. A new build takes over the web with the daemon: after a daemon restart onto a new binary the
   web runs that binary.
5. The daemon side is a generic addon capability (A3): the core supervises "an addon's service"
   and never names the web addon.
6. `claudio web --foreground` keeps today's behaviour exactly (the test suites and anyone who
   wants a foreground server use it).

## Non-goals

- Starting at boot or login (no systemd unit, launchd agent, linger).
- Hot reload of the listener when `config.toml` is hand-edited: a hand edit takes effect at
  the next `claudio web` or daemon start.
- Showing the web's state in the TUI or in `claudio daemon status`.
- More than one service per addon, or services for addons other than web (the API allows
  them; nothing else uses it in this work).
- Any change to the web's auth model, TLS, page, protocol (`docs/web-protocol.md`) or DTOs.

## Current state

- `claudio web` flags and their combination rules: `addons/web/src/lib.rs:56-190`
  (`WebArgs`, `parse_args`, `combination_ok`); one-shots `--set-password`, `--print-cert`
  (`one_shot`, `lib.rs:316`); `--print-url` and `--rotate-token` handled in `run`
  (`run`, `lib.rs:242-275`).
- Start-up prints the token URL on stdout (`announce`, `lib.rs:415`; for `--remote` the banner's
  stderr lines include `on this machine: https://…/#t=<token>`, `remote.rs:166`).
- `remote::prepare` (`remote.rs:190`) refuses to go on without a password, then loads or makes
  the certificate (`--rotate-cert`).
- The web hub connects the local daemon with `client::ensure_local_daemon` (`hub_io.rs:22`),
  which starts the daemon and can replace an older one (`client.rs:776`).
- `WebAddon` (`addons/web/src/addon.rs`) has `Commands`, `Launch` and `Ui`, no `DaemonOps`,
  and declares no config section.
- `DaemonOps` (`crates/core/src/addon/daemon.rs`) answers `AddonOp`s (≤ 8 in flight, 150 s) and
  gets `on_claude_installed`; there is no hook at daemon start or stop. `DaemonCtx` offers
  `claude_bin`, `path`, `claude_flags`, `cache_dir`, `updating`.
- The daemon's `Config` (`daemon/mod.rs:69`) already reads this host's `config.toml`
  (`skip_permissions`, `latex`) and knows its own binary (`Config::claudio`, the hook relay).
- Daemon shutdown: `Msg::Shutdown` (`server.rs:423`) notifies `Daemon::shutdown`; `daemon::run`
  then shuts the runtime down with a 1 s grace (`mod.rs:177-221`). A local daemon is started by
  `client::ensure_daemon` (`client.rs:712`: `setsid`, stderr to `<runtime>/daemon.log`).

## Design

### 1. Core: addon services (generic, `crates/core`)

`DaemonOps` gains one optional method that maps the addon's own config sections to the argv of
a long-running child, and `DaemonCtx` gains the means to re-ask it and read its state:

```rust
/// What the daemon should keep running for this addon: argv after the claudio binary,
/// from the addon's declared config sections; `None`: nothing. Pure, no I/O.
fn service(&self, config: &dyn Fn(&str) -> toml::Table) -> Option<Vec<String>> { None }

// DaemonCtx
/// Re-ask `service()` and converge: stop, start, or restart when the argv changed
/// (or `force`). Resolves once the child is ready, has failed, or is off (≤ 20 s).
fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState>;
fn service_state(&self) -> ServiceState;
```

`ServiceState` is `Off | Starting | Running { pid, since } | Failed { exit, tail, retry_in }`.
`tail` is the last ≤ 4 KiB of the service log, through `text::sanitize`. Names are the plan's to
fix; the behaviour below is not.

The daemon's **supervisor** (new module under `crates/core/src/daemon/`):

- At daemon start, after the socket listens, it asks every addon with `DaemonOps` for its
  `service()`, reading `config.toml` off the runtime and handing each addon only its declared
  sections.
- **Spawn:** `Config::claudio` with that argv, in its own process group. Env: the daemon's,
  with `PATH = Config::path` and `SESSION_MARKERS` removed. stdin is a pipe the daemon holds and
  never writes: the child exits on EOF, so a daemon killed with SIGKILL takes its service with it
  on Linux and macOS alike. stdout is a pipe for the ready line. stderr is
  `<runtime>/<addon>.log`, 0600, renamed to `<addon>.log.1` before a start when it is over
  1 MiB.
- **Ready:** the child writes `ready\n` on stdout once it serves; until then the state is
  `Starting`. If no ready line comes within 20 s, the child is killed and counts as failed.
- **Exit:** exit 0 means "not wanted" (state `Off`, no restart). Any other exit restarts it
  after a backoff of 1 s, doubling to at most 60 s; the backoff resets after 5 min of running.
- **Stop** (daemon shutdown, a reload to `None`, or a restart): close stdin, SIGTERM to the
  group, SIGKILL after 3 s. Daemon shutdown waits for it, inside the existing grace.
- **Reloads:** one at a time per addon. The service's state is per daemon, never journaled.

### 2. The web addon (`addons/web`)

- `WebAddon` declares the config section `web` and implements `DaemonOps`.
- `service()` maps `[web]` to `["web", "--service", <listener flags>]` when `enabled = true`,
  else `None`.
- Ops: `web/apply { force }` calls `service_reload(force)` and returns the state;
  `web/status` returns `service_state()`. Bodies carry no secret.

`[web]` (written only by `claudio web`, through `ctx.edit_config`/`app::config_edit`):

```toml
[web]
enabled = true
listen = "127.0.0.1:7337"         # loopback mode; absent with remote
remote = "0.0.0.0:7337"           # --remote mode (TLS, password); absent with listen
allow_hosts = ["claudio.example.org"]
```

Only listener settings are stored. One-shot flags (`--no-open`, `--print-url`,
`--rotate-token`, `--rotate-cert`, `--set-password`, `--print-cert`) never are. On read, `[web]`
goes through the same checks as the flags (`listen_addr`, `remote_addr`, `host_name_ok`,
`combination_ok`). An invalid section is a `Failed` state naming the key, never a server on an
address the flags would refuse.

**CLI** (`claudio web …`):

| Command | Effect |
|---|---|
| `claudio web [listener flags] [--allow-host …] [--no-open] [--rotate-token] [--rotate-cert]` | Parse and check as today. With `--remote`, check the password and load or make the certificate in the terminal (`remote::prepare`), so those errors show here. Listener flags given replace the stored listener settings; none given keeps the stored ones (or the defaults). Write `[web]` with `enabled = true`. Ensure the local daemon (`ensure_local_daemon`), then `web/apply { force: rotate-token or rotate-cert }`. On `Running`: print the token URL (as `--print-url` would), open the browser unless `--no-open`, exit 0. On `Failed`: print the error tail and the log path, exit 1 (the config stays enabled, and the daemon keeps retrying). |
| `claudio web stop` | `enabled = false`, `web/apply`; exit 0 when `Off`. |
| `claudio web status` | Enabled or not, the listener, the daemon's `web/status` (running since, or failed with the tail), the URL without the token. Exit 0 if running, 3 if not. |
| `claudio web --set-password` | As today, then `web/apply { force: true }` if `[web] enabled` and the daemon runs (a new password takes effect; browsers sign in again). |
| `claudio web --foreground [flags]` | Today's `claudio web [flags]`, unchanged; never reads or writes `[web]`. |
| `--print-url`, `--print-cert` | Unchanged. |
| `claudio web --service [listener flags]` | Internal; what the daemon runs. Serves like `--foreground`, but: never prints or logs the token (no `announce` URL, no `on this machine:` banner line), never opens a browser, connects the local daemon with plain `client::connect` plus reconnect (never `ensure_local_daemon`: the parent daemon is the authority, and a child must not replace it), writes `ready` on stdout after the listener is bound, and shuts down gracefully on stdin EOF as on SIGTERM. |

**Older daemon.** If the local daemon lists no `addon:web` (older and busy, so `freshness` left
it running), `claudio web` prints `the daemon is older: claudio daemon restart when idle, or
claudio web --foreground`. It exits 1 with `[web]` already saved: the next daemon start brings
the web up.

### Alternatives considered

| Option | Why not |
|---|---|
| Serve inside the daemon process | One crash or exploit in the network-facing server takes every terminal session down. A daemon kept running across an upgrade (busy) would keep an old web. The web is a client of the daemon, and would connect to itself. |
| systemd user unit / launchd agent | Platform-specific, and outside the daemon the owner asked for. Also the boot-time goal the owner ruled out. |
| `claudio web` daemonises itself (double-fork) | Nothing restarts it after a crash or an upgrade, it has no status, and it still dies at logout like any other process. |
| Child always started; it exits 0 when `[web]` is off | A process spawn at every daemon start on every host, and "off" is indistinguishable from "just exited". |
| Web addon spawns the child itself from an op handler | No hook at daemon start (so no autostart), and process supervision would live outside the core's process-group and shutdown handling. |

## Compatibility and rollout

- **Behaviour change:** a bare `claudio web` no longer stays in the foreground. A script or unit
  that ran `claudio web` as a long-running process must add `--foreground`. Release notes say so.
- **Mixed versions:** the new `addon:web` feature appears in `Welcome.features`.
  - New CLI, old daemon: `AddonError::Missing`, handled as above.
  - Old CLI, new daemon: never sends web ops; an old `claudio web` still serves in the foreground.
  - Old daemon, `[web]` written by a new CLI: an old daemon ignores the section (it reads only
    `[claude]`), and the old web build's `config::load` ignores unknown sections.
- **Two daemons** (PROTO bump, old and new side by side): only a daemon of this build or later
  runs services, so only the new one does.
- **Rollback:** `claudio web stop` (or deleting `[web]`), then the previous binary.
- Core-only build (A4): the supervisor compiles and runs with no service; nothing changes.

## Failure modes and security

- **Token never in a log.** In `--service` mode the token URL is printed nowhere: not on stdout
  (a pipe the daemon reads), not on stderr (`<addon>.log`). The CLI prints it, from the 0600
  token file, as `--print-url` does today. A test greps both logs for the token.
- **Listener rules unchanged:** `[web]` passes the same validation as the flags (non-loopback
  only with `remote`, TLS and a password). A hand-edited `listen = "0.0.0.0:…"` fails with the
  flag's message and never binds.
- **Password missing at service start** (deleted after enabling): the child fails as the
  foreground one does today (`remote::prepare`, before binding). The state is `Failed` with
  that message, retried with backoff.
- **Port taken** (a `--foreground` web on the same port): `Failed` with the bind error, retried
  with backoff; `claudio web` reports it.
- **Orphans:** stdin EOF ends the child when the daemon dies, however it dies. Stop kills the
  child's whole process group.
- **Crash loops** are bounded by the 60 s backoff cap and the log's 1 MiB rotation.
- **Env:** the child gets the daemon's env minus `SESSION_MARKERS`. Secrets never go in its
  argv: the token, password and certificate stay in their 0600 files.
- **Concurrency:** two `claudio web` runs at once serialise on the `config.toml` lock and on the
  per-addon reload; the last write wins.

## Acceptance criteria

- [ ] AC1: `[web]` ↔ argv mapping and the "flags replace, none keep" merge are unit-tested,
  including an invalid section → error naming the key (web addon unit tests).
- [ ] AC2: the supervisor, with a `TestAddon` service (a script child), is unit/daemon-tested
  for: started at daemon start; `ready` → `Running`; crash → restart with backoff; exit 0 →
  `Off`, no restart; reload with the same argv is a no-op, with a changed argv or `force`
  restarts; daemon shutdown stops the group; the child exits when the daemon is SIGKILLed
  (stdin EOF); no ready line in 20 s → `Failed`.
- [ ] AC3: `addons/web/tests/web.rs`, isolated env, `claudio web --listen 127.0.0.1:<port>
  --no-open` exits 0, prints the token URL, and that URL logs in. `claudio web status` exits 0.
  `claudio daemon restart` → the web is back. `claudio web stop` → the port is closed, `enabled =
  false`, `status` exits 3.
- [ ] AC4: `claudio web --remote 127.0.0.1:<port> --no-open` with the fixture password serves
  TLS through the daemon; without a password it exits non-zero and leaves `config.toml`
  untouched.
- [ ] AC5: after AC3 and AC4, neither `<runtime>/web.log` nor `daemon.log` contains the token.
- [ ] AC6: every existing web suite (Rust and Playwright) passes with `--foreground`, its only
  change. `--foreground` output is byte-identical to today's start-up.
- [ ] AC7: against a daemon playing the previous release (`old_peer`), `claudio web` prints the
  older-daemon message, exits 1, and leaves `[web]` saved.
- [ ] AC8: `make check` and `make matrix` pass (A2–A4: the core never names `web`; core-only
  build has no service).

## Constraints

- ACCEPTANCE.md throughout. A2/A3: the supervisor is generic and the core never names the web
  addon. A4: the core-only build stays working. Secrets: never in argv, logs or `AddonOp` bodies.
- `app.rs` stays pure; no TUI change.
- `docs/web-protocol.md` does not change. `docs/web.md` (usage, `--remote` steps, "keeps the old
  password until it restarts"), `docs/addons.md` ("Daemon ops", config sections) and
  `docs/daemon-protocol.md` (features) are updated in the same branch. So are `CLAUDE.md`'s web
  and daemon lines.
- Tests use fake claude and the isolated env of the existing web suites; manager suites stay
  single-threaded.
- Conventional commits `type(scope): lowercase summary`, no AI attribution or co-author trailers.

## Open questions

None.
