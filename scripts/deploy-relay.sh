#!/usr/bin/env bash
# Upgrade synapse-relay binary on zx0623.duckdns.org (keeps DB + TLS certs).
# Run from repo root (needs SSH):
#   RELAY_SSH=root@192.3.179.202 ./scripts/deploy-relay.sh
#
# Or build locally first:
#   cargo build --release -p synapse-relay --target x86_64-unknown-linux-gnu
#   RELAY_SSH=root@host ./scripts/deploy-relay.sh

set -euo pipefail

RELAY_SSH="${RELAY_SSH:-root@192.3.179.202}"
RELAY_HOST="${RELAY_HOST:-zx0623.duckdns.org}"
INSTALL_PREFIX="${INSTALL_PREFIX:-/opt/synapse-relay}"
SERVICE_NAME="${SERVICE_NAME:-synapse-relay}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

info() { printf '==> %s\n' "$*"; }

pick_binary() {
  local candidates=(
    "${ROOT}/target/x86_64-unknown-linux-gnu/release/synapse-relay"
    "${ROOT}/target/release/synapse-relay"
  )
  local b
  for b in "${candidates[@]}"; do
    if [[ -x "$b" ]]; then
      echo "$b"
      return 0
    fi
  done
  return 1
}

if ! BIN="$(pick_binary)"; then
  info "No Linux synapse-relay binary — building x86_64-unknown-linux-gnu release…"
  cargo build --release -p synapse-relay --target x86_64-unknown-linux-gnu
  BIN="$(pick_binary)" || { echo "error: build failed" >&2; exit 1; }
fi

info "Deploy ${BIN} -> ${RELAY_SSH}:${INSTALL_PREFIX}/bin/synapse-relay"
scp "${BIN}" "${RELAY_SSH}:/tmp/synapse-relay.new"
scp "${ROOT}/scripts/vps-upgrade-relay.sh" "${RELAY_SSH}:/tmp/vps-upgrade-relay.sh"
ssh "${RELAY_SSH}" "chmod +x /tmp/vps-upgrade-relay.sh && bash /tmp/vps-upgrade-relay.sh /tmp/synapse-relay.new && rm -f /tmp/synapse-relay.new /tmp/vps-upgrade-relay.sh"

info "Verify health + exchange endpoint…"
curl -fsS "https://${RELAY_HOST}/api/health" >/dev/null || true
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  "https://${RELAY_HOST}/api/v1/pairing-codes/exchange" \
  -H 'Content-Type: application/json' \
  -d '{"code":"000000"}')"
if [[ "${code}" != "404" ]]; then
  echo "error: expected exchange invalid-code -> 404, got ${code}" >&2
  exit 1
fi
info "Done. exchange API live on https://${RELAY_HOST}"
