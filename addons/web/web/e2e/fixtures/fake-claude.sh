#!/bin/sh
# The e2e fake claude. FAKE_DIR is the scratch dir the server fixture made.
case "$1" in
  --version)
    if [ -e "$FAKE_DIR/updated" ]; then echo 'claude 0.0.1-fake'; else echo 'claude 0.0.0-fake'; fi
    exit 0 ;;
  --help)
    printf '  --agents <json>  Agents\n  --mcp-config <configs...>  MCP\n'
    exit 0 ;;
  update)
    sleep 1
    touch "$FAKE_DIR/updated"
    echo updated
    exit 0 ;;
esac
name=$(pwd -P | tr / _)
# Everything the fake writes is private (0600 files, 0700 dirs).
umask 077
mkdir -p "$FAKE_DIR/settings" "$FAKE_DIR/input" "$FAKE_DIR/env"
# Record only the env vars named in FAKE_ENV_KEYS (KEY=value when set), never
# the whole env: the server inherits the developer's env.
if [ -n "$FAKE_ENV_KEYS" ]; then
  {
    echo '# env'
    for k in $FAKE_ENV_KEYS; do
      case "$k" in ''|[0-9]*|*[!A-Za-z0-9_]*) continue ;; esac
      if eval "[ -n \"\${$k+x}\" ]"; then eval "printf '%s=%s\\n' \"\$k\" \"\$$k\""; fi
    done
  } > "$FAKE_DIR/env/$name.txt.tmp" && mv "$FAKE_DIR/env/$name.txt.tmp" "$FAKE_DIR/env/$name.txt"
fi
printf '%s' "$2" > "$FAKE_DIR/settings/$name.json"
echo "FAKE_CLAUDE_BANNER nonce=$$"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$FAKE_DIR/input/$name.log"
  printf 'line:%s\n' "$line"
done
