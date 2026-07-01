#!/usr/bin/env bash
# Sync install scripts + release cache to the public mirror on zx0623.duckdns.org.
# Run from repo root (needs SSH access to the relay VPS):
#   RELAY_SSH=root@192.3.179.202 ./scripts/deploy-mirror.sh

set -euo pipefail

RELAY_SSH="${RELAY_SSH:-root@192.3.179.202}"
RELAY_HOST="${RELAY_HOST:-zx0623.duckdns.org}"
VERSION="${SYNAPSE_VERSION:-v0.2.0}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

info() { printf '==> %s\n' "$*"; }

info "Upload install scripts to ${RELAY_SSH}..."
ssh "${RELAY_SSH}" 'mkdir -p /var/www/synapse/install /var/www/synapse/releases'
scp "${ROOT}/scripts/install.sh" "${ROOT}/scripts/install-relay.sh" "${RELAY_SSH}:/var/www/synapse/install/"
ssh "${RELAY_SSH}" 'cp /var/www/synapse/install/install.sh /var/www/synapse/install.sh
cp /var/www/synapse/install/install-relay.sh /var/www/synapse/install-relay.sh
chmod 755 /var/www/synapse/install.sh /var/www/synapse/install-relay.sh /var/www/synapse/install/*.sh'

info "Ensure release tarballs cached (${VERSION})..."
ssh "${RELAY_SSH}" "bash -s" <<REMOTE
set -euo pipefail
cd /var/www/synapse/releases
ver="${VERSION#v}"
for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin; do
  f="synapse-\${ver}-\${target}.tar.gz"
  if [ ! -f "\$f" ]; then
    curl -fsSL -L "https://github.com/zhaoxini/synapse/releases/download/${VERSION}/\$f" -o "\$f"
  fi
done
ls -lh
REMOTE

info "Verify mirror..."
curl -fsS "https://${RELAY_HOST}/install.sh" | head -3
curl -fsS -o /dev/null -w "releases: HTTP %{http_code}\n" \
  "https://${RELAY_HOST}/releases/synapse-${VERSION#v}-x86_64-unknown-linux-gnu.tar.gz"
info "Done. Users can run: curl -fsSL https://${RELAY_HOST}/install.sh | bash"
