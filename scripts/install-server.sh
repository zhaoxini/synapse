#!/usr/bin/env bash
# Install synapse-server and synapse-relay from a release tarball into /usr/local/bin.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DEST="${INSTALL_DIR:-/usr/local/bin}"

install -m 755 "${ROOT}/bin/synapse-server" "${DEST}/synapse-server"
install -m 755 "${ROOT}/bin/synapse-relay" "${DEST}/synapse-relay"

echo "Installed:"
echo "  ${DEST}/synapse-server"
echo "  ${DEST}/synapse-relay"
echo ""
echo "Run: synapse-server"
