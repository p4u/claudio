# The daemon protocol

Every claudio client (the console TUI, `claudio --plain`, `claudio web`, the SSH bridge's peer)
talks to a host's session daemon (`claudio --daemon`) over its Unix socket, directly or through
`ssh HOST claudio --slave`. This page is the contract for what an addon and a mixed-version pair
of peers rely on. The types are in `crates/core/src/proto.rs` (and `crates/core/src/proto/spawn.rs`), the
daemon's handling in `crates/core/src/daemon/server.rs`.

- [Framing](#framing)
- [Handshake and features](#handshake-and-features)
- [Requests and replies](#requests-and-replies)
- [Addon ops](#addon-ops)
- [Spawn env: `env_remove`](#spawn-env-env_remove)
- [Optional args: `optional`](#optional-args-optional)
- [Mixed versions](#mixed-versions)
- [Refused ops](#refused-ops)

## Framing

A frame is `[u32 BE length][u8 tag][payload]`; `length` counts the tag and the payload and is at
most `MAX_FRAME` (1 MiB).

- Tag `J`: a JSON control message, `Envelope { req, msg }`, one flat object: a `Msg`, tagged by
  its `op` field (`snake_case`), plus `req`. A request carries a `req` id and its reply echoes
  it; a pushed event carries none. Unknown fields are ignored, and an `op` a peer does not know decodes to
  `Msg::Unknown`.
- Tag `D`: terminal bytes for one session: a 16-byte session id, then raw bytes (input from the
  client, PTY output from the daemon). The first `D` frame after `Attached` is the screen
  snapshot.

The protocol version `PROTO` is in the socket's file name, so daemons of two protocol versions
never share a socket. Within one version, peers one release apart must work together (B1).

## Handshake and features

The client's first message is `Hello { claudio_version, proto, colors }`; the daemon answers
`Welcome { claudio_version, proto, host, build, build_time, features }`. `features` says what
the daemon can do beyond the base protocol; a daemon that omits it has none of them.

| Feature | The daemon… |
|---|---|
| `lens` | takes `SpawnSpec.extensions` and sends `SessionEvent::Lens` |
| `search` | answers `Search` |
| `last_prompt` | answers `LastPrompt` |
| `attention` | sends `SessionEvent::Attention` and fills `SessionInfo.attention` |
| `worktree` | answers `GitWorktree` and `GitWorktreeDiff` |
| `addon:<name>` | runs addon `<name>`'s daemon ops (one entry per daemon-side addon) |

A core-only daemon lists no `addon:` feature. A client sends a request that depends on a
feature only to a daemon that lists it.

## Requests and replies

A request gets one reply with the same `req`: its own reply message, `Ok`, or `Error { message
}`. A daemon answers an op it does not know with `Error "unsupported op: …"` (`unknown` for an
op newer than the daemon); a client turns that into "daemon too old: run `claudio daemon
restart`" and goes on without the feature.

Fast requests are answered in order on the connection's loop. Slow ones (reading transcripts,
`UpdateClaude`, `AddonOp`) run on their own task and reply when done, so the
connection keeps answering in the meantime: a `Ping` sent after a slow request may be answered
first. Replies are matched by `req`, never by order.

## Addon ops

```
→ AddonOp    { addon, op, body }
← AddonReply { body }        or   Error { message }
```

`Msg` is tagged by its `op` key, so the addon's op travels as `name`:

```json
{"req": 7, "op": "addon_op", "addon": "echo", "name": "sleep", "body": {"secs": 2}}
{"req": 7, "op": "addon_reply", "body": {"slept": 2.0}}
```

`body` is any JSON value (`null` when absent). The daemon finds the addon in its registry and
runs its `DaemonOps::handle(op, body)` on a task, off the connection's loop.

| Limit or refusal | `Error` message |
|---|---|
| an older daemon (no `addon:<name>` feature; it decodes the op as unknown) | `unsupported op: unknown` |
| the addon is not registered, or has no daemon ops | `no such addon <name>` |
| a request body over 512 KiB encoded | `body over 512 KiB` |
| more than 8 ops of one addon in flight | `addon <name> busy` |
| no answer within 150 s | `addon <name> timed out` |
| the addon's handler failed | its message |
| a reply body over 512 KiB encoded | `body over 512 KiB` |

Both sides check the 512 KiB cap (`proto::ADDON_BODY_MAX`, `proto::addon_body_ok`): the client
before it sends, the daemon on what it receives and on what it would answer. Every frame stays
under `MAX_FRAME`.

A body never carries a secret value. It may name secrets (a plugin's proxy env keys); the values
reach a session only through its `Spawn`/`Respawn` env. The daemon logs an `AddonOp` as its
`addon`, `op` and the body's encoded length, never the body.

`DaemonOps::handle` gets a `DaemonCtx` (docs/addons.md): the claude binary, the daemon's PATH,
the flags this claude knows, `<cache>/<addon>`, and the lock held while `claude update` or a
`claude plugin …` runs. After the daemon installs claude it calls each daemon-side addon's
`on_claude_installed`.

## Spawn env: `env_remove`

A claude session's process starts from the daemon's own environment, then loses some variables
and gets the spawn's `env` on top. `SpawnSpec` and `RespawnSpec` say which ones it loses:

```json
{"op": "spawn", "id": "…", "cwd": "~/w", "env": [["ANTHROPIC_AUTH_TOKEN", "…"]],
 "env_remove": ["ANTHROPIC_API_KEY"], "rows": 24, "cols": 80}
```

| `env_remove` | The process loses |
|---|---|
| absent (`None`): an older client | the **compatibility scrub**: the session markers (`claude::SESSION_MARKERS`); with `ANTHROPIC_AUTH_TOKEN` in `env`, also `ANTHROPIC_API_KEY`, and `CLAUDE_CODE_USE_GATEWAY` unless `env` sets it (the rule of the previous release) |
| `Some(list)` | exactly the session markers plus `list` (`[]`: the markers only) |

A terminal (`SpawnShell`) always loses the markers only. The list holds names, never values: at
most 64, each `[A-Za-z_][A-Za-z0-9_]{0,127}`; anything else refuses the spawn with `Error "bad
env_remove: …"`. The session's `env` then applies, after the daemon's `TERM`, `COLORTERM` and
login-shell `PATH` (an explicit `PATH` in `env` wins).

The env and `env_remove` of a session are never journaled. The daemon keeps the last pair of
each live or dormant session in memory, so the fresh retry after a failed `--resume` (claude
exits at once because the conversation is gone) starts with the same env as the spawn it
replaces. A Kill or a session's clean exit drops the pair, and so does the daemon's end; a
retry the daemon has no pair for (it restarted meanwhile) starts with no env and logs a
warning.

## Optional args: `optional`

`SpawnSpec.optional` names claude flags of `args` that the session can do without:

```json
{"op": "spawn", "…": "…", "args": ["--agents", "{…}", "--mcp-config", "{…}"],
 "optional": ["--agents", "--mcp-config"]}
```

When this host's claude does not list such a flag in its `--help`, the daemon leaves it out of
the command line with its value (the next argument, or the `=value` of `--flag=value`), so an
older claude still starts. The daemon applies the same rule to `--plugin-dir`, `--agents` and
`--mcp-config` whether `optional` names them or not (the legacy list, for older clients). At
most 16 names, each `--[a-z0-9][a-z0-9-]{0,39}`; anything else refuses the spawn with `Error
"bad optional: …"`.

`optional` is journaled with the args: a Respawn, a recovery after a daemon restart and the
fresh retry drop the same flags again when this claude lacks them, and a newer claude gets them
back. `RespawnSpec` has no `optional`; the journal's applies.

## Mixed versions

A client and a daemon one release apart work together in both directions.

| Peers | What happens |
|---|---|
| new client → new daemon | `AddonOp`, `env_remove: Some(..)` |
| new client → old daemon (busy, deferred, failed check) | no `addon:vocdoni` in features → no `AddonOp`; the launch gets the proxy env without the plugins' share and one notice ("daemon too old, plugins after `claudio daemon restart`"). The old daemon applies its own `env_diff` (a superset of what the addon sends in `env_remove`). |
| old client → new daemon | `Spawn`/`Respawn` without `env_remove` (`None`) → the daemon's **compatibility scrub** applies today's `env_diff` rule. Its `Manifest` op is refused (`unsupported op`) → the old client's existing "daemon too old" path, session without plugins. |
| new client → old daemon, `optional` | ignored; the old daemon drops its own fixed three (`--plugin-dir`, `--agents`, `--mcp-config`) when its claude lacks them, which covers every flag a client marks optional within the window. |

Debug builds take `CLAUDIO_TEST_OLD_DAEMON=1` (tests only; a release build ignores it): the
daemon then plays the previous release. It lists no `addon:` feature, answers `AddonOp` with
`unsupported op: addon_op`, treats every `env_remove` as absent and ignores `optional`.

**Removal date.** The compatibility paths for older peers (the compatibility scrub for a spawn
without `env_remove`, the legacy optional-flag list, the refused-`Manifest` test, and the legacy
fields clients keep writing) go in the first release after the one that ships addons (planned
v0.10.0), not before 2027-01-31. Each is marked `// compat(v0.10)` in the source.

## Refused ops

An op that a release before addons answered and this one no longer knows decodes to
`Msg::Unknown` and is answered `Error "unsupported op: unknown"`, which an older client already
turns into "daemon too old".

| Op | Since | Instead |
|---|---|---|
| `manifest` (and its reply `applied`) | addons (P3) | `AddonOp { addon: "vocdoni", op: "apply" }`, docs/addons.md, "The Vocdoni addon". An older client's session starts without its plugins. |

The test that pins the refusal (`a_manifest_op_from_an_old_client_is_refused_as_unsupported`) is
a compatibility path: it goes with the others at the removal date above (`// compat(v0.10)`).
