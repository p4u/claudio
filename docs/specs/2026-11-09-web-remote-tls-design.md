# `claudio web --remote`: TLS with a self-signed certificate and a password

Status: draft   Class: architectural (security-sensitive: keys, authentication, a non-loopback
listener)
Owner decisions so far (2026-11-09):
- Remote access without a domain or a certificate authority: TLS with a self-signed
  certificate that the browser pins as an exception the first time (fingerprint shown in the
  terminal to check against). Application-layer crypto in the page was rejected: the page itself
  would travel over plain HTTP and a browser over plain HTTP on a remote address disables
  WebCrypto, keyboard lock and clipboard writes.
- **One port, TLS everywhere** when `--remote` is given: the one listener serves https on the
  chosen address, loopback included. Without `--remote` nothing changes (plain HTTP on loopback).
- **Password only** for remote browsers: `claudio web --set-password` stores an argon2id hash;
  `--remote` refuses to start without one; the token link keeps working for loopback peers only.
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
   start; `--rotate-cert` replaces it.
3. **G3** Remote peers (any non-loopback TCP peer address) log in with a password only; loopback
   peers may still use the token link. The password is set with `claudio web --set-password` and
   stored as an argon2id hash; `--remote` refuses to start when none is set.
4. **G4** The page keeps every feature over the remote TLS connection once the certificate
   exception is accepted (checked by hand in Brave and Chromium; see AC9), and shows a password
   login form when it has no token and no session.
5. **G5** Every check of today's guard keeps holding over TLS (Host 421, Origin 403, cookie 401,
   CSP and headers), with the accepted Host/Origin set extended to the names and addresses the
   remote listener answers to.

## Non-goals

- A certificate authority, ACME, Let's Encrypt, `tailscale cert`, or user-supplied certificate
  files (a later option).
- Client certificates, user accounts, several passwords, or password reset by mail.
- An HTTP listener beside the TLS one, or an HTTP→HTTPS redirect, while `--remote` is on.
- HSTS (it would make the browser refuse the self-signed exception later).
- Changes to the daemon protocol, the TUI, or other addons.

## Current state

- **Listen**: `parse_args` (`lib.rs:86-116`) and `listen_addr` (`:147-172`, loopback only);
  `serve` (`:233-284`) binds a `tokio::net::TcpListener` and runs `axum::serve` (axum 0.8,
  which accepts a custom `axum::serve::Listener`).
- **Auth** (`auth.rs`): `web-token` (`:65-84`, 0600), `POST /api/login {"token"}` (`:391-415`)
  rate-limited globally to 5 failures per minute (`:220-238`), cookie `claudio_web` HttpOnly,
  SameSite=Strict, `Secure` only with `--allow-host` (`:158-160`, `:304-310`); `guard`/`check`
  (`:327-382`): Host 421, Origin 403 for non-GET or upgrades, cookie 401 for `/ws` and `/api/*`
  except login; CSP and headers on every response.
- **Page**: `boot.js` trades `#t=` for the cookie; without a token it asks `/api/me`.
- **Dependencies**: reqwest with rustls (a rustls crypto provider is already in `Cargo.lock`);
  sha2, base64, libc.

## Design

### Command line

```
claudio web [--listen 127.0.0.1:7337] [--remote [ADDR][:PORT]] [--rotate-cert]
            [--allow-host NAME[:PORT]]... [--no-open] [--print-url] [--rotate-token]
claudio web --set-password
```

- `--remote` with no value means `0.0.0.0:7337`; `--remote :8443` means `0.0.0.0:8443`; an
  address without a port uses 7337. `--remote` and `--listen` together are refused (one port).
  A loopback `--remote` address is allowed (TLS on loopback only, for testing).
- `--set-password` reads the password twice from the terminal without echo (refuses when stdin
  is not a terminal, so it never lands in argv, shell history or a pipe log), requires 12–1024
  characters, writes the hash, and exits. Setting it again replaces it; existing sessions keep
  working until they expire or the server restarts.
- `--rotate-cert` (with `--remote`) replaces the certificate and key before serving.

### Certificate

- Generated with `rcgen` (pure Rust): ECDSA P-256 key, self-signed, valid 10 years, CN
  `claudio web`, SANs: the host name (`gethostname`), `localhost`, `127.0.0.1`, `::1`, every
  address of the machine's up, non-loopback interfaces at generation time (`getifaddrs`), the
  `--remote` address when it is not unspecified, and each `--allow-host` name.
- Stored in `<config>/web-tls/cert.pem` (0600) and `key.pem` (0600) in a 0700 directory, written
  atomically (`paths::write_atomic`); reused while both parse and the key matches the
  certificate; a file that is not a private regular file of this uid is refused with a message
  naming `--rotate-cert` (never silently replaced, so the fingerprint never changes behind the
  member's back).
- At every start, stderr shows: `claudio web: TLS certificate SHA-256
  AB:CD:…:EF (names: host, 192.168.1.20, …)`. When a current interface address is missing from
  the SANs, one more line says the browser will warn about the name too and that
  `--rotate-cert` adds it (the fingerprint then changes).
- TLS via `tokio-rustls` with the rustls crypto provider already in the tree, TLS 1.2 and 1.3,
  default cipher suites, no client auth, ALPN `http/1.1` (WebSocket upgrades need HTTP/1.1).
  A failed handshake (a plain-HTTP request, a client that rejects the certificate) closes that
  connection only, logged at debug without client data; handshakes have a 10 s deadline and at
  most 64 run at once.

### Password

- `<config>/web-password`: one PHC string, `argon2id` with m = 19 MiB, t = 2, p = 1 (OWASP's
  baseline), 16-byte random salt; 0600, written atomically. Read and parsed at start; an
  unreadable, non-private or malformed file stops `--remote` with a message naming
  `--set-password`.
- Verification runs in `spawn_blocking` (about 50 ms of CPU), after the rate limit.

### Login and guard

- The TCP peer address comes from the connection (axum `ConnectInfo`), never from a header.
  `loopback = peer.ip().is_loopback()` (an IPv4-mapped loopback counts).
- `POST /api/login` accepts `{"token": …}` from loopback peers and `{"password": …}` from any
  peer; a token from a remote peer is refused (401, counted). Body ≤ 4096 bytes as today.
- Rate limit: per peer IP, 5 failures per minute (then 429 without hashing), and a global cap of
  30 failures per minute across all peers (then 429 for everyone but loopback token logins), so
  one remote attacker cannot lock the member out of loopback use and a spread attack is still
  bounded. At most 1024 tracked IPs (oldest dropped).
- Over TLS the cookie always gets `Secure` (plus HttpOnly, SameSite=Strict, Path=/ as today).
  A session made by a password login is the same kind of session; idle expiry and the cap stay.
- `Allowed` (`auth.rs:97-161`) with `--remote`: the forms of today plus, at the remote port, the
  host name, every interface address the server saw at start, the certificate's SANs and each
  `--allow-host`; Origins accepted as `https://` only for these (and `http://` no longer, since
  the listener speaks only TLS). DNS-rebinding protection stays: a Host outside that set is 421.
- Headers: as today; `Strict-Transport-Security` deliberately **not** sent.

### Page

- With no `#t=` token and `/api/me` 401, the page shows a login form (password field, "Log in",
  the error states 401/429 in words; DESIGN.md: Graphite dialog, mono labels, Deep Teal
  primary). It never stores the password; it posts it once and relies on the cookie.
- `docs/web-protocol.md`: `POST /api/login` documents `{"password"}` and the loopback-only token
  rule (doc commit first, B2). The WebSocket protocol is unchanged.

### Startup output

- stdout keeps exactly one line (the tests read it): without `--remote` unchanged; with
  `--remote`, `claudio web: https://<host name>:<port>/` (no token).
- stderr: the loopback token URL `https://127.0.0.1:<port>/#t=<token>` (also what the browser is
  opened on when one is wanted), the fingerprint and names, and a reminder that remote browsers
  log in with the password.

### Alternatives considered

| Option | Why not |
|---|---|
| Encryption in the page over plain HTTP, key pinned in the browser | The page itself is untrusted on every visit; no secure context (WebCrypto, keyboard lock, clipboard off). |
| Two listeners (loopback HTTP + remote TLS) | Owner chose one port. |
| Token link for remote peers too | Owner chose password only for remote peers. |
| Regenerating the certificate every start | The browser would warn at every restart, teaching the member to click through. |
| `axum-server` crate | `axum::serve` with a small TLS `Listener` adapter needs no new server crate. |

## Compatibility and rollout

- No change without `--remote`: same defaults, same stdout line, same files.
- New files: `<config>/web-password`, `<config>/web-tls/{cert,key}.pem`. The web protocol's
  login op gains an accepted body shape (additive, documented first).
- New dependencies: `rcgen`, `tokio-rustls` (or `rustls` directly), `argon2`; each justified in
  its commit (D7), pure Rust where possible, using the crypto provider already in the tree so the
  static musl build keeps working. They live in claudio-web only (the core takes none, D7).
- Rollback: revert the merge; the new files are ignored by an older build.

## Failure modes and security

- **Threats addressed**: passive and active network attackers between the browser and the host
  (TLS with a pinned self-signed certificate after the first visit); password guessing (argon2id,
  per-IP and global rate limits); DNS rebinding (Host check); CSRF and cross-site WebSocket
  hijacking (Origin check, SameSite=Strict); cookie theft over the network (Secure, HttpOnly).
- **Accepted risk, documented**: the first visit's certificate check is the member's (compare the
  fingerprint); a member who clicks through a changed-certificate warning without checking can be
  attacked. The docs say how to check.
- **Secrets**: the password never in argv, logs, notices or the page's storage; the hash and the
  key are 0600; neither is logged; the login log line names the peer IP and the status only.
- **Untrusted input**: TLS handshakes bounded (deadline, concurrency); login body capped;
  argon2 runs off the executor and only after the rate limit; the tracked-IP table capped.
- **Failure**: a bad certificate or password file stops `--remote` at start with a clear message
  (fail closed; never plain HTTP on a non-loopback address).

## Acceptance criteria

- [ ] AC1: without `--remote`, `claudio web` behaves as before (the existing web suites pass
  unchanged; `listen_addr` still refuses non-loopback addresses).
- [ ] AC2: `--remote` without a password file exits non-zero with a message naming
  `--set-password` and binds nothing (test).
- [ ] AC3: `--set-password` refuses a non-terminal stdin and a password under 12 characters;
  the stored file is a PHC argon2id string, 0600 (unit tests on the store; the prompt by hand).
- [ ] AC4: with `--remote 127.0.0.1:<port>` (tests), a rustls client pinning the printed
  fingerprint completes TLS; a plain-HTTP request to the port gets no HTTP response; the
  certificate is reused across restarts (same fingerprint) and replaced by `--rotate-cert`;
  `key.pem`/`cert.pem` are 0600 in a 0700 directory (tests).
- [ ] AC5: login rules: a loopback peer logs in with the token or the password; a non-loopback
  peer (the peer-address rule unit-tested on `check`/`login`, and an integration test on a
  non-loopback address of the test host when one exists) is refused the token and accepted with
  the password; per-IP and global rate limits answer 429 without running argon2 (tests).
- [ ] AC6: over TLS the cookie carries `Secure`; Host and Origin outside the accepted set give
  421/403; `http://` Origins are refused with `--remote` (tests).
- [ ] AC7: the page shows the password form without a token, logs in, and the session works
  (Playwright against `--remote 127.0.0.1:<port>` with `ignoreHTTPSErrors`; zero CSP
  violations).
- [ ] AC8: `make check`, `make matrix` and every web suite pass; the static musl build succeeds.
- [ ] AC9 (by hand, owner): from another machine in Brave, accept the certificate after
  checking its fingerprint, log in with the password, and confirm Capture all keys, clipboard
  copy and the installable window work.

## Constraints

- ACCEPTANCE.md throughout (C1 secrets, C2 bounded input, C4 file modes, C5 web perimeter, D7
  dependencies, A8 sizes, E1–E3 tests, F1 docs, F3 DESIGN.md).
- `docs/web-protocol.md` and `docs/web.md` (security model, "From another machine") change
  before or with the code (B2, F1).
- Never push or tag without the owner. No AI attribution in commits.

## Open questions

None blocking. AC9 needs the owner (a second machine and Brave).
