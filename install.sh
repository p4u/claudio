#!/usr/bin/env bash
# Install claudio from its GitHub releases (Linux and macOS, x86_64 and arm64).
#
#   curl -fsSL https://raw.githubusercontent.com/p4u/claudio/main/install.sh | bash
#
# Environment:
#   CLAUDIO_VERSION      release tag to install (default: latest)
#   CLAUDIO_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   GITHUB_TOKEN         token for a private repository (otherwise an
#                        authenticated `gh` CLI is used if present)
#
# Kept compatible with bash 3.2 (the macOS system bash).
set -euo pipefail

REPO="p4u/claudio"
VERSION="${CLAUDIO_VERSION:-latest}"
INSTALL_DIR="${CLAUDIO_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'claudio-install: %s\n' "$*"; }
die() { printf 'claudio-install: error: %s\n' "$*" >&2; exit 1; }

# ── platform ──────────────────────────────────────────────────────────────────
case "$(uname -s)" in
  Linux)  os=linux ;;
  Darwin) os=darwin ;;
  *)      die "unsupported OS: $(uname -s) (Linux and macOS only)" ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *)             die "unsupported architecture: $(uname -m)" ;;
esac
asset="claudio-${os}-${arch}"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# ── download ──────────────────────────────────────────────────────────────────
# fetch NAME DEST: download release asset NAME into DEST. Returns non-zero if
# the asset does not exist, so callers can fall back.
if [ -n "${GITHUB_TOKEN:-}" ]; then
  api="https://api.github.com/repos/$REPO/releases"
  if [ "$VERSION" = latest ]; then rel_url="$api/latest"; else rel_url="$api/tags/$VERSION"; fi
  curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$rel_url" -o "$tmp/release.json" \
    || die "cannot read release '$VERSION' of $REPO (check GITHUB_TOKEN)"
  fetch() {
    # Each asset object lists its API url before its name; avoid needing jq.
    local id
    id="$(tr -d '\n' < "$tmp/release.json" \
      | grep -o '"url": *"[^"]*/releases/assets/[0-9]*"[^}]*"name": *"'"$1"'"' \
      | grep -o 'assets/[0-9]*' | head -n1 | cut -d/ -f2)" || true
    [ -n "$id" ] || return 1
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/octet-stream" \
      "https://api.github.com/repos/$REPO/releases/assets/$id" -o "$2"
  }
elif command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  fetch() {
    if [ "$VERSION" = latest ]; then
      gh release download -R "$REPO" -p "$1" -O "$2" --clobber >/dev/null 2>&1
    else
      gh release download "$VERSION" -R "$REPO" -p "$1" -O "$2" --clobber >/dev/null 2>&1
    fi
  }
else
  if [ "$VERSION" = latest ]; then base="https://github.com/$REPO/releases/latest/download"
  else base="https://github.com/$REPO/releases/download/$VERSION"; fi
  fetch() { curl -fsSL "$base/$1" -o "$2"; }
fi

say "downloading $asset ($VERSION)"
if ! fetch "$asset" "$tmp/claudio"; then
  # Releases before per-platform assets shipped a single Linux x86_64 binary.
  [ "$asset" = claudio-linux-x86_64 ] || die "no $asset in release '$VERSION' of $REPO"
  asset=claudio
  fetch "$asset" "$tmp/claudio" || die "no claudio binary in release '$VERSION' of $REPO"
fi
fetch "$asset.sha256" "$tmp/claudio.sha256" || die "missing checksum $asset.sha256"

want="$(cut -d' ' -f1 < "$tmp/claudio.sha256")"
got="$(sha256 "$tmp/claudio")"
[ "$want" = "$got" ] || die "checksum mismatch for $asset (want $want, got $got)"
say "checksum ok"

# ── install ───────────────────────────────────────────────────────────────────
# Copy beside the target and rename: replacing a running binary in place fails
# with "Text file busy", while a rename leaves the running daemon untouched.
mkdir -p "$INSTALL_DIR"
chmod 755 "$tmp/claudio"
mv -f "$tmp/claudio" "$INSTALL_DIR/.claudio.new"
mv -f "$INSTALL_DIR/.claudio.new" "$INSTALL_DIR/claudio"
say "installed $INSTALL_DIR/claudio"

# A daemon from a previous install keeps running the old code: restart it. Its
# sessions come back with `claude --resume` when claudio reconnects. Only a
# binary with the session manager knows `daemon` (older ones pass every
# argument through to claude), so check that before running it.
runtime_dir="${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/claudio}"
runtime_dir="${runtime_dir:-${TMPDIR:-/tmp}/claudio-$(id -u)}"
if ls "${runtime_dir%/}"/daemon-v*.sock >/dev/null 2>&1 \
   && grep -qa '__probe' "$INSTALL_DIR/claudio"; then
  "$INSTALL_DIR/claudio" daemon restart || say "could not restart the daemon; run 'claudio daemon restart'"
fi

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "add $INSTALL_DIR to your PATH, e.g.: echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.profile" ;;
esac
command -v claude >/dev/null 2>&1 \
  || say "note: Claude Code (claude) was not found; install it from https://claude.com/claude-code"
say "done — run 'claudio' to open the session manager"
