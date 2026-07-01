#!/usr/bin/env bash
# Launch synapse-server, stopping any previous instance on the listen port first.
set -euo pipefail

BIN_DIR="$(cd "$(dirname "$0")" && pwd)"
REAL="${BIN_DIR}/synapse-server.real"
PORT="${SYNAPSE_PORT:-4173}"

if [[ ! -x "${REAL}" ]]; then
  echo "error: missing ${REAL} — re-run the Synapse installer" >&2
  exit 1
fi

stop_stale() {
  command -v lsof >/dev/null 2>&1 || return 0
  local pid args
  for pid in $(lsof -nP -iTCP:"${PORT}" -sTCP:LISTEN -t 2>/dev/null || true); do
    args="$(ps -p "${pid}" -o args= 2>/dev/null || true)"
    if [[ "${args}" == *synapse-server* ]]; then
      echo "  Stopping previous synapse-server (pid ${pid})…"
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done
  sleep 0.4
  for pid in $(lsof -nP -iTCP:"${PORT}" -sTCP:LISTEN -t 2>/dev/null || true); do
    args="$(ps -p "${pid}" -o args= 2>/dev/null || true)"
    if [[ "${args}" == *synapse-server* ]]; then
      echo "  Force stopping synapse-server (pid ${pid})…"
      kill -9 "${pid}" 2>/dev/null || true
    fi
  done
  sleep 0.2
}

stop_stale
exec "${REAL}" "$@"
