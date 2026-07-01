#!/usr/bin/env bash
# Local web UI verification before push. Starts static server + mock WS, runs
# automated checks and optional screenshots.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WEB="$ROOT/crates/app/web"
PORT="${SYNAPSE_WEB_PORT:-8765}"
PID_FILE="/tmp/synapse-verify-web-${PORT}.pid"

cleanup() {
  if [[ -f "$PID_FILE" ]]; then
    kill "$(cat "$PID_FILE")" 2>/dev/null || true
    rm -f "$PID_FILE"
  fi
}
trap cleanup EXIT

if ! curl -sf "http://127.0.0.1:${PORT}/" >/dev/null 2>&1; then
  echo "Starting static server on :${PORT}..."
  python3 -m http.server "$PORT" --directory "$WEB" >/tmp/synapse-web-${PORT}.log 2>&1 &
  echo $! > "$PID_FILE"
  sleep 0.5
fi

cd "$WEB"
if [[ ! -d node_modules ]]; then
  npm ci
fi
npx playwright install chromium >/dev/null 2>&1 || true

echo "=== verify-ui.mjs ==="
node verify-ui.mjs

echo "=== verify-native-shell.mjs ==="
node verify-native-shell.mjs

echo "=== capture-screenshot.mjs ==="
node capture-screenshot.mjs

echo "=== OK — web UI verified ==="
