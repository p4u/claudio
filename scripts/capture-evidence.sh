#!/usr/bin/env bash
# Capture reproducible evidence for the disclosure report. Runs the wrapper
# against the real, locally-authenticated `claude` and saves each run's output.
#
# Usage:  ./scripts/capture-evidence.sh [output-dir]
set -u

OUT="${1:-../evidence}"
BIN="./target/release/claudio"
mkdir -p "$OUT"

echo "Building release binary..."
cargo build --release --quiet || { echo "build failed"; exit 1; }

run() {
  local name="$1"; shift
  echo "── $name ──"
  ( set -x; "$BIN" "$@" ) >"$OUT/$name.out" 2>"$OUT/$name.err"
  echo "exit=$? → $OUT/$name.{out,err}"
}

# 1. transparent passthrough — no -p, execs the real claude (native --version).
run transparent-version --version
# 2. text print mode
run text -p --dangerously-skip-permissions "Reply with exactly the word: TEXT_OK"
# 3. json (real usage object)
run json -p --output-format json --dangerously-skip-permissions "Reply with exactly the word: JSON_OK"
# 4. stream-json
run stream-json -p --output-format stream-json --dangerously-skip-permissions "Reply with exactly the word: STREAM_OK"
# 5. tool use (terminal-stop handling across a tool_use turn)
run tool-use -p --dangerously-skip-permissions "Use the Bash tool to run 'echo hello42', then tell me exactly what it printed."
# 6. variadic flag before the prompt (prompt must not be eaten by --allowedTools)
run variadic -p --output-format json --dangerously-skip-permissions --allowedTools Bash Read "Reply with exactly: VARIADIC_OK"
# 7. passthrough --model
run model-passthrough -p --dangerously-skip-permissions --model opus "Reply with exactly: MODEL_OK"
# 8. file hook transport (no loopback TCP)
CLAUDIO_HOOK_TRANSPORT=file run file-transport -p --dangerously-skip-permissions "Reply with exactly: FILE_OK"
# 9. debug timing trace (wrapper-only knob via env)
CLAUDIO_DEBUG=1 run debug-trace -p --dangerously-skip-permissions "Reply with exactly: DEBUG_OK"

echo
echo "Wrote evidence to $OUT/"
ls -la "$OUT"
