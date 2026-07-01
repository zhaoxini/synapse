#!/usr/bin/env bash
# Sync install scripts + release cache to the public mirror on zx0623.duckdns.org.
# Run from repo root (needs SSH access to the relay VPS):
#   RELAY_SSH=root@192.3.179.202 ./scripts/deploy-mirror.sh

set -euo pipefail

RELAY_SSH="${RELAY_SSH:-root@192.3.179.202}"
RELAY_HOST="${RELAY_HOST:-zx0623.duckdns.org}"
VERSION="${SYNAPSE_VERSION:-v0.2.1}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

info() { printf '==> %s\n' "$*"; }

info "Upload mirror scripts to ${RELAY_SSH}..."
ssh "${RELAY_SSH}" "mkdir -p /opt/synapse/scripts /opt/synapse/mirror"
scp "${ROOT}/scripts/install.sh" "${ROOT}/scripts/install-relay.sh" \
  "${ROOT}/scripts/sync-mirror-vps.sh" "${ROOT}/scripts/vps-sync-web.sh" \
  "${RELAY_SSH}:/opt/synapse/scripts/"
ssh "${RELAY_SSH}" 'chmod +x /opt/synapse/scripts/sync-mirror-vps.sh /opt/synapse/scripts/vps-sync-web.sh && SYNAPSE_ROOT=/opt/synapse bash /opt/synapse/scripts/sync-mirror-vps.sh'

info "Verify mirror..."
curl -fsS "https://${RELAY_HOST}/install.sh" | head -3
curl -fsS -o /dev/null -w "releases: HTTP %{http_code}\n" \
  "https://${RELAY_HOST}/releases/synapse-${VERSION#v}-x86_64-unknown-linux-gnu.tar.gz"
info "Done. Users: curl -fsSL https://${RELAY_HOST}/install.sh | bash"
