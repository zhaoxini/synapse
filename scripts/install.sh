#!/usr/bin/env bash
# Install synapse-server (and synapse-relay) from GitHub Releases.
#
# One-liner:
#   curl -fsSL https://github.com/zhaoxini/synapse/releases/latest/download/install.sh | bash
#
# Pin a version:
#   SYNAPSE_VERSION=v0.2.0 curl -fsSL ... | bash

set -euo pipefail

REPO="${SYNAPSE_REPO:-zhaoxini/synapse}"
VERSION="${SYNAPSE_VERSION:-latest}"
TMPDIR_INSTALL=""

info() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
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
  local dest="$1" root="$2" use_sudo=""
  if [ "${dest}" = "__sudo__/usr/local/bin" ]; then
    dest="/usr/local/bin"
    use_sudo="sudo"
  fi
  mkdir -p "${dest}"
  ${use_sudo} install -m 755 "${root}/bin/synapse-server" "${dest}/synapse-server"
  ${use_sudo} install -m 755 "${root}/bin/synapse-relay" "${dest}/synapse-relay"
  echo "${dest}"
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
  info "URL:      ${url}"

  TMPDIR_INSTALL="$(mktemp -d)"

  curl -fsSL "${url}" -o "${TMPDIR_INSTALL}/${archive}"
  tar -xzf "${TMPDIR_INSTALL}/${archive}" -C "${TMPDIR_INSTALL}"
  root="${TMPDIR_INSTALL}/synapse-${ver_no}-${target}"
  [ -d "${root}" ] || die "unexpected archive layout (missing ${root})"
  [ -x "${root}/bin/synapse-server" ] || die "synapse-server binary not found in archive"

  dest="$(install_bins "$(pick_install_dir)" "${root}")"

  rm -rf "${TMPDIR_INSTALL}"
  TMPDIR_INSTALL=""

  info "Installed synapse-server and synapse-relay to ${dest}"
  if ! echo ":${PATH}:" | grep -q ":${dest}:"; then
    warn "${dest} is not on your PATH"
    echo "Add this to your shell profile:"
    echo "  export PATH=\"${dest}:\$PATH\""
  fi
  echo ""
  echo "Run: synapse-server"
}

main "$@"
