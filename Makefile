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
#   make e2e        → E2E suite only (alias for the integration subset of make test)
#   make evidence   → run the PoC against real claude and capture artifacts
#   make docker     → build the Docker image (docker compose)
#   make install    → install the static binary to $(PREFIX)/bin
#   make clean

CARGO       ?= cargo
PREFIX      ?= $(HOME)/.local
BIN         := claudio
# Static target. Override ARCH for cross builds, e.g. ARCH=aarch64.
ARCH        ?= x86_64
MUSL_TARGET := $(ARCH)-unknown-linux-musl
DIST        := dist

.PHONY: all build static static-docker test test-unit e2e fmt clean install uninstall docker evidence help

all: build

## Native (dynamically linked) release build.
build:
	$(CARGO) build --release --locked
	@echo "→ target/release/$(BIN)"

## Portable static binary via musl. rustup's bundled toolchain links musl
## self-contained (no musl-gcc needed). Output copied to dist/.
static:
	rustup target add $(MUSL_TARGET)
	$(CARGO) build --release --locked --target $(MUSL_TARGET)
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
install: static
	@mkdir -p $(PREFIX)/bin
	@cp $(DIST)/$(BIN) $(PREFIX)/bin/$(BIN)
	@echo "installed $(PREFIX)/bin/$(BIN)"

uninstall:
	@rm -f $(PREFIX)/bin/$(BIN)
	@echo "removed $(PREFIX)/bin/$(BIN)"

clean:
	$(CARGO) clean
	@rm -rf $(DIST)

help:
	@grep -E '^(##|[a-zA-Z_-]+:)' $(MAKEFILE_LIST) | sed 's/^## /  /' | sed 's/:.*//'
