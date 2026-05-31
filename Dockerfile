# syntax=docker/dockerfile:1

# ─── build stage ───────────────────────────────────────────────────────────
# Compile the static-ish release binary with the pinned Cargo.lock.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# ─── runtime stage ─────────────────────────────────────────────────────────
# Claude Code is a Node CLI, so the runtime needs Node + the claude package.
FROM node:22-bookworm-slim

# Claude Code CLI. Pin via build arg if you need a specific version.
ARG CLAUDE_VERSION=latest
RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_VERSION}" \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/claudio /usr/local/bin/claudio

# The node base image ships an unprivileged `node` user (uid 1000). Running as
# non-root also avoids Claude Code's refusal to use --dangerously-skip-permissions
# as root. The host's ~/.claude is mounted onto this user's home (see compose).
ENV HOME=/home/node

# When run with `--api`, bind to all interfaces so the server is reachable once
# the port is published (`-p` / compose `ports`). Ignored in CLI mode, and the
# native binary still defaults to 127.0.0.1. Override with -e CLAUDIO_API_BIND=…
ENV CLAUDIO_API_BIND=0.0.0.0:8080
EXPOSE 8080

USER node
WORKDIR /work

# Default entrypoint is the CLI (`-p`, transparent passthrough). For the
# OpenAI-compatible server, append `--api` — e.g.
#   docker run -p 8080:8080 claudio --api
#   docker compose up claudio-api
ENTRYPOINT ["claudio"]
