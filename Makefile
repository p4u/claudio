# claudio — build a portable static binary that wraps the real `claude`.
#
# The wrapper finds `claude` on PATH (or $CLAUDIO_CLAUDE_PATH) and proxies
# every invocation. With -p/--print it emulates print mode by driving the
# interactive TUI; without it, it execs the real claude transparently.
#
# Common targets:
#   make            → native release build           (target/release/claudio)
#   make static     → portable static musl binary    (dist/claudio; ARCH=aarch64 for arm64)
#   make static-docker → static binary via container (no host Rust toolchain)
#   make test       → unit tests + full E2E suite against real claude
#   make test-unit  → unit tests only (no claude needed, ~0.1 s)
#   make test-web   → browser app: node unit tests + Playwright e2e (debug build, fake claude/proxy)
#   make e2e        → E2E suite only (alias for the integration subset of make test)
#   make evidence   → run the PoC against real claude and capture artifacts
#   make docker     → build the Docker image (docker compose)
#   make install    → install the static binary to $(PREFIX)/bin
#   make dev        → install a local test build (no release, daemons replaced once idle)
#   make clean

CARGO       ?= cargo
PREFIX      ?= $(HOME)/.local
BIN         := claudio
# Static target. Override ARCH for cross builds, e.g. ARCH=aarch64.
ARCH        ?= x86_64
MUSL_TARGET := $(ARCH)-unknown-linux-musl
DIST        := dist

.PHONY: all build static static-docker test test-unit test-web e2e fmt clean install dev dev-undo uninstall docker evidence help

all: build

## Native (dynamically linked) release build.
build:
	$(CARGO) build --release --locked
	@echo "→ target/release/$(BIN)"

## Portable static binary via musl. rustup's bundled toolchain links musl
## self-contained (no musl-gcc needed). Output copied to dist/.
## The musl std lives inside rustup's toolchain, which a distro cargo/rustc
## earlier on PATH cannot see — so build with rustup's binaries explicitly.
RUSTUP_CARGO := $(shell rustup which cargo 2>/dev/null)
RUSTUP_RUSTC := $(shell rustup which rustc 2>/dev/null)
static:
	rustup target add $(MUSL_TARGET)
	$(if $(RUSTUP_RUSTC),RUSTC=$(RUSTUP_RUSTC)) $(or $(RUSTUP_CARGO),$(CARGO)) build --release --locked --target $(MUSL_TARGET)
	@mkdir -p $(DIST)
	@cp target/$(MUSL_TARGET)/release/$(BIN) $(DIST)/$(BIN)
	@echo "→ $(DIST)/$(BIN)"
	@file $(DIST)/$(BIN) || true
	@echo "static check:"; ldd $(DIST)/$(BIN) 2>&1 || true

## Reproducible static build with zero host toolchain — uses a musl container.
## Works on any machine with Docker.
static-docker:
	@mkdir -p $(DIST)
	docker run --rm -v "$(CURDIR)":/src -w /src rust:1-alpine sh -c '\
		apk add --no-cache musl-dev >/dev/null && \
		cargo build --release --locked && \
		cp target/release/$(BIN) /src/$(DIST)/$(BIN)'
	@echo "→ $(DIST)/$(BIN)"
	@file $(DIST)/$(BIN) || true

## Full test suite: unit tests first, then E2E against real claude.
## Cadence typing is disabled (CLAUDIO_CADENCE=0) so E2E runs complete
## in ~5 min rather than ~15 min; stealth is verified by the detect_* tests.
## Requires an authenticated `claude` on PATH.
test: test-unit
	CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 $(CARGO) test --locked --test integration \
		-- --test-threads=1 --nocapture

## Unit tests only — no network, no claude, ~0.1 s.
test-unit:
	$(CARGO) test --locked --bin claudio

## Session-manager integration tests (fake claude, no API needed).
test-manager:
	$(CARGO) test --locked --test manager -- --test-threads=1

## Browser app: node unit tests, then Playwright e2e against a debug build
## (fake claude and proxy; uses the browsers cached in ~/.cache/ms-playwright).
test-web:
	node --test web/tests/*.test.mjs
	$(CARGO) build --locked
	cd web/e2e && PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund && npx playwright test

## E2E integration tests only (alias; same as the integration portion of `make test`).
e2e:
	CLAUDIO_E2E=1 CLAUDIO_CADENCE=0 $(CARGO) test --locked --test integration \
		-- --test-threads=1 --nocapture

fmt:
	$(CARGO) fmt

## Run representative cases against the real, locally-authenticated claude.
evidence: build
	./scripts/capture-evidence.sh ../evidence

## Build the container image (bundles Node + claude + the wrapper).
docker:
	docker compose build

## Install the static binary. NOTE: do NOT install it as `claude` — the wrapper
## must be able to find the real `claude` on PATH. Point your tooling at
## `claudio -p ...` (or alias it).
## Copy to a temp name, then rename: a running daemon keeps the old binary
## ("Text file busy" otherwise). A running daemon is then restarted on the new
## binary; its sessions come back with `claude --resume`.
install: static
	@mkdir -p $(PREFIX)/bin
	@install -m755 $(DIST)/$(BIN) $(PREFIX)/bin/.$(BIN).new
	@mv -f $(PREFIX)/bin/.$(BIN).new $(PREFIX)/bin/$(BIN)
	@echo "installed $(PREFIX)/bin/$(BIN)"
	@if $(PREFIX)/bin/$(BIN) daemon status >/dev/null 2>&1; then \
		$(PREFIX)/bin/$(BIN) daemon restart; \
	fi

## Install a build of this checkout for testing on this host, without a release.
## The page's footer then reads e.g. `claudio 0.8.0+dev.20f0b49` (`.dirty` with
## uncommitted changes). The previous binary is kept as $(BIN).prev (undo:
## `make dev-undo`). Running daemons are NOT restarted: each is replaced by this
## build once it is idle, when a claudio client (TUI or `claudio web`) connects.
## Restart a running `claudio web` yourself (Ctrl+C, run it again, reload the page).
## Remote hosts get this build too when the TUI or `claudio web` connects to them.
dev:
	CLAUDIO_BUILD_LABEL="+dev.$$(git rev-parse --short HEAD)$$(git diff --quiet HEAD -- 2>/dev/null || echo .dirty)" \
		$(MAKE) --no-print-directory static
	@mkdir -p $(PREFIX)/bin
	@if [ -f $(PREFIX)/bin/$(BIN) ]; then cp -p $(PREFIX)/bin/$(BIN) $(PREFIX)/bin/$(BIN).prev; fi
	@install -m755 $(DIST)/$(BIN) $(PREFIX)/bin/.$(BIN).new
	@mv -f $(PREFIX)/bin/.$(BIN).new $(PREFIX)/bin/$(BIN)
	@echo "installed $(PREFIX)/bin/$(BIN) (local build; previous one in $(BIN).prev)"
	@echo "next: restart \`claudio web\` (Ctrl+C, run it again) and reload the page"

## Put back the binary `make dev` replaced.
dev-undo:
	@test -f $(PREFIX)/bin/$(BIN).prev || { echo "no $(PREFIX)/bin/$(BIN).prev"; exit 1; }
	@mv -f $(PREFIX)/bin/$(BIN).prev $(PREFIX)/bin/$(BIN)
	@echo "restored $(PREFIX)/bin/$(BIN); a daemon already on the test build keeps it"
	@echo "(only newer builds replace a daemon): run \`claudio daemon restart\` to switch it back"

uninstall:
	@rm -f $(PREFIX)/bin/$(BIN)
	@echo "removed $(PREFIX)/bin/$(BIN)"

clean:
	$(CARGO) clean
	@rm -rf $(DIST)

help:
	@grep -E '^(##|[a-zA-Z_-]+:)' $(MAKEFILE_LIST) | sed 's/^## /  /' | sed 's/:.*//'
