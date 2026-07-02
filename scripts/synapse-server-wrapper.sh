#!/usr/bin/env bash
# Front-end for installed synapse-server (synapse-server.real).
#
#   synapse-server          # start in background (default)
#   synapse-server stop
#   synapse-server pairing-code | status | login | …  → forwarded to .real
set -euo pipefail

BIN_DIR="$(cd "$(dirname "$0")" && pwd)"
REAL="${BIN_DIR}/synapse-server.real"
[[ -x "$REAL" ]] || REAL="$(command -v synapse-server.real 2>/dev/null || true)"
[[ -x "$REAL" ]] || { echo "error: missing synapse-server.real — re-run the installer" >&2; exit 1; }

STATE_DIR="${SYNAPSE_STATE_DIR:-$HOME/.synapse}"
PIDFILE="${SYNAPSE_PIDFILE:-$STATE_DIR/server.pid}"
WEB_PIDFILE="${SYNAPSE_WEB_PIDFILE:-$STATE_DIR/web.pid}"
LOGFILE="${SYNAPSE_LOG:-$STATE_DIR/server.log}"
PORT="${SYNAPSE_PORT:-4173}"
WEB_PORT="${SYNAPSE_WEB_PORT:-8000}"
WEB_DIR="${SYNAPSE_WEB_DIR:-$STATE_DIR/web}"
START_TIMEOUT_SEC="${SYNAPSE_START_TIMEOUT:-25}"

mkdir -p "$STATE_DIR"
[[ -f "${STATE_DIR}/env" ]] && source "${STATE_DIR}/env"

pid_alive() { kill -0 "$1" 2>/dev/null; }
port_open() { lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; }
web_port_open() { lsof -nP -iTCP:"$WEB_PORT" -sTCP:LISTEN >/dev/null 2>&1; }

# Keep ~/.synapse/web in sync so :8000 never serves a stale bundle after git pull.
sync_web_bundle() {
  local src="" repo_root
  if [[ -n "${SYNAPSE_WEB_SRC:-}" && -f "${SYNAPSE_WEB_SRC}/index.html" ]]; then
    src="${SYNAPSE_WEB_SRC}"
  elif repo_root="$(git -C "${BIN_DIR}" rev-parse --show-toplevel 2>/dev/null)" \
      && [[ -f "${repo_root}/crates/app/web/index.html" ]]; then
    src="${repo_root}/crates/app/web"
  elif [[ -f "${HOME}/code/synapse/crates/app/web/index.html" ]]; then
    src="${HOME}/code/synapse/crates/app/web"
  elif [[ -f "${BIN_DIR}/../share/synapse/web/index.html" ]]; then
    src="${BIN_DIR}/../share/synapse/web"
  fi
  [[ -n "$src" ]] || return 0
  mkdir -p "$WEB_DIR"
  if command -v rsync >/dev/null 2>&1; then
    rsync -a --delete "${src}/" "${WEB_DIR}/"
  else
    rm -rf "${WEB_DIR:?}"/*
    cp -R "${src}/." "${WEB_DIR}/"
  fi
}

resolve_web_dir() {
  if [[ -f "${WEB_DIR}/index.html" ]]; then
    printf '%s' "${WEB_DIR}"
    return 0
  fi
  local share="${BIN_DIR}/../share/synapse/web"
  if [[ -f "${share}/index.html" ]]; then
    printf '%s' "${share}"
    return 0
  fi
  return 1
}

start_web() {
  # synapse-server (>=0.2.7) serves the web UI itself on :8000 in account mode.
  if web_port_open; then
    return 0
  fi
  local dir pid
  if ! dir="$(resolve_web_dir)"; then
    echo "WARN  web UI files missing — reinstall synapse or set SYNAPSE_WEB_DIR" >&2
    return 0
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    echo "WARN  python3 not found — cannot serve web UI on :${WEB_PORT}" >&2
    return 0
  fi
  nohup python3 -m http.server "${WEB_PORT}" --bind 127.0.0.1 --directory "${dir}" >>"${LOGFILE}" 2>&1 &
  pid=$!
  echo "$pid" >"$WEB_PIDFILE"
  local deadline=$((SECONDS + 5))
  while (( SECONDS < deadline )); do
    if web_port_open; then
      return 0
    fi
    pid_alive "$pid" || break
    sleep 0.2
  done
  echo "WARN  web UI did not open port ${WEB_PORT}" >&2
}

stop_web() {
  local pid=""
  [[ -f "$WEB_PIDFILE" ]] && pid="$(cat "$WEB_PIDFILE" 2>/dev/null || true)"
  if [[ -n "$pid" ]] && pid_alive "$pid"; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 15); do pid_alive "$pid" || break; sleep 0.2; done
    pid_alive "$pid" && kill -9 "$pid" 2>/dev/null || true
  fi
  rm -f "$WEB_PIDFILE"
  if web_port_open && command -v lsof >/dev/null 2>&1; then
    for pid in $(lsof -nP -iTCP:"${WEB_PORT}" -sTCP:LISTEN -t 2>/dev/null || true); do
      kill "$pid" 2>/dev/null || true
    done
  fi
}

stop_stale() {
  command -v lsof >/dev/null 2>&1 || return 0
  local pid args
  for pid in $(lsof -nP -iTCP:"${PORT}" -sTCP:LISTEN -t 2>/dev/null || true); do
    args="$(ps -p "${pid}" -o args= 2>/dev/null || true)"
    if [[ "${args}" == *synapse-server* ]]; then
      kill -TERM "${pid}" 2>/dev/null || true
    fi
  done
  sleep 0.4
  for pid in $(lsof -nP -iTCP:"${PORT}" -sTCP:LISTEN -t 2>/dev/null || true); do
    args="$(ps -p "${pid}" -o args= 2>/dev/null || true)"
    if [[ "${args}" == *synapse-server* ]]; then
      kill -9 "${pid}" 2>/dev/null || true
    fi
  done
  sleep 0.2
}

pairing_code() {
  if [[ -f "${STATE_DIR}/pairing-code" ]]; then
    tr -d '[:space:]' <"${STATE_DIR}/pairing-code" | grep -Eo '[0-9]{6}' | head -1 && return
  fi
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
  sync_web_bundle
  local pid
  if [[ -f "$PIDFILE" ]]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && pid_alive "$pid" && port_open; then
      start_web
      print_ok "$pid"
      echo "    (already running — web bundle refreshed)"
      return 0
    fi
    rm -f "$PIDFILE"
  fi

  stop_stale
  rm -f "$PIDFILE"

  if port_open; then
    echo "FAIL  port $PORT still in use — run: synapse-server stop" >&2
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
      start_web
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
  stop_web
  [[ -f "$PIDFILE" ]] && pid="$(cat "$PIDFILE" 2>/dev/null || true)"
  if [[ -n "$pid" ]] && pid_alive "$pid"; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 25); do pid_alive "$pid" || break; sleep 0.2; done
    pid_alive "$pid" && kill -9 "$pid" 2>/dev/null || true
    echo "OK  stopped synapse-server (pid $pid)"
  else
    stop_stale
  fi
  rm -f "$PIDFILE"
  if port_open; then
    echo "WARN port $PORT still in use" >&2
    exit 1
  fi
  [[ -n "$pid" ]] || echo "OK  synapse-server was not running"
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
