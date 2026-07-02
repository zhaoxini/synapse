#!/usr/bin/env bash
# Install synapse-server (and synapse-relay) from GitHub Releases.
#
# China-friendly (mirror on relay VPS):
#   curl -fsSL https://zx0623.duckdns.org/install.sh | bash
#
# Direct from GitHub Releases:
#   curl -fsSL https://github.com/zhaoxini/synapse/releases/latest/download/install.sh | bash
#
# Pin a version:
#   SYNAPSE_VERSION=v0.2.4 curl -fsSL https://zx0623.duckdns.org/install.sh | bash
#
# Auto-start after install:
#   SYNAPSE_AUTO_START=1 curl -fsSL https://zx0623.duckdns.org/install.sh | bash
#
# Force direct GitHub (skip mirror):
#   SYNAPSE_MIRROR= curl -fsSL https://zx0623.duckdns.org/install.sh | bash

set -euo pipefail

REPO="${SYNAPSE_REPO:-zhaoxini/synapse}"
VERSION="${SYNAPSE_VERSION:-latest}"
MIRROR="${SYNAPSE_MIRROR:-https://zx0623.duckdns.org}"
TMPDIR_INSTALL=""

info() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

gh_api_base() {
  if [ -n "${MIRROR}" ]; then
    printf '%s/ghapi' "${MIRROR}"
  else
    printf '%s' "https://api.github.com"
  fi
}

release_base() {
  if [ -n "${MIRROR}" ]; then
    printf '%s/ghrel' "${MIRROR}"
  else
    printf '%s' "https://github.com"
  fi
}

curl_get() {
  local url="$1"
  if curl -fsSL "${url}"; then
    return 0
  fi
  if [ -n "${MIRROR}" ] && [[ "${url}" == https://api.github.com/* ]]; then
    curl -fsSL "${MIRROR}/ghapi${url#https://api.github.com}"
    return $?
  fi
  if [ -n "${MIRROR}" ] && [[ "${url}" == https://github.com/* ]]; then
    curl -fsSL "${MIRROR}/ghrel${url#https://github.com}"
    return $?
  fi
  return 1
}

curl_get_file() {
  local url="$1" out="$2"
  local archive github_url try

  archive="${url##*/}"
  if [[ "${url}" == https://github.com/* ]]; then
    github_url="${url}"
  elif [[ "${url}" == *"/releases/download/"* ]]; then
    github_url="https://github.com/${REPO}/releases/download/${VERSION}/${archive}"
  else
    github_url="${url}"
  fi

  # 1) VPS static cache (fast when synced after each release)
  if [ -n "${MIRROR}" ]; then
    try="${MIRROR}/releases/${archive}"
    if curl -fsSL "${try}" -o "${out}" 2>/dev/null; then
      return 0
    fi
  fi

  # 2) GitHub Releases CDN (works even when ghrel proxy is down)
  if curl -fsSL -L "${github_url}" -o "${out}" 2>/dev/null; then
    return 0
  fi

  # 3) ghrel mirror (fallback when GitHub is blocked)
  if [ -n "${MIRROR}" ]; then
    try="${MIRROR}/ghrel/${REPO}/releases/download/${VERSION}/${archive}"
    if curl -fsSL "${try}" -o "${out}" 2>/dev/null; then
      return 0
    fi
  fi

  warn "download failed for ${archive} (tried: mirror /releases, GitHub, ghrel)"
  return 1
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
    Darwin) echo "${arch}-apple-darwin" ;;
    *) die "unsupported OS: ${os} (use Linux or macOS)" ;;
  esac
}

resolve_version() {
  if [ "${VERSION}" = "latest" ]; then
    VERSION="$(
      curl_get "$(gh_api_base)/repos/${REPO}/releases/latest" \
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

pick_install_dir() {
  if [ -n "${INSTALL_DIR:-}" ]; then
    mkdir -p "${INSTALL_DIR}"
    echo "${INSTALL_DIR}"
    return
  fi
  if [ -w "/usr/local/bin" ]; then
    echo "/usr/local/bin"
    return
  fi
  if [ -d "/usr/local/bin" ] && command -v sudo >/dev/null 2>&1; then
    echo "__sudo__/usr/local/bin"
    return
  fi
  echo "${HOME}/.local/bin"
}

install_bins() {
  local dest="$1" root="$2" use_sudo="" wrapper="${root}/synapse-server-wrapper.sh"
  if [ "${dest}" = "__sudo__/usr/local/bin" ]; then
    dest="/usr/local/bin"
    use_sudo="sudo"
  fi
  [ -f "${wrapper}" ] || die "broken release: missing synapse-server-wrapper.sh in tarball"
  head -1 "${wrapper}" | grep -q '^#!' \
    || die "broken release: synapse-server-wrapper.sh is not a shell script"
  mkdir -p "${dest}"
  ${use_sudo} install -m 755 "${root}/bin/synapse-server" "${dest}/synapse-server.real"
  ${use_sudo} install -m 755 "${root}/bin/synapse-relay" "${dest}/synapse-relay"
  ${use_sudo} install -m 755 "${wrapper}" "${dest}/synapse-server"
  head -1 "${dest}/synapse-server" | grep -q '^#!' \
    || die "install failed: ${dest}/synapse-server is not a shell script"
  echo "${dest}"
}

install_web_ui() {
  local root="$1"
  local web_root="${SYNAPSE_STATE_DIR:-$HOME/.synapse}/web"
  if [ ! -d "${root}/web" ]; then
    return 0
  fi
  mkdir -p "${web_root}"
  cp -R "${root}/web/." "${web_root}/"
  info "Web UI installed to ${web_root}"
}

main() {
  need_cmd curl
  need_cmd tar
  need_cmd uname
  need_cmd install

  local target ver_no archive url root dest
  target="$(detect_target)"
  resolve_version
  ver_no="${VERSION#v}"
  archive="synapse-${ver_no}-${target}.tar.gz"
  url="https://github.com/${REPO}/releases/download/${VERSION}/${archive}"

  info "Synapse installer"
  info "Release:  ${VERSION} (${target})"
  [ -n "${MIRROR}" ] && info "Mirror:   ${MIRROR}"
  info "URL:      ${url}"

  TMPDIR_INSTALL="$(mktemp -d)"

  curl_get_file "${url}" "${TMPDIR_INSTALL}/${archive}"
  tar -xzf "${TMPDIR_INSTALL}/${archive}" -C "${TMPDIR_INSTALL}"
  root="${TMPDIR_INSTALL}/synapse-${ver_no}-${target}"
  [ -d "${root}" ] || die "unexpected archive layout (missing ${root})"
  [ -x "${root}/bin/synapse-server" ] || die "synapse-server binary not found in archive"

  dest="$(install_bins "$(pick_install_dir)" "${root}")"
  install_web_ui "${root}"

  rm -rf "${TMPDIR_INSTALL}"
  TMPDIR_INSTALL=""

  info "Installed synapse-server and synapse-relay to ${dest}"
  if ! echo ":${PATH}:" | grep -q ":${dest}:"; then
    warn "${dest} is not on your PATH"
    echo "Add this to your shell profile:"
    echo "  export PATH=\"${dest}:\$PATH\""
  fi
  echo ""
  echo "Start server (background):  synapse-server"
  echo "Stop:                         synapse-server stop"
  echo "Pairing code:                 synapse-server pairing-code"
  echo ""

  if [ "${SYNAPSE_AUTO_START:-0}" = "1" ]; then
    info "SYNAPSE_AUTO_START=1 — starting server…"
    "${dest}/synapse-server" start || warn "auto-start failed; run: synapse-server start"
  fi
}

main "$@"
