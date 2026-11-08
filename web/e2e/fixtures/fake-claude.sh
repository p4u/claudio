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
mkdir -p "$FAKE_DIR/settings" "$FAKE_DIR/input" "$FAKE_DIR/env"
if [ "$FAKE_DUMP_ENV" = 1 ]; then
  env > "$FAKE_DIR/env/$name.txt.tmp" && mv "$FAKE_DIR/env/$name.txt.tmp" "$FAKE_DIR/env/$name.txt"
fi
printf '%s' "$2" > "$FAKE_DIR/settings/$name.json"
echo "FAKE_CLAUDE_BANNER nonce=$$"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$FAKE_DIR/input/$name.log"
  printf 'line:%s\n' "$line"
done
