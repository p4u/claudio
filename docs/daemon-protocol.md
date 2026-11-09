# The daemon protocol

Every claudio client (the console TUI, `claudio --plain`, `claudio web`, the SSH bridge's peer)
talks to a host's session daemon (`claudio --daemon`) over its Unix socket, directly or through
`ssh HOST claudio --slave`. This page is the contract for what an addon and a mixed-version pair
of peers rely on. The types are in `src/proto.rs` (and `src/proto/spawn.rs`), the daemon's
handling in `src/daemon/server.rs`.

- [Framing](#framing)
- [Handshake and features](#handshake-and-features)
- [Requests and replies](#requests-and-replies)
- [Addon ops](#addon-ops)
- [Spawn env: `env_remove`](#spawn-env-env_remove)
- [Optional args: `optional`](#optional-args-optional)
- [Mixed versions](#mixed-versions)

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
`UpdateClaude`, `Manifest`, `AddonOp`) run on their own task and reply when done, so the
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

Filled in by the next protocol change (`SpawnSpec.env_remove`, `RespawnSpec.env_remove`).

## Optional args: `optional`

Filled in by the next protocol change (`SpawnSpec.optional`).

## Mixed versions

A client and a daemon one release apart work together in both directions.

| Peers | What happens |
|---|---|
| new client → new daemon | `AddonOp`, `env_remove: Some(..)` |
| new client → old daemon (busy, deferred, failed check) | no `addon:vocdoni` in features → no `AddonOp`; the launch gets the proxy env without the plugins' share and one notice ("daemon too old, plugins after `claudio daemon restart`"). The old daemon applies its own `env_diff` (a superset of what the addon sends in `env_remove`). |
| old client → new daemon | `Spawn`/`Respawn` without `env_remove` (`None`) → the daemon's **compatibility scrub** applies today's `env_diff` rule. Its `Manifest` op is refused (`unsupported op`) → the old client's existing "daemon too old" path, session without plugins. |

**Removal date.** The compatibility paths for older peers (the compatibility scrub for a spawn
without `env_remove`, the legacy optional-flag list, the refused-`Manifest` test, and the legacy
fields clients keep writing) go in the first release after the one that ships addons (planned
v0.10.0), not before 2027-01-31. Each is marked `// compat(v0.10)` in the source.
