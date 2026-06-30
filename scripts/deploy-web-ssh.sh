#!/usr/bin/env bash
# Rsync crates/app/web to a remote server for mobile preview.
#
# Usage:
#   DEPLOY_HOST=203.0.113.10 \
#   DEPLOY_USER=ubuntu \
#   DEPLOY_PATH=/var/www/synapse-preview \
#   ./scripts/deploy-web-ssh.sh
#
# Or with SSH alias:
#   DEPLOY_HOST=my-vps DEPLOY_USER=root DEPLOY_PATH=/var/www/synapse ./scripts/deploy-web-ssh.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${DEPLOY_HOST:?set DEPLOY_HOST}"
USER="${DEPLOY_USER:-ubuntu}"
PATH_ON_SERVER="${DEPLOY_PATH:?set DEPLOY_PATH}"
PORT="${DEPLOY_PORT:-22}"

DEST="${USER}@${HOST}:${PATH_ON_SERVER}/"
echo "==> Deploying web bundle to ${DEST}"
rsync -avz --delete -e "ssh -p ${PORT}" \
  "${ROOT}/crates/app/web/" \
  "${DEST}"

echo "==> Done."
echo "    Preview: http://${HOST}/  (configure nginx to serve ${PATH_ON_SERVER})"
