#!/usr/bin/env bash
# Cloud VM dev stack: pull/build/restart + watch git for auto-redeploy.
#
# Usage:
#   ./scripts/cloud-dev-watch.sh          # deploy once, then watch origin/master
#   ./scripts/cloud-dev-watch.sh --once   # deploy once and exit
#
# Writes public phone URL to /tmp/synapse-public-url.txt when tunnels are up.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BRANCH="${SYNAPSE_GIT_BRANCH:-master}"
POLL_SECS="${SYNAPSE_POLL_SECS:-30}"
SERVER_PORT="${SYNAPSE_SERVER_PORT:-4173}"
WEB_PORT="${SYNAPSE_WEB_PORT:-8770}"
TOKEN="${SYNAPSE_TOKEN:-CODE}"
CWD="${SYNAPSE_CWD:-/tmp/synapse-demo}"
CLAUDE_BIN="${CLAUDE_BIN:-$HOME/.npm-global/bin/claude}"
LOG="${SYNAPSE_DEPLOY_LOG:-/tmp/synapse-deploy.log}"
URL_FILE="${SYNAPSE_PUBLIC_URL_FILE:-/tmp/synapse-public-url.txt}"

log() { echo "[$(date -Iseconds)] $*" | tee -a "$LOG"; }

ensure_tmux_session() {
  local name="$1"
  local dir="$2"
  if ! tmux -f /exec-daemon/tmux.portal.conf has-session -t "=$name" 2>/dev/null; then
    tmux -f /exec-daemon/tmux.portal.conf new-session -d -s "$name" -c "$dir" -- bash -l
  fi
}

restart_in_tmux() {
  local session="$1"
  local cmd="$2"
  ensure_tmux_session "$session" "$ROOT"
  tmux -f /exec-daemon/tmux.portal.conf send-keys -t "${session}:0.0" C-c 2>/dev/null || true
  sleep 1
  tmux -f /exec-daemon/tmux.portal.conf send-keys -t "${session}:0.0" "$cmd" C-m
}

build_stack() {
  local need_server=1 need_app=0
  if [[ -n "${1:-}" ]]; then
    need_server=0 need_app=0
    case "$1" in
      *crates/server/*) need_server=1 ;;
      *crates/app/*) need_app=1 ;;
      *Cargo.toml|*Cargo.lock) need_server=1 need_app=1 ;;
      *) need_server=1 need_app=1 ;;
    esac
  fi
  if (( need_server )); then
    log "cargo build -p synapse-server"
    cargo build -p synapse-server >>"$LOG" 2>&1
  fi
  if (( need_app )); then
    log "cargo build -p synapse-app (synapse-web binary)"
    cargo build -p synapse-app >>"$LOG" 2>&1
  fi
}

start_server() {
  mkdir -p "$CWD"
  restart_in_tmux synapse-server \
    "cd '$ROOT' && ./target/debug/synapse-server --port $SERVER_PORT --token '$TOKEN' --cwd '$CWD' --bin '$CLAUDE_BIN' --dev 2>&1 | tee /tmp/synapse-server.log"
  log "synapse-server on :$SERVER_PORT"
}

start_web_static() {
  # Serves crates/app/web from disk — git pull / local edits show on browser refresh (no rebuild).
  restart_in_tmux web-live \
    "cd '$ROOT' && python3 -m http.server $WEB_PORT --directory crates/app/web 2>&1 | tee -a '$LOG'"
  log "web static on :$WEB_PORT (refresh to pick up file changes)"
}

start_tunnel() {
  local name="$1" port="$2" logfile="$3"
  tmux -f /exec-daemon/tmux.portal.conf kill-session -t "=$name" 2>/dev/null || true
  sleep 1
  : >"$logfile"
  ensure_tmux_session "$name" "$HOME"
  tmux -f /exec-daemon/tmux.portal.conf send-keys -t "${name}:0.0" \
    "cloudflared tunnel --url http://127.0.0.1:${port} --no-autoupdate 2>&1 | tee '$logfile'" C-m
  sleep 12
  local url
  url=$(grep -aoE 'https://[a-z0-9-]+\.trycloudflare\.com' "$logfile" | tail -1 || true)
  log "tunnel $name -> 127.0.0.1:$port ($url)"
  printf '%s' "$url"
}

write_public_url() {
  local server_url web_url
  server_url=$(grep -aoE 'https://[a-z0-9-]+\.trycloudflare\.com' /tmp/cf-server.log 2>/dev/null | tail -1 || true)
  web_url=$(grep -aoE 'https://[a-z0-9-]+\.trycloudflare\.com' /tmp/cf-web.log 2>/dev/null | tail -1 || true)
  if [[ -n "$web_url" && -n "$server_url" ]]; then
    local host="${server_url#https://}"
    local phone="${web_url}/?host=${host}&port=443&token=${TOKEN}&tls=1"
    printf '%s\n' "$phone" >"$URL_FILE"
    log "phone URL -> $URL_FILE : $phone"
  else
    log "tunnels not ready yet (check /tmp/cf-*.log)"
  fi
}

deploy_once() {
  local change_hint="${1:-}"
  log "=== deploy start ==="
  build_stack "$change_hint"
  start_server
  start_web_static
  if command -v cloudflared >/dev/null 2>&1; then
    start_tunnel cf-server "$SERVER_PORT" /tmp/cf-server.log >/dev/null
    start_tunnel cf-web "$WEB_PORT" /tmp/cf-web.log >/dev/null
    write_public_url
  fi
  log "=== deploy done ==="
}

git_pull_if_needed() {
  git fetch origin "$BRANCH" >>"$LOG" 2>&1
  local local_rev remote_rev
  local_rev=$(git rev-parse HEAD)
  remote_rev=$(git rev-parse "origin/$BRANCH")
  if [[ "$local_rev" == "$remote_rev" ]]; then
    return 1
  fi
  log "new commits on origin/$BRANCH ($local_rev -> $remote_rev)"
  git pull --ff-only origin "$BRANCH" >>"$LOG" 2>&1
  git diff --name-only "$local_rev" "$remote_rev" | tr '\n' ' '
}

watch_loop() {
  log "watching origin/$BRANCH every ${POLL_SECS}s (Ctrl+C to stop)"
  local tick=0
  while true; do
    if changed=$(git_pull_if_needed); then
      log "pulled changes: $changed"
      deploy_once "$changed"
    elif (( tick % 10 == 0 )) && command -v cloudflared >/dev/null 2>&1; then
      # Quick tunnels drop silently; restart if public health returns 530.
      local sh wh
      sh=$(grep -aoE 'https://[a-z0-9-]+\.trycloudflare\.com' /tmp/cf-server.log 2>/dev/null | tail -1 || true)
      wh=$(grep -aoE 'https://[a-z0-9-]+\.trycloudflare\.com' /tmp/cf-web.log 2>/dev/null | tail -1 || true)
      if [[ -n "$sh" ]] && ! curl -sf -o /dev/null "${sh}/api/health" 2>/dev/null; then
        log "server tunnel unhealthy (530?) — restarting cf-server"
        start_tunnel cf-server "$SERVER_PORT" /tmp/cf-server.log >/dev/null
        write_public_url
      fi
      if [[ -n "$wh" ]] && ! curl -sf -o /dev/null "${wh}/" 2>/dev/null; then
        log "web tunnel unhealthy — restarting cf-web"
        start_tunnel cf-web "$WEB_PORT" /tmp/cf-web.log >/dev/null
        write_public_url
      fi
    fi
    tick=$((tick + 1))
    sleep "$POLL_SECS"
  done
}

main() {
  : >"$LOG"
  log "cloud-dev-watch root=$ROOT branch=$BRANCH"
  deploy_once
  if [[ "${1:-}" == "--once" ]]; then
    exit 0
  fi
  watch_loop
}

main "$@"
