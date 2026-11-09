# `claudio web --remote`: TLS with a self-signed certificate and a password

Status: audited (2026-11-09)   Class: architectural (security-sensitive: keys, authentication, a
non-loopback listener)
Audit: `.delivery/2026-11-09-web-remote-tls/audit-spec.md` (APPROVE WITH CHANGES, F1–F16 folded
in; F1 as amended by ruling R1: tests use a committed fixture hash, `--set-password` stays
TTY-only).
Owner decisions so far (2026-11-09):
- Remote access without a domain or a certificate authority: TLS with a self-signed
  certificate that the browser accepts as an exception the first time (fingerprint shown in the
  terminal to check against), or that the member installs as trusted (`--print-cert`).
  Application-layer crypto in the page was rejected: the page itself would travel over plain
  HTTP and a browser over plain HTTP on a remote address disables WebCrypto, keyboard lock and
  clipboard writes.
- **One port, TLS everywhere** when `--remote` is given: the one listener serves https on the
  chosen address, loopback included. Without `--remote` nothing changes (plain HTTP on loopback).
- **Password only** for remote browsers: `claudio web --set-password` stores an argon2id hash;
  `--remote` refuses to start without one; the token link keeps working for local requests only.
- **The certificate is generated once and kept**, its SHA-256 fingerprint printed at every
  start; `--rotate-cert` makes a new one.

## Problem

`claudio web` listens on loopback only (`addons/web/src/lib.rs:147-172` refuses anything else)
and authenticates with a 64-hex token carried in the URL fragment (`auth.rs:3-12`). Reaching it
from another device needs an SSH tunnel or a TLS reverse proxy with a domain
(`docs/web.md`, "From another machine"). The owner wants to open it from another machine (or a
phone) with a password, without a domain, while keeping everything that the page needs a secure
context for (WebCrypto, `navigator.keyboard.lock`, clipboard writes, the installable window).

## Goals

1. **G1** `claudio web --remote [ADDR][:PORT]` serves the app over TLS on one listener (default
   `0.0.0.0:7337`; `[::]` and a specific interface address accepted); every client, loopback
   included, speaks TLS to it. Without `--remote`, behaviour and the loopback-only rule are
   unchanged.
2. **G2** A self-signed certificate is generated on the first `--remote` start, kept, and reused;
   its SHA-256 fingerprint (colon-separated hex) and the names it covers are printed at every
   start; `--rotate-cert` replaces it; `--print-cert` prints it for installing as trusted.
3. **G3** Remote requests log in with a password only; local requests (a loopback peer that used
   a loopback `Host`) may still use the token link. The password is set with
   `claudio web --set-password` and stored as an argon2id hash; `--remote` refuses to start when
   none is set.
4. **G4** The page keeps every feature over the remote TLS connection once the certificate
   exception is accepted or the certificate installed (checked by hand in Brave and Chromium;
   see AC9), and shows a password login form when it has no token and no session and the server
   offers a password login.
5. **G5** Every check of today's guard keeps holding over TLS (Host 421, Origin 403, cookie 401,
   CSP and headers), with the accepted Host/Origin set extended to the names and addresses the
   remote listener answers to.

## Non-goals

- A certificate authority, ACME, Let's Encrypt, `tailscale cert`, or user-supplied certificate
  files (a later option).
- Client certificates, user accounts, several passwords, or password reset by mail.
- An HTTP listener beside the TLS one, or an HTTP→HTTPS redirect, while `--remote` is on.
- HSTS (it would make the browser refuse the self-signed exception later).
- A TLS reverse proxy in front of `--remote` (unsupported; a proxy setup keeps using plain
  `--listen` + `--allow-host`, as today).
- Changes to the daemon protocol, the TUI, or other addons.

## Current state

- **Listen**: `parse_args` (`lib.rs:86-116`) and `listen_addr` (`:147-172`, loopback only);
  `serve` (`:233-284`) binds a `tokio::net::TcpListener` and runs `axum::serve` (axum 0.8.9,
  which accepts a custom `axum::serve::Listener`; its `Connected` impls cover only
  `IncomingStream<TcpListener>` and `TapIo`, `serve` awaits `Listener::accept` sequentially, and
  its hyper builder has no timer, so no header-read or idle timeout).
- **Auth** (`auth.rs`): `web-token` (`:65-84`, 0600), `POST /api/login {"token"}` (`:391-415`)
  rate-limited globally to 5 failures per minute, counted after the compare (`:220-238`), cookie
  `claudio_web` HttpOnly, SameSite=Strict, `Secure` only with `--allow-host` (`:158-160`,
  `:304-310`); `guard`/`check` (`:327-382`): Host 421, Origin 403 for non-GET or upgrades,
  cookie 401 for `/ws` and `/api/*` except login; CSP and headers on every response.
- **Page**: `boot.js` trades `#t=` for the cookie; without a token it asks `/api/me` and on 401
  shows "open the printed URL".
- **Dependencies**: claudio-web has axum, sha2, base64, serde, tokio; reqwest with rustls lives in
  claudio-core, so `rustls 0.23.45` → `ring 0.17.14` is in `Cargo.lock` (no `aws-lc-rs`), as are
  `tokio-rustls 0.26.6` and `rustls-webpki 0.103.15`. `libc` is a dependency of claudio-core and
  claudio-vocdoni, not of claudio-web.

## Design

### Command line

```
claudio web [--listen 127.0.0.1:7337] [--remote [ADDR][:PORT]] [--rotate-cert]
            [--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
claudio web --set-password
claudio web --print-cert
```

- `--remote` takes an optional value only in the `--remote=VALUE` form or when the next word
  does not start with `-`. `VALUE` is `ADDR`, `ADDR:PORT`, `:PORT`, or `[V6]`/`[V6]:PORT`; a
  bare IPv6 address must be bracketed; the default is `0.0.0.0:7337` (IPv4 only; use `[::]` for
  dual stack). `--remote` with `--listen` is refused (one port); `--rotate-cert` without
  `--remote` is refused ("needs --remote"). `--print-url` with `--remote` prints the loopback
  token URL (`https://127.0.0.1:<port>/#t=…`), the one a local browser can use; a remote URL
  carries no token and is the stdout line. A loopback `--remote` address is allowed (TLS on
  loopback only, for testing).
- An `--allow-host NAME:PORT` whose port differs from the listen port is refused with
  `--remote` (the TLS listener answers on one port); the SAN takes `NAME` only.
- `--set-password` reads the password twice from the terminal without echo (refuses when stdin
  is not a terminal, so it never lands in argv, shell history or a pipe log; there is no
  `--stdin` form), requires 12–1024 **bytes** of UTF-8 (NFC not applied; the body cap of 4 KiB
  leaves room for JSON escaping), writes the hash, reads the file back and verifies the password
  against it, and exits. Setting it again replaces the file; a running `claudio web` keeps the
  hash it read at start, so the command ends with "restart claudio web to use it (that also
  signs every browser out)". The prompt restores the terminal's echo on every exit path, SIGINT
  included. The no-echo prompt is a core service (`claudio_core::term::secret`), moved there
  from the Vocdoni addon's token prompt, which then uses it too.
- `--rotate-cert` (with `--remote`) replaces the certificate and key before serving.
- `--print-cert` writes `cert.pem` to stdout (no key) and exits; it fails with a message naming
  `claudio web --remote` when there is no certificate yet.

### Certificate

- Generated with `rcgen` (pure Rust): ECDSA P-256 key, self-signed, CN `claudio web`, valid
  10 years with `notBefore` one day before generation (phone clocks drift).
- SANs: `localhost`, `127.0.0.1`, `::1`, the host name (`gethostname`, when it is a valid DNS
  name) and `<hostname>.local` (its first label; mDNS, what a phone on the LAN uses), the
  `--remote` address when it is a specific one, and each `--allow-host` name. No interface
  enumeration.
- Stored in `<config>/web-tls/cert.pem` (0600) and `key.pem` (0600) in a 0700 directory, written
  atomically (`paths::write_atomic`, the key first); reused while both parse and the key matches
  the certificate (rustls' `with_single_cert` fails with a key mismatch otherwise). The file
  check is `symlink_metadata` (a symlink is refused, like `load_or_create_token`,
  `auth.rs:68-71`), mode `& 0o077 == 0`, uid ours, regular file, at most 16 KiB. The directory
  is created with `paths::ensure_private_dir` (0700). A file that fails the check, or a pair
  that does not parse or match, is refused with a message naming `--rotate-cert` (never silently
  replaced, so the fingerprint never changes behind the member's back). The fingerprint is
  SHA-256 of the DER certificate (what browsers show), uppercase hex, colon-separated.
- At every start, stderr shows `claudio web: TLS certificate SHA-256 AB:CD:…:EF` and the names
  it covers. When one of today's SAN names (host name, `.local` name, `--remote` address,
  `--allow-host`) is missing from the kept certificate (checked with `rustls-webpki`, already in
  the tree), one more line says the browser will warn about the name too and that
  `--rotate-cert` adds it (the fingerprint then changes).
- `claudio web --print-cert` writes `cert.pem` to stdout (no key), so the member can install
  the certificate as trusted on a laptop or phone (`docs/web.md` gives the Chromium, Firefox,
  Android and iOS steps). With the certificate trusted there is no interstitial, the pin is
  the trust-store entry, and the installable window and future service workers work. The
  exception path stays the zero-setup default.
- TLS via `tokio-rustls`: an `axum::serve::Listener` adapter owning the `TcpListener`. It
  accepts TCP, spawns one handshake task per connection (10 s deadline; at most 64 in flight,
  8 at once per address, loopback exempt, so two addresses cannot take every handshake slot;
  further connections are closed at once) and yields completed `TlsStream<TcpStream>` with
  the peer address through a bounded channel to `accept`. The peer address reaches handlers as
  `ConnectInfo<Peer>` where `Peer(SocketAddr)` is claudio-web's newtype implementing
  `axum::extract::connect_info::Connected<IncomingStream<'_, TlsListener>>` (the orphan rule
  forbids the impl on `SocketAddr`); the plain listener uses the same `Peer`. Open connections
  are capped at 256 (a semaphore permit travels inside the `Io` wrapper and drops with it), and
  at 32 per non-loopback peer (IPv4 address or IPv6 /64; a plan addition, so one address cannot
  take every slot); an accepted connection that has not sent the end of its first request's
  headers (`\r\n\r\n`) within 30 s is closed by a wrapper deadline (axum's hyper builder has no
  timer); the deadline covers that first request only, so an idle keep-alive connection
  afterwards keeps its slot until the client closes it (bounded by the connection caps). TLS 1.2 and 1.3, default suites, no client auth, ALPN `http/1.1` only (the
  workspace's axum has no `http2` feature). The `ServerConfig` is built from an explicit
  `rustls::crypto::ring::default_provider()`. A failed handshake closes that connection only,
  logged at debug with the peer address and the rustls error, never bytes.

### Password

- **File format is the contract, not the prompt.** `<config>/web-password` holds one PHC
  string (`$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`, one line, trailing newline
  optional): argon2id with m = 19 MiB, t = 2, p = 1 (OWASP's baseline), 16-byte random salt,
  32-byte output; 0600, written atomically. `--set-password` is one writer of that file; a test
  writes it directly. The repository carries one fixture hash of a known test password
  (`addons/web/web/e2e/fixtures/web-password`, read by the Rust suites too), generated once with
  the same code; tests copy it into the isolated config dir. Verification of that fixture is a
  unit test (E1), so a change of parameters cannot silently invalidate it.
- Read and parsed at start; an unreadable, non-private, oversized (> 512 bytes) or malformed
  file, or one with other parameters than the above (which would change the memory bound),
  stops `--remote` with a message naming `--set-password`.
- Verification runs in `spawn_blocking` (about 15 ms of CPU optimised, 19 MiB), after the rate
  limit, under the hashing semaphore (below). The debug profile builds `argon2` and `blake2` at
  `opt-level = 3`, as it does `sha2`, so the suites' debug binary verifies in milliseconds.

### Login and guard

- The TCP peer address comes from the connection (`ConnectInfo`), never from a header. A
  request is **local** when the peer address is loopback (`IpAddr::to_canonical()`, so
  `::ffff:127.0.0.1` counts) **and** its `Host` is one of the loopback forms at the listen port
  (`127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`, and the listen address itself when it
  is loopback). A reverse proxy that keeps the browser's `Host` (as `docs/web.md` requires)
  therefore never produces a local request, and a proxy that rewrites `Host` to loopback is a
  misconfiguration the docs name.
- `POST /api/login` accepts `{"token": …}` from local requests only and `{"password": …}` from
  any; a token in a non-local request is 401 and counts. Body ≤ 4096 bytes as today. Without
  `--remote` nothing changes: the token is accepted from every request (every peer is loopback,
  and a `--listen` reverse proxy keeps working) and a password body answers 400.
- With `--remote`, `--allow-host NAME[:PORT]` means a name the TLS listener itself answers to
  (a DNS or `/etc/hosts` name for this machine); it is added to the SANs and to `Allowed`. A
  TLS reverse proxy in front of `--remote` is unsupported and `docs/web.md` says so.
- A debug build only (`cfg!(debug_assertions)`) reads `CLAUDIO_TEST_REMOTE_PEER=<ip>`: that
  loopback address then counts as a remote peer, the one way a hermetic suite gets a non-local
  peer through the real listener. It can only take the token away, never grant access; a
  release build compiles it out.
- Rate limit, applied **before** hashing and as a reservation: a login attempt first takes a
  slot under the lock (the peer's window and the global window are both counted at that
  moment, as attempts, not as failures); a success releases the peer's slot, a failure keeps
  it. Peer key: the canonical IPv4 address, or the IPv6 /64 (every loopback address is one
  key). Limits: 5 attempts per peer per minute (429 before any hashing), 30 per minute globally
  (429 for every password login; local token logins are exempt), at most 1024 tracked peers
  (the one idle longest is dropped). Argon2 verifications run in `spawn_blocking` under a
  semaphore of 4 permits; a request that cannot get a permit in 2 s answers 503 and does not
  count. Memory bound: 4 × 19 MiB. Without `--remote` the same limiter replaces today's global
  window; observable behaviour is unchanged (every peer is the one loopback key).
- `GET /api/me` without a live session answers `401` with body `{"login": [...]}` listing the
  methods this request may use: `["token"]` (no `--remote`), `["token","password"]` (a local
  request with `--remote`) or `["password"]` (a non-local request). Documented in
  `docs/web-protocol.md` §1 first (B2).
- Over TLS the cookie always gets `Secure` (plus HttpOnly, SameSite=Strict, Path=/ as today).
  A session made by a password login is the same kind of session; idle expiry and the cap stay.
- `Allowed` with `--remote`: today's loopback forms at the listen port (never an unspecified
  address), the listen address when it is a specific one, any syntactically valid IPv4 or
  bracketed IPv6 literal at the listen port that is neither unspecified nor multicast (an IP
  literal cannot be the target of DNS rebinding), the host name and `<hostname>.local`, and each
  `--allow-host`; each also bare when the port is 443. Origins are accepted as `https://` only.
  A DNS name outside this set is 421.
- Headers: as today; `Strict-Transport-Security` deliberately **not** sent.

### Page

- The page shows the password form only when `/api/me`'s 401 lists `"password"`, and today's
  "open the printed URL" text otherwise (a password form on a plain loopback server could only
  fail). The form: password field, "Log in", the error states 401/429/503 in words; DESIGN.md:
  Carbon card, mono labels, Deep Teal primary. It never stores the password; it posts it once
  and relies on the cookie.
- `docs/web-protocol.md`: `POST /api/login` documents `{"password"}`, `503`, the loopback-only
  token rule and the reservation limit; `GET /api/me`'s 401 body; the `--remote` Host/Origin set
  and cookie (doc commit first, B2). The WebSocket protocol is unchanged (the page already picks
  `wss://` on an `https:` page).

### Startup output

- stdout keeps exactly one line (the tests read it): without `--remote` unchanged; with
  `--remote`, `claudio web: https://<host>:<port>/` (no token), where `<host>` is the `--remote`
  address when it is a specific one and the host name otherwise. The format stays
  `claudio web: <url>`; with `--remote` the URL has no fragment. The Rust harness's
  `start_remote` parses the port from it and never expects a token there.
- stderr, printed before the stdout line: the fingerprint and names; the loopback token URL
  `https://127.0.0.1:<port>/#t=<token>` (`[::1]` for `[::]`, the address itself for a loopback
  one; omitted when the listen address is a specific non-loopback one, which a loopback request
  cannot reach); a reminder that remote browsers log in with the password. The browser, when
  one is wanted, opens the token URL (or the stdout URL when there is none).
- When the listen address is unspecified, stderr adds one line naming the port and that a host
  firewall (ufw, firewalld, macOS's) may still block it; the stdout URL uses the host name,
  which another machine may not resolve: the docs tell the member to use the printed `.local`
  name or the machine's address.
- The e2e fixture (`server.mjs`) grows a `remote: true` option: it writes the fixture
  password file, starts `claudio web --remote 127.0.0.1:<port> --no-open`, reads the stdout
  line for the port only, logs in with the password over `https` (node `https`/`wss` with
  `rejectUnauthorized: false`, `Origin: https://127.0.0.1:<port>`), and Playwright runs with
  `ignoreHTTPSErrors: true`. The Rust harness (`tests/web.rs`, `web_harness/mod.rs`) gets a
  `start_remote` that pins the served certificate's SHA-256 (read from stderr, checked against
  `cert.pem`) in a rustls client. Existing tests keep the `http://…#t=` stdout assertion
  unchanged (AC1).

### Alternatives considered

| Option | Why not |
|---|---|
| Encryption in the page over plain HTTP, key pinned in the browser | The page itself is untrusted on every visit; no secure context (WebCrypto, keyboard lock, clipboard off). |
| Two listeners (loopback HTTP + remote TLS) | Owner chose one port. |
| Token link for remote peers too | Owner chose password only for remote peers. |
| Regenerating the certificate every start | The browser would warn at every restart, teaching the member to click through. |
| `axum-server` crate | `axum::serve` with a small TLS `Listener` adapter (and a `Peer` newtype for `ConnectInfo`) needs no new server crate. |
| Only the browser exception, no `--print-cert` | Chromium keeps an exception 7 days, refuses service workers and the install prompt on a certificate error, and a phone cannot show the fingerprint; installing the certificate fixes all three. The exception stays the zero-setup default. |
| `--set-password --stdin` for scripts and tests | Keeps passwords out of pipes; tests use a committed fixture hash instead (ruling R1). |
| Interface enumeration (`getifaddrs`) for SANs and `Allowed` | IP literals need no allowlist; a self-signed certificate already fails validation, so a SAN mismatch adds no second warning; enumeration adds `unsafe` FFI and goes stale on DHCP or VPN changes. |

## Compatibility and rollout

- No change without `--remote`: same defaults, same stdout line, same files. `/api/me`'s 401
  gains a body (additive); the page reads it.
- New files: `<config>/web-password`, `<config>/web-tls/{cert,key}.pem`. The web protocol's
  login op gains an accepted body shape (additive, documented first).
- New dependencies, in `[workspace.dependencies]` and used by claudio-web only:
  `tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "tls12"] }`
  (never `aws_lc_rs`: the lock already resolves `rustls 0.23` to `ring` through reqwest and a
  second provider breaks `make static` and the builder's default-provider rule);
  `rcgen = { version = "0.14", default-features = false, features = ["crypto", "pem", "ring"] }`;
  `argon2 = "0.6"` (pure Rust; `password-hash` for PHC strings); `rustls-webpki = { version =
  "0.103", default-features = false, features = ["std"] }` (already in the tree, for the SAN
  check). claudio-web needs no `libc`: `gethostname` and the no-echo prompt are core services
  (`claudio_core::sys::hostname`, `claudio_core::term::secret`). Each is justified in its commit
  (D7), pure Rust except `ring`, which the tree already builds. The core takes none (D7).
- Rollback: revert the merge; the new files are ignored by an older build.

## Failure modes and security

- **Threats addressed**: passive and active network attackers between the browser and the host
  (TLS with a self-signed certificate the member checked once, or installed); password guessing
  (argon2id, reservation-based per-peer and global rate limits, bounded hashing); DNS rebinding
  (Host check); CSRF and cross-site WebSocket hijacking (Origin check, SameSite=Strict); cookie
  theft over the network (Secure, HttpOnly); slow or stalled clients (handshake deadline and
  concurrency, a first-request header deadline, connection caps); a token leak (the token works only for local
  requests).
- **Accepted risk, documented**: the first visit's certificate check is the member's (compare the
  fingerprint); the browser remembers the exception per host and certificate (Chromium for 7
  days, Firefox until removed); a changed certificate shows a new warning. On a phone the
  fingerprint cannot be read from the warning: install the certificate (`--print-cert`) instead
  of clicking through. A member who clicks through a changed-certificate warning without
  checking can be attacked. The docs say how to check.
- **Accepted risk, documented**: a remote attacker who keeps the global window full locks every
  remote password login (the owner's included) while the attack lasts; loopback use is
  unaffected. An attacker holding idle connections is bounded by 32 per address and 256 in all;
  a spread attack from many addresses can fill the 256 while it lasts.
- **Secrets**: the password never in argv, logs, notices or the page's storage; the hash and the
  key are 0600; neither is logged; the login log line names the status and whether the request
  was local, never the body.
- **Untrusted input**: TLS handshakes bounded (deadline, concurrency); login body capped;
  argon2 runs off the executor, only after the rate limit, under 4 permits; the tracked-IP
  table capped; the password, PEM and token files size-capped before reading.
- **Failure**: a bad certificate or password file stops `--remote` at start with a clear message
  (fail closed; never plain HTTP on a non-loopback address).

## Acceptance criteria

- [ ] AC1: without `--remote`, `claudio web` behaves as before (the existing web suites pass
  unchanged; `listen_addr` still refuses non-loopback addresses).
- [ ] AC2: `--remote` without a password file exits non-zero with a message naming
  `--set-password` and binds nothing (test).
- [ ] AC3: `--set-password` refuses a non-terminal stdin (integration test) and a password
  under 12 bytes; the stored file is a PHC argon2id string with the fixed parameters, 0600, and
  the committed fixture verifies against its test password (unit tests on the store; the prompt
  by hand).
- [ ] AC4: with `--remote 127.0.0.1:<port>` (tests), a rustls client pinning the printed
  fingerprint completes TLS; a plain-HTTP request to the port gets no HTTP response; the
  certificate is reused across restarts (same fingerprint) and replaced by `--rotate-cert`;
  `key.pem`/`cert.pem` are 0600 in a 0700 directory; `--print-cert` prints the certificate and
  no key (tests).
- [ ] AC5: login rules: a local request logs in with the token or the password; a non-local
  request is refused the token and accepted with the password. The peer-address rule is
  unit-tested on the classifier (loopback, `::ffff:127.0.0.1`, a private address, `Host`
  forms); on a `[::]` listener a 127.0.0.1 peer is local (unit test on the classifier with
  `::ffff:127.0.0.1`); an integration test binds `--remote 127.0.0.1:<port>` and connects from
  `127.0.0.2` with `CLAUDIO_TEST_REMOTE_PEER=127.0.0.2` (Linux routes all of 127/8 to `lo`; the
  test returns early on macOS as `web_answers_on_its_listen_address_and_a_proxy_port` does,
  `tests/web.rs:205-209`); a loopback peer whose `Host` is `<name>:<port>` (an `--allow-host`
  name) is refused the token and accepted with the password (test); per-IP and global rate
  limits answer 429 without running argon2, and 20 concurrent wrong passwords from one peer
  give exactly five 401s and fifteen 429s (tests).
- [ ] AC6: over TLS the cookie carries `Secure`; Host and Origin outside the accepted set give
  421/403; `http://` Origins are refused with `--remote`; no `Strict-Transport-Security`
  header (tests).
- [ ] AC7: the page shows the password form without a token, logs in, and the session works
  (Playwright against `--remote 127.0.0.1:<port>` with the fixture password and
  `ignoreHTTPSErrors`; zero CSP violations).
- [ ] AC8: `make check`, `make matrix` and every web suite pass; the static musl build succeeds.
- [ ] AC9 (by hand, owner, from another machine, Brave and Chromium): (a) with the exception
  alone: `window.isSecureContext === true` in the console; Capture all keys enters fullscreen
  and `navigator.keyboard.lock()` resolves; copy from the reader works; note whether the
  install affordance appears (expected: not). (b) with `--print-cert` installed in the OS trust
  store: no interstitial, and the install affordance appears. The result of (a) and (b) goes
  into `docs/web.md` as the supported paths.

## Constraints

- ACCEPTANCE.md throughout (C1 secrets, C2 bounded input, C4 file modes, C5 web perimeter, D7
  dependencies, A8 sizes, E1–E3 tests, F1 docs, F3 DESIGN.md).
- `docs/web-protocol.md` and `docs/web.md` (security model, "From another machine") change
  before or with the code (B2, F1).
- Never push or tag without the owner. No AI attribution in commits.

## Open questions

None blocking. AC9 needs the owner (a second machine and Brave).
