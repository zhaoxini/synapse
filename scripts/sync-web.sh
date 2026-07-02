#!/usr/bin/env bash
# Copy crates/app/web → ~/.synapse/web (browser :8000 reads this).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${SYNAPSE_WEB_SRC:-$ROOT/crates/app/web}"
DEST="${SYNAPSE_WEB_DIR:-${SYNAPSE_STATE_DIR:-$HOME/.synapse}/web}"
[[ -f "$SRC/index.html" ]] || { echo "error: missing $SRC/index.html" >&2; exit 1; }
mkdir -p "$DEST"
rsync -a --delete "$SRC/" "$DEST/"
echo "==> Web UI synced: $SRC → $DEST"
