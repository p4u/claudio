#!/usr/bin/env bash
# scripts/screenshots.sh — generate release-repo/screenshots/*.png
#
# Usage:
#   bash scripts/screenshots.sh
#   bash scripts/screenshots.sh --out /tmp/my-shots
#
# Requirements: Python 3.8+, inkscape or ImageMagick magick in PATH.
# Re-runnable: previous PNGs are overwritten.
#
# The generator is scripts/gen_screenshots.py (pure Python, no pip deps).
# All content is hardcoded demo data; a privacy grep verifies no real
# user paths or hostnames appear in the rendered SVG source.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

OUT="${1:---out}"
if [[ "${OUT}" == "--out" ]]; then
  OUT_ARG="--out ${REPO_ROOT}/release-repo/screenshots"
else
  OUT_ARG="--out ${2:-${REPO_ROOT}/release-repo/screenshots}"
fi

python3 "${SCRIPT_DIR}/gen_screenshots.py" ${OUT_ARG}
