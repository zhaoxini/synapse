#!/usr/bin/env bash
# One-shot bootstrap for Synapse on a VPS (fixed domain, e.g. DuckDNS).
#
# Run ON THE VPS as root (after SSH login):
#   export DEEPSEEK_API_KEY='sk-...'
#   export SYNAPSE_DOMAIN='zx0623.duckdns.org'   # optional
#   export SYNAPSE_TOKEN='CODE'                  # optional
#   bash scripts/bootstrap-vps.sh
#
# Requires: git clone of this repo, or run from repo root.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DOMAIN="${SYNAPSE_DOMAIN:-zx0623.duckdns.org}"
TOKEN="${SYNAPSE_TOKEN:-CODE}"
WORKSPACE="${SYNAPSE_CWD:-/var/synapse/workspace}"
DEEPSEEK_KEY="${DEEPSEEK_API_KEY:?export DEEPSEEK_API_KEY first}"
CLAUDE_HOME="${CLAUDE_HOME:-/root/.npm-global}"

log() { echo "[bootstrap] $*"; }

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root on the VPS" >&2
  exit 1
fi

log "installing docker (if missing)…"
if ! command -v docker >/dev/null 2>&1; then
  curl -fsSL https://get.docker.com | sh
  systemctl enable --now docker
fi

log "installing node + claude CLI (if missing)…"
if ! command -v node >/dev/null 2>&1; then
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
  apt-get install -y nodejs
fi
export npm_config_prefix="$CLAUDE_HOME"
if ! command -v "$CLAUDE_HOME/bin/claude" >/dev/null 2>&1; then
  npm install -g @anthropic-ai/claude-code
fi

log "writing DeepSeek config for Claude Code…"
mkdir -p /root/.claude
cat >/root/.claude/settings.json <<EOF
{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
    "ANTHROPIC_AUTH_TOKEN": "${DEEPSEEK_KEY}",
    "ANTHROPIC_API_KEY": "${DEEPSEEK_KEY}",
    "ANTHROPIC_MODEL": "deepseek-v4-flash",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-flash",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-flash",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "deepseek-v4-flash",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
  }
}
EOF
chmod 600 /root/.claude/settings.json

log "workspace ${WORKSPACE}"
mkdir -p "$WORKSPACE"

log "opening firewall 80/443 (if ufw active)…"
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q active; then
  ufw allow 80/tcp
  ufw allow 443/tcp
fi

export HOME=/root
export SYNAPSE_TOKEN="$TOKEN"
export SYNAPSE_CWD="$WORKSPACE"
export SITE_ADDRESS="$DOMAIN"
export CLAUDE_BIN_HOST="$CLAUDE_HOME/bin/claude"

log "building and starting stack (Caddy + synapse-server)…"
docker compose -f deploy/docker-compose.yml -f deploy/docker-compose.vps.yml up -d --build

PHONE="https://${DOMAIN}/?host=${DOMAIN}&port=443&token=${TOKEN}&tls=1"
log "done."
echo ""
echo "Fixed phone URL:"
echo "  ${PHONE}"
echo ""
echo "Health: curl -sf https://${DOMAIN}/api/health"
echo "Logs:   docker compose -f deploy/docker-compose.yml -f deploy/docker-compose.vps.yml logs -f"
