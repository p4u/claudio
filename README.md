# claudio

A terminal-native session manager for [Claude Code](https://claude.ai/claude-code). Run many Claude Code sessions — local and SSH — in one terminal window. Sessions survive closing the UI or an SSH drop; everything picks up where it left off.

---

## Screenshots

<!-- Regenerate: CLAUDIO_SCREENSHOTS=docs/screenshots cargo test --test screenshots -- --nocapture -->

![Sessions view — active session, tab states, status bar](docs/screenshots/sessions.png)

*Active session showing Claude Code output. Tab bar displays all sessions with state glyphs; amber and red tabs flag sessions that need attention.*

![New-session wizard](docs/screenshots/wizard.png)

*New-session wizard: LOCAL section lists recent directories with git branch (⎇) and claude-activity (✻) badges. REMOTE section lists SSH hosts from `~/.ssh/config`.*

![Directory explorer with search](docs/screenshots/directories.png)

*Directory picker with a live search query. Matching text is highlighted; git and claude-activity metadata appears right-aligned on each row.*

![Overview popup](docs/screenshots/overview.png)

*Alt+g overview: all sessions at a glance, with state, location, and age columns.*

---

## Install

```bash
curl -fsSL https://github.com/p4u/claudio/releases/latest/download/install.sh | bash
```

**Options** (set as environment variables before the command):

| Variable | Default | Description |
|---|---|---|
| `CLAUDIO_VERSION` | latest | Pinned release tag, e.g. `v0.2.0` |
| `CLAUDIO_INSTALL_DIR` | `~/.local/bin` | Where to place the `claudio` binary |

**From source:**

```bash
make install      # builds a static musl binary, installs it, and restarts a running daemon
```

**Platforms:** Linux and macOS, x86\_64 and arm64.

---

## Quick start

### Without a proxy

**Requirements:** [Claude Code](https://claude.ai/claude-code) installed and logged in (`claude` works in your shell), and `~/.local/bin` (or your `CLAUDIO_INSTALL_DIR`) on your `PATH`.

1. [Install](#install) claudio.
2. Run `claudio`.
3. The new-session wizard opens:
   - **Start screen:** under *Local*, pick **Explore local dirs…** or one of your recent directories. Under *Remote*, pick an SSH host.
   - **Directory:** type a path or a fragment of one, `Tab` completes, `Enter` picks. Hidden (`.`) directories are not listed; `Alt+.` shows them.
   - **New or resume:** start a new Claude Code session, or resume a previous one from that directory.
4. Work as usual. These keys are handled by claudio; everything else goes to Claude Code:

   | Key | Action |
   |---|---|
   | `Alt+←` / `Alt+→` | Switch session |
| `Alt+Shift+1`…`9`, `0` | Go to session 1…9, 10 (needs a kitty-keyboard terminal) |
   | `Alt+n` | New session |
   | `Alt+r` | Rename session |
   | `Alt+x` | Close session |
   | `Ctrl+D` twice | Exit Claude Code; the tab closes on its own |
   | `Alt+g` | Overview of all sessions |
   | `Alt+h` | Help (all keys) |
   | `Alt+q` | Quit the UI; sessions keep running |

5. Run `claudio` again later: every session is restored where you left it.

**SSH in one line:** pick a host from the *Remote* section of the wizard (needs key-based `ssh <host>`, e.g. `ssh devbox`, to work without a password). claudio installs itself on the remote, and those sessions survive disconnects too. `claude` must be installed on the remote host unless you use a proxy (below).

### With claude-proxy

[claude-proxy](https://github.com/p4u/claude-proxy) is a multi-subscription credential proxy for Claude Code: sessions authenticate through it instead of a local `claude` login.

```bash
claudio proxy login https://proxy.example.com   # prompts for your token; it is never an argument
claudio                                         # new sessions now use the proxy by default
```

- `claudio --no-proxy` or `claudio --proxy <profile>` overrides the default for one run.
- `CLAUDIO_PROXY_URL=<token>@proxy.example.com claudio` uses a proxy for one run without saving a profile.
- `claudio proxy status` shows your profiles, live stats and pool health. Inside the UI, `Alt+s` opens the stats for the active session.

### Everyday commands

| Command | What it does |
|---|---|
| `claudio` | Open the session manager |
| `claudio sessions` | List sessions as a table (for scripts) |
| `claudio daemon status\|stop\|restart` | Inspect or control the background daemon |
| `claudio proxy login\|status\|use\|logout` | Manage proxy profiles |
| `claudio upgrade [--check]` | Install the latest release, or only check for one |
| `claudio -p "…"` | Drop-in for `claude -p` ([details](docs/print-and-api.md)) |
| `claudio --api` | OpenAI-compatible API server ([details](docs/print-and-api.md)) |

---

## What it does

Run `claudio` with no arguments to open the session manager. A per-host background daemon owns all PTYs, so sessions keep running after you close the UI or lose an SSH connection. On restart every session is restored, and interrupted ones are recovered automatically with `claude --resume`.

### New-session wizard

Press `Alt+n` (or launch `claudio` with no sessions) to open the wizard:

1. **Start screen** — two sections: LOCAL (an "Explore" entry plus your recently used directories, each annotated with the git branch and last claude-activity timestamp) and REMOTE (SSH host aliases from `~/.ssh/config`). Type to filter both sections at once; Tab switches focus between sections.
2. **Directory picker** — navigate or type a path; Tab-completes. Each entry shows `⎇ branch` and `✻ last-used` badges. Directories starting with `.` are hidden by default; `Alt+.` toggles them (the picker shows `Alt+. hidden: off|on`), and typing a leading `.` (e.g. `~/.con`) reveals the matching ones.
3. **Resume or new** — if the chosen directory has prior Claude Code sessions you can resume one; otherwise a fresh session starts.

### Attention colors

Background sessions that need attention (`NeedsInput`, `NeedsApproval`, `Error`) are highlighted in the tab bar and trigger an OSC 9 desktop notification to the outer terminal.

### Status bar

The two-line status bar shows CPU and memory sparklines for the active session's host (updated every few seconds), plus session metadata: session name, directory, state glyph, and — when a proxy is active — pool and token-usage stats. When a newer release is available the right side shows `↑ vX.Y.Z` in dim cyan.

### Key bindings

All manager keys use `Alt` so they never clash with Claude Code's own bindings. Every binding can be overridden in `~/.config/claudio/config.toml` under `[keys]`.

| Key | Action |
|---|---|
| `Alt+←` / `Alt+→` | Switch between sessions |
| `Alt+Shift+1`…`9`, `0` | Go to session 1…9, 10 (kitty keyboard protocol terminals; fixed, not rebindable) |
| `Alt+n` | New session (opens wizard) |
| `Alt+g` | Overview of all sessions |
| `Alt+a` | Jump to next session needing attention |
| `Alt+r` | Rename session |
| `Alt+x` | Close / kill session |
| `Ctrl+D` twice, `/exit` | Claude Code's own exit: the session ends and its tab closes, no confirmation |
| `Alt+s` | Proxy stats popup |
| `Alt+h` | Help |
| `Alt+.` | Wizard only: show/hide hidden directories |
| `Alt+q` | Quit UI (sessions keep running in daemon) |

### Session states

The tab bar shows a glyph for each session:

| Glyph | State | Tab color |
|---|---|---|
| ⠸ (animated) | Working | cyan |
| ◆ | Needs approval | red / bold |
| ? | Needs input | amber / bold |
| ✓ | Idle | — |
| ✗ | Error | red / bold |
| ⇄ (dim) | Reconnecting | — |

---

## Remote sessions

SSH hosts are listed from `~/.config/claudio/hosts.json` (most-recently-used first) followed by every `Host` alias in `~/.ssh/config`. The claudio binary is automatically installed on the remote at `~/.local/bin/claudio` and kept in sync by SHA-256 comparison. The remote daemon runs under `systemd-run --user` when available (survives `KillUserProcesses`), with `setsid` as fallback. No remote dotfiles are modified.

Requirements: SSH key or agent authentication (`BatchMode` — no password prompts), and `claude` installed on the remote host (or `CLAUDIO_PROXY_URL` set so the remote sessions use the proxy instead).

---

## Drop-in `-p` (print mode)

`claudio -p` is a drop-in for `claude -p`. It drives the interactive Claude Code TUI under a real PTY, forwards every flag verbatim, waits for the `Stop` lifecycle hook, and reads the exact answer and real token usage from the session JSONL. Point any tool that runs `claude -p ...` at `claudio -p ...`.

See [docs/print-and-api.md](docs/print-and-api.md) for details.

---

## OpenAI-compatible API server

`claudio --api` exposes an OpenAI-compatible HTTP server backed by your local Claude Code install. Any OpenAI client works with it out of the box — point it at `http://127.0.0.1:8080/v1`.

See [docs/print-and-api.md](docs/print-and-api.md) for details.

---

## claude-proxy integration

claudio integrates with [claude-proxy](https://github.com/p4u/claude-proxy), an optional self-hosted gateway. After `claudio proxy login <url>`, remote sessions authenticate through the gateway instead of requiring a local `claude` login on each host. The proxy can be configured with a 1M-context model as its default, and `claudio proxy status` shows live pool and token-usage stats.

See [docs/print-and-api.md](docs/print-and-api.md) for details.

---

## Upgrade

When a newer release is available, the status bar shows `↑ vX.Y.Z` in dim cyan. Run:

```bash
claudio upgrade
```

to download and replace the local binary in place. The update check is anonymous, cached for 24 hours, and never blocks. Disable with `CLAUDIO_NO_UPDATE_CHECK=1` or `[update] check = false` in `config.toml`.

---

## Configuration

`~/.config/claudio/config.toml` — all fields are optional:

```toml
[keys]
prev_session = "alt+left"
next_session = "alt+right"
overview     = "alt+g"
quit         = "alt+q"

[ui]
notify = true   # OSC 9 desktop notifications for attention states

[update]
check = true
```

---

## Maintainer

Bump `Cargo.toml` version, tag `vX.Y.Z`, push the tag → the release workflow publishes binaries to this repo.
