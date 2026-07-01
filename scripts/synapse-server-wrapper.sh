#!/usr/bin/env bash
# Front-end for installed synapse-server (placed at bin/synapse-server by install.sh).
# Real binary: synapse-server.real
#
#   synapse-server          # start in background (default)
#   synapse-server start
#   synapse-server stop
#   synapse-server pairing-code | status | login | …  → forwarded to .real
set -euo pipefail

REAL="$(cd "$(dirname "$0")" && pwd)/synapse-server.real"
[[ -x "$REAL" ]] || REAL="$(command -v synapse-server.real 2>/dev/null || true)"
[[ -x "$REAL" ]] || { echo "error: synapse-server.real not found" >&2; exit 1; }

STATE_DIR="${SYNAPSE_STATE_DIR:-$HOME/.synapse}"
PIDFILE="${SYNAPSE_PIDFILE:-$STATE_DIR/server.pid}"
LOGFILE="${SYNAPSE_LOG:-$STATE_DIR/server.log}"
PORT="${SYNAPSE_PORT:-4173}"
START_TIMEOUT_SEC="${SYNAPSE_START_TIMEOUT:-25}"

mkdir -p "$STATE_DIR"

pid_alive() { kill -0 "$1" 2>/dev/null; }
port_open() { lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; }

pairing_code() {
  "$REAL" pairing-code 2>/dev/null | grep -Eo '[0-9]{6}' | head -1 || true
}

relay_line() {
  "$REAL" status 2>/dev/null | grep -E 'Relay:|Device:' | tr '\n' ' ' || true
}

listen_addr() {
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN 2>/dev/null | awk 'NR==2 {print $9}' | head -1
}

log_tail() {
  [[ -f "$LOGFILE" ]] || return 0
  echo "---- last 15 lines of $LOGFILE ----" >&2
  sed 's/\x1b\[[0-9;]*m//g' "$LOGFILE" 2>/dev/null | tail -15 >&2 || true
  echo "-----------------------------------" >&2
}

print_ok() {
  local pid="$1" code listen relay
  code="$(pairing_code)"
  listen="$(listen_addr || echo "127.0.0.1:${PORT}")"
  relay="$(relay_line)"
  echo "OK  synapse-server running (background)"
  echo "    pid:    $pid"
  echo "    listen: ${listen}  (local bridge for relay)"
  [[ -n "$relay" ]] && echo "    ${relay}"
  [[ -n "$code" ]] && echo "    code:   ${code}"
  [[ -n "$code" ]] && echo "    web:    http://127.0.0.1:8000/?code=${code}"
  echo "    log:    ${LOGFILE}"
  echo ""
  echo "Stop: synapse-server stop"
}

do_start() {
  local pid
  if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && pid_alive "$pid" && port_open; then
      print_ok "$pid"
      echo "    (already running)"
      return 0
    fi
    rm -f "$PIDFILE"
  fi
  if port_open; then
    echo "FAIL  port $PORT already in use — run: synapse-server stop" >&2
    exit 1
  fi

  echo "Starting synapse-server in background …" >&2
  : >"$LOGFILE"
  nohup "$REAL" >>"$LOGFILE" 2>&1 &
  pid=$!
  echo "$pid" >"$PIDFILE"

  local deadline=$((SECONDS + START_TIMEOUT_SEC))
  while (( SECONDS < deadline )); do
    if ! pid_alive "$pid"; then
      rm -f "$PIDFILE"
      echo "FAIL  synapse-server exited during startup" >&2
      log_tail
      exit 1
    fi
    if port_open; then
      sleep 1
      print_ok "$pid"
      return 0
    fi
    sleep 0.3
  done

  kill "$pid" 2>/dev/null || true
  rm -f "$PIDFILE"
  echo "FAIL  did not open port $PORT within ${START_TIMEOUT_SEC}s" >&2
  log_tail
  exit 1
}

do_stop() {
  local pid=""
  [[ -f "$PIDFILE" ]] && pid="$(cat "$PIDFILE" 2>/dev/null || true)"
  if [[ -n "$pid" ]] && pid_alive "$pid"; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 25); do pid_alive "$pid" || break; sleep 0.2; done
    pid_alive "$pid" && kill -9 "$pid" 2>/dev/null || true
    echo "OK  stopped synapse-server (pid $pid)"
  elif port_open; then
    echo "WARN port $PORT still in use" >&2
    exit 1
  else
    echo "OK  synapse-server was not running"
  fi
  rm -f "$PIDFILE"
}

case "${1:-start}" in
  start|"") do_start ;;
  stop) do_stop ;;
  -h|--help)
    cat <<EOF
Usage: synapse-server [start|stop|COMMAND…]

  start       run in background (default when no args)
  stop        stop background server
  pairing-code, status, login, …  → synapse-server.real
EOF
    ;;
  *)
    exec "$REAL" "$@"
    ;;
esac
