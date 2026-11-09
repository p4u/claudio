# `claudio web` as a daemon service: design

Status: audited (2026-11-09; awaiting owner approval)   Class: architectural
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
   server, prints what a foreground start prints today (and opens the browser as today), and
   exits.
2. While `[web] enabled = true`, every start of that host's daemon starts the web server, and the
   daemon restarts it whenever it exits unasked.
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
  (`lib.rs:242-275`).
- Start-up prints one URL on stdout: the token URL, or with `--remote` the public URL
  (`announce`, `lib.rs:415`). With `--remote` the banner's stderr lines carry the certificate
  fingerprint, the names it covers and `on this machine: https://…/#t=<token>`
  (`remote.rs:143-176`).
- `remote::prepare` (`remote.rs:190`) refuses to go on without a password, then loads or makes
  the certificate (`--rotate-cert`).
- The web logs every refused login at info, the limiter's 429 included (`login.rs:45-50`).
- The web hub connects the local daemon with `client::ensure_local_daemon` (`hub_io.rs:22`),
  which starts the daemon and can replace an older one (`client.rs:776`).
- `WebAddon` (`addons/web/src/addon.rs`) has `Commands`, `Launch` and `Ui`, no `DaemonOps`,
  and declares no config section. `web` is free to claim (`addon/registry.rs:10`).
- `DaemonOps` (`crates/core/src/addon/daemon.rs`) answers `AddonOp`s (≤ 8 in flight, 150 s) and
  gets `on_claude_installed`; there is no hook at daemon start or stop. `DaemonCtx` offers
  `claude_bin`, `path`, `claude_flags`, `cache_dir`, `updating`.
- The daemon's `Config` (`daemon/mod.rs:69`) reads `config.toml` once in `from_env`
  (`mod.rs:113`, through the global `paths::config_file()`; no path field) and knows its own
  binary path (`Config::claudio`, `current_exe()` at start, `mod.rs:116`). The file at that
  path is replaced under a running daemon by `claudio upgrade` and `make install`/`dev`.
- Daemon shutdown: `Msg::Shutdown` (`server.rs:423`) notifies `Daemon::shutdown`; `daemon::run`
  then calls `runtime.shutdown_timeout(1 s)` (blocking tasks only) and drops the startup lock
  (`mod.rs:177-225`). The daemon has no SIGTERM handler; `freshness` and `daemon stop` fall back
  to killing it by pid. A local daemon is started by `client::ensure_daemon` (`client.rs:712`:
  `setsid`, stderr to `<runtime>/daemon.log`), which waits 5 s for a predecessor's lock.

## Design

### 1. Core: addon services (generic, `crates/core`)

`DaemonOps` gains one optional method that maps the addon's own config sections to the argv of
a long-running child, and `DaemonCtx` gains the means to re-ask it and read its state:

```rust
/// What the daemon should keep running for this addon: argv after the claudio binary,
/// from the addon's declared config sections. `Ok(None)`: nothing. `Err(msg)`: the
/// sections are invalid; `msg` names the key. Pure, no I/O.
fn service(&self, config: &dyn Fn(&str) -> toml::Table) -> Result<Option<Vec<String>>, String> {
    Ok(None)
}

// DaemonCtx
/// Re-ask `service()` and converge: stop, start, or restart when the argv changed (or
/// `force`). A service that is `Failed` is started now and its backoff reset, even with
/// the same argv. Resolves once the child is ready, has failed its first attempt, or is
/// off (≤ 25 s: a 3 s stop and a 20 s start). A second reload for the same addon waits for
/// the first; reloads that queue behind it run as one.
fn service_reload(&self, force: bool) -> BoxFuture<'static, ServiceState>;
fn service_state(&self) -> ServiceState;
```

`ServiceState` is `Off | Starting | Running { pid, since, argv } | Failed { exit, signal, tail,
retry_in }`. `tail` is the last ≤ 4 KiB the child wrote to stderr since this start (held in
memory by the supervisor, below), through `text::sanitize`. `Err(msg)` from `service()` gives
`Failed { exit: None, signal: None, tail: msg, retry_in: None }` (`msg` cleaned and capped at
4096 bytes like any addon string). Nothing is spawned and nothing is retried until the next
reload or daemon start. A `config.toml` that cannot be read or is not TOML is the same failure
for every addon ("config.toml does not parse"), never a service built from defaults.

The daemon's **supervisor** (new module under `crates/core/src/daemon/`, which names no addon):

- `daemon::Config` gains `config_file: Option<PathBuf>`: `paths::config_file()` in `from_env`,
  `None` in `TestDaemon::config`. The supervisor reads only that path, in `spawn_blocking`, at
  daemon start (after the socket listens) and at every reload, and hands each addon with
  `DaemonOps` only its declared sections. `None` means no addon has a service. A daemon
  playing the previous release (`Config::old_peer`) runs no service.
- **Spawn:** `Config::claudio` with that argv, in its own process group. Env: the daemon's,
  with `PATH = Config::path` and `SESSION_MARKERS` removed.
  - stdin is a pipe the daemon holds and never writes, so the child sees its owner's death
    (C3 amended, below).
  - stdout is a pipe for the ready line.
  - stderr is a pipe too: a daemon task copies it into `<runtime>/<addon>.log` (0600), renaming
    the file to `<addon>.log.1` whenever a write would take it past 1 MiB. It also keeps the
    last 4 KiB of this start in memory for `tail`. An addon's logs never exceed 2 MiB, however
    much the child writes (a `--remote` listener logs every refused login, so the size must not
    depend on what remote peers send).
- **Ready:** the child writes a line starting with `ready` on stdout once it serves; until then
  the state is `Starting`. The daemon reads at most 4 KiB waiting for it (more counts as a
  failed start). Afterwards it keeps reading and discarding stdout until EOF and never closes
  its end while the child runs. No ready line within 20 s: the child is stopped and counts as
  failed.
- **Exit:** an exit the supervisor did not ask for is a failure whatever its status (0
  included). The service restarts after a backoff of 1 s, doubling to at most 60 s; the backoff
  resets after 5 min of running. `Off` is reached only when `service()` returns `Ok(None)`.
- **Stop** (daemon shutdown, a reload to `None`, or a restart): close stdin, SIGTERM to the
  group, SIGKILL to the group after 3 s, then reap.
  - At daemon shutdown every service is stopped in parallel, after `serve()` has returned and
    before `runtime.shutdown_timeout(SHUTDOWN_GRACE)` (`daemon/mod.rs:219`). The startup lock
    is released only after that, so a successor's service never starts beside it.
  - `SHUTDOWN_GRACE` (1 s, blocking tasks only) does not bound the stop: the worst case is
    3 s + 1 s. To keep a margin, the 5 s `wait_for_lock` in `client::ensure_daemon` and
    `bridge::ensure_daemon_detached` becomes 10 s (`freshness::EXIT_WAIT`).
- **A daemon killed by a signal** runs no stop. That covers the pid fallbacks of `freshness`
  and `daemon stop`, a logout and the test harness. The child then ends by stdin EOF (its
  watchdog, §2). A successor's first start may find the port still held; that is a `Failed`
  start retried after 1 s, not an error to report as final.
- **Reloads** are serialised per addon as `service_reload` says. The service's state is per
  daemon, never journaled.

### 2. The web addon (`addons/web`)

- `WebAddon` declares the config section `web` and implements `DaemonOps`.
- `service()` runs the flag checks itself (`listen_addr`, `remote_addr`, `host_name_ok`,
  `combination_ok`, and the value types). It returns `Err` naming the key, or
  `["web", "--service", "--listen=<addr>" | "--remote=<addr>", "--allow-host=<name>"…]` when
  `enabled = true`, else `Ok(None)`. Every value is in the same word as its flag, so no value
  can be read as another flag. `--service` refuses every one-shot flag.
- Ops (wire names `apply` and `status` of addon `web`; documented in `docs/addons.md`, "The web
  addon", before the code):
  - `apply` body `{"force": bool}` calls `service_reload(force)`; `status` body `{}` calls
    `service_state()`.
  - Both reply with one object: `{"state":"off"}` | `{"state":"starting"}` |
    `{"state":"running","pid":u32,"since":<unix seconds>,"argv":[…]}` |
    `{"state":"failed","exit":<code|null>,"signal":<number|null>,"tail":"…","retry_in":<seconds|null>}`.
  - Unknown fields are ignored; an unknown `state` is reported as "the daemon is newer than
    this claudio". Bodies carry no secret. The CLI builds the URL it prints from the reply's
    `argv`, so it always describes what runs.

`[web]` (written only by `claudio web`, through `app::config_edit`):

```toml
[web]
enabled = true
listen = "127.0.0.1:7337"         # loopback mode; absent with remote
remote = "0.0.0.0:7337"           # --remote mode (TLS, password); absent with listen
allow_hosts = ["claudio.example.org"]
```

`listen`/`remote` and `allow_hosts` are stored as one unit. One-shot flags (`--no-open`,
`--print-url`, `--rotate-token`, `--rotate-cert`, `--set-password`, `--print-cert`) never are.

**CLI** (`claudio web …`):

| Command | Effect |
|---|---|
| `claudio web [--listen …\|--remote …] [--allow-host …] [--no-open] [--rotate-token] [--rotate-cert]` | Parse and check as today. The token is created or rotated in the terminal first, before `[web]` is written and before any op; the service only ever loads it. If any of `--listen`, `--remote` or `--allow-host` is given, the command line replaces the whole stored unit (what is not given takes its default: `127.0.0.1:7337`, no allowed hosts). If none is given, the stored unit is used unchanged (or the defaults when there is none). `combination_ok` runs on the result; `--rotate-cert` needs the result to be a `remote` one. With `remote`, check the password and load or make the certificate in the terminal (`remote::prepare`), so those errors show here. Write `[web]` with `enabled = true`. Ensure the local daemon (`ensure_local_daemon`), then send `apply { force: rotate-token or rotate-cert }`. On `Running`: print what a foreground start prints, from the terminal's own `remote::prepare`. Without `remote` that is `claudio web: <token URL>` on stdout. With it, the banner's stderr lines (certificate SHA-256, covered and missing names, the listening note, `on this machine: <token URL>`, the password hint) and `claudio web: <public URL>` on stdout. Then open the browser unless `--no-open`, and exit 0. On `Failed`: print the tail and the log path, exit 1 (the config stays enabled, and the daemon keeps retrying). A `config.toml` that `config_edit` refuses: exit 1, nothing started. |
| `claudio web stop` | `enabled = false`, then `apply`; exit 0 when `Off`. |
| `claudio web status` | Enabled or not, the listener, the daemon's `status` (running since, or failed with the tail), the URL without the token, and with `remote` the certificate's SHA-256 fingerprint. Exit 0 if running, 3 if not. |
| `claudio web --set-password` | As today, then `apply { force: true }` if `[web] enabled` and the daemon lists `addon:web` (a new password takes effect; browsers sign in again). |
| `claudio web --foreground [flags]` | Today's `claudio web [flags]`, unchanged; never reads or writes `[web]`. |
| `--print-url` | The URL of the stored `[web]` settings when no listener flag is given (else of the flags, as today); never writes `[web]`. With `--rotate-token` it also sends `apply { force: true }` when `[web] enabled` and the daemon lists `addon:web`. |
| `--print-cert` | Unchanged. |
| `claudio web --service [--listen=…\|--remote=…] [--allow-host=…]…` | Internal; what the daemon runs. Serves like `--foreground`, but: never prints or logs the token (no `announce` URL, no `on this machine:` banner line); never opens a browser; connects the local daemon with plain `client::connect` plus reconnect (never `ensure_local_daemon`: the parent daemon is the authority, and a child must not start or replace it); writes `ready` on stdout after the listener is bound; and shuts down gracefully on stdin EOF as on SIGTERM. A dedicated OS thread (not a runtime task) reads stdin; at EOF it starts the graceful shutdown and, 3 s later, exits the process if it still runs, so a blocked runtime or an open connection cannot leave an orphan on the port. |

`stop`, `status`, `--set-password` and `--print-url` reach the daemon with plain
`client::connect`: they never start or replace one. With no daemon running, `stop` writes
`enabled = false` and exits 0, and `status` prints `daemon: not running` and exits 3. With a
daemon that has no `addon:web`, `stop` exits 0 (nothing of ours runs there) and `status` exits 3.

**Daemon without the service.** The CLI decides on `Welcome.features` only: without `addon:web`
it sends no op, and it never matches on an error's text (`unsupported op` from a pre-addon
daemon, `no such addon web` from the last release). `claudio web` then prints `the daemon has
no web service (older, or built without the web addon): claudio daemon restart when idle, or
claudio web --foreground`. It exits 1 with `[web]` already saved: the next daemon start brings
the web up.

### Alternatives considered

| Option | Why not |
|---|---|
| Serve inside the daemon process | One crash or exploit in the network-facing server takes every terminal session down. The web is a client of the daemon, and would connect to itself. |
| systemd user unit / launchd agent | Platform-specific, and outside the daemon the owner asked for. Also the boot-time goal the owner ruled out. |
| `claudio web` daemonises itself (double-fork) | Nothing restarts it after a crash or an upgrade, and it has no status. (It would survive a logout exactly as the daemon does: both are `setsid` processes.) |
| Child always started; it exits 0 when `[web]` is off | A process spawn at every daemon start on every host, and "off" is indistinguishable from "just exited". |
| Web addon spawns the child itself from an op handler | No hook at daemon start (so no autostart), and process supervision would live outside the core's process-group and shutdown handling. |
| Supervisor rotates the log on a timer (copy + truncate) | Loses lines, bounds size only per interval, and the error tail must be read back from a file; piping stderr through the daemon bounds it exactly. |

## Compatibility and rollout

- **Behaviour change:** a bare `claudio web` no longer stays in the foreground. A script or unit
  that ran `claudio web` as a long-running process must add `--foreground`. Browsers are
  signed out whenever the service restarts, which now includes every daemon restart (an
  upgrade, `claudio daemon restart`, an outdated idle daemon being replaced). `claudio web`
  prints the local link again. Release notes and `docs/web.md` say so.
- **Mixed versions:** the new `addon:web` feature appears in `Welcome.features`.
  - New CLI, daemon without `addon:web`: handled as above.
  - Old CLI, new daemon: never sends web ops; an old `claudio web` still serves in the foreground.
  - Old daemon, `[web]` written by a new CLI: an old daemon ignores the section (it reads only
    `[claude]`), and the old web build's `config::load` ignores unknown sections.
  - A daemon playing the previous release (`Config::old_peer`) runs no service and ignores
    `[web]`, like the release it plays. `docs/daemon-protocol.md`, "Mixed versions", says so in
    a commit before the code (B2).
- **The service contract is a B1 interface.** The daemon runs the binary at `Config::claudio`,
  which an upgrade replaces under a running daemon, so a daemon and its service child can be
  one release apart in either direction.
  - For one release, these may only change compatibly: `web --service` with
    `--listen=`/`--remote=`/`--allow-host=`; the first stdout line `ready` (the daemon accepts
    any first line that starts with `ready`); stdin EOF means stop; any unrequested exit is a
    failure.
  - A child that does not know `--service` (a downgraded binary) exits 2: an ordinary `Failed`,
    retried at the backoff cap, with the usage text as its tail.
  - A daemon left running across an upgrade keeps its old web child until that child is
    restarted; a restart then runs the new binary under the old daemon (new client, old
    daemon: B1).
- **Two daemons** on one host share `config.toml` (two `PROTO` versions side by side, or two
  runtime dirs: `XDG_RUNTIME_DIR` set in one login and not in another). The first service to
  bind keeps the port; the other is `Failed` ("address in use"), retried at the 60 s cap, and
  `claudio web status` in that environment says so. In this release there is one `PROTO`; the
  release that bumps it decides which daemon runs services.
- **Rollback:** `claudio web stop`, install the previous binary, `claudio daemon restart`. A
  newer daemon is never replaced by an older build (B4), and would keep starting a binary without
  `--service`. `[web]` may stay: the previous release ignores it.
- Core-only build (A4): the supervisor compiles and runs with no service; nothing changes.

## Failure modes and security

- **Token never in a log.** In `--service` mode the token URL is printed nowhere: not on stdout
  (a pipe the daemon reads), not on stderr (`<addon>.log`). The CLI prints it, from the 0600
  token file, as `--print-url` does today. A test greps both logs for the token.
- **Listener rules unchanged:** `[web]` passes the same validation as the flags (non-loopback
  only with `remote`, TLS and a password). A hand-edited `listen = "0.0.0.0:…"` is `Failed`
  with the flag's message and never binds.
- **Password missing at service start** (deleted after enabling): the child fails as the
  foreground one does today (`remote::prepare`, before binding). The state is `Failed` with
  that message, retried with backoff.
- **Port taken** (a `--foreground` web on the same port): `Failed` with the bind error, retried
  with backoff; `claudio web` reports it.
- **Orphans:** stdin EOF ends the child within 3 s when the daemon dies, however it dies (the
  child's own watchdog thread, which does not depend on the runtime). Stop kills the child's
  whole process group. Programs the child started in their own group (the extension host's) are
  stopped by its graceful shutdown; after a SIGKILL they end on their own stdin EOF.
- **Log growth** is bounded at 2 MiB per addon while the child runs, not only across restarts.
  Crash loops are bounded by the 60 s backoff cap.
- **Env:** the child gets the daemon's env minus `SESSION_MARKERS`, with `PATH = Config::path`.
  That is the env of whatever started the daemon, not of the terminal that ran `claudio web`.
  So the web's SSH hosts use the daemon's `SSH_AUTH_SOCK`, which may be stale or absent when the
  daemon was started over ssh or by `systemd-run`. `CLAUDIO_*` variables set in the terminal do
  not reach it either. `docs/web.md` says so and names the fix (`claudio daemon restart` from a
  shell with the wanted env, or `--foreground`). Secrets never go in its argv: the token,
  password and certificate stay in their 0600 files.
- **Concurrency:** two `claudio web` runs at once serialise on the `config.toml` lock and on the
  per-addon reload; the last write wins, and each prints the URL of the argv its reply says runs.

## Acceptance criteria

- [ ] AC1: `[web]` ↔ argv mapping and the merge (any of `--listen`/`--remote`/`--allow-host`
  replaces the stored unit, hosts included; none keeps it) are unit-tested, including an invalid
  section → error naming the key (web addon unit tests).
- [ ] AC2: the supervisor, with a `TestAddon` service (a script child), is unit/daemon-tested
  for each of the following:
  - started at daemon start; `ready` → `Running`;
  - a crash and an unrequested exit 0 both restart with backoff; a reload to `None` → `Off`,
    no restart;
  - a reload with the same argv is a no-op; a changed argv or `force` restarts; a reload while
    `Failed` with the same argv starts the child at once;
  - daemon shutdown stops the group; the child exits when the daemon is SIGKILLed (stdin EOF);
    no ready line in 20 s → `Failed`;
  - a child that writes 3 MiB to stderr while it stays up leaves at most 2 MiB in `<addon>.log`
    + `<addon>.log.1`;
  - the child's argv[0] is the daemon's `Config::claudio`.
- [ ] AC3: in `addons/web/tests/web.rs` with the isolated env:
  - `claudio web --listen 127.0.0.1:<port> --no-open` exits 0, prints the token URL, and that
    URL logs in; `claudio web status` exits 0;
  - after `claudio daemon restart` the web is back;
  - `claudio daemon restart` also succeeds when the service child ignores SIGTERM (it is
    SIGKILLed after 3 s and the new daemon still starts);
  - after `claudio web stop` the port is closed, `enabled = false`, and `status` exits 3.
- [ ] AC4: `claudio web --remote 127.0.0.1:<port> --no-open` with the fixture password serves
  TLS through the daemon and prints the fingerprint banner; without a password it exits
  non-zero and leaves `config.toml` untouched.
- [ ] AC5: after AC3 and AC4, neither `<runtime>/web.log` nor `daemon.log` contains the token.
- [ ] AC6: every existing web suite (Rust and Playwright) passes with `--foreground`, its only
  change. `--foreground` output is byte-identical to today's start-up.
- [ ] AC7: against a daemon playing the previous release (`old_peer`), `claudio web` prints the
  no-web-service message, exits 1, leaves `[web]` saved, sends no `AddonOp`, and nothing listens
  on the port. The same daemon restarted without `old_peer` serves it.
- [ ] AC8: `make check` and `make matrix` pass (A2 and the core-only build are checked there; A3
  is reviewed, see AC10).
- [ ] AC9: with the web running through the daemon, after a SIGKILL of the service pid
  `claudio web status` exits 0 with a different pid within 5 s.
- [ ] AC10: `grep -rn '"web"' crates/core/src/daemon crates/core/src/addon` matches only tests
  and doc examples; the supervisor module contains no addon name.
- [ ] AC11: each test of AC3, AC4, AC7 and AC9 ends with no process of its own left: it waits
  for the service pid to be gone before its dirs are removed.

## Constraints

- ACCEPTANCE.md throughout. A2/A3: the supervisor is generic and the core never names the web
  addon. A4: the core-only build stays working. Secrets: never in argv, logs or `AddonOp` bodies.
- C3 is amended in its own commit, before the supervisor: "…and stdin `/dev/null` unless
  interactive; a supervised service's stdin is a pipe its owner holds and never writes, so the
  child sees its owner's death."
- Commit order:
  1. `docs/daemon-protocol.md` (features, `old_peer`, the service contract) and
     `docs/addons.md` ("Daemon ops": `service`, `service_reload`, `service_state`; the web ops'
     bodies);
  2. the core supervisor with its `TestAddon` tests, no web code (A3);
  3. the web addon.

  `make check` passes at each.
- `app.rs` stays pure; no TUI change.
- `docs/web-protocol.md` does not change. These are updated in the same branch:
  - `docs/web.md`: usage, the `--remote` steps, "keeps the old password until it restarts",
    the env and sign-out notes, and that the web survives a logout only where the daemon does;
  - `docs/addons.md`, `docs/daemon-protocol.md`;
  - `README.md`: the `claudio web` lines and the `--remote` paragraph;
  - the `Makefile`'s `dev` notes;
  - `CLAUDE.md`'s web and daemon lines.
- Tests use fake claude and the isolated env of the existing web suites; manager suites stay
  single-threaded.
- Conventional commits `type(scope): lowercase summary`, no AI attribution or co-author trailers.

## Open questions

None.
