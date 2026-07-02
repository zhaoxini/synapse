#!/usr/bin/env bash
# One-time: install vps-auto-upgrade timer on the relay VPS.
#   ./scripts/setup-vps-autoupgrade.sh
set -euo pipefail

RELAY_SSH="${RELAY_SSH:-root@192.3.179.202}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

info() { printf '==> %s\n' "$*"; }

info "Upload auto-upgrade scripts to ${RELAY_SSH}..."
ssh "${RELAY_SSH}" "mkdir -p /opt/synapse/scripts"
scp "${ROOT}/scripts/vps-auto-upgrade.sh" \
  "${ROOT}/scripts/vps-upgrade-relay.sh" \
  "${ROOT}/scripts/sync-mirror-vps.sh" \
  "${RELAY_SSH}:/opt/synapse/scripts/"
ssh "${RELAY_SSH}" "chmod +x /opt/synapse/scripts/vps-auto-upgrade.sh /opt/synapse/scripts/vps-upgrade-relay.sh"

info "Install systemd timer (every 10 min)..."
ssh "${RELAY_SSH}" 'cat > /etc/systemd/system/synapse-auto-upgrade.service <<'"'"'EOF'"'"'
[Unit]
Description=Synapse relay + mirror auto-upgrade from GitHub Releases
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/opt/synapse/scripts/vps-auto-upgrade.sh
EOF
cat > /etc/systemd/system/synapse-auto-upgrade.timer <<'"'"'EOF'"'"'
[Unit]
Description=Poll GitHub Releases for Synapse upgrades

[Timer]
OnBootSec=2min
OnUnitActiveSec=10min
Persistent=true

[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now synapse-auto-upgrade.timer
systemctl list-timers synapse-auto-upgrade.timer --no-pager'

info "Seed deployed version and run once..."
ssh "${RELAY_SSH}" 'echo v0.2.6 > /opt/synapse-relay/.deployed-version; /opt/synapse/scripts/vps-auto-upgrade.sh || true'

info "Done. Timer: systemctl status synapse-auto-upgrade.timer"
