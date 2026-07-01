#!/usr/bin/env bash
# Local web UI verification before push. Builds Ionic bundle, serves dist/ + mock WS.
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

cd "$WEB"
if [[ ! -d node_modules ]]; then
  npm ci
fi
npm run build

if [[ -f "$PID_FILE" ]]; then
  kill "$(cat "$PID_FILE")" 2>/dev/null || true
  rm -f "$PID_FILE"
fi

echo "Starting Vite preview on :${PORT} (dist/)..."
npx vite preview --port "$PORT" --host 127.0.0.1 >/tmp/synapse-web-${PORT}.log 2>&1 &
echo $! > "$PID_FILE"
sleep 1.2

npx playwright install chromium >/dev/null 2>&1 || true

echo "=== verify-ui.mjs ==="
node verify-ui.mjs

echo "=== verify-native-shell.mjs ==="
node verify-native-shell.mjs

echo "=== capture-screenshot.mjs ==="
node capture-screenshot.mjs

echo "=== OK — web UI verified ==="
