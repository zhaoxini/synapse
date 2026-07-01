#!/usr/bin/env bash
# Install synapse-relay on a Linux VPS with TLS (Let's Encrypt).
#
# One-liner (run as root on your VPS):
#   curl -fsSL https://github.com/zhaoxini/synapse/releases/latest/download/install-relay.sh | sudo bash
#
# Custom domain / email:
#   RELAY_DOMAIN=relay.example.com RELAY_EMAIL=you@example.com sudo bash install-relay.sh

set -euo pipefail

REPO="${SYNAPSE_REPO:-zhaoxini/synapse}"
VERSION="${SYNAPSE_VERSION:-latest}"
RELAY_DOMAIN="${RELAY_DOMAIN:-zx0623.duckdns.org}"
RELAY_PORT="${RELAY_PORT:-443}"
RELAY_EMAIL="${RELAY_EMAIL:-admin@${RELAY_DOMAIN}}"
INSTALL_PREFIX="${INSTALL_PREFIX:-/opt/synapse-relay}"
SERVICE_NAME="${SERVICE_NAME:-synapse-relay}"
TMPDIR_INSTALL=""

info() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    die "run as root: curl ... | sudo bash"
  fi
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${arch}" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) die "unsupported architecture: ${arch}" ;;
  esac
  case "${os}" in
    Linux) echo "${arch}-unknown-linux-gnu" ;;
    *) die "unsupported OS: ${os} (relay installer supports Linux only)" ;;
  esac
}

resolve_version() {
  if [ "${VERSION}" = "latest" ]; then
    VERSION="$(
      curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n1
    )"
    [ -n "${VERSION}" ] || die "could not resolve latest release for ${REPO}"
  fi
  case "${VERSION}" in
    v*) ;;
    *) VERSION="v${VERSION}" ;;
  esac
}

install_packages() {
  if command -v apt-get >/dev/null 2>&1; then
    info "Installing certbot (apt)..."
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq curl ca-certificates certbot >/dev/null
  elif command -v dnf >/dev/null 2>&1; then
    info "Installing certbot (dnf)..."
    dnf install -y curl ca-certificates certbot
  elif command -v yum >/dev/null 2>&1; then
    info "Installing certbot (yum)..."
    yum install -y curl ca-certificates certbot
  else
    need_cmd curl
    need_cmd certbot
  fi
}

stop_conflicting() {
  if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
    info "Stopping existing ${SERVICE_NAME}..."
    systemctl stop "${SERVICE_NAME}"
  fi
  for p in 80 "${RELAY_PORT}"; do
    if ss -tln "sport = :${p}" 2>/dev/null | grep -q LISTEN; then
      warn "port ${p} is in use — stopping listeners for certbot standalone"
      fuser -k "${p}/tcp" 2>/dev/null || true
    fi
  done
  sleep 1
}

obtain_cert() {
  local cert_dir="/etc/letsencrypt/live/${RELAY_DOMAIN}"
  if [ -f "${cert_dir}/fullchain.pem" ] && [ -f "${cert_dir}/privkey.pem" ]; then
    info "TLS certificate already exists for ${RELAY_DOMAIN}"
    return
  fi
  info "Requesting Let's Encrypt certificate for ${RELAY_DOMAIN}..."
  certbot certonly --standalone --non-interactive --agree-tos \
    -m "${RELAY_EMAIL}" -d "${RELAY_DOMAIN}"
  [ -f "${cert_dir}/fullchain.pem" ] || die "certbot did not create ${cert_dir}/fullchain.pem"
}

install_binary() {
  local target ver_no archive url root
  target="$(detect_target)"
  resolve_version
  ver_no="${VERSION#v}"
  archive="synapse-${ver_no}-${target}.tar.gz"
  url="https://github.com/${REPO}/releases/download/${VERSION}/${archive}"

  info "Downloading ${VERSION} (${target})..."
  TMPDIR_INSTALL="$(mktemp -d)"
  curl -fsSL "${url}" -o "${TMPDIR_INSTALL}/${archive}"
  tar -xzf "${TMPDIR_INSTALL}/${archive}" -C "${TMPDIR_INSTALL}"
  root="${TMPDIR_INSTALL}/synapse-${ver_no}-${target}"
  [ -x "${root}/bin/synapse-relay" ] || die "synapse-relay binary not found in archive"

  mkdir -p "${INSTALL_PREFIX}/bin" "${INSTALL_PREFIX}/data"
  install -m 755 "${root}/bin/synapse-relay" "${INSTALL_PREFIX}/bin/synapse-relay"
  rm -rf "${TMPDIR_INSTALL}"
  TMPDIR_INSTALL=""
}

write_systemd() {
  local cert="/etc/letsencrypt/live/${RELAY_DOMAIN}/fullchain.pem"
  local key="/etc/letsencrypt/live/${RELAY_DOMAIN}/privkey.pem"
  local unit="/etc/systemd/system/${SERVICE_NAME}.service"

  info "Creating systemd service ${SERVICE_NAME}..."
  cat >"${unit}" <<EOF
[Unit]
Description=Synapse relay (${RELAY_DOMAIN})
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
WorkingDirectory=${INSTALL_PREFIX}/data
ExecStart=${INSTALL_PREFIX}/bin/synapse-relay \\
  --host 0.0.0.0 \\
  --port ${RELAY_PORT} \\
  --public-host ${RELAY_DOMAIN} \\
  --tls-cert ${cert} \\
  --tls-key ${key} \\
  --db ${INSTALL_PREFIX}/data/synapse-relay.db
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

  systemctl daemon-reload
  systemctl enable "${SERVICE_NAME}"
  systemctl restart "${SERVICE_NAME}"
}

setup_renewal() {
  local hook="/etc/letsencrypt/renewal-hooks/deploy/${SERVICE_NAME}.sh"
  mkdir -p "$(dirname "${hook}")"
  cat >"${hook}" <<EOF
#!/bin/sh
systemctl reload ${SERVICE_NAME} 2>/dev/null || systemctl restart ${SERVICE_NAME}
EOF
  chmod +x "${hook}"
}

open_firewall() {
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
    info "Opening ports 80 and ${RELAY_PORT} in ufw..."
    ufw allow 80/tcp || true
    ufw allow "${RELAY_PORT}/tcp" || true
  fi
  if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
    info "Opening ports 80 and ${RELAY_PORT} in firewalld..."
    firewall-cmd --permanent --add-port=80/tcp || true
    firewall-cmd --permanent --add-port="${RELAY_PORT}/tcp" || true
    firewall-cmd --reload || true
  fi
}

verify() {
  sleep 2
  if systemctl is-active --quiet "${SERVICE_NAME}"; then
    info "${SERVICE_NAME} is running"
  else
    systemctl status "${SERVICE_NAME}" --no-pager || true
    die "${SERVICE_NAME} failed to start — check: journalctl -u ${SERVICE_NAME} -n 50"
  fi
  if curl -fsS "https://${RELAY_DOMAIN}/api/health" >/dev/null 2>&1; then
    info "Health check OK: https://${RELAY_DOMAIN}/api/health"
  else
    warn "health check failed — DNS may still be propagating or port ${RELAY_PORT} blocked"
  fi
}

main() {
  require_root
  need_cmd uname
  need_cmd systemctl

  info "Synapse relay installer"
  info "Domain:  ${RELAY_DOMAIN}"
  info "Port:    ${RELAY_PORT}"

  install_packages
  stop_conflicting
  obtain_cert
  install_binary
  write_systemd
  setup_renewal
  open_firewall
  verify

  echo ""
  echo "Relay ready: wss://${RELAY_DOMAIN}"
  echo "Users run:   synapse-server   (default relay is baked into release builds)"
  echo "Status:      systemctl status ${SERVICE_NAME}"
  echo "Logs:        journalctl -u ${SERVICE_NAME} -f"
}

main "$@"
