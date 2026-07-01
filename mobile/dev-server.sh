#!/usr/bin/env bash
# Dev synapse-server — background start with explicit success/failure.
# Matches iOS DEBUG / web URL: port 4173, code 071111.
#
# Usage:
#   ./mobile/dev-server.sh          # start (or no-op if already healthy)
#   ./mobile/dev-server.sh stop     # stop our background server
#   ./mobile/dev-server.sh status   # show pid / port / health
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CODE="${SYNAPSE_CODE:-071111}"
PORT="${SYNAPSE_PORT:-4173}"
HOST="${SYNAPSE_HOST:-127.0.0.1}"
BIN="${ROOT}/target/debug/synapse-server"
STATE_DIR="${SYNAPSE_STATE_DIR:-$HOME/.synapse}"
PIDFILE="${SYNAPSE_PIDFILE:-$STATE_DIR/dev-server.pid}"
LOGFILE="${SYNAPSE_LOG:-$STATE_DIR/dev-server.log}"
CURL_HOST="${SYNAPSE_CURL_HOST:-127.0.0.1}"
START_TIMEOUT_SEC="${SYNAPSE_START_TIMEOUT:-15}"

mkdir -p "$STATE_DIR"

log_tail() {
  if [[ -f "$LOGFILE" ]]; then
    echo "---- last 20 lines of $LOGFILE ----" >&2
    tail -n 20 "$LOGFILE" >&2 || true
    echo "-----------------------------------" >&2
  fi
}

pid_alive() {
  local pid="$1"
  kill -0 "$pid" 2>/dev/null
}

port_listener_pid() {
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null | head -1
}

verify_server() {
  curl -sf --max-time 2 "http://${CURL_HOST}:${PORT}/api/health" >/dev/null \
    && curl -sf --max-time 2 "http://${CURL_HOST}:${PORT}/api/pair?code=${CODE}" >/dev/null
}

print_ok() {
  local pid="$1"
  echo "OK  synapse-server running"
  echo "    pid:  $pid"
  echo "    addr: ${HOST}:${PORT}"
  echo "    code: ${CODE}"
  echo "    log:  ${LOGFILE}"
  echo "    web:  http://127.0.0.1:8000/?host=${CURL_HOST}&port=${PORT}&code=${CODE}"
}

fail() {
  echo "FAIL  $*" >&2
  log_tail
  exit 1
}

ensure_binary() {
  if [[ ! -x "$BIN" ]]; then
    echo "Building synapse-server…" >&2
    cargo build -p synapse-server --manifest-path "$ROOT/Cargo.toml"
  fi
}

stop_server() {
  local pid=""
  if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
  fi
  if [[ -n "$pid" ]] && pid_alive "$pid"; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do
      pid_alive "$pid" || break
      sleep 0.2
    done
    if pid_alive "$pid"; then
      kill -9 "$pid" 2>/dev/null || true
    fi
    echo "OK  stopped synapse-server (pid $pid)"
  else
    local other
    other="$(port_listener_pid || true)"
    if [[ -n "$other" ]]; then
      echo "WARN port $PORT still held by pid $other (not our pidfile)" >&2
      exit 1
    fi
    echo "OK  synapse-server was not running"
  fi
  rm -f "$PIDFILE"
}

status_server() {
  local pid listener
  pid=""
  if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
  fi
  listener="$(port_listener_pid || true)"
  if [[ -n "$pid" ]] && pid_alive "$pid"; then
    if verify_server; then
      print_ok "$pid"
      exit 0
    fi
    echo "WARN pid $pid alive but health/pair check failed" >&2
    log_tail
    exit 1
  fi
  if [[ -n "$listener" ]]; then
    echo "WARN port $PORT in use by pid $listener (no healthy pidfile entry)" >&2
    exit 1
  fi
  echo "STOPPED  synapse-server not running on port $PORT"
  exit 1
}

start_server() {
  local pid listener
  if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && pid_alive "$pid" && verify_server; then
      print_ok "$pid"
      echo "    (already running)"
      exit 0
    fi
    rm -f "$PIDFILE"
  fi

  listener="$(port_listener_pid || true)"
  if [[ -n "$listener" ]]; then
    fail "port $PORT already in use by pid $listener — run: $0 stop  (or kill that process)"
  fi

  ensure_binary

  echo "Starting $BIN on ${HOST}:${PORT}  code=${CODE} …" >&2
  : >"$LOGFILE"
  nohup "$BIN" --port "$PORT" --code "$CODE" --host "$HOST" >>"$LOGFILE" 2>&1 &
  pid=$!
  echo "$pid" >"$PIDFILE"

  local deadline=$((SECONDS + START_TIMEOUT_SEC))
  while (( SECONDS < deadline )); do
    if ! pid_alive "$pid"; then
      rm -f "$PIDFILE"
      fail "synapse-server exited during startup (pid $pid)"
    fi
    if verify_server; then
      print_ok "$pid"
      exit 0
    fi
    sleep 0.3
  done

  kill "$pid" 2>/dev/null || true
  rm -f "$PIDFILE"
  fail "synapse-server did not become ready on ${CURL_HOST}:${PORT} within ${START_TIMEOUT_SEC}s"
}

case "${1:-start}" in
  start|"") start_server ;;
  stop) stop_server ;;
  status) status_server ;;
  -h|--help)
    echo "Usage: $0 [start|stop|status]" >&2
    exit 0
    ;;
  *)
    echo "Unknown command: $1 (try start|stop|status)" >&2
    exit 1
    ;;
esac
