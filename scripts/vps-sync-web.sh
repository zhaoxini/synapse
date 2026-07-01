#!/usr/bin/env bash
# Run ON the VPS after git pull (web-only sync; server binary via GitHub Actions).
set -euo pipefail
ROOT="${SYNAPSE_ROOT:-/opt/synapse}"
WEB_DST="${SYNAPSE_WEB_DST:-/var/www/synapse}"
if [[ ! -d "$ROOT/crates/app/web" ]]; then
  echo "missing $ROOT/crates/app/web" >&2
  exit 1
fi
rsync -a --delete "$ROOT/crates/app/web/" "$WEB_DST/"
echo "[$(date -Iseconds)] web synced to $WEB_DST"
