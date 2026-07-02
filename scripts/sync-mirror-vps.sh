#!/usr/bin/env bash
# Run ON the relay VPS to refresh the public install mirror (outside /var/www/synapse).
set -euo pipefail

ROOT="${SYNAPSE_ROOT:-/opt/synapse}"
MIRROR="${SYNAPSE_MIRROR_ROOT:-/opt/synapse/mirror}"
VERSION="${SYNAPSE_VERSION:-v0.2.6}"

mkdir -p "${MIRROR}/install" "${MIRROR}/releases" "${MIRROR}/scripts"

if [[ -f "${ROOT}/scripts/install.sh" ]]; then
  cp "${ROOT}/scripts/install.sh" "${MIRROR}/install.sh"
  cp "${ROOT}/scripts/install-relay.sh" "${MIRROR}/install-relay.sh"
  cp "${ROOT}/scripts/synapse-server-wrapper.sh" "${MIRROR}/scripts/synapse-server-wrapper.sh"
  cp "${ROOT}/scripts/install.sh" "${MIRROR}/install/install.sh"
  cp "${ROOT}/scripts/install-relay.sh" "${MIRROR}/install/install-relay.sh"
  chmod 755 "${MIRROR}/install.sh" "${MIRROR}/install-relay.sh" "${MIRROR}/install/"*.sh
  chmod 755 "${MIRROR}/scripts/synapse-server-wrapper.sh"
fi

ver="${VERSION#v}"
for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin; do
  f="synapse-${ver}-${target}.tar.gz"
  if [[ "${SYNAPSE_FORCE_SYNC:-0}" = "1" ]] || [[ ! -f "${MIRROR}/releases/${f}" ]]; then
    echo "Downloading ${f}..."
    curl -fsSL -L "https://github.com/zhaoxini/synapse/releases/download/${VERSION}/${f}" \
      -o "${MIRROR}/releases/${f}" || echo "warning: could not fetch ${f}" >&2
  fi
done

echo "[$(date -Iseconds)] mirror synced to ${MIRROR}"
