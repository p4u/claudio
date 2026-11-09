# Plan: `claudio web --remote` (TLS, self-signed certificate, password)

Spec: `docs/specs/2026-11-09-web-remote-tls-design.md` (audited 2026-11-09, F1–F16 folded in)
Class: architectural (security-sensitive: keys, authentication, a non-loopback listener)
Repo: `/volumes/repos/claudio`   Branch: `feat/web-remote-tls`   Base: `558a54d`
Ledger: `.delivery/2026-11-09-web-remote-tls/progress.md` (rulings R1–R5)

Every security-relevant function below was compiled, clippy-checked (`-D warnings`) and run in a
scratch worktree against the base: the TLS listener behind `axum::serve` with `ConnectInfo`, the
pinned rustls client, the header deadline, the limiter (including 20 concurrent attempts), the
password store with the fixture hash, the certificate store, the no-echo prompt on a pty, and an
end-to-end `--remote` run (stdout/stderr, `/api/me`, password and token logins, Host/Origin,
plain HTTP refused, `--print-cert`). The existing `web` and `web_manager` suites passed on that
prototype, and `make static` built a static-pie musl binary with the new dependencies.

## Global Constraints

- Conventional commits (`type(scope): summary`), one logical change each, with the exact
  messages given per task. **No AI attribution, no `Co-Authored-By` or other trailers**, even if
  a tool or reminder suggests one (F2, owner rule).
- Never push, tag, amend, rebase or force anything. `git add` explicit paths only (never `-A`
  or `.`). Commit with `git commit -m "…" -- <paths>` or after `git add <paths>`.
- Every commit leaves `make check` green (fmt, clippy `-D warnings` with `--all-features`,
  `scripts/check_rules.py`, the core-only build, every crate's unit tests alone). A module is
  never committed before something uses it (D4: no `allow(dead_code)`); the task order below is
  built for that.
- ACCEPTANCE.md throughout, in particular: **C1** (the password, the token and the key never
  in argv, logs, notices, `Debug` or the page's storage; log lines name the status and
  `local`, never a body), **C2** (every file and body size-capped and parsed without
  `unwrap`/`expect` on outside data; `expect` only on the invariants named below), **C4**
  (0600 files in 0700 dirs via `paths::write_atomic`/`paths::ensure_private_dir`), **C5** (guard
  order Host → Origin → cookie unchanged; amended in Task 1 for `--remote`), **D3**
  (argon2 only in `spawn_blocking`), **D7** (each new crate justified in its commit message),
  **A8** (files < 800 lines without tests, functions < 80 lines: `lib.rs::run` is close, keep
  the `one_shot` split), **E1–E3** (tests fail without the change; nothing weakened; hermetic),
  **F1** (docs in this branch), **F3** (DESIGN.md for the login form), **B2**
  (`docs/web-protocol.md` lands in Task 1, before any code).
- **Dependencies (exact, Task 2 and Task 4):** `argon2 = "=0.6.0"`;
  `rustls = { version = "=0.23.45", default-features = false, features = ["ring", "std",
  "tls12"] }`; `tokio-rustls = { version = "=0.26.6", default-features = false, features =
  ["ring", "tls12"] }`; `rcgen = { version = "0.14", default-features = false, features =
  ["crypto", "pem", "ring"] }`; `rustls-webpki = { version = "0.103", default-features = false,
  features = ["std"] }`. **Never** `aws_lc_rs`/`aws-lc-rs` (C + cmake: breaks `make static`,
  and two providers make rustls' default-provider lookup panic). After adding them,
  `cargo tree -e normal --target all | grep -c aws-lc` must print `0`. The lock gains about 27
  entries (some, such as `x509-parser`, are recorded by the resolver as rcgen's optional
  dependencies but never built: `cargo tree -i x509-parser` prints nothing). Run a plain
  `cargo build` once after editing a `Cargo.toml` so `Cargo.lock` updates, and commit it; `make
  check` runs `--locked`.
- The static musl build must keep working: `make static` at the end of Task 4 and in the final
  check (it needs the `x86_64-unknown-linux-musl` rustup target, installed here).
- Scratch work only under `~/.cache/claudio/` (never `/tmp`; unit tests keep the repo's
  `claudio_core::paths::socket_test_dir` convention). **Never touch the owner's daemon or
  `claudio web`:** run claudio only through the suites (isolated `XDG_*`, `HOME`,
  `CLAUDIO_CLAUDE_PATH`); never `claudio daemon stop/restart` without the isolated env.
- Integration suites run single-threaded: `cargo test --test web -- --test-threads=1`.
- New stable `data-testid`s (the e2e's interface): `login-form`, `login-password`,
  `login-submit`, `login-error`. No existing test id changes.
- Toolchain note: the distro `cargo` (1.93) and rustup's (1.96) coexist here. Use the
  `make` targets (they pick rustup's); a bare `cargo clippy` after a rustup build can fail with
  `E0514` (crate compiled by another rustc), which is not a code problem.

## Review Focus

Where a reviewer should spend the time (paths after the change; line numbers of today's code
for context):

1. **The local rule** (`addons/web/src/auth.rs`, `Auth::caller`, `Auth::token_allowed`):
   local = loopback peer (canonical) **and** a loopback `Host` (`Allowed::local_hosts`). The
   token must never be accepted from a non-local request with `--remote`, and must keep working
   for every request without it (today's proxy setup, `docs/web.md:44-48`).
2. **The limiter** (`addons/web/src/limit.rs`): the slot is taken under the lock before any
   compare or hash; only a success releases it; a 503 (no hashing slot) releases it; the
   global window applies to every password attempt and every non-local token attempt.
3. **Hashing bounds** (`addons/web/src/login.rs::password_login`): 4 permits, 2 s wait,
   `spawn_blocking`, the permit dropped inside the blocking closure; the password never logged.
4. **The TLS listener** (`addons/web/src/tls/listener.rs`): handshakes off the accept loop
   (10 s, 64), connection caps (256, 32 per non-loopback peer) carried in `TlsIo`, the 30 s
   first-headers deadline, and the `Connected` impl (peer from the socket, never a header).
5. **The certificate store** (`addons/web/src/tls/cert.rs`): never silently replaced
   (missing half, foreign mode, symlink, mismatch: all refused naming `--rotate-cert`); key
   written before the certificate; ring provider named explicitly.
6. **Fail closed at start** (`addons/web/src/remote.rs::prepare`, `lib.rs::run`): no password →
   exit 1 before any certificate is made or port bound; `listen_addr` (today `lib.rs:147-172`)
   untouched (AC1).
7. **The guard** (today `auth.rs:327-382`): order unchanged; `/api/me`'s 401 body is the only
   new output; `https://` Origins only with `--remote`; no HSTS.
8. **The debug-only test hook** `CLAUDIO_TEST_REMOTE_PEER` (`auth.rs::test_remote_peer`): only
   `cfg!(debug_assertions)`, only ever removes the token from a peer.
9. **Dependencies** (`Cargo.toml` `[workspace.dependencies]`): exact pins for what touches
   network bytes, ring only, `cargo tree` free of aws-lc.
10. **`--print-cert` installability** (Task 7 docs, AC9b): the certificate is a self-signed
    leaf without `CA:TRUE` (a CA certificate used as a server certificate is refused by Firefox
    and rustls-webpki). Whether each platform's trust store accepts a leaf as an anchor
    (Chromium on Linux, Android's CA installer, iOS trust settings) is **unverified**; the docs
    mark those steps as checked only by AC9b. If AC9b fails on a platform, the exception path is
    the documented one there.

## Tasks

| # | Task | Role | Commits |
|---|---|---|---|
| 1 | Protocol doc and the C5 amendment (B2) | standard | 2 |
| 2 | Core services (no-echo prompt, host name) and `--set-password` | strong | 2 |
| 3 | Login reservations and the request's peer on today's server | strong | 1 |
| 4 | `--remote`: certificate store, TLS listener, CLI, start-up, token rule | strong | 1 |
| 5 | Password login over `--remote` | strong | 1 |
| 6 | The page's password form and the `--remote` e2e | standard | 2 |
| 7 | User docs, README and CLAUDE.md | standard | 2 |

After Task 7 the controller runs the final check (end of this file).

---

## Task 1: protocol doc and the C5 amendment (standard)

**Files:** `docs/web-protocol.md`, `ACCEPTANCE.md`.

### 1a. `docs/web-protocol.md`

In §1 (`## 1. HTTP surface`, today lines 19–57):

- Replace the `POST /api/login` row with:
  `| POST /api/login body {"token":"<64 hex>"} or {"password":"<12–1024 bytes>"} (≤ 4 KiB) | no | yes | 204 + Set-Cookie; 401 wrong token or password, or a token from a non-local request with --remote; 429 locked; 400 bad body (neither or both fields, or a password without --remote); 413 too large; 503 no hashing slot within 2 s (does not count) |`
- Replace the `GET /api/me` row with:
  `| GET /api/me (decided here) | yes | no | 204: the cookie is live. 401 without one, body {"login":[…]}: the methods this request may use, "token" and/or "password" (below) |`
- After the paragraph "Checks run in this order…", add:

  > **Local requests (`--remote`).** A request is local when its TCP peer (from the connection,
  > never a header; `::ffff:127.0.0.1` counts as `127.0.0.1`) is a loopback address **and** its
  > `Host` is one of the loopback forms at the listen port: `127.0.0.1:<port>`,
  > `localhost:<port>`, `[::1]:<port>`, or the listen address itself when it is loopback. With
  > `--remote` the token is accepted from local requests only; the password from any request.
  > Without `--remote` every request may use the token and there is no password login.
  > `/api/me`'s 401 lists `["token"]` without `--remote`, `["token","password"]` for a local
  > request with it, `["password"]` for any other request.

- Replace the "Accepted `Host` values" bullet with two:

  > - Without `--remote`, accepted `Host` values: the `--listen` address itself (e.g.
  >   `127.0.0.2:<port>`), `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` (each also
  >   without the port when it is 80), every `--allow-host NAME` as `NAME` or `NAME:<port>`, and
  >   every `--allow-host NAME:PORT` as exactly that, compared case-insensitively.
  > - With `--remote` (TLS only, one port): the loopback forms above, the listen address when it
  >   is a specific one (never `0.0.0.0`/`[::]`), any IPv4 or bracketed IPv6 literal that is
  >   neither unspecified nor multicast, this machine's host name and `<first label>.local`, and
  >   each `--allow-host` name; all at the listen port, and also bare when the port is 443.

- Replace the "Accepted `Origin`" bullet with:

  > - Accepted `Origin`: `https://` (and, without `--remote`, `http://`) followed by an accepted
  >   Host value, nothing after it. `null` is refused.

- Replace the "Cookie:" bullet's `Secure` clause with: "plus `; Secure` when the server runs with
  `--remote` or with any `--allow-host`".
- Replace the "Login limit" bullet with:

  > - Login limits, taken **before** a token is compared or a password hashed (a reservation,
  >   so concurrent attempts cannot all pass): 5 attempts per peer per 60 s (every loopback
  >   address is one peer; an IPv4 address is one, an IPv6 address counts as its /64), 30 per
  >   60 s over all peers for password attempts and non-local token attempts (local token
  >   attempts are exempt), at most 1024 peers tracked. A successful login gives its attempt
  >   back; a failed one counts for 60 s; past a limit the answer is `429` without any check.
  >   Password checks run at most 4 at once; one that waits 2 s for a slot answers `503` and
  >   does not count. Only requests that passed the Origin check count.

- Add after the headers bullet: "With `--remote` no `Strict-Transport-Security` header is sent
  (it would stop a browser from accepting the self-signed certificate later)."
- In §2's first line, replace "(or `wss://` behind a TLS proxy)" with "(`wss://` with
  `--remote` or behind a TLS proxy; the page picks the scheme from its own URL)".

### 1b. `ACCEPTANCE.md`

Replace the C5 rule (line 108–109) with:

```
**C5. The web UI keeps its perimeter.** Loopback only, or with `--remote` one TLS listener where
every non-local request needs the password (docs/web.md); guard order Host → Origin → cookie,
the import map is the only inline script, no eval or CDN, vendored files byte-identical.
```

### Verify and commit

```bash
git diff --stat   # two files
git commit -m "docs(web-protocol): password login and local requests for claudio web --remote" -- docs/web-protocol.md
git commit -m "docs(acceptance): C5 admits claudio web --remote (TLS and a password for non-local requests)" \
  -m "The owner chose remote access by TLS with a self-signed certificate and a password (docs/specs/2026-11-09-web-remote-tls-design.md); loopback-only stays the default." -- ACCEPTANCE.md
```

---

## Task 2: core services and `--set-password` (strong)

### 2a. `claudio_core::term::secret` and `claudio_core::sys::hostname`

The Vocdoni addon's no-echo token prompt (`addons/vocdoni/src/cmd.rs:340-458`) moves to the core
as a generic service (A3: own commit, generic, documented, tested without the addon; A5: one
implementation); the daemon's private `hostname()` (`crates/core/src/daemon/host.rs:58-67`)
becomes `claudio_core::sys::hostname`. Both are used in this commit (vocdoni and the daemon), so
nothing is dead.

**New `crates/core/src/term/secret.rs`:**

```rust
//! A secret typed at the terminal: one line read with echo off. The
//! terminal's mode is restored on every exit path, Ctrl+C included (a
//! temporary SIGINT handler restores it, then re-raises).

use std::io::{self, BufRead, IsTerminal, Write};
use std::mem::MaybeUninit;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicI32, Ordering};

/// The longest line read; the rest of a longer one is left unread.
pub const MAX_LINE: u64 = 4096;

/// Print `prompt` on stderr and read one line from stdin, which must be a
/// terminal, with echo off. The line comes back without its line ending
/// (nothing else is trimmed). An error when stdin is not a terminal or its
/// echo cannot be turned off: a secret is never read with echo on.
pub fn read_line_no_echo(prompt: &str) -> io::Result<String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return Err(io::Error::other("stdin is not a terminal"));
    }
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let line = no_echo_line(stdin.as_raw_fd(), stdin.lock());
    eprintln!();
    line
}

/// File descriptor saved for the SIGINT handler; -1 when none is installed.
static SIGINT_FD: AtomicI32 = AtomicI32::new(-1);

/// The terminal mode the SIGINT handler restores.
struct TermCell(std::cell::UnsafeCell<MaybeUninit<libc::termios>>);
// SAFETY: written before SIGINT_FD is set (Release), read only by the
// handler after it saw that store (Acquire); no write while installed.
unsafe impl Sync for TermCell {}
static SIGINT_SAVED: TermCell = TermCell(std::cell::UnsafeCell::new(MaybeUninit::uninit()));

extern "C" fn on_sigint(_: libc::c_int) {
    let fd = SIGINT_FD.load(Ordering::Acquire);
    if fd >= 0 {
        // SAFETY: initialised before SIGINT_FD became non-negative.
        unsafe {
            let saved = (*SIGINT_SAVED.0.get()).assume_init_ref();
            libc::tcsetattr(fd, libc::TCSANOW, saved);
        }
    }
    // SAFETY: async-signal-safe calls only.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::raise(libc::SIGINT);
    }
}

/// Restores the terminal mode and the previous SIGINT handler on drop.
struct Restore {
    fd: RawFd,
    saved: libc::termios,
    old_sigint: libc::sighandler_t,
}

impl Drop for Restore {
    fn drop(&mut self) {
        // SAFETY: `saved` came from tcgetattr on `fd`; `old_sigint` from signal().
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
            libc::signal(libc::SIGINT, self.old_sigint);
        }
        SIGINT_FD.store(-1, Ordering::Release);
    }
}

/// One line of `input` (at most [`MAX_LINE`] bytes) with echo off on the
/// terminal `fd`.
fn no_echo_line(fd: RawFd, input: impl BufRead) -> io::Result<String> {
    let mut saved = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr writes the struct on success.
    if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr succeeded.
    let saved = unsafe { saved.assume_init() };
    // SAFETY: no handler reads it yet (SIGINT_FD is -1).
    unsafe { (*SIGINT_SAVED.0.get()).write(saved) };
    SIGINT_FD.store(fd, Ordering::Release);
    // SAFETY: on_sigint is a valid extern "C" handler.
    let old_sigint =
        unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t) };
    let restore = Restore { fd, saved, old_sigint };
    let mut quiet = saved;
    quiet.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
    // SAFETY: a valid termios for `fd`.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut line = String::new();
    input.take(MAX_LINE).read_line(&mut line)?;
    drop(restore);
    let end = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(end);
    Ok(line)
}
```

Its tests (same file, `#[cfg(test)] mod tests`):

```rust
    use super::*;
    use std::ffi::CStr;
    use std::fs::File;
    use std::os::unix::io::FromRawFd;

    /// A pseudo-terminal: (master, slave).
    fn pty() -> (File, File) {
        // SAFETY: plain libc calls; every fd is checked and owned by a File.
        unsafe {
            let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(m >= 0);
            assert_eq!(libc::grantpt(m), 0);
            assert_eq!(libc::unlockpt(m), 0);
            let name = CStr::from_ptr(libc::ptsname(m)).to_owned();
            let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(s >= 0);
            (File::from_raw_fd(m), File::from_raw_fd(s))
        }
    }

    #[test]
    fn a_line_is_read_without_echo_and_the_mode_comes_back() {
        let (mut master, slave) = pty();
        let fd = slave.as_raw_fd();
        let echo_on = |fd| {
            let mut t = MaybeUninit::<libc::termios>::uninit();
            assert_eq!(unsafe { libc::tcgetattr(fd, t.as_mut_ptr()) }, 0);
            unsafe { t.assume_init() }.c_lflag & libc::ECHO != 0
        };
        assert!(echo_on(fd));
        master.write_all(b"  s3cret pass \n").unwrap();
        let line = no_echo_line(fd, io::BufReader::new(&slave)).unwrap();
        assert_eq!(line, "  s3cret pass ", "only the line ending goes");
        assert!(echo_on(fd), "echo is back");
        assert_eq!(SIGINT_FD.load(Ordering::Acquire), -1);
    }

    #[test]
    fn a_pipe_is_refused() {
        if io::stdin().is_terminal() {
            return; // run from a terminal: nothing to check
        }
        assert!(read_line_no_echo("x: ").is_err());
    }
```

`crates/core/src/term/mod.rs`: add `pub mod secret;` after `pub mod screen;`.

**New `crates/core/src/sys.rs`:**

```rust
//! Small facts about this machine.

/// This machine's host name (`gethostname`), or `""` when it cannot be read.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the pointer and length describe a valid, writable buffer.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}
```

Plus a unit test `hostname_is_what_the_system_says`: the result has no NUL, and when a
`hostname` command exists (`std::process::Command::new("hostname").output()` succeeds) it
equals that command's trimmed stdout; otherwise the test returns. `crates/core/src/lib.rs`: `pub mod sys;` after `pub mod remote;`.
`crates/core/src/daemon/host.rs`: delete `fn hostname()` (lines 58–67) and call
`crate::sys::hostname()` at line 50.

**`addons/vocdoni/src/cmd.rs`:** replace everything from `/// Read a token with no-echo (raw
TTY or plain stdin).` (line 341) to just before `// ── status ──` with:

```rust
/// Read a token: with echo off from a terminal, else one line of stdin.
fn read_token_from_tty_or_stdin() -> String {
    use std::io::IsTerminal;

    if io::stdin().is_terminal() {
        match claudio_core::term::secret::read_line_no_echo("Token: ") {
            Ok(line) => line.trim().to_owned(),
            Err(e) => {
                // Never fall back to echoing a secret token.
                eprintln!(
                    "claudio vocdoni login: cannot read the token with echo off ({e}); aborting"
                );
                std::process::exit(1);
            }
        }
    } else {
        eprint!("Token: ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        eprintln!(); // mimic the newline the terminal would print.
        line.trim().to_owned()
    }
}
```

(The piped form keeps working for vocdoni, as today.) A1: nothing in the core names vocdoni.

```bash
make check
cargo test -p claudio-core --lib term::secret   # 2 passed
cargo test --test vocdoni_manager -- --test-threads=1   # unchanged, green
git add crates/core/src/term/secret.rs crates/core/src/term/mod.rs crates/core/src/sys.rs \
  crates/core/src/lib.rs crates/core/src/daemon/host.rs addons/vocdoni/src/cmd.rs
git commit -m "refactor(core): term::secret (a no-echo line) and sys::hostname as core services" \
  -m "The Vocdoni addon's token prompt moves to the core unchanged in behaviour, so the web addon's --set-password can use it without a second copy (A3, A5); the daemon's host name becomes sys::hostname for the same reason. The prompt now returns an error instead of exiting and caps a line at 4 KiB."
```

### 2b. `--set-password` and the password store

**Dependencies.** Root `Cargo.toml` `[workspace.dependencies]`, alphabetical (after
`alacritty_terminal`):

```toml
# `claudio web --set-password` and --remote's password check: argon2id, pure
# Rust. Pinned exactly: it hashes what a browser sends (D7).
argon2 = "=0.6.0"
```

and at the end of the file, beside `[profile.dev.package.sha2]`:

```toml
# A debug build verifies the --remote password in the suites: unoptimized
# argon2 takes ~250 ms a check, optimized ~15 ms.
[profile.dev.package.argon2]
opt-level = 3

[profile.dev.package.blake2]
opt-level = 3
```

`addons/web/Cargo.toml` `[dependencies]`: `argon2.workspace = true`.

**New `addons/web/src/password.rs`** (`mod password;` in `lib.rs`):

```rust
//! The `--remote` password: one argon2id PHC string in `<config>/web-password`
//! (0600). `claudio web --set-password` writes it; a test writes the
//! committed fixture directly. Parameters are fixed (OWASP's baseline), and a
//! file with any other parameters is refused, so a verification's memory is
//! always [`M_COST_KIB`] KiB.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};

use claudio_core::paths;

/// argon2id memory cost in KiB (19 MiB), time cost and lanes.
pub(crate) const M_COST_KIB: u32 = 19 * 1024;
pub(crate) const T_COST: u32 = 2;
pub(crate) const P_COST: u32 = 1;
/// A password is 12 to 1024 bytes of UTF-8.
pub(crate) const MIN_BYTES: usize = 12;
pub(crate) const MAX_BYTES: usize = 1024;
/// A PHC string with these parameters is about 100 bytes; anything past
/// this is not one.
const FILE_MAX: u64 = 512;

/// `<config>/web-password`.
pub(crate) fn path() -> PathBuf {
    paths::config_dir().join("web-password")
}

fn argon2() -> Argon2<'static> {
    // The parameters are constants within argon2's limits.
    let params = Params::new(M_COST_KIB, T_COST, P_COST, None).expect("valid argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Whether `pw` may be a password: its length in bytes.
pub(crate) fn acceptable(pw: &str) -> Result<(), String> {
    match pw.len() {
        n if n < MIN_BYTES => Err(format!("the password needs at least {MIN_BYTES} bytes")),
        n if n > MAX_BYTES => Err(format!("the password takes at most {MAX_BYTES} bytes")),
        _ => Ok(()),
    }
}

/// A new PHC string for `pw` (random 16-byte salt). CPU-heavy: call off
/// the executor.
pub(crate) fn hash(pw: &str) -> Result<String, String> {
    acceptable(pw)?;
    argon2()
        .hash_password(pw.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| format!("cannot hash the password: {e}"))
}

/// The stored hash, checked: argon2id, v19, exactly this module's
/// parameters, a salt and a 32-byte output.
#[derive(Clone)]
pub(crate) struct Stored(PasswordHash);

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stored(argon2id)")
    }
}

impl Stored {
    pub(crate) fn parse(text: &str) -> Result<Stored, String> {
        let bad = |why: &str| format!("not a claudio password hash ({why})");
        let h = PasswordHash::new(text.trim_end_matches('\n')).map_err(|_| bad("PHC syntax"))?;
        if h.algorithm.as_str() != "argon2id" {
            return Err(bad("not argon2id"));
        }
        let params = Params::try_from(&h).map_err(|_| bad("parameters"))?;
        let version_ok = h.version.is_none_or(|v| v == 0x13);
        let fixed =
            params.m_cost() == M_COST_KIB && params.t_cost() == T_COST && params.p_cost() == P_COST;
        let salt_ok = h.salt.as_ref().is_some_and(|s| s.len() >= 16);
        let out_ok = h.hash.as_ref().is_some_and(|o| o.len() == 32);
        if !(version_ok && fixed && salt_ok && out_ok) {
            return Err(bad(
                "other parameters than m=19456,t=2,p=1, a 16-byte salt and a 32-byte hash",
            ));
        }
        Ok(Stored(h))
    }

    /// Whether `pw` is the password. CPU-heavy (19 MiB, ~15 ms optimised):
    /// call in `spawn_blocking`, under the login semaphore.
    pub(crate) fn verify(&self, pw: &str) -> bool {
        pw.len() <= MAX_BYTES && argon2().verify_password(pw.as_bytes(), &self.0).is_ok()
    }
}

/// Read the stored hash: a regular file of ours, not readable by others,
/// at most 512 bytes, one valid PHC string. Every failure says how to fix it.
pub(crate) fn load(path: &Path) -> Result<Stored, String> {
    let fix = "set one with `claudio web --set-password`";
    let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => format!("--remote needs a password: {fix}"),
        _ => format!("cannot read {}: {e}; {fix}", path.display()),
    })?;
    let private =
        meta.file_type().is_file() && meta.mode() & 0o077 == 0 && meta.uid() == paths::uid();
    if !private || meta.len() > FILE_MAX {
        return Err(format!(
            "{} must be a private (0600) regular file of yours holding one hash; {fix}",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}; {fix}", path.display()))?;
    Stored::parse(&text).map_err(|e| format!("{}: {e}; {fix}", path.display()))
}

/// `claudio web --set-password`: the password twice from the terminal
/// (never a pipe), hashed, written, read back and verified.
pub(crate) fn set_password() -> Result<String, String> {
    use claudio_core::term::secret::read_line_no_echo;
    let read = |prompt: &str| {
        read_line_no_echo(prompt).map_err(|e| {
            format!("--set-password reads the password from a terminal ({e}); run it in one")
        })
    };
    let pw = read("New password: ")?;
    acceptable(&pw)?;
    if read("Again: ")? != pw {
        return Err("the two passwords differ; nothing was saved".to_owned());
    }
    let phc = hash(&pw)?;
    let path = path();
    save(&path, &phc).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    if !load(&path)?.verify(&pw) {
        return Err(format!(
            "{} does not verify; run --set-password again",
            path.display()
        ));
    }
    Ok(format!(
        "password saved to {}. Restart claudio web to use it (that also signs every browser out).",
        path.display()
    ))
}

/// Write `phc` (one line) atomically, 0600.
pub(crate) fn save(path: &Path, phc: &str) -> io::Result<()> {
    paths::write_atomic(path, format!("{phc}\n").as_bytes())
}
```

**The fixture** `addons/web/web/e2e/fixtures/web-password` (one line plus `\n`; generated with
`hash("claudio-e2e-password")` above; `build.rs` skips `e2e/`, so it is never embedded):

```
$argon2id$v=19$m=19456,t=2,p=1$A1oGv2flg/OJx6oFnKzAZw$2inPbz1i0HSMtmzQMb4y8/ojf2UWanePTc73H4WA1L4
```

**Unit tests** (`password.rs`, `#[cfg(test)] mod tests`; dirs from
`claudio_core::paths::socket_test_dir("web-password")`, removed at the end):

- `the_fixture_verifies_against_the_test_password`:
  `const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");`
  `let s = Stored::parse(FIXTURE).unwrap(); assert!(s.verify("claudio-e2e-password"));
  assert!(!s.verify("claudio-e2e-passwore")); assert!(!s.verify(""));`
- `a_new_hash_is_argon2id_with_the_fixed_parameters`: `let phc = hash("twelve bytes").unwrap();
  assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$")); assert!(Stored::parse(&phc).unwrap()
  .verify("twelve bytes")); assert_ne!(hash("twelve bytes").unwrap(), phc, "a new salt each time")`.
- `password_length_is_12_to_1024_bytes`: `acceptable("x".repeat(11))` is `Err` naming 12;
  `"x".repeat(12)` and `"é".repeat(6)` (12 bytes) are `Ok`; `"x".repeat(1025)` is `Err`;
  `hash(&"x".repeat(11))` is `Err`; `Stored::parse(FIXTURE).unwrap().verify(&"x".repeat(1025))`
  is `false` (no hashing of an oversized body).
- `other_parameters_are_refused`: each of these is `Err`:
  `$argon2id$v=19$m=65536,t=2,p=1$<FIXTURE salt>$<FIXTURE hash>` (m changed),
  the same with `t=3`, with `p=2`, `$argon2i$v=19$m=19456,t=2,p=1$…`, `$argon2id$v=16$…`,
  a salt of 8 bytes (`c2FsdHNhbHQ`), a missing hash (`$argon2id$v=19$m=19456,t=2,p=1$<salt>`),
  `"not a hash"`, `""`. (Build them by `FIXTURE.replace(…)`.)
- `the_file_must_be_private_and_small`: `save(&p, &hash(...))` then
  `metadata(&p).mode() & 0o777 == 0o600` and `load(&p)` is `Ok`; chmod 0644 → `Err` containing
  `--set-password`; a symlink to a good file (`std::os::unix::fs::symlink`) → `Err`; a 600-byte
  0600 file → `Err`; a missing file → `Err` containing `--remote needs a password`.

**CLI** (`addons/web/src/lib.rs`):

- `WebArgs` gains `pub set_password: bool`; `parse_args` learns `"--set-password" => out.set_password = true`.
  Switch the iterator to `args.iter().peekable()` now (Task 4 needs `next_if`), and `value()`'s
  parameter to `&mut std::iter::Peekable<std::slice::Iter<'_, String>>`.
- After the loop, `combination_ok(&out, args.len())?;` with (Task 4 extends it):

```rust
/// The flags that do not go together.
fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
    if a.set_password && n_args != 1 {
        return Err("--set-password takes no other argument".to_owned());
    }
    Ok(())
}
```

- `USAGE` becomes a three-line string; the third line arrives in Task 4:

```rust
const USAGE: &str = "usage: claudio web [--listen 127.0.0.1:7337] [--allow-host NAME[:PORT]]... \
[--no-open] [--print-url] [--rotate-token]
       claudio web --set-password";
```

- In `run`, right after `parse_args`:

```rust
    if let Some(code) = one_shot(&args) {
        return code;
    }
```

```rust
/// `--set-password` (and, from Task 4, `--print-cert`): one thing, then exit.
fn one_shot(args: &WebArgs) -> Option<ExitCode> {
    if args.set_password {
        return Some(match password::set_password() {
            Ok(done) => {
                eprintln!("claudio web: {done}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("claudio web: {e}");
                ExitCode::from(2)
            }
        });
    }
    None
}
```

- Extend `args_parse_flags_and_defaults`: `parse_args(&args(&["--set-password"])).unwrap().set_password`;
  `parse_args(&args(&["--set-password", "--no-open"]))` is `Err`.

**Integration test** (`addons/web/tests/web.rs`, new section `// ── --set-password (AC3) ──`):

```rust
#[test]
fn web_set_password_refuses_a_pipe() {
    let h = ManagerHarness::new();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--set-password"]);
    apply_env(&h, &mut cmd);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // It may exit before reading: a broken pipe here is fine.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"a-long-enough-password\na-long-enough-password\n");
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("terminal"), "{err}");
    assert!(!err.contains("a-long-enough-password"), "never echoed");
    assert!(!h.config_home.join("claudio/web-password").exists());
}
```

The prompt itself (two reads, mismatch, too short, success message) is checked by hand (AC3):
`XDG_CONFIG_HOME=~/.cache/claudio/scratch/pw target/debug/claudio web --set-password` in a
terminal, then `cat` the file and `stat -c %a` it (600). Use that scratch `XDG_CONFIG_HOME`,
never the real config.

```bash
make check
cargo test --test web -- --test-threads=1 web_set_password   # 1 passed
git add Cargo.toml Cargo.lock addons/web/Cargo.toml addons/web/src/password.rs addons/web/src/lib.rs \
  addons/web/web/e2e/fixtures/web-password addons/web/tests/web.rs
git commit -m "feat(web): claudio web --set-password stores an argon2id hash" \
  -m "argon2 0.6.0 (RustCrypto, pure Rust, MIT/Apache-2.0) hashes and verifies the password for the coming --remote; pinned exactly because it hashes what a browser sends (D7). Parameters fixed at OWASP's baseline (m=19 MiB, t=2, p=1); a file with others is refused so the memory bound holds. The prompt reads a terminal only (claudio_core::term::secret) and verifies the written file. The committed fixture is the test password's hash (ruling R1); argon2 and blake2 build at opt-level 3 in the dev profile so debug suites verify in milliseconds."
```

---

## Task 3: login reservations and the request's peer (strong)

Today's server gets the pieces `--remote` builds on, with no visible change but `/api/me`'s 401
body: the peer from the connection, the reservation limiter (replacing the global failure
window), the local rule, and a login body that names its method.

**New `addons/web/src/peer.rs`** (`mod peer;`):

```rust
//! The TCP peer of a request, from the connection (never a header).

use std::net::{IpAddr, SocketAddr};

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use tokio::net::TcpListener;

/// The peer address as `ConnectInfo<Peer>`: claudio-web's own type, since
/// the orphan rule forbids `Connected<IncomingStream<'_, TlsListener>>` for
/// `SocketAddr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Peer(pub SocketAddr);

/// What the login limits and the connection caps count by: every loopback
/// address is one peer, an IPv4 address its own, an IPv6 address its /64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PeerKey {
    Loopback,
    V4(u32),
    V6Net(u64),
    /// A request without a peer (never from a server: it always has one).
    Unknown,
}

impl PeerKey {
    /// `ip`'s key without the loopback rule: its IPv4 address or IPv6 /64.
    pub(crate) fn of(ip: IpAddr) -> PeerKey {
        match ip.to_canonical() {
            IpAddr::V4(v4) => PeerKey::V4(u32::from(v4)),
            IpAddr::V6(v6) => PeerKey::V6Net((u128::from(v6) >> 64) as u64),
        }
    }
}

impl Peer {
    /// The canonical address: `::ffff:a.b.c.d` is `a.b.c.d`.
    pub(crate) fn ip(&self) -> IpAddr {
        self.0.ip().to_canonical()
    }

    pub(crate) fn key(&self) -> PeerKey {
        match self.ip() {
            ip if ip.is_loopback() => PeerKey::Loopback,
            ip => PeerKey::of(ip),
        }
    }
}

impl Connected<IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Peer {
        Peer(*stream.remote_addr())
    }
}
```

Tests: `keys_fold_loopback_and_ipv6_nets`: `Peer("[::ffff:127.0.0.1]:1")` and `Peer("127.0.0.9:1")`
and `Peer("[::1]:1")` → `Loopback`; `Peer("[::ffff:10.0.0.1]:1").key() == PeerKey::V4(0x0a00_0001)`;
`[2001:db8::1]` and `[2001:db8::ffff:1]` share a key; `[2001:db8:0:1::1]` does not.

**New `addons/web/src/limit.rs`** (`mod limit;`), complete:

```rust
//! The login rate limit. An attempt takes its slot **before** any check
//! (a reservation): concurrent attempts cannot all pass a "fewer than five"
//! test that none of them has failed yet. A success gives its slots back;
//! a failure keeps them for [`WINDOW`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::peer::PeerKey;

pub(crate) const WINDOW: Duration = Duration::from_secs(60);
/// Attempts per peer per window.
pub(crate) const PER_PEER: usize = 5;
/// Attempts per window over every peer (local token logins are exempt).
pub(crate) const GLOBAL: usize = 30;
/// Peers tracked at once; past it the one idle longest is forgotten.
pub(crate) const MAX_PEERS: usize = 1024;

/// A taken slot; give it to [`Limiter::release`] after a success.
#[derive(Debug)]
pub(crate) struct Reservation {
    key: PeerKey,
    id: u64,
    global: bool,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    peers: HashMap<PeerKey, VecDeque<(Instant, u64)>>,
    global: VecDeque<(Instant, u64)>,
}

#[derive(Default)]
pub(crate) struct Limiter {
    inner: Mutex<Inner>,
}

fn expire(q: &mut VecDeque<(Instant, u64)>, now: Instant) {
    while q
        .front()
        .is_some_and(|(t, _)| now.saturating_duration_since(*t) >= WINDOW)
    {
        q.pop_front();
    }
}

impl Limiter {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take a slot for `key` (and a global one when `global`), or `Err(())`
    /// (answer 429) when either window is full. Nothing is taken on `Err`.
    pub(crate) fn reserve(
        &self,
        key: PeerKey,
        global: bool,
        now: Instant,
    ) -> Result<Reservation, ()> {
        let mut inner = self.lock();
        let inner = &mut *inner;
        expire(&mut inner.global, now);
        if global && inner.global.len() >= GLOBAL {
            return Err(());
        }
        if let Some(q) = inner.peers.get_mut(&key) {
            expire(q, now);
            if q.len() >= PER_PEER {
                return Err(());
            }
        } else if inner.peers.len() >= MAX_PEERS {
            inner.peers.retain(|_, q| {
                expire(q, now);
                !q.is_empty()
            });
            if inner.peers.len() >= MAX_PEERS {
                let idle = inner
                    .peers
                    .iter()
                    .min_by_key(|(_, q)| q.back().map(|(t, _)| *t))
                    .map(|(k, _)| *k);
                if let Some(k) = idle {
                    inner.peers.remove(&k);
                }
            }
        }
        inner.next_id += 1;
        let id = inner.next_id;
        inner.peers.entry(key).or_default().push_back((now, id));
        if global {
            inner.global.push_back((now, id));
        }
        Ok(Reservation { key, id, global })
    }

    /// Give back a successful attempt's slots.
    pub(crate) fn release(&self, r: Reservation) {
        let mut inner = self.lock();
        if let Some(q) = inner.peers.get_mut(&r.key) {
            q.retain(|(_, id)| *id != r.id);
            if q.is_empty() {
                inner.peers.remove(&r.key);
            }
        }
        if r.global {
            inner.global.retain(|(_, id)| *id != r.id);
        }
    }
}
```

Its tests (all five passed in the prototype):

```rust
    use super::*;

    const A: PeerKey = PeerKey::V4(0x0a00_0001);
    const B: PeerKey = PeerKey::V4(0x0a00_0002);

    #[test]
    fn a_peer_gets_five_attempts_a_minute_reserved_before_any_check() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..PER_PEER {
            l.reserve(A, true, t0).unwrap();
        }
        assert!(l.reserve(A, true, t0).is_err(), "the sixth, before any of the five failed");
        assert!(l.reserve(B, true, t0).is_ok(), "another peer is not affected");
        assert!(l.reserve(A, true, t0 + Duration::from_secs(59)).is_err());
        assert!(l.reserve(A, true, t0 + WINDOW).is_ok(), "the window moved");
    }

    #[test]
    fn a_success_gives_its_slots_back() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..PER_PEER - 1 {
            l.reserve(A, true, t0).unwrap();
        }
        let ok = l.reserve(A, true, t0).unwrap();
        l.release(ok);
        assert!(l.reserve(A, true, t0).is_ok());
        assert!(l.reserve(A, true, t0).is_err());
    }

    #[test]
    fn the_global_window_bounds_a_spread_attack_but_spares_exempt_attempts() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..GLOBAL as u32 {
            l.reserve(PeerKey::V4(i), true, t0).unwrap();
        }
        assert!(l.reserve(PeerKey::V4(9999), true, t0).is_err());
        assert!(l.reserve(PeerKey::Loopback, false, t0).is_ok(), "a local token login");
        assert!(l.reserve(PeerKey::V4(9999), true, t0 + WINDOW).is_ok());
    }

    #[test]
    fn at_most_max_peers_are_tracked() {
        let l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..(MAX_PEERS as u32 + 10) {
            let _ = l.reserve(PeerKey::V4(i), false, t0 + Duration::from_millis(i as u64));
        }
        assert!(l.lock().peers.len() <= MAX_PEERS);
    }

    #[test]
    fn concurrent_attempts_from_one_peer_take_at_most_five_slots() {
        let l = std::sync::Arc::new(Limiter::default());
        let t0 = Instant::now();
        let ok: usize = (0..20)
            .map(|_| {
                let l = std::sync::Arc::clone(&l);
                std::thread::spawn(move || l.reserve(A, true, t0).is_ok())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum();
        assert_eq!(ok, PER_PEER);
    }
```

**`addons/web/src/auth.rs` changes:**

- Delete `MAX_FAILURES`, `FAILURE_WINDOW`, `Inner::failures`, `Auth::login`, `LoginBody` and the
  `login` handler (it moves to `login.rs`); drop the `VecDeque` and `serde::Deserialize` imports.
  `set_cookie` becomes `pub(crate)`.
- `Allowed` gains `local_hosts: Vec<String>` and `is_local_host`; `new` builds both lists
  (behaviour of `host_ok`, `origin_ok`, `secure_cookie` unchanged; the four existing `Allowed`
  tests pass untouched):

```rust
/// `name:port`, and `name` alone when `port` is the scheme's default.
fn at_port(name: &str, port: u16, default: u16) -> Vec<String> {
    let mut v = vec![format!("{name}:{port}")];
    if port == default {
        v.push(name.to_owned());
    }
    v
}

/// An IP as a `Host` names it: IPv6 in brackets.
fn host_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

impl Allowed {
    pub(crate) fn new(listen: SocketAddr, allow_hosts: &[String]) -> Allowed {
        let port = listen.port();
        let mut local_hosts = at_port(&host_ip(listen.ip()), port, 80);
        for name in ["127.0.0.1", "localhost", "[::1]"] {
            local_hosts.extend(at_port(name, port, 80));
        }
        let mut hosts = local_hosts.clone();
        for name in allow_hosts {
            let name = name.trim().to_ascii_lowercase();
            if !name.contains(':') {
                hosts.push(format!("{name}:{port}"));
            }
            hosts.push(name);
        }
        for list in [&mut hosts, &mut local_hosts] {
            list.sort();
            list.dedup();
        }
        Allowed { hosts, local_hosts, proxied: !allow_hosts.is_empty() }
    }

    /// Whether `host` is one of the loopback forms at the listen port.
    pub(crate) fn is_local_host(&self, host: &str) -> bool {
        self.local_hosts.contains(&host.trim().to_ascii_lowercase())
    }
}
```

  (Task 4 turns the tail into `Allowed::sorted(…)` when it adds fields.)
- Imports: `use crate::limit::Limiter;` and `use crate::peer::{Peer, PeerKey};`.
- New items:

```rust
/// A request's sender as the login rules see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caller {
    pub local: bool,
    pub key: PeerKey,
}

/// The request's `Host` (or its URI authority), `""` when none.
pub(crate) fn host_of(req: &Request) -> &str {
    req.headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()))
        .unwrap_or("")
}

/// The TCP peer, from the connection (`ConnectInfo`), never a header.
pub(crate) fn peer_of(req: &Request) -> Option<Peer> {
    req.extensions()
        .get::<axum::extract::ConnectInfo<Peer>>()
        .map(|c| c.0)
}
```

  and on `Auth` (which gains `pub(crate) limiter: Limiter`, initialised `Limiter::default()`;
  `allowed` becomes `pub(crate)`):

```rust
    /// Who sent a request: its key for the limits, and whether it is local
    /// (a loopback peer that used a loopback `Host`). No peer: not local.
    pub(crate) fn caller(&self, peer: Option<Peer>, host: &str) -> Caller {
        let Some(peer) = peer else {
            return Caller { local: false, key: PeerKey::Unknown };
        };
        Caller {
            local: peer.ip().is_loopback() && self.allowed.is_local_host(host),
            key: peer.key(),
        }
    }

    /// Whether `token` is the token, in constant time.
    pub(crate) fn token_ok(&self, token: &str) -> bool {
        ct_eq(token.as_bytes(), self.token.as_bytes())
    }
```

- `check` uses `host_of(req)` instead of its inline Host lookup (same order, same answers).
- `guard`: a 401 on `/api/me` carries the login methods (the only new output):

```rust
    let mut res = match check(&auth, &req) {
        Ok(()) => next.run(req).await,
        Err(StatusCode::UNAUTHORIZED) if path == "/api/me" => {
            let body = serde_json::json!({ "login": ["token"] });
            (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
        }
        Err(status) => status.into_response(),
    };
```

- The old `login_locks_after_five_failures_for_a_minute` test moves to `login.rs` (below) with
  the same assertions (E2).
- New test `local_means_a_loopback_peer_and_a_loopback_host` (plain mode):
  `let a = Auth::new("ab".repeat(32), Allowed::new(lo(7337), &names(&["proxy.example"])), csp(None)).unwrap();`
  local: `(127.0.0.1:5000, "127.0.0.1:7337")`, `([::ffff:127.0.0.1]:5000, "localhost:7337")`,
  `([::1]:5000, "[::1]:7337")`; not local: `(127.0.0.1:5000, "proxy.example")`,
  `(192.168.1.5:5000, "127.0.0.1:7337")`, `(None, "127.0.0.1:7337")`; keys:
  `caller(127.0.0.9:1, _).key == PeerKey::Loopback`, `caller(None, _).key == PeerKey::Unknown`.

**New `addons/web/src/login.rs`** (`mod login;`; the router's `/api/login` route becomes
`post(login::login)`), in this task's form:

```rust
//! `POST /api/login`: a token (local requests, or any without `--remote`)
//! or, from Task 5, the password. Every attempt reserves its rate-limit slot
//! before anything is compared or hashed (`limit.rs`).

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::auth::{self, Auth, Caller};

/// Exactly one of the two. Unknown fields are ignored, as before.
#[derive(Deserialize)]
struct LoginBody {
    token: Option<String>,
    password: Option<String>,
}

pub(crate) async fn login(State(auth): State<Arc<Auth>>, req: Request) -> Response {
    let caller = auth.caller(auth::peer_of(&req), auth::host_of(&req));
    let body = match axum::body::to_bytes(req.into_body(), auth::LOGIN_BODY_MAX).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let status = match serde_json::from_slice(&body) {
        Ok(LoginBody { token: Some(token), password: None }) => {
            token_login(&auth, &caller, &token, Instant::now())
        }
        // No password login without --remote (Task 5 adds it).
        _ => Err(StatusCode::BAD_REQUEST),
    };
    if let Err(status) = status {
        tracing::info!(status = status.as_u16(), local = caller.local, "web: login refused");
        return status.into_response();
    }
    match auth.create_session(Instant::now()) {
        Ok(value) => {
            let mut res = StatusCode::NO_CONTENT.into_response();
            let cookie = auth::set_cookie(&value, auth.allowed.secure_cookie());
            res.headers_mut().insert(header::SET_COOKIE, cookie);
            res
        }
        Err(e) => {
            tracing::error!(error = %e, "web: no random bytes for a session");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A token attempt: its slot first (local ones are exempt from the global
/// window), then the constant-time compare. Task 4 adds the local rule.
fn token_login(auth: &Auth, caller: &Caller, token: &str, now: Instant) -> Result<(), StatusCode> {
    let slot = auth
        .limiter
        .reserve(caller.key, !caller.local, now)
        .map_err(|()| StatusCode::TOO_MANY_REQUESTS)?;
    if auth.token_ok(token) {
        auth.limiter.release(slot);
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
```

Tests in `login.rs`:

```rust
    use super::*;
    use crate::auth::{csp, Allowed};
    use crate::peer::PeerKey;
    use std::time::Duration;

    fn plain() -> Auth {
        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], 7337));
        Auth::new("ab".repeat(32), Allowed::new(lo, &[]), csp(None)).unwrap()
    }

    const LOCAL: Caller = Caller { local: true, key: PeerKey::Loopback };

    #[test]
    fn token_logins_lock_after_five_failures_for_a_minute() {
        let a = plain();
        let right = "ab".repeat(32);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(token_login(&a, &LOCAL, "wrong", t0), Err(StatusCode::UNAUTHORIZED));
        }
        assert_eq!(token_login(&a, &LOCAL, "wrong", t0), Err(StatusCode::TOO_MANY_REQUESTS));
        let t59 = t0 + Duration::from_secs(59);
        assert_eq!(token_login(&a, &LOCAL, &right, t59), Err(StatusCode::TOO_MANY_REQUESTS));
        let t61 = t0 + Duration::from_secs(61);
        assert_eq!(token_login(&a, &LOCAL, &right, t61), Ok(()));
    }

    #[test]
    fn successes_do_not_count() {
        let a = plain();
        let t0 = Instant::now();
        for _ in 0..20 {
            assert_eq!(token_login(&a, &LOCAL, &"ab".repeat(32), t0), Ok(()));
        }
    }
```

**`addons/web/src/lib.rs`:** serve with the peer:

```rust
    let app = router(state).into_make_service_with_connect_info::<peer::Peer>();
    axum::serve(listener, app)
        .with_graceful_shutdown(stop)
        .await
        .map_err(|e| format!("server error: {e}"))
```

**Integration test** (`web.rs`, after `web_login_is_rate_limited`; that test and every other
existing one stay unchanged):

```rust
#[test]
fn web_login_bodies_and_me_name_the_methods() {
    let web = Web::start();
    let me = web.get("/api/me", &[]);
    assert_eq!(me.status, 401);
    let body: Value = serde_json::from_str(&me.body).unwrap();
    assert_eq!(body["login"], json!(["token"]));
    let origin = web.origin();
    let post = |body: Value| web.post("/api/login", Some(&origin), &body.to_string(), &[]).status;
    assert_eq!(post(json!({"password": "twelve bytes!"})), 400, "no password without --remote");
    assert_eq!(post(json!({"token": web.token, "password": "x"})), 400);
    assert_eq!(post(json!({})), 400);
    assert_eq!(post(json!({"token": web.token, "extra": 1})), 204, "unknown fields ignored");
}
```

```bash
make check
cargo test --test web --test web_manager -- --test-threads=1     # every test passes
cargo test --test vocdoni_web -- --test-threads=1
git add addons/web/src/peer.rs addons/web/src/limit.rs addons/web/src/login.rs \
  addons/web/src/auth.rs addons/web/src/lib.rs addons/web/tests/web.rs
git commit -m "feat(web): login attempts are reserved per peer before any check" \
  -m "The server now knows each request's TCP peer (ConnectInfo<Peer>) and whether the request is local (a loopback peer with a loopback Host). The login limit becomes a reservation taken before the token is compared, 5 per peer and 30 global per minute, so concurrent attempts cannot pass it; without --remote every peer is the one loopback key and nothing observable changes. /api/me's 401 names the login methods (docs/web-protocol.md)."
```

---

## Task 4: `--remote` serves TLS (strong)

One commit: the certificate store, the TLS listener, the `--remote` CLI and start-up, the
`--remote` `Host`/`Origin` set and the token rule. Remote peers cannot log in yet (no password
login until Task 5): fail closed.

**Dependencies** (root `[workspace.dependencies]`, alphabetical, with this comment block):

```toml
# `claudio web --remote` (claudio-web only). TLS on ring, the provider the
# tree already builds through reqwest: never aws_lc_rs (C and cmake; it
# breaks `make static`, and two providers break rustls' default lookup).
# rustls and tokio-rustls parse what the network sends: pinned exactly (D7).
rustls = { version = "=0.23.45", default-features = false, features = ["ring", "std", "tls12"] }
tokio-rustls = { version = "=0.26.6", default-features = false, features = ["ring", "tls12"] }
# The --remote self-signed certificate (local input only).
rcgen = { version = "0.14", default-features = false, features = ["crypto", "pem", "ring"] }
# Which names the kept certificate covers; already in the tree via rustls.
rustls-webpki = { version = "0.103", default-features = false, features = ["std"] }
```

`addons/web/Cargo.toml` `[dependencies]`: `rcgen`, `rustls`, `rustls-webpki`, `tokio-rustls`
(`.workspace = true`). Root `[dev-dependencies]`: `tokio-rustls.workspace = true` (the suites'
pinned client). Then `cargo build` and check `cargo tree -e normal --target all | grep -c aws-lc`
prints `0`.

**New `addons/web/src/tls/mod.rs`:** `pub(crate) mod cert;` and `pub(crate) mod listener;`
(`mod tls;` in `lib.rs`).

**New `addons/web/src/tls/cert.rs`**, complete (uses `rustls::…` directly):

```rust
//! The `--remote` certificate: self-signed, ECDSA P-256, made once and kept
//! in `<config>/web-tls/{key,cert}.pem` (0600, in a 0700 directory) so its
//! fingerprint stays what the member checked. `--rotate-cert` replaces it.

use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};

use claudio_core::paths;

/// A PEM file larger than this is not one we wrote.
const PEM_MAX: u64 = 16 * 1024;
const VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);
const BACKDATE: Duration = Duration::from_secs(24 * 60 * 60);

/// `<config>/web-tls`.
pub(crate) fn dir() -> PathBuf {
    paths::config_dir().join("web-tls")
}

/// The certificate in use and its TLS server config.
pub(crate) struct Cert {
    pub der: CertificateDer<'static>,
    pub config: Arc<ServerConfig>,
}

impl Cert {
    /// SHA-256 of the DER certificate, uppercase hex, colon-separated (what
    /// browsers show).
    pub(crate) fn fingerprint(&self) -> String {
        fingerprint(&self.der)
    }

    /// Whether the certificate covers `name` (a host name or an IP literal).
    pub(crate) fn covers(&self, name: &str) -> bool {
        let Ok(cert) = webpki::EndEntityCert::try_from(&self.der) else {
            return false;
        };
        let server_name = match name.parse::<IpAddr>() {
            Ok(ip) => ServerName::IpAddress(ip.into()),
            Err(_) => match ServerName::try_from(name) {
                Ok(n) => n,
                Err(_) => return false,
            },
        };
        cert.verify_is_valid_for_subject_name(&server_name).is_ok()
    }
}

pub(crate) fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The kept certificate, or a new one for `names` when there is none or
/// `rotate`. An unreadable, foreign, non-private or mismatched pair is an
/// error naming `--rotate-cert`: never replaced silently. Blocking I/O.
pub(crate) fn load_or_create(dir: &Path, names: &[String], rotate: bool) -> Result<Cert, String> {
    let (key_path, cert_path) = (dir.join("key.pem"), dir.join("cert.pem"));
    paths::ensure_private_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let exists = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if rotate || (!exists(&key_path) && !exists(&cert_path)) {
        let (key_pem, cert_pem) = generate(names)?;
        // The key first: a crash in between leaves a mismatched pair, which
        // the next start refuses (a key mismatch) rather than reuses.
        paths::write_atomic(&key_path, key_pem.as_bytes())
            .and_then(|()| paths::write_atomic(&cert_path, cert_pem.as_bytes()))
            .map_err(|e| format!("cannot write {}: {e}", dir.display()))?;
    }
    let fix = |what: String| {
        format!("{what}; `claudio web --remote --rotate-cert` makes a new certificate")
    };
    let key_pem = read_private(&key_path).map_err(fix)?;
    let cert_pem = read_private(&cert_path).map_err(fix)?;
    let der = CertificateDer::from_pem_slice(cert_pem.as_bytes())
        .map_err(|e| fix(format!("{}: {e}", cert_path.display())))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|e| fix(format!("{}: {e}", key_path.display())))?;
    let config =
        server_config(der.clone(), key).map_err(|e| fix(format!("{}: {e}", dir.display())))?;
    Ok(Cert { der, config })
}

/// `--print-cert`: the kept `cert.pem`, checked like every read of it.
pub(crate) fn read_cert_pem(dir: &Path) -> Result<String, String> {
    let path = dir.join("cert.pem");
    if std::fs::symlink_metadata(&path).is_err() {
        return Err("no certificate yet: start `claudio web --remote` once to make it".to_owned());
    }
    let pem = read_private(&path)?;
    CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(pem)
}

/// A private regular file of ours, at most [`PEM_MAX`] bytes, as text.
fn read_private(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let private =
        meta.file_type().is_file() && meta.mode() & 0o077 == 0 && meta.uid() == paths::uid();
    if !private || meta.len() > PEM_MAX {
        return Err(format!(
            "{} is not a private (0600) regular file of yours",
            path.display()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A new key and self-signed certificate (PEM): CN `claudio web`, SANs
/// `names`, valid from a day ago for ten years.
fn generate(names: &[String]) -> Result<(String, String), String> {
    let err = |e: rcgen::Error| format!("cannot make a certificate: {e}");
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(err)?;
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(err)?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "claudio web");
    params.distinguished_name = dn;
    let now = SystemTime::now();
    params.not_before = (now - BACKDATE).into();
    params.not_after = (now + VALIDITY).into();
    let cert = params.self_signed(&key).map_err(err)?;
    Ok((key.serialize_pem(), cert.pem()))
}

/// TLS 1.2 and 1.3 with ring (named, never the process default), no
/// client auth, ALPN `http/1.1`. Fails when the key does not match.
fn server_config(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
```

Unit tests (`cert.rs`; a dir from `paths::socket_test_dir("web-cert")`, removed at the end;
`NAMES = ["localhost", "127.0.0.1", "::1", "box", "box.local"]`):

- `a_certificate_is_made_once_and_kept`: first `load_or_create(&dir, &NAMES, false)` → fingerprint
  F (95 chars: 32 uppercase hex pairs with colons); second call → F; files 0600, dir 0700;
  `covers` true for each of NAMES, false for `other.example` and `10.0.0.1`. (`notBefore` a day
  back is reviewed in code, `BACKDATE`; no test parses it.)
- `rotate_replaces_it`: `load_or_create(&dir, &NAMES, true)` → fingerprint ≠ F.
- `a_bad_pair_is_refused_never_replaced`: (a) replace `key.pem` with another dir's key (0600)
  → `Err` containing `--rotate-cert` and `cert.pem` unchanged (bytes equal before/after);
  (b) chmod `cert.pem` 0644 → `Err` containing `--rotate-cert`; (c) delete `key.pem` only →
  `Err` (never regenerated: `cert.pem` bytes unchanged); (d) `cert.pem` a symlink to a good
  0600 file → `Err`; (e) a 17 KiB 0600 `cert.pem` → `Err`.
- `print_cert_reads_only_the_certificate`: `read_cert_pem(&dir)` starts with
  `-----BEGIN CERTIFICATE-----`, has no `PRIVATE KEY`; on an empty dir → `Err` containing
  `claudio web --remote`.

**New `addons/web/src/tls/listener.rs`**, complete:

```rust
//! The `--remote` listener: an `axum::serve::Listener` that accepts TCP,
//! runs each TLS handshake in its own task (10 s deadline, at most 64 at
//! once) and hands finished streams to `accept` through a bounded channel,
//! so one slow client never stalls the others. Open connections are capped
//! (256 overall, 32 per peer but loopback); a connection that has not sent
//! the end of its first request's headers within 30 s is closed (axum's
//! hyper builder has no timer).

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::time::Sleep;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

use crate::peer::{Peer, PeerKey};

/// The listener's bounds; [`Limits::DEFAULT`] in a server, shorter in tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub handshake: Duration,
    pub handshakes: usize,
    pub connections: usize,
    pub per_peer: usize,
    pub headers: Duration,
}

impl Limits {
    pub(crate) const DEFAULT: Limits = Limits {
        handshake: Duration::from_secs(10),
        handshakes: 64,
        connections: 256,
        per_peer: 32,
        headers: Duration::from_secs(30),
    };
}

/// Open connections per peer key (loopback is not counted).
type PeerCounts = Arc<Mutex<HashMap<PeerKey, usize>>>;

pub(crate) struct TlsListener {
    rx: mpsc::Receiver<(TlsIo, Peer)>,
    local: SocketAddr,
    accept_loop: tokio::task::JoinHandle<()>,
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

impl TlsListener {
    pub(crate) fn new(
        tcp: TcpListener,
        acceptor: TlsAcceptor,
        limits: Limits,
    ) -> io::Result<TlsListener> {
        let local = tcp.local_addr()?;
        let (tx, rx) = mpsc::channel(limits.handshakes);
        let accept_loop = tokio::spawn(accept_loop(tcp, acceptor, tx, limits));
        Ok(TlsListener { rx, local, accept_loop })
    }
}

async fn accept_loop(
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    tx: mpsc::Sender<(TlsIo, Peer)>,
    limits: Limits,
) {
    let handshakes = Arc::new(Semaphore::new(limits.handshakes));
    let connections = Arc::new(Semaphore::new(limits.connections));
    let per_peer: PeerCounts = Arc::default();
    loop {
        let (stream, addr) = match tcp.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                // EMFILE and friends: back off as axum's own listener does.
                tracing::debug!(error = %e, "web: tcp accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let peer = Peer(addr);
        // Every refusal drops the stream: the client sees a reset.
        let Ok(handshake) = Arc::clone(&handshakes).try_acquire_owned() else {
            continue;
        };
        let Ok(open) = Arc::clone(&connections).try_acquire_owned() else {
            continue;
        };
        let Some(slot) = PeerSlot::take(&per_peer, peer, limits.per_peer) else {
            continue;
        };
        let (acceptor, tx) = (acceptor.clone(), tx.clone());
        tokio::spawn(async move {
            let tls = match tokio::time::timeout(limits.handshake, acceptor.accept(stream)).await {
                Ok(Ok(tls)) => tls,
                Ok(Err(e)) => {
                    tracing::debug!(peer = %peer.0, error = %e, "web: TLS handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::debug!(peer = %peer.0, "web: TLS handshake timed out");
                    return;
                }
            };
            drop(handshake);
            let io = TlsIo {
                inner: tls,
                header_deadline: Some(Box::pin(tokio::time::sleep(limits.headers))),
                tail: [0; 3],
                _open: open,
                _slot: slot,
            };
            let _ = tx.send((io, peer)).await;
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsIo;
    type Addr = Peer;

    async fn accept(&mut self) -> (TlsIo, Peer) {
        match self.rx.recv().await {
            Some(pair) => pair,
            // The accept loop ended (only at drop): wait; the graceful
            // shutdown stops the server.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Peer> {
        Ok(Peer(self.local))
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Peer {
        *stream.remote_addr()
    }
}

/// A peer's share of `Limits::per_peer`, given back on drop.
struct PeerSlot {
    counts: PeerCounts,
    key: Option<PeerKey>,
}

impl PeerSlot {
    fn take(counts: &PeerCounts, peer: Peer, max: usize) -> Option<PeerSlot> {
        let key = peer.key();
        if key == PeerKey::Loopback {
            return Some(PeerSlot { counts: Arc::clone(counts), key: None });
        }
        let mut map = counts.lock().unwrap_or_else(|e| e.into_inner());
        let n = map.entry(key).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(PeerSlot { counts: Arc::clone(counts), key: Some(key) })
    }
}

impl Drop for PeerSlot {
    fn drop(&mut self) {
        let Some(key) = self.key else { return };
        let mut map = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = map.get_mut(&key) {
            *n -= 1;
            if *n == 0 {
                map.remove(&key);
            }
        }
    }
}

/// A served TLS connection: its permits travel with it, and its first
/// request's headers have `Limits::headers` to arrive.
pub(crate) struct TlsIo {
    inner: TlsStream<TcpStream>,
    /// Until the first `\r\n\r\n` was read.
    header_deadline: Option<Pin<Box<Sleep>>>,
    /// The last three bytes read, to find `\r\n\r\n` across reads.
    tail: [u8; 3],
    _open: OwnedSemaphorePermit,
    _slot: PeerSlot,
}

impl TlsIo {
    /// Whether `\r\n\r\n` ends within `tail` + `new`.
    fn saw_headers_end(&mut self, new: &[u8]) -> bool {
        let mut window = Vec::with_capacity(self.tail.len() + new.len());
        window.extend_from_slice(&self.tail);
        window.extend_from_slice(new);
        let found = window.windows(4).any(|w| w == b"\r\n\r\n");
        // `window` holds at least the three tail bytes.
        self.tail.copy_from_slice(&window[window.len() - 3..]);
        found
    }
}

impl AsyncRead for TlsIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if let Some(deadline) = this.header_deadline.as_mut() {
            if deadline.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no request headers in time",
                )));
            }
        }
        let before = buf.filled().len();
        let res = Pin::new(&mut this.inner).poll_read(cx, buf);
        if this.header_deadline.is_some()
            && matches!(res, Poll::Ready(Ok(())))
            && this.saw_headers_end(&buf.filled()[before..])
        {
            this.header_deadline = None;
        }
        res
    }
}

impl AsyncWrite for TlsIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}
```

Unit tests (`listener.rs`, `#[tokio::test]`; a certificate from `cert::load_or_create` in a
`socket_test_dir`; a router with one route `GET /` whose handler is
`|ConnectInfo(p): ConnectInfo<Peer>| async move { p.0.to_string() }`, served with `axum::serve(listener,
app.into_make_service_with_connect_info::<Peer>())` in a spawned task; a client
`rustls::ClientConfig` with a test-only `ServerCertVerifier` that accepts exactly the expected
SHA-256 and checks signatures with `rustls::crypto::verify_tls1{2,3}_signature` — the same
`Pinned` verifier as the harness below, duplicated in the test module):

- `the_listener_serves_tls_and_passes_the_peer`: a pinned GET answers `HTTP/1.1 200` and its
  body is the client's own `local_addr()`; a client pinning another fingerprint fails the
  handshake; plain `GET / HTTP/1.1` over TCP gets bytes that do not start with `HTTP/`
  (a TLS alert), read to EOF within 5 s.
- `a_connection_without_headers_is_closed` (`Limits { headers: 300 ms, ..DEFAULT }`): a pinned
  connection that writes `GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n` (no blank line) reads EOF
  within 2 s; one that completes its headers is answered 200 (the prototype measured 301 ms).
- `a_stalled_handshake_does_not_block_others` (`Limits { handshake: 500 ms, ..DEFAULT }`): open
  a TCP connection that sends nothing; meanwhile a pinned GET on a second connection answers
  200 within 1 s; the silent one is closed (EOF) within 2 s.
- `per_peer_slots_are_counted_and_given_back`: `PeerSlot::take` for `Peer(10.0.0.1:1)` with
  `max = 2` twice → `Some`, third → `None`; drop one → `Some` again; loopback peers are never
  refused (`max = 0` still `Some`); after dropping all, the map is empty.

**New `addons/web/src/remote.rs`** (`mod remote;`), complete except `Remote.password` (Task 5):

```rust
//! `--remote`: its address grammar, the names its certificate and `Host`
//! check use, and what it prints at start.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::DEFAULT_PORT;

/// `--remote`'s value: `ADDR`, `ADDR:PORT`, `:PORT`, `[V6]` or `[V6]:PORT`
/// (an IPv6 address needs brackets). The port defaults to 7337, the
/// address to `0.0.0.0`.
pub(crate) fn remote_addr(v: &str) -> Result<SocketAddr, String> {
    let bad = || {
        format!(
            "--remote {v:?}: not ADDR, ADDR:PORT, :PORT or [V6]:PORT \
             (an IPv6 address needs brackets: --remote [::]:7337)"
        )
    };
    let port = |p: &str| {
        p.parse::<u16>()
            .ok()
            .filter(|n| *n > 0 && p.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(bad)
    };
    if let Some(p) = v.strip_prefix(':') {
        return Ok(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port(p)?));
    }
    if let Some(rest) = v.strip_prefix('[') {
        let (ip, tail) = rest.split_once(']').ok_or_else(bad)?;
        let ip: Ipv6Addr = ip.parse().map_err(|_| bad())?;
        let port = match tail {
            "" => DEFAULT_PORT,
            t => port(t.strip_prefix(':').ok_or_else(bad)?)?,
        };
        return Ok(SocketAddr::new(ip.into(), port));
    }
    let (ip, p) = match v.split_once(':') {
        Some((ip, p)) => (ip, port(p)?),
        None => (v, DEFAULT_PORT),
    };
    let ip: Ipv4Addr = ip.parse().map_err(|_| bad())?;
    Ok(SocketAddr::new(ip.into(), p))
}

/// The host name as a DNS name (lowercase `[a-z0-9.-]`), if it is one.
pub(crate) fn dns_name(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let ok = (1..=253).contains(&host.len())
        && !host.starts_with(['-', '.'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    ok.then_some(host)
}

/// This machine's names: the host name and its mDNS `<first label>.local`.
pub(crate) fn own_names(hostname: &str) -> Vec<String> {
    let Some(host) = dns_name(hostname) else {
        return Vec::new();
    };
    let short = host.split('.').next().unwrap_or_default().to_owned();
    let mut names = vec![host];
    let local = format!("{short}.local");
    if !names.contains(&local) {
        names.push(local);
    }
    names
}

/// The certificate's SANs: loopback names, this machine's names, the
/// listen address when specific, and each `--allow-host` name (no port).
pub(crate) fn cert_names(hostname: &str, listen: SocketAddr, allow_hosts: &[String]) -> Vec<String> {
    let mut names: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).to_vec();
    names.extend(own_names(hostname));
    if !listen.ip().is_unspecified() {
        names.push(listen.ip().to_string());
    }
    for name in allow_hosts {
        names.push(name.split(':').next().unwrap_or_default().to_owned());
    }
    let mut seen = Vec::new();
    names.retain(|n| {
        let new = !seen.contains(n);
        seen.push(n.clone());
        new
    });
    names
}

/// A `Host` form of `ip`: IPv6 in brackets.
fn url_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

/// The URL a local browser uses with the token, when a loopback request
/// can reach the listener: the address itself when loopback, `127.0.0.1`
/// for `0.0.0.0`, `[::1]` for `[::]`.
pub(crate) fn token_url(listen: SocketAddr, token: &str) -> Option<String> {
    let host = match listen.ip() {
        ip if ip.is_loopback() => url_host(ip),
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_owned(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_owned(),
        _ => return None,
    };
    Some(format!("https://{host}:{}/#t={token}", listen.port()))
}

/// The one stdout URL: the specific address, else the host name.
pub(crate) fn public_url(listen: SocketAddr, hostname: &str) -> String {
    let host = if listen.ip().is_unspecified() {
        dns_name(hostname).unwrap_or_else(|| "localhost".to_owned())
    } else {
        url_host(listen.ip())
    };
    format!("https://{host}:{}/", listen.port())
}

/// What `--remote` prints at start, every line prefixed `claudio web: `.
pub(crate) struct Banner {
    pub stdout: String,
    pub stderr: Vec<String>,
}

/// The facts the banner shows.
pub(crate) struct BannerFacts<'a> {
    pub listen: SocketAddr,
    pub hostname: &'a str,
    pub fingerprint: &'a str,
    /// The SAN candidates the kept certificate covers, and those it lacks.
    pub covered: &'a [String],
    pub missing: &'a [String],
    pub token_url: Option<&'a str>,
}

pub(crate) fn banner(f: &BannerFacts<'_>) -> Banner {
    let mut err = vec![
        format!("TLS certificate SHA-256 {}", f.fingerprint),
        format!("the certificate names {}", f.covered.join(", ")),
    ];
    if !f.missing.is_empty() {
        err.push(format!(
            "the certificate does not name {}: a browser also warns about that name; \
             --rotate-cert adds it (the fingerprint changes)",
            f.missing.join(", ")
        ));
    }
    if f.listen.ip().is_unspecified() {
        let local = own_names(f.hostname)
            .pop()
            .unwrap_or_else(|| "this machine's address".to_owned());
        err.push(format!(
            "listening on every interface, port {}; a host firewall (ufw, firewalld, macOS's) \
             may still block it. Other machines use {local} or this machine's address",
            f.listen.port()
        ));
    }
    if let Some(url) = f.token_url {
        err.push(format!("on this machine: {url}"));
    }
    err.push("other machines log in with the password (claudio web --set-password)".to_owned());
    Banner {
        stdout: format!("claudio web: {}", public_url(f.listen, f.hostname)),
        stderr: err.into_iter().map(|l| format!("claudio web: {l}")).collect(),
    }
}

/// What `--remote` serves with, made before the runtime starts.
pub(crate) struct Remote {
    pub addr: SocketAddr,
    pub cert: crate::tls::cert::Cert,
    pub allowed: crate::auth::Allowed,
    pub banner: Banner,
}

/// Check the password file first (nothing is made or bound without one),
/// then load or make the certificate. Blocking I/O.
pub(crate) fn prepare(args: &crate::WebArgs, addr: SocketAddr, token: &str) -> Result<Remote, String> {
    // Task 5 keeps the loaded hash (`Remote.password`).
    crate::password::load(&crate::password::path())?;
    let hostname = claudio_core::sys::hostname();
    let names = cert_names(&hostname, addr, &args.allow_hosts);
    let cert = crate::tls::cert::load_or_create(&crate::tls::cert::dir(), &names, args.rotate_cert)?;
    let (covered, missing): (Vec<String>, Vec<String>) =
        names.into_iter().partition(|n| cert.covers(n));
    let token_url = token_url(addr, token);
    let banner = banner(&BannerFacts {
        listen: addr,
        hostname: &hostname,
        fingerprint: &cert.fingerprint(),
        covered: &covered,
        missing: &missing,
        token_url: token_url.as_deref(),
    });
    let mut allowed_names = own_names(&hostname);
    allowed_names.extend(args.allow_hosts.iter().cloned());
    Ok(Remote {
        addr,
        cert,
        allowed: crate::auth::Allowed::remote(addr, &allowed_names),
        banner,
    })
}
```

Unit tests (`remote.rs`, last in the file: clippy's `items_after_test_module`):

```rust
    use super::*;

    #[test]
    fn remote_addresses_follow_the_grammar() {
        let ok = |v: &str| remote_addr(v).unwrap().to_string();
        assert_eq!(ok("0.0.0.0"), "0.0.0.0:7337");
        assert_eq!(ok(":8443"), "0.0.0.0:8443");
        assert_eq!(ok("192.168.1.20:9000"), "192.168.1.20:9000");
        assert_eq!(ok("[::]"), "[::]:7337");
        assert_eq!(ok("[::]:8443"), "[::]:8443");
        assert_eq!(ok("[fe80::1]:7337"), "[fe80::1]:7337");
        for bad in [
            "::1", "fe80::1:7337", "[::1", "[::1]8443", ":0", ":x", "host:7337", "1.2.3.4:",
            "1.2.3.4:+1", "", "localhost",
        ] {
            assert!(remote_addr(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_cover_loopback_the_host_and_allowed_names() {
        let any: SocketAddr = "0.0.0.0:7337".parse().unwrap();
        let n = cert_names("Box.lan", any, &["claudio.test:7337".to_owned()]);
        assert_eq!(n, ["localhost", "127.0.0.1", "::1", "box.lan", "box.local", "claudio.test"]);
        let n = cert_names("bad_name", "192.168.1.20:7337".parse().unwrap(), &[]);
        assert_eq!(n, ["localhost", "127.0.0.1", "::1", "192.168.1.20"]);
    }

    #[test]
    fn urls_and_the_banner() {
        let any: SocketAddr = "0.0.0.0:7337".parse().unwrap();
        assert_eq!(token_url(any, "t").as_deref(), Some("https://127.0.0.1:7337/#t=t"));
        assert_eq!(token_url("[::]:1".parse().unwrap(), "t").as_deref(), Some("https://[::1]:1/#t=t"));
        assert_eq!(token_url("127.0.0.1:5".parse().unwrap(), "t").as_deref(), Some("https://127.0.0.1:5/#t=t"));
        assert_eq!(token_url("192.168.1.20:5".parse().unwrap(), "t"), None);
        assert_eq!(public_url(any, "box"), "https://box:7337/");
        assert_eq!(public_url("127.0.0.1:5".parse().unwrap(), "box"), "https://127.0.0.1:5/");
        let b = banner(&BannerFacts {
            listen: any,
            hostname: "box",
            fingerprint: "AB:CD",
            covered: &["localhost".into()],
            missing: &["box.local".into()],
            token_url: Some("https://127.0.0.1:7337/#t=t"),
        });
        assert_eq!(b.stdout, "claudio web: https://box:7337/");
        assert!(b.stderr.iter().all(|l| l.starts_with("claudio web: ")));
        assert!(b.stderr[0].ends_with("SHA-256 AB:CD"));
        assert!(b.stderr.iter().any(|l| l.contains("--rotate-cert")));
        assert!(b.stderr.iter().any(|l| l.contains("firewall") && l.contains("box.local")));
    }
```

**`addons/web/src/auth.rs` (Task 4 part):**

- `Allowed` becomes (doc comments as in the prototype):

```rust
pub(crate) struct Allowed {
    hosts: Vec<String>,
    local_hosts: Vec<String>,
    /// `--remote`: any IP literal at this port is accepted too (an address
    /// typed by hand cannot be the target of DNS rebinding).
    ip_port: Option<u16>,
    /// `--remote`: TLS only (`https://` Origins, a `Secure` cookie).
    tls: bool,
    proxied: bool,
}
```

  `new` ends with `Allowed::sorted(hosts, local_hosts, None, false, !allow_hosts.is_empty())`;
  add:

```rust
    /// `--remote`: the loopback forms (local), the listen address when it is
    /// a specific one, any IP literal, and `names` (this machine's host
    /// names and each `--allow-host`, whose port, if any, is the listen
    /// port), all at the listen port (also bare when it is 443).
    pub(crate) fn remote(listen: SocketAddr, names: &[String]) -> Allowed {
        let port = listen.port();
        let mut local_hosts = Vec::new();
        if listen.ip().is_loopback() {
            local_hosts.extend(at_port(&host_ip(listen.ip()), port, 443));
        }
        for name in ["127.0.0.1", "localhost", "[::1]"] {
            local_hosts.extend(at_port(name, port, 443));
        }
        let mut hosts = local_hosts.clone();
        if !listen.ip().is_unspecified() {
            hosts.extend(at_port(&host_ip(listen.ip()), port, 443));
        }
        for name in names {
            let bare = name.split(':').next().unwrap_or_default();
            hosts.extend(at_port(&bare.trim().to_ascii_lowercase(), port, 443));
        }
        Allowed::sorted(hosts, local_hosts, Some(port), true, false)
    }

    fn sorted(
        mut hosts: Vec<String>,
        mut local_hosts: Vec<String>,
        ip_port: Option<u16>,
        tls: bool,
        proxied: bool,
    ) -> Allowed {
        for list in [&mut hosts, &mut local_hosts] {
            list.sort();
            list.dedup();
        }
        Allowed { hosts, local_hosts, ip_port, tls, proxied }
    }

    pub(crate) fn host_ok(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        !host.is_empty()
            && (self.hosts.contains(&host) || self.ip_port.is_some_and(|p| ip_literal_at(&host, p)))
    }

    /// `https://` (and, without `--remote`, `http://`) and an accepted host,
    /// nothing after it. `null` and every other scheme are refused.
    pub(crate) fn origin_ok(&self, origin: &str) -> bool {
        let origin = origin.trim().to_ascii_lowercase();
        let rest = match origin.strip_prefix("https://") {
            Some(rest) => Some(rest),
            None if !self.tls => origin.strip_prefix("http://"),
            None => None,
        };
        match rest {
            Some(host) if !host.contains('/') => self.host_ok(host),
            _ => false,
        }
    }

    pub(crate) fn secure_cookie(&self) -> bool {
        self.proxied || self.tls
    }

    pub(crate) fn tls(&self) -> bool {
        self.tls
    }
```

```rust
/// `host` (lowercase) is an IPv4 or bracketed IPv6 literal at `port` (bare
/// at 443), neither unspecified nor multicast.
fn ip_literal_at(host: &str, port: u16) -> bool {
    let ip = match host.parse::<SocketAddr>() {
        Ok(sock) if sock.port() == port => sock.ip(),
        Ok(_) => return false,
        Err(_) if port == 443 => {
            let v6 = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
            match v6 {
                Some(v6) => match v6.parse::<std::net::Ipv6Addr>() {
                    Ok(ip) => IpAddr::V6(ip),
                    Err(_) => return false,
                },
                None => match host.parse::<std::net::Ipv4Addr>() {
                    Ok(ip) => IpAddr::V4(ip),
                    Err(_) => return false,
                },
            }
        }
        Err(_) => return false,
    };
    !ip.is_unspecified() && !ip.is_multicast()
}
```

- `Auth` gains `test_remote_peer: Option<IpAddr>` (set from `test_remote_peer()` in `new`):

```rust
/// `CLAUDIO_TEST_REMOTE_PEER=<ip>`, honoured by debug builds only: that
/// loopback address then counts as a remote peer, the one way a hermetic
/// suite gets one through the real listener. It can only take the token
/// away from a peer, never give access.
fn test_remote_peer() -> Option<IpAddr> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("CLAUDIO_TEST_REMOTE_PEER").ok()?.parse().ok()
}
```

  `caller` becomes:

```rust
    pub(crate) fn caller(&self, peer: Option<Peer>, host: &str) -> Caller {
        let Some(peer) = peer else {
            return Caller { local: false, key: PeerKey::Unknown };
        };
        let ip = peer.ip();
        let forced = self.test_remote_peer == Some(ip);
        let key = if forced { PeerKey::of(ip) } else { peer.key() };
        Caller {
            local: ip.is_loopback() && !forced && self.allowed.is_local_host(host),
            key,
        }
    }

    /// Whether a token login is open to this caller: always without
    /// `--remote` (every peer is loopback), else to local requests only.
    pub(crate) fn token_allowed(&self, caller: &Caller) -> bool {
        !self.allowed.tls() || caller.local
    }

    /// The login methods `/api/me` lists for this caller.
    pub(crate) fn login_methods(&self, caller: &Caller) -> Vec<&'static str> {
        let mut methods = Vec::new();
        if self.token_allowed(caller) {
            methods.push("token");
        }
        methods
    }
```

- `guard`'s `/api/me` branch uses the caller:

```rust
        Err(StatusCode::UNAUTHORIZED) if path == "/api/me" => {
            let caller = auth.caller(peer_of(&req), host_of(&req));
            let body = serde_json::json!({ "login": auth.login_methods(&caller) });
            (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
        }
```

- `login.rs::token_login` adds the rule: `if auth.token_allowed(caller) && auth.token_ok(token)`.
- Unit tests (`auth.rs`):
  - `remote_hosts_are_loopback_names_ip_literals_and_this_machines_names`:
    `let a = Allowed::remote("0.0.0.0:7337".parse().unwrap(), &names(&["box", "box.local", "claudio.test:7337"]));`
    ok: `127.0.0.1:7337`, `localhost:7337`, `[::1]:7337`, `box:7337`, `BOX.local:7337`,
    `claudio.test:7337`, `192.168.1.20:7337`, `[fd00::1]:7337`; refused: `0.0.0.0:7337`,
    `[::]:7337`, `224.0.0.1:7337`, `192.168.1.20:7338`, `box`, `evil.example:7337`,
    `claudio.test`, `127.0.0.1`, `[fd00::1`; Origins ok `https://box.local:7337`,
    `https://192.168.1.20:7337`; refused `http://127.0.0.1:7337`, `http://box:7337`, `null`;
    `a.secure_cookie()`; `a.is_local_host("localhost:7337")`, not `box:7337` nor
    `192.168.1.20:7337`. Port 443: `Allowed::remote("0.0.0.0:443", &names(&["box"]))` accepts
    `box`, `10.0.0.1`, `[fd00::1]` and Origin `https://box`. A specific address:
    `Allowed::remote("192.168.1.20:7337", &[])` accepts `192.168.1.20:7337`.
  - `with_remote_the_token_is_for_local_requests_only` (`Auth` over that `Allowed`): local and
    `token_allowed` for `(127.0.0.1:1, "127.0.0.1:7337")` and `([::ffff:127.0.0.1]:1,
    "localhost:7337")` (a `[::]` listener's IPv4 peer); not for `(127.0.0.1:1, "box:7337")`,
    `(192.168.1.5:1, "127.0.0.1:7337")`, `(None, …)`; `[2001:db8::1]` and
    `[2001:db8::ffff]` share a key; `login_methods` is `["token"]` local, `[]` otherwise.
    Plain `auth()` keeps `token_allowed` true even for `(None, "proxy.example")`.
  - `the_test_peer_hook_only_takes_the_token_away`: set `a.test_remote_peer =
    Some("127.0.0.2".parse().unwrap())` in the test; `caller(127.0.0.2:1, "127.0.0.1:7337")`
    is not local and its key is `PeerKey::V4(0x7f00_0002)`; `127.0.0.1` stays local.
- `login.rs` test `with_remote_a_non_local_token_is_refused_and_counts`: an `Auth` over
  `Allowed::remote(…)`, `Caller { local: false, key: PeerKey::V4(1) }`: the right token five
  times → 401, the sixth → 429; a local caller with the right token → `Ok`.

**`addons/web/src/lib.rs` (Task 4 part):**

- `WebArgs` gains `listen_given`, `remote: Option<SocketAddr>`, `rotate_cert`, `print_cert`;
  `parse_args` (now over `args.iter().peekable()`):

```rust
            "--listen" => {
                out.listen = listen_addr(&value(inline, &mut it, flag)?)?;
                out.listen_given = true;
            }
            "--remote" => {
                // A value only as `--remote=V` or a next word not starting with `-`.
                let v = inline.or_else(|| it.next_if(|next| !next.starts_with('-')).cloned());
                out.remote = Some(match v {
                    Some(v) => remote::remote_addr(&v)?,
                    None => SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
                });
            }
            "--rotate-cert" => out.rotate_cert = true,
            "--print-cert" => out.print_cert = true,
```

- `combination_ok` grows:

```rust
fn combination_ok(a: &WebArgs, n_args: usize) -> Result<(), String> {
    if (a.set_password || a.print_cert) && n_args != 1 {
        return Err("--set-password and --print-cert take no other argument".to_owned());
    }
    if a.remote.is_some() && a.listen_given {
        return Err("--remote is the one listener: drop --listen".to_owned());
    }
    if a.rotate_cert && a.remote.is_none() {
        return Err("--rotate-cert needs --remote".to_owned());
    }
    if let Some(remote) = a.remote {
        let port = remote.port().to_string();
        let foreign = a
            .allow_hosts
            .iter()
            .find(|h| h.split_once(':').is_some_and(|(_, p)| p != port));
        if let Some(h) = foreign {
            return Err(format!("--allow-host {h}: with --remote the one port is {port}"));
        }
    }
    Ok(())
}
```

- `USAGE`:

```rust
const USAGE: &str = "usage: claudio web [--listen 127.0.0.1:7337 | --remote [ADDR][:PORT] [--rotate-cert]] \
[--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
       claudio web --set-password
       claudio web --print-cert";
```

- `listen_addr`'s refusal text gains one line (the test still finds `ssh -L` and
  `--allow-host`): `"Or serve it over TLS with a password: claudio web --remote (docs/web.md)."`
- `one_shot` ends with `args.print_cert.then(print_cert)`, and:

```rust
/// `--print-cert`: the kept certificate (never the key) on stdout.
fn print_cert() -> ExitCode {
    match tls::cert::read_cert_pem(&tls::cert::dir()) {
        Ok(pem) => {
            print!("{pem}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("claudio web: {e}");
            ExitCode::FAILURE
        }
    }
}
```

- In `run`, after the token is loaded, the URL and `--remote`'s preparation (before the
  tracing subscriber and the runtime):

```rust
    let url = match args.remote {
        // `SocketAddr`'s `Display` brackets an IPv6 address.
        None => format!("http://{}/#t={token}", args.listen),
        Some(addr) => remote::token_url(addr, &token)
            .unwrap_or_else(|| remote::public_url(addr, &claudio_core::sys::hostname())),
    };
    if args.print_url {
        println!("{url}");
        return ExitCode::SUCCESS;
    }
    let remote = match args.remote.map(|addr| remote::prepare(&args, addr, &token)) {
        None => None,
        Some(Ok(r)) => Some(r),
        Some(Err(e)) => {
            eprintln!("claudio web: {e}");
            return ExitCode::FAILURE;
        }
    };
```

  and `runtime.block_on(serve(args, token, url, remote, addons))`. `run` stays under 80 lines
  (`scripts/check_rules.py` passed on the prototype with `one_shot` split out; without it, 84).
- `serve` takes `remote: Option<remote::Remote>`:

```rust
    let listen = remote.as_ref().map_or(args.listen, |r| r.addr);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    let assets = Arc::new(assets::Assets::load());
    let allowed = match &remote {
        Some(r) => r.allowed.clone(),
        None => auth::Allowed::new(args.listen, &args.allow_hosts),
    };
    let auth = auth::Auth::new(token, allowed, auth::csp(assets.importmap_sha256()))
        .map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
```

  replace the stdout block with `announce(&url, remote.as_ref());`, log
  `tracing::info!(listen = %listen, tls = remote.is_some(), "web: serving");`, and end with:

```rust
    let app = router(state).into_make_service_with_connect_info::<peer::Peer>();
    let served = match remote {
        None => axum::serve(listener, app).with_graceful_shutdown(stop).await,
        Some(r) => {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::clone(&r.cert.config));
            let limits = tls::listener::Limits::DEFAULT;
            let tls = tls::listener::TlsListener::new(listener, acceptor, limits)
                .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
            axum::serve(tls, app).with_graceful_shutdown(stop).await
        }
    };
    served.map_err(|e| format!("server error: {e}"))
}

/// The start lines: with `--remote` the banner's stderr lines first, then
/// the one stdout line the tests read.
fn announce(url: &str, remote: Option<&remote::Remote>) {
    let line = match remote {
        Some(r) => {
            for l in &r.banner.stderr {
                eprintln!("{l}");
            }
            r.banner.stdout.clone()
        }
        None => format!("claudio web: {url}"),
    };
    let mut out = io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
```

- `args_parse_flags_and_defaults` gains: `--remote` alone → `0.0.0.0:7337`; `--remote
  --no-open` → `0.0.0.0:7337` and `no_open`; `--remote=:8443` → `0.0.0.0:8443`; `--remote
  [::]:9000`; `--remote --listen 127.0.0.1:1` → `Err`; `--rotate-cert` alone → `Err`;
  `--remote :7999 --allow-host a.example:8443` → `Err`; `--remote :7999 --allow-host
  a.example:7999` → `Ok`; `--print-cert --no-open` → `Err`.

**The suites' TLS client.** Root `Cargo.toml` `[dev-dependencies]`: `tokio-rustls.workspace =
true`. `addons/web/tests/web_harness/mod.rs`: add `pub mod tls;` (not a glob re-export: the
module is also compiled into `vocdoni_web`, where an unused `pub use` fails clippy), and factor
`http_at`'s request text and parse into two `pub fn`s used by both paths:

```rust
/// One raw HTTP/1.1 request's text (`Connection: close`).
pub fn request_text(method: &str, path: &str, host: &str, extra: &[(&str, &str)], body: &str) -> String {
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    req
}

/// A raw HTTP/1.1 answer: status (0 when there is none), headers, body.
pub fn parse_reply(raw: &[u8]) -> Reply { /* today's parsing in http_at, moved verbatim */ }
```

  (`http_at` becomes connect + `request_text` + write + read + `parse_reply`.)

**New `addons/web/tests/web_harness/tls.rs`** (compiled and run in the prototype):

```rust
//! `claudio web --remote` in the suites: a server on a loopback port with
//! the fixture password, and a rustls client that trusts exactly the
//! certificate whose SHA-256 the server printed.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::{fs, thread};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{self, CryptoProvider};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::{apply_env, free_port, parse_reply, request_text, Reply, Web};
use crate::common::{ManagerHarness, BINARY, DAEMON_WAIT, WAIT};

/// The committed fixture: an argon2id hash of [`TEST_PASSWORD`].
pub const TEST_PASSWORD_HASH: &str = include_str!("../../web/e2e/fixtures/web-password");
pub const TEST_PASSWORD: &str = "claudio-e2e-password";

/// SHA-256 of a DER certificate as the server prints it.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Write the fixture password file (0600) into `h`'s config dir.
pub fn write_test_password(h: &ManagerHarness) {
    let path = h.config_home.join("claudio/web-password");
    fs::write(&path, TEST_PASSWORD_HASH).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// The `claudio_web` value of a `Set-Cookie` header.
pub fn cookie_of(set_cookie: &str) -> String {
    set_cookie
        .strip_prefix("claudio_web=")
        .and_then(|rest| rest.split(';').next())
        .expect("claudio_web=…")
        .to_owned()
}

/// A `--remote` server and the fingerprint it printed.
pub struct RemoteWeb {
    pub web: Web,
    pub fingerprint: String,
}

impl RemoteWeb {
    /// `claudio web --remote 127.0.0.1:<free port> --no-open` plus `args`,
    /// with the fixture password. Asserts the stdout line
    /// (`https://127.0.0.1:<port>/`, no token) and that the printed
    /// fingerprint is `cert.pem`'s.
    pub fn start(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> RemoteWeb {
        write_test_password(&h);
        let port = free_port();
        let mut cmd = Command::new(BINARY);
        cmd.arg("web")
            .arg("--remote")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--no-open")
            .args(args);
        apply_env(&h, &mut cmd);
        setup(&mut cmd);
        let log_path = h.root.join("web.log");
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
        let mut child = cmd.spawn().expect("start claudio web --remote");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let line = rx
            .recv_timeout(DAEMON_WAIT)
            .expect("claudio web --remote printed no URL");
        let token = fs::read_to_string(h.config_home.join("claudio/web-token"))
            .unwrap()
            .trim()
            .to_owned();
        // Dropped (killed) on every path from here.
        let web = Web { h: Some(h), child, port, token };
        assert_eq!(line, format!("claudio web: https://127.0.0.1:{port}/"));
        // stderr is written before the stdout line.
        let log = fs::read_to_string(&log_path).unwrap();
        let printed = log
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("claudio web: TLS certificate SHA-256 "))
            .expect("the fingerprint line")
            .to_owned();
        let pem = web.h().config_home.join("claudio/web-tls/cert.pem");
        let der = CertificateDer::from_pem_file(&pem).unwrap();
        assert_eq!(printed, fingerprint(&der), "the printed fingerprint is cert.pem's");
        RemoteWeb { web, fingerprint: printed }
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.web.port)
    }

    pub fn origin(&self) -> String {
        format!("https://127.0.0.1:{}", self.web.port)
    }

    /// One request over pinned TLS from `src` with `Host: host`.
    pub async fn req_as(&self, src: &str, host: &str, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> Reply {
        https_from(src, self.web.port, &self.fingerprint, &request_text(method, path, host, extra, body))
            .await
            .expect("TLS")
    }

    /// One request from 127.0.0.1 with `Host: 127.0.0.1:<port>`.
    pub async fn req(&self, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> Reply {
        self.req_as("127.0.0.1", &self.host(), method, path, extra, body).await
    }

    /// A login with `body` from the page's origin.
    pub async fn login(&self, body: &serde_json::Value) -> Reply {
        let origin = self.origin();
        self.req("POST", "/api/login", &[("Origin", &origin)], &body.to_string()).await
    }
}

/// Trusts exactly one certificate, by SHA-256; signatures are still checked.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the pinned certificate".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls12_signature(msg, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature(msg, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// A TLS connection from `src` (a loopback address) to 127.0.0.1:`port`
/// that accepts only the certificate `fingerprint`.
pub async fn tls_connect(
    src: &str,
    port: u16,
    fingerprint: &str,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, String> {
    let provider = Arc::new(crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            fingerprint: fingerprint.to_owned(),
            provider,
        }))
        .with_no_client_auth();
    let socket = tokio::net::TcpSocket::new_v4().map_err(|e| e.to_string())?;
    socket
        .bind(SocketAddr::new(src.parse().map_err(|_| "bad src")?, 0))
        .map_err(|e| e.to_string())?;
    let tcp = socket
        .connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|e| e.to_string())?;
    let name = ServerName::try_from("127.0.0.1").map_err(|e| e.to_string())?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|e| e.to_string())
}

/// One raw request over a pinned TLS connection from `src`.
pub async fn https_from(src: &str, port: u16, fingerprint: &str, request: &str) -> Result<Reply, String> {
    let mut tls = tls_connect(src, port, fingerprint).await?;
    tls.write_all(request.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(WAIT, tls.read_to_end(&mut raw)).await;
    Ok(parse_reply(&raw))
}

/// A WebSocket over pinned TLS: the first text frame (the hello), or the
/// HTTP status that refused the upgrade.
pub async fn wss_hello(port: u16, fingerprint: &str, cookie: &str, origin: &str) -> Result<serde_json::Value, u16> {
    use futures::StreamExt;
    let tls = tls_connect("127.0.0.1", port, fingerprint).await.expect("TLS");
    let mut req = format!("wss://127.0.0.1:{port}/ws").into_client_request().unwrap();
    req.headers_mut().insert("Origin", origin.parse().unwrap());
    req.headers_mut()
        .insert("Cookie", format!("claudio_web={cookie}").parse().unwrap());
    let (mut ws, _) = match tokio_tungstenite::client_async(req, tls).await {
        Ok(pair) => pair,
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => return Err(resp.status().as_u16()),
        Err(e) => panic!("wss: {e}"),
    };
    let msg = tokio::time::timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
    Ok(serde_json::from_str(msg.to_text().unwrap()).unwrap())
}
```

`web.rs` imports `use web_harness::tls::*;` beside `use web_harness::*;`.

**Integration tests** (`web.rs`, section `// ── --remote: TLS (AC2, AC4, AC6) ──`):

```rust
#[test]
fn web_remote_refuses_to_start_without_a_password() {
    let h = ManagerHarness::new();
    let port = free_port();
    let mut cmd = Command::new(BINARY);
    cmd.arg("web").arg("--remote").arg(format!("127.0.0.1:{port}")).arg("--no-open");
    apply_env(&h, &mut cmd);
    let out = cmd.stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--set-password"), "{err}");
    assert!(out.stdout.is_empty(), "no URL: nothing served");
    assert!(!h.config_home.join("claudio/web-tls").exists(), "no certificate made");
}

#[test]
fn web_remote_arguments_are_checked() {
    let h = ManagerHarness::new();
    for (args, needle) in [
        (&["--remote", "--listen", "127.0.0.1:7999"][..], "--listen"),
        (&["--rotate-cert"][..], "needs --remote"),
        (&["--remote", "fe80::1"][..], "brackets"),
        (&["--remote", ":7999", "--allow-host", "a.example:8443"][..], "one port"),
        (&["--print-cert", "--no-open"][..], "no other argument"),
    ] {
        let mut cmd = Command::new(BINARY);
        cmd.arg("web").args(args);
        apply_env(&h, &mut cmd);
        let out = cmd.stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(needle), "{args:?}: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_serves_tls_with_a_kept_certificate() {
    let r = RemoteWeb::start(ManagerHarness::new(), &[], |_| {});
    let page = r.req("GET", "/", &[], "").await;
    assert_eq!(page.status, 200);
    assert!(page.header("content-security-policy").is_some());
    assert_eq!(page.header("strict-transport-security"), None, "no HSTS");
    let other = request_text("GET", "/", &r.host(), &[], "");
    assert!(https_from("127.0.0.1", r.web.port, "00:11", &other).await.is_err(), "another pin fails");
    let plain = http(r.web.port, "GET", "/", &r.host(), &[], "");
    assert_eq!(plain.status, 0, "plain HTTP gets no HTTP answer: {plain:?}");
    let dir = r.web.h().config_home.join("claudio/web-tls");
    let mode = |p: std::path::PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.clone()), 0o700);
    assert_eq!(mode(dir.join("cert.pem")), 0o600);
    assert_eq!(mode(dir.join("key.pem")), 0o600);
    let first = r.fingerprint.clone();
    let h = r.web.stop();
    let mut cmd = Command::new(BINARY);
    cmd.args(["web", "--print-cert"]);
    apply_env(&h, &mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(printed, fs::read_to_string(dir.join("cert.pem")).unwrap());
    assert!(!printed.contains("PRIVATE KEY"));
    let r = RemoteWeb::start(h, &[], |_| {});
    assert_eq!(r.fingerprint, first, "kept across restarts");
    let h = r.web.stop();
    let r = RemoteWeb::start(h, &["--rotate-cert"], |_| {});
    assert_ne!(r.fingerprint, first, "replaced by --rotate-cert");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_guard_holds_over_tls() {
    let r = RemoteWeb::start(ManagerHarness::new(), &["--allow-host", "claudio.test"], |_| {});
    let port = r.web.port;
    for (host, status) in [
        (format!("evil.example:{port}"), 421),
        (format!("claudio.test:{port}"), 200),
        (format!("10.1.2.3:{port}"), 200),
        (format!("[fd00::1]:{port}"), 200),
        (format!("0.0.0.0:{port}"), 421),
        (format!("10.1.2.3:{}", port + 1), 421),
    ] {
        let reply = r.req_as("127.0.0.1", &host, "GET", "/", &[], "").await;
        assert_eq!(reply.status, status, "{host}");
    }
    let token = json!({"token": r.web.token});
    let http_origin = format!("http://127.0.0.1:{port}");
    let refused = r.req("POST", "/api/login", &[("Origin", &http_origin)], &token.to_string()).await;
    assert_eq!(refused.status, 403, "no http:// Origin with --remote");
    let ok = r.login(&token).await;
    assert_eq!(ok.status, 204, "{ok:?}");
    let set = ok.header("set-cookie").unwrap();
    for flag in ["HttpOnly", "SameSite=Strict", "Path=/", "Secure"] {
        assert!(set.contains(flag), "{set}");
    }
    let cookie = cookie_of(set);
    assert_eq!(wss_hello(port, &r.fingerprint, &cookie, &r.origin()).await.unwrap()["t"], "hello");
    assert_eq!(wss_hello(port, &r.fingerprint, &cookie, &http_origin).await.err(), Some(403));
    // A loopback peer that names this machine is not local: no token.
    let host = format!("claudio.test:{port}");
    let origin = format!("https://claudio.test:{port}");
    let rep = r.req_as("127.0.0.1", &host, "POST", "/api/login", &[("Origin", &origin)], &token.to_string()).await;
    assert_eq!(rep.status, 401);
    let me = r.req_as("127.0.0.1", &host, "GET", "/api/me", &[], "").await;
    assert_eq!(me.status, 401);
}
```

```bash
make check
cargo test --test web --test web_manager -- --test-threads=1   # all pass (old and new)
cargo test --test vocdoni_web -- --test-threads=1
cargo tree -e normal --target all | grep -c aws-lc               # 0
make static                                                       # static-pie musl binary in dist/
git add Cargo.toml Cargo.lock addons/web/Cargo.toml addons/web/src/tls addons/web/src/remote.rs \
  addons/web/src/auth.rs addons/web/src/login.rs addons/web/src/lib.rs \
  addons/web/tests/web_harness/mod.rs addons/web/tests/web_harness/tls.rs addons/web/tests/web.rs
git commit -m "feat(web): claudio web --remote serves TLS with a kept self-signed certificate" \
  -m "New dependencies, claudio-web only, ring as the one provider (the tree already builds it through reqwest; no aws-lc, so make static keeps working): rustls =0.23.45 and tokio-rustls =0.26.6 (TLS server; pinned exactly, they parse network bytes, D7), rcgen 0.14 (the self-signed P-256 certificate), rustls-webpki 0.103 (already in the tree; which names the kept certificate covers). All pure Rust but ring, MIT/Apache/ISC. The listener runs each handshake in its own task (10 s, 64 at once), caps connections (256, 32 per non-loopback address) and closes a connection without request headers in 30 s. --remote needs the password file at start (fail closed), accepts the token from local requests only, https:// Origins only, IP-literal Hosts, and prints the fingerprint; remote peers get a password login in the next commit."
```

---

## Task 5: password login over `--remote` (strong)

**`addons/web/src/remote.rs`:** `Remote` gains `pub password: crate::password::Stored`;
`prepare` keeps it: `let password = crate::password::load(&crate::password::path())?;` and
`Ok(Remote { addr, cert, password, … })`.

**`addons/web/src/auth.rs`:**

```rust
/// Concurrent argon2 verifications (each 19 MiB).
pub(crate) const HASH_PERMITS: usize = 4;

/// `--remote`'s password check: the stored hash and the slots its
/// verifications run under.
pub(crate) struct PasswordLogin {
    pub stored: password::Stored,
    pub hashing: Arc<Semaphore>,
}
```

`Auth` gains `pub(crate) password: Option<PasswordLogin>` (`None` in `new`), plus:

```rust
    /// With `--remote`: password logins against `stored`.
    pub(crate) fn with_password(mut self, stored: password::Stored) -> Auth {
        self.password = Some(PasswordLogin {
            stored,
            hashing: Arc::new(Semaphore::new(HASH_PERMITS)),
        });
        self
    }
```

and `login_methods` pushes `"password"` when `self.password.is_some()` (after `"token"`).

**`addons/web/src/lib.rs::serve`:** `let mut auth = …; if let Some(r) = &remote { auth =
auth.with_password(r.password.clone()); }`.

**`addons/web/src/login.rs`:** the module doc says "or the password (`--remote`)… argon2 runs
off the executor under `auth::HASH_PERMITS` slots"; add:

```rust
/// How long a password login waits for a hashing slot before `503`.
pub(crate) const HASH_WAIT: Duration = Duration::from_secs(2);
```

the body match gains the password arm:

```rust
        Ok(LoginBody { token: None, password: Some(pw) }) => {
            password_login(&auth, &caller, pw, Instant::now()).await
        }
```

and:

```rust
/// A password attempt: `400` without `--remote`; its slot (peer and global
/// windows) before any hashing; a hashing slot within [`HASH_WAIT`] or
/// `503` (the attempt is given back: it did not count); argon2 in
/// `spawn_blocking`, the permit held until it is done.
async fn password_login(
    auth: &Auth,
    caller: &Caller,
    pw: String,
    now: Instant,
) -> Result<(), StatusCode> {
    let Some(login) = auth.password.as_ref() else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let slot = auth
        .limiter
        .reserve(caller.key, true, now)
        .map_err(|()| StatusCode::TOO_MANY_REQUESTS)?;
    let permit =
        match tokio::time::timeout(HASH_WAIT, Arc::clone(&login.hashing).acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            _ => {
                auth.limiter.release(slot);
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
        };
    let stored = login.stored.clone();
    let ok = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        stored.verify(&pw)
    })
    .await
    .unwrap_or(false);
    if ok {
        auth.limiter.release(slot);
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
```

**Unit tests** (`login.rs`; `const FIXTURE: &str = include_str!("../web/e2e/fixtures/web-password");`
and `fn remote() -> Auth` = `Auth::new("ab".repeat(32), Allowed::remote("0.0.0.0:7337".parse()
.unwrap(), &[]), csp(None)).unwrap().with_password(password::Stored::parse(FIXTURE).unwrap())`):

- `#[tokio::test] a_password_logs_in_and_a_wrong_one_counts`: remote caller
  `Caller { local: false, key: PeerKey::V4(1) }`: right password → `Ok`; five wrong → `401`
  each; sixth (right or wrong) → `429`.
- `#[tokio::test] without_remote_a_password_body_is_400`: `plain()` → `Err(BAD_REQUEST)`, and
  `plain().limiter` still lets five token attempts through (nothing counted).
- `#[tokio::test(start_paused = true)] no_hashing_slot_answers_503_and_does_not_count`:
  `let held = Arc::clone(&auth.password.as_ref().unwrap().hashing).acquire_many_owned(HASH_PERMITS as u32).await.unwrap();`
  ten wrong attempts → `503` each (paused time advances the 2 s at once); `drop(held)`; then
  five wrong → `401`, the sixth → `429` (the 503s took nothing).
- `#[tokio::test] the_global_window_stops_passwords_but_not_local_tokens`: reserve `GLOBAL` slots
  on `auth.limiter` for `PeerKey::V4(i)`, `i in 0..30`; a password from `V4(999)` → `429`
  without hashing; a local token login (`Caller { local: true, key: PeerKey::Loopback }`, right
  token) → `Ok`.
- `auth.rs`: `login_methods` is `["token","password"]` for a local caller and `["password"]`
  for a remote one with `with_password`.

**Integration tests** (`web.rs`, section `// ── --remote: logins (AC5) ──`):

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_login_rules() {
    let server = RemoteWeb::start(ManagerHarness::new(), &["--allow-host", "claudio.test"], |_| {});
    let r = &server; // the closures below copy the reference
    let port = r.web.port;
    let methods = |reply: &Reply| serde_json::from_str::<Value>(&reply.body).unwrap()["login"].clone();
    // Local: both methods.
    let me = r.req("GET", "/api/me", &[], "").await;
    assert_eq!((me.status, methods(&me)), (401, json!(["token", "password"])));
    let pw = r.login(&json!({"password": TEST_PASSWORD})).await;
    assert_eq!(pw.status, 204, "{pw:?}");
    assert!(pw.header("set-cookie").unwrap().contains("; Secure"));
    assert_eq!(r.login(&json!({"token": r.web.token})).await.status, 204);
    // Not local (a loopback peer that names this machine): the password only.
    let host = format!("claudio.test:{port}");
    let origin = format!("https://claudio.test:{port}");
    let post = |body: Value| {
        let (host, origin) = (host.clone(), origin.clone());
        async move {
            r.req_as("127.0.0.1", &host, "POST", "/api/login", &[("Origin", &origin)], &body.to_string())
                .await
                .status
        }
    };
    let me = r.req_as("127.0.0.1", &host, "GET", "/api/me", &[], "").await;
    assert_eq!(methods(&me), json!(["password"]));
    assert_eq!(post(json!({"token": r.web.token})).await, 401);
    assert_eq!(post(json!({"password": TEST_PASSWORD})).await, 204);
    assert_eq!(post(json!({"password": "not-the-password"})).await, 401);
    assert_eq!(post(json!({"password": TEST_PASSWORD, "token": r.web.token})).await, 400);
    assert_eq!(post(json!({"password": "x".repeat(5000)})).await, 413);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_peer_on_another_address_needs_the_password() {
    // Linux routes all of 127/8 to lo; macOS only 127.0.0.1 unless aliased.
    if std::net::TcpListener::bind("127.0.0.2:0").is_err() {
        return;
    }
    let server = RemoteWeb::start(ManagerHarness::new(), &[], |cmd| {
        cmd.env("CLAUDIO_TEST_REMOTE_PEER", "127.0.0.2");
    });
    let r = &server;
    let origin = r.origin();
    let from_2 = |method: &str, path: &str, body: String| {
        let (method, path, origin, host) = (method.to_owned(), path.to_owned(), origin.clone(), r.host());
        async move { r.req_as("127.0.0.2", &host, &method, &path, &[("Origin", &origin)], &body).await }
    };
    let me = from_2("GET", "/api/me", String::new()).await;
    assert_eq!(serde_json::from_str::<Value>(&me.body).unwrap()["login"], json!(["password"]));
    let token = json!({"token": r.web.token}).to_string();
    assert_eq!(from_2("POST", "/api/login", token.clone()).await.status, 401);
    let pw = json!({"password": TEST_PASSWORD}).to_string();
    assert_eq!(from_2("POST", "/api/login", pw).await.status, 204);
    assert_eq!(r.login(&json!({"token": r.web.token})).await.status, 204, "127.0.0.1 stays local");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_remote_concurrent_wrong_passwords_hash_at_most_five() {
    let r = RemoteWeb::start(ManagerHarness::new(), &[], |_| {});
    let wrong = json!({"password": "wrong-password-xx"});
    let replies = futures::future::join_all((0..20).map(|_| r.login(&wrong))).await;
    let count = |s| replies.iter().filter(|r| r.status == s).count();
    // A 401 is only ever answered after argon2 ran: exactly five hashed.
    assert_eq!((count(401), count(429)), (5, 15));
    let right = json!({"password": TEST_PASSWORD});
    assert_eq!(r.login(&right).await.status, 429, "the peer's minute is spent");
}
```

(The closures copy `r: &RemoteWeb` into their `async move` blocks; if the borrow checker
still objects, write them as small `async fn`s taking `&RemoteWeb`. The prototype's equivalents
passed: `(5, 15)` on a fresh server; a refused token from the same peer earlier takes one of
the five, which is why this test starts its own server.)

```bash
make check
cargo test --test web -- --test-threads=1 web_remote   # 7 passed (Linux; the 127.0.0.2 one returns early on macOS)
cargo test --test web --test web_manager --test vocdoni_web -- --test-threads=1
git add addons/web/src/auth.rs addons/web/src/login.rs addons/web/src/remote.rs addons/web/src/lib.rs addons/web/tests/web.rs
git commit -m "feat(web): password login for claudio web --remote" \
  -m "A password attempt reserves its peer and global slots before hashing, waits at most 2 s for one of 4 hashing slots (503 otherwise, not counted) and verifies in spawn_blocking. /api/me lists the password for every request with --remote. Tests: local and non-local rules through the TLS listener (a 127.0.0.2 peer via the debug-only CLAUDIO_TEST_REMOTE_PEER), 20 concurrent wrong passwords give five 401s and fifteen 429s."
```

---

## Task 6: the page's password form and the e2e (standard)

### 6a. `addons/web/web/app/boot.js` and `styles/base.css`

`boot.js` (plain DOM, no imports before `main.js`; the password is never stored: the field is
cleared before the request and nothing goes to `localStorage`/`sessionStorage`):

```js
const TOKEN_BAD = "This link's token is not valid. Run `claudio web --print-url`.";
const TOKEN_ELSEWHERE = 'Token links work only on the machine claudio web runs on. Log in with the password.';
const NO_SESSION = 'Open the URL `claudio web` printed. It carries the sign-in token.';
const LOGIN_FAILED = {
  401: 'Wrong password.',
  429: 'Too many attempts: wait a minute.',
  503: 'The server is busy: try again in a moment.',
};

/** The login methods a 401 from /api/me lists ([] for anything else). */
async function methodsOf(r) {
  if (r.status !== 401) return [];
  try {
    const body = await r.json();
    return Array.isArray(body?.login) ? body.login : [];
  } catch { return []; }
}

async function offered() {
  try { return await methodsOf(await fetch('/api/me', { credentials: 'same-origin' })); } catch { return []; }
}

function showLogin(note) {
  const root = document.getElementById('app');
  root.replaceChildren();
  const form = document.createElement('form');
  form.className = 'boot-card login';
  form.setAttribute('data-testid', 'login-form');
  form.noValidate = true;
  const h = document.createElement('h1');
  h.textContent = 'claudio';
  const label = document.createElement('label');
  label.htmlFor = 'login-password';
  label.textContent = 'Password';
  const input = document.createElement('input');
  Object.assign(input, { id: 'login-password', type: 'password', className: 'input', autocomplete: 'current-password' });
  input.setAttribute('data-testid', 'login-password');
  const error = document.createElement('p');
  error.className = 'login-error';
  error.setAttribute('role', 'alert');
  error.setAttribute('data-testid', 'login-error');
  error.textContent = note;
  const button = document.createElement('button');
  button.type = 'submit';
  button.className = 'btn btn-primary';
  button.textContent = 'Log in';
  button.setAttribute('data-testid', 'login-submit');
  form.append(h, label, input, error, button);
  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const password = input.value;
    if (!password) return;
    input.value = ''; // posted once, then the cookie; never kept
    button.disabled = true;
    let r = null;
    try {
      r = await fetch('/api/login', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password }), credentials: 'same-origin',
      });
    } catch { /* r stays null */ }
    if (r?.status === 204) return start();
    error.textContent = r ? (LOGIN_FAILED[r.status] ?? `Sign-in failed (${r.status}).`) : 'Cannot reach the server.';
    button.disabled = false;
    input.focus();
  });
  const wrap = document.createElement('div');
  wrap.className = 'boot';
  wrap.append(form);
  root.append(wrap);
  input.focus();
}
```

In `boot()`: in the token branch, before the final `return show(…)`:

```js
    if (r.status === 401) {
      const methods = await offered();
      if (methods.includes('password') && !methods.includes('token')) return showLogin(TOKEN_ELSEWHERE);
    }
```

and replace the last two lines with:

```js
  if (r.status === 204) return start();
  if ((await methodsOf(r)).includes('password')) return showLogin('');
  show('claudio', NO_SESSION);
```

`styles/base.css`, after the `.boot-card code` rule (tokens only, F3):

```css
.login { display: grid; gap: var(--space-3); }
.login h1 { margin-bottom: 0; }
.login label { font-family: var(--font-mono); font-size: var(--fs-2xs); letter-spacing: var(--track-mono-wide); text-transform: uppercase; color: var(--text-muted); }
.login .login-error { min-height: 1.4em; color: var(--state-error); }
.login .btn { justify-self: start; }
```

Commit: `git commit -m "feat(web): the page offers a password login when the server has one" -- addons/web/web/app/boot.js addons/web/web/styles/base.css`
(body: "Shown only when /api/me's 401 lists \"password\", so a plain loopback server keeps its
'open the printed URL' text; a token link refused as non-local falls back to the form. DESIGN.md:
Carbon card, mono label, Deep Teal primary, coral error. The password is cleared before the
request and never stored.")

### 6b. The e2e fixture and spec

`addons/web/web/e2e/fixtures/server.mjs`:

- `import https from 'node:https';` and `export const TEST_PASSWORD = 'claudio-e2e-password';`
- `postLogin(port, body, remote)` takes the body object and the mode:

```js
function postLogin(port, body, remote) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const scheme = remote ? 'https' : 'http';
    const req = (remote ? https : http).request({
      host: '127.0.0.1', port, path: '/api/login', method: 'POST',
      ...(remote ? { rejectUnauthorized: false } : {}),
      headers: { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(data), Origin: `${scheme}://127.0.0.1:${port}` },
    }, (res) => { res.resume(); res.on('end', () => resolve(res)); });
    req.on('error', reject);
    req.end(data);
  });
}
```

- `connectApi(port, body, remote)`: `postLogin(port, body, remote)`; the socket
  `new WebSocket(`${remote ? 'wss' : 'ws'}://127.0.0.1:${port}/ws`, { ...(remote ? { rejectUnauthorized: false } : {}), headers: { Origin: `${remote ? 'https' : 'http'}://127.0.0.1:${port}`, Cookie: cookie } })`.
- `startServer({ proxy = false, hosts = [], remote = false, allowHosts = [] } = {})`: when
  `remote`, copy `path.join(here, 'web-password')` to `cfg/claudio/web-password` and
  `chmodSync(…, 0o600)`; spawn
  `remote ? ['web', '--remote', `127.0.0.1:${port}`, '--no-open', ...allowHosts.flatMap((h) => ['--allow-host', h])] : ['web', '--listen', `127.0.0.1:${port}`, '--no-open']`;
  after the URL line, `token = remote ? fs.readFileSync(path.join(root, 'cfg/claudio/web-token'), 'utf8').trim() : /#t=(.+)$/.exec(url)[1]`;
  `api = await connectApi(port, remote ? { password: TEST_PASSWORD } : { token }, remote)`;
  return `remote` and `origin: `${remote ? 'https' : 'http'}://127.0.0.1:${port}`` too. Without
  `remote` every line behaves as today.

New `addons/web/web/e2e/tests/remote.spec.mjs`:

```js
import { test, expect } from '@playwright/test';
import { startServer, TEST_PASSWORD } from '../fixtures/server.mjs';

// claudio.test resolves to 127.0.0.1 in this browser only: a non-local name.
test.use({ ignoreHTTPSErrors: true, launchOptions: { args: ['--host-resolver-rules=MAP claudio.test 127.0.0.1'] } });

let srv;
test.beforeAll(async () => { srv = await startServer({ remote: true, allowHosts: ['claudio.test'] }); });
test.afterAll(async () => { await srv?.stop(); });

test('over --remote the page asks for the password and the session works', async ({ page }) => {
  expect(srv.url).toBe(`https://127.0.0.1:${srv.port}/`);
  await page.goto(srv.url);
  await expect(page.getByTestId('login-form')).toBeVisible();
  await page.getByTestId('login-password').fill('not-the-password');
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('login-error')).toHaveText('Wrong password.');
  await expect(page.getByTestId('login-password')).toHaveValue('');
  await page.getByTestId('login-password').fill(TEST_PASSWORD);
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('rail')).toBeVisible();
  const sid = await srv.spawn(srv.dirs.a);
  await expect(page.locator(`[data-testid="session-card"][data-sid="${sid}"]`)).toBeVisible();
  expect(await page.evaluate(() => window.isSecureContext)).toBe(true);
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
  const cookie = (await page.context().cookies()).find((c) => c.name === 'claudio_web');
  expect(cookie?.secure).toBe(true);
  expect(cookie?.httpOnly).toBe(true);
  const stored = await page.evaluate(() => JSON.stringify(localStorage) + JSON.stringify(sessionStorage));
  expect(stored).not.toContain(TEST_PASSWORD);
  await srv.reset();
});

test('a token link on another name falls back to the password', async ({ page }) => {
  await page.goto(`https://claudio.test:${srv.port}/#t=${srv.token}`);
  await expect(page.getByTestId('login-form')).toBeVisible();
  await expect(page.getByTestId('login-error')).toContainText('only on the machine');
  expect(await page.evaluate(() => location.hash)).toBe('');
  await page.getByTestId('login-password').fill(TEST_PASSWORD);
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('rail')).toBeVisible();
  expect(await page.evaluate(() => window.__claudioCsp ?? [])).toEqual([]);
});
```

(Failures from 127.0.0.1 in this file: one wrong password and one refused token, under the
five a minute.) The existing `auth.spec.mjs` stays as it is (plain loopback, token).

```bash
node --test addons/web/web/tests/*.test.mjs
make test-web                     # node tests, then Playwright: every spec incl. remote.spec.mjs passes
git add addons/web/web/e2e/fixtures/server.mjs addons/web/web/e2e/tests/remote.spec.mjs
git commit -m "test(web): e2e over claudio web --remote with the fixture password"
```

---

## Task 7: docs (standard)

### 7a. `docs/web.md`

- **Usage** block gains:

```bash
claudio web --set-password       # set the password other machines log in with (asks twice)
claudio web --remote             # TLS on 0.0.0.0:7337 for other machines; needs the password
claudio web --remote [::]:8443   # IPv6 and IPv4, port 8443
claudio web --print-cert         # the certificate, to install as trusted on a laptop or phone
claudio web --remote --rotate-cert   # a new certificate (browsers warn once more)
```

  and the flags paragraph names `--remote [ADDR][:PORT]` (grammar from the spec: `ADDR`,
  `ADDR:PORT`, `:PORT`, `[V6]`, `[V6]:PORT`; a bare IPv6 address needs brackets; default
  `0.0.0.0:7337`, IPv4 only), `--rotate-cert`, `--set-password`, `--print-cert`, and that
  `--remote` with `--listen` is refused.
- **From another machine** becomes three ways: the SSH tunnel (unchanged); **`--remote`**
  (new, below); a TLS reverse proxy (unchanged text, plus: "keep using plain `--listen` with
  it; a proxy in front of `--remote` is unsupported, and one that rewrites `Host` to a loopback
  name would make every proxied browser local, so never configure that").
- New subsection **`--remote`: TLS and a password**, in steps:
  1. `claudio web --set-password` (12–1024 bytes, asked twice, never echoed; a running server
     keeps the old one until restarted, which signs every browser out).
  2. `claudio web --remote`. stderr shows `TLS certificate SHA-256 AB:CD:…`, the names the
     certificate covers, and on this machine the token link; stdout shows the URL for others.
     If a firewall (ufw, firewalld, macOS's) blocks the port, open it; another machine may not
     resolve the host name: use `<host>.local` or the machine's address.
  3. On the other machine, open `https://<host>.local:7337/`. The browser warns that the
     certificate is not trusted. **Check the fingerprint** before accepting: Chromium/Brave:
     "Not secure" → Certificate details → SHA-256 fingerprint; Firefox: Advanced → View
     Certificate → SHA-256. It must equal the printed one. Then log in with the password.
  4. **Or install the certificate** (no warning, and the install-as-app button appears):
     `claudio web --print-cert > claudio.pem`, copy it to the device, and add it to the
     trust store: Firefox: Settings → Privacy & Security → Certificates → View Certificates →
     Servers → Add Exception, or Authorities → Import; Chromium on Linux: Settings → Privacy and
     security → Security → Manage certificates → Import; macOS: Keychain Access → System →
     import, then Get Info → Trust → Always Trust; Android: Settings → Security → Encryption &
     credentials → Install a certificate → CA certificate; iOS: open the file, install the
     profile, then Settings → General → About → Certificate Trust Settings → turn it on.
     **Mark these steps "checked by hand in AC9(b)"**, and state per platform what AC9 found
     (until then: "not yet checked"; the certificate is a self-signed server certificate, not a
     CA, and some trust stores may refuse it, in which case use the exception path).
  5. What differs with the exception alone (AC9a): Chromium keeps it 7 days, then asks again;
     the install-as-app affordance is not expected; Capture all keys and copy still work
     (the page is a secure context). On a phone the warning cannot show the fingerprint:
     install the certificate instead of clicking through.
- **Security model**: "Loopback only" becomes "Loopback only, unless `--remote`: then one TLS
  listener on the address given, and every request that is not local needs the password";
  add bullets: **Local requests** (definition, the token only for them with `--remote`);
  **The password** (argon2id m=19 MiB t=2 p=1 in `web-password`, 0600; never in argv, logs or
  the page's storage; 12–1024 bytes); **The certificate** (`web-tls/`, 0600 in 0700, kept,
  fingerprint at every start, `--rotate-cert`, never replaced silently; no HSTS, on purpose);
  **Limits** (5 attempts per address per minute, 30 for all passwords, reserved before the
  check; 4 checks at once, 503 past 2 s; handshakes 10 s and 64 at once; 256 connections, 32
  per address; 30 s for a request's headers); **Accepted risks** (the first fingerprint check is
  yours; an attacker who keeps the global window full locks remote password logins while it
  lasts, loopback use unaffected; many addresses can fill the connection slots while an attack
  lasts). Update **The token** bullet: with `--remote` it is printed on stderr (the loopback
  link) and accepted only from local requests. Update **Cookie**: `Secure` with `--remote` too.
  Update **Host and Origin**: with `--remote`, IP-literal Hosts, `<host>.local`, `https://`
  Origins only. **What is logged**: add `web: login refused` with the status and `local`.
- **TUI parity**, row "Login and security": Web action "token URL, cookie; password form with
  `--remote`"; Test adds `over --remote the page asks for the password and the session works`,
  `web_remote_login_rules`, `web_remote_serves_tls_with_a_kept_certificate`.

### 7b. `README.md` and `CLAUDE.md`

- `README.md` (line 290): "It listens on loopback only; use `ssh -L 7337:127.0.0.1:7337 host`,
  a TLS reverse proxy, or `claudio web --remote` (TLS with a self-signed certificate and a
  password, set with `claudio web --set-password`) to reach it from elsewhere."
- `CLAUDE.md`:
  - The core map (`crates/core` line in "Workspace"): add `sys` to the module list; under
    "`crates/core/src/term/`" add "`secret.rs` — `read_line_no_echo`: one line from a terminal
    with echo off, restored on every exit path (SIGINT included); the web's `--set-password` and
    the Vocdoni addon's token prompt". Add "`crates/core/src/sys.rs` — `hostname()`" to the small
    shared modules.
  - "The services addons use today": add `term::secret` (web and vocdoni) and `sys::hostname`
    (web).
  - Web UI `addons/web/src/` list: `lib.rs` args gain `--remote`, `--rotate-cert`,
    `--set-password`, `--print-cert`; `auth.rs` line becomes "token file, cookie sessions, the
    `Host`/`Origin` sets (loopback, or `--remote`'s with IP literals and https only), the local
    rule (`Auth::caller`), `guard` (Host 421, Origin 403, cookie 401, `/api/me`'s login
    methods), the CSP and security headers"; new lines: "`login.rs` — `POST /api/login`: token
    (local requests with `--remote`) or password (argon2 in `spawn_blocking`, 4 at once)";
    "`limit.rs` — login reservations: 5 per peer, 30 global per minute, taken before any
    check"; "`peer.rs` — `Peer` (`ConnectInfo`), `PeerKey`"; "`password.rs` — `web-password`
    (argon2id, fixed parameters), `--set-password`"; "`remote.rs` — `--remote`'s address
    grammar, certificate names, start-up banner, `prepare`"; "`tls/` — `cert.rs` (self-signed
    certificate kept in `web-tls/`, fingerprint, `--print-cert`), `listener.rs` (the
    `axum::serve::Listener` over TLS: handshake tasks, connection caps, header deadline)".
  - "Non-obvious constraints (web)": add "**`--remote` is the only non-loopback listener, and
    it is TLS.** `listen_addr` still refuses non-loopback `--listen`; with `--remote` the token
    works for local requests only (loopback peer **and** loopback `Host`), so a reverse proxy
    on the same host never turns internet users into token holders. No HSTS: it would stop
    browsers from accepting the self-signed certificate later." and "**The suites' remote peer
    is a debug-only hook** (`CLAUDIO_TEST_REMOTE_PEER`); it only ever takes the token away."
  - Test commands: `make test-web` already runs the new spec; mention that the web suites'
    `--remote` tests use the committed fixture password
    (`addons/web/web/e2e/fixtures/web-password`, password `claudio-e2e-password`).

```bash
git commit -m "docs(web): claudio web --remote, its security model and how to trust the certificate" -- docs/web.md
git commit -m "docs: README and CLAUDE.md for claudio web --remote" -- README.md CLAUDE.md
```

---

## Final check (controller, after Task 7)

```bash
make check
make matrix
cargo test --test web --test web_manager --test vocdoni_web -- --test-threads=1
make test-web
make static && ldd dist/claudio      # "not a dynamic executable" / static-pie
cargo tree -e normal --target all | grep -c aws-lc    # 0
git log --format='%H %s%n%b' 558a54d..HEAD | grep -ci 'co-authored\|claude\|generated'   # 0
```

Then the gate audit, then AC9 by the owner (a second machine, Brave and Chromium; results go
into `docs/web.md` step 4 and 5 in a follow-up `docs(web)` commit).
