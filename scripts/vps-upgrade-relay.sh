#!/usr/bin/env bash
# Upgrade synapse-relay binary in place (keeps DB + nginx TLS termination).
# Run on the VPS or via SSH. Args: path to new synapse-relay binary.
set -euo pipefail

BIN="${1:?usage: vps-upgrade-relay.sh /path/to/synapse-relay}"
INSTALL_PREFIX="${INSTALL_PREFIX:-/opt/synapse-relay}"
SERVICE_NAME="${SERVICE_NAME:-synapse-relay}"
RELAY_DOMAIN="${RELAY_DOMAIN:-zx0623.duckdns.org}"

test -x "${BIN}"

mkdir -p "${INSTALL_PREFIX}/bin"
install -m 755 "${BIN}" "${INSTALL_PREFIX}/bin/synapse-relay.new"

# nginx reverse proxy: relay listens on localhost:8080, public TLS at :443.
UNIT="/etc/systemd/system/${SERVICE_NAME}.service"
cat >"${UNIT}" <<EOF
[Unit]
Description=Synapse relay (account + WebSocket bridge)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
WorkingDirectory=${INSTALL_PREFIX}/data
ExecStart=${INSTALL_PREFIX}/bin/synapse-relay \\
  --host 127.0.0.1 \\
  --port 8080 \\
  --public-host ${RELAY_DOMAIN} \\
  --public-port 443 \\
  --public-tls \\
  --db ${INSTALL_PREFIX}/data/synapse-relay.db
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl stop "${SERVICE_NAME}" || true
install -m 755 "${INSTALL_PREFIX}/bin/synapse-relay.new" "${INSTALL_PREFIX}/bin/synapse-relay"
rm -f "${INSTALL_PREFIX}/bin/synapse-relay.new"
systemctl enable "${SERVICE_NAME}"
systemctl restart "${SERVICE_NAME}"
sleep 2
systemctl is-active "${SERVICE_NAME}"
