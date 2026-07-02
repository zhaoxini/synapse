#!/usr/bin/env bash
# Run on zx0623.duckdns.org VPS: poll GitHub Releases and upgrade relay + install mirror.
# Installed by scripts/setup-vps-autoupgrade.sh (systemd timer, every 10 min).
set -euo pipefail

REPO="${SYNAPSE_REPO:-zhaoxini/synapse}"
ROOT="${SYNAPSE_ROOT:-/opt/synapse}"
MIRROR_ROOT="${SYNAPSE_MIRROR_ROOT:-/opt/synapse/mirror}"
INSTALL_PREFIX="${INSTALL_PREFIX:-/opt/synapse-relay}"
STATE_FILE="${INSTALL_PREFIX}/.deployed-version"
LOG_TAG="synapse-auto-upgrade"

info() { logger -t "${LOG_TAG}" "$*"; printf '[%s] %s\n' "$(date -Iseconds)" "$*"; }

latest_release_tag() {
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n1
}

fetch_release_scripts() {
  local version="$1"
  mkdir -p "${ROOT}/scripts"
  curl -fsSL -L "https://github.com/${REPO}/releases/download/${version}/install.sh" \
    -o "${ROOT}/scripts/install.sh"
  curl -fsSL -L "https://github.com/${REPO}/releases/download/${version}/install-relay.sh" \
    -o "${ROOT}/scripts/install-relay.sh"
  chmod 755 "${ROOT}/scripts/install.sh" "${ROOT}/scripts/install-relay.sh"
}

upgrade_relay_from_release() {
  local version="$1"
  local ver_no="${version#v}"
  local archive="synapse-${ver_no}-x86_64-unknown-linux-gnu.tar.gz"
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' RETURN

  info "Downloading ${version} (${archive})..."
  curl -fsSL -L "https://github.com/${REPO}/releases/download/${version}/${archive}" \
    -o "${tmp}/${archive}"
  tar -xzf "${tmp}/${archive}" -C "${tmp}"
  local bin="${tmp}/synapse-${ver_no}-x86_64-unknown-linux-gnu/bin/synapse-relay"
  test -x "${bin}"

  # Extract wrapper for mirror (bundled in server tarball).
  local wrapper="${tmp}/synapse-${ver_no}-x86_64-unknown-linux-gnu/synapse-server-wrapper.sh"
  if [[ -f "${wrapper}" ]]; then
    install -m 755 "${wrapper}" "${ROOT}/scripts/synapse-server-wrapper.sh"
  fi

  bash "${ROOT}/scripts/vps-upgrade-relay.sh" "${bin}"
}

sync_mirror() {
  local version="$1"
  if [[ -x "${ROOT}/scripts/sync-mirror-vps.sh" ]]; then
    SYNAPSE_ROOT="${ROOT}" SYNAPSE_VERSION="${version}" SYNAPSE_FORCE_SYNC=1 \
      bash "${ROOT}/scripts/sync-mirror-vps.sh"
  fi
}

verify_exchange_api() {
  local body
  body="$(curl -fsSL -X POST "https://zx0623.duckdns.org/api/v1/pairing-codes/exchange" \
    -H 'Content-Type: application/json' \
    -d '{"code":"000000"}' 2>/dev/null || true)"
  if echo "${body}" | grep -q 'invalid or expired pairing code'; then
    info "exchange API OK"
    return 0
  fi
  info "warning: exchange API check failed: ${body:-empty}"
  return 1
}

main() {
  local latest current
  latest="$(latest_release_tag)"
  [[ -n "${latest}" ]] || { info "error: could not resolve latest release"; exit 1; }

  current="$(cat "${STATE_FILE}" 2>/dev/null || echo "")"
  if [[ "${latest}" == "${current}" ]]; then
    exit 0
  fi

  info "Upgrading ${current:-none} -> ${latest}"
  fetch_release_scripts "${latest}"
  upgrade_relay_from_release "${latest}"
  sync_mirror "${latest}"
  echo "${latest}" > "${STATE_FILE}"
  verify_exchange_api || true
  info "Upgrade complete: ${latest}"
}

main "$@"
