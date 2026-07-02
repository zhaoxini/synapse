#!/usr/bin/env bash
# Configure GitHub Actions deploy secrets (run as zhaoxini repo admin once).
# Enables release.yml + deploy-relay.yml to SSH-upgrade relay and sync mirror.
set -euo pipefail

REPO="${SYNAPSE_REPO:-zhaoxini/synapse}"
KEY="${RELAY_SSH_KEY_FILE:-${HOME}/.ssh/synapse_relay_deploy}"

if ! gh api "repos/${REPO}" --jq '.permissions.admin' 2>/dev/null | grep -q true; then
  echo "error: need repo admin on ${REPO} to set secrets (current gh account lacks permission)" >&2
  echo "  gh auth login   # as zhaoxini" >&2
  echo "  then re-run:    ./scripts/setup-github-deploy-secrets.sh" >&2
  exit 1
fi

[[ -f "${KEY}" ]] || { echo "error: missing ${KEY}" >&2; exit 1; }

gh secret set RELAY_SSH --repo "${REPO}" --body 'root@192.3.179.202'
gh secret set RELAY_SSH_KEY --repo "${REPO}" < "${KEY}"
echo "OK: RELAY_SSH + RELAY_SSH_KEY configured for ${REPO}"
