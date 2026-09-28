#!/usr/bin/env bash
# AgentKube installer for macOS and Linux.
#
# Installs `akctl`, `agentkube-api`, and `agentkube-operator` from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentkube/main/install.sh | bash
#   curl -fsSL .../install.sh | bash -s -- v0.1.0
#   curl -fsSL .../install.sh | bash -s -- --to ~/.local/bin
#   ./install.sh v0.1.0 --to /usr/local/bin --only akctl
#
# Env:
#   AGENTKUBE_VERSION   version to install (e.g. v0.1.0 or 0.1.0, or "latest")
#   AGENTKUBE_INSTALL_DIR install directory (overridden by --to)
#   GITHUB_TOKEN        optional, for higher API rate limits

set -euo pipefail

REPO="devdanielvaldez/agentkube"
BINARIES="akctl agentkube-api agentkube-operator"
VERSION="${AGENTKUBE_VERSION:-latest}"
INSTALL_DIR="${AGENTKUBE_INSTALL_DIR:-}"
ONLY="all"

usage() {
  cat <<'EOF'
Usage: install.sh [VERSION] [--to DIR] [--only akctl|agentkube-api|agentkube-operator] [--help]

  VERSION   e.g. v0.1.0, 0.1.0, or "latest" (default: latest)
  --to DIR  install directory (default: /usr/local/bin if writable, else ~/.local/bin)
  --only    install only one binary (default: both)
  --help    show this help
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --to) INSTALL_DIR="${2:?--to requires a directory}"; shift 2 ;;
    --to=*) INSTALL_DIR="${1#--to=}"; shift ;;
    --only) ONLY="${2:?--only requires akctl, agentkube-api, or agentkube-operator}"; shift 2 ;;
    --only=*) ONLY="${1#--only=}"; shift ;;
    -*) echo "install.sh: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) VERSION="$1"; shift ;;
  esac
done

case "$ONLY" in
  all|akctl|agentkube-api|agentkube-operator) ;;
  *) echo "install.sh: --only must be akctl, agentkube-api, or agentkube-operator, got: $ONLY" >&2; exit 2 ;;
esac

if [ "$ONLY" = "all" ]; then
  WANTED="$BINARIES"
else
  WANTED="$ONLY"
fi

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "install.sh: required command not found: $1" >&2; exit 1; }
}
need curl
need tar

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Darwin) OS_PART="apple-darwin" ;;
  Linux) OS_PART="unknown-linux-gnu" ;;
  *) echo "install.sh: unsupported OS: $OS (use install.ps1 on Windows)" >&2; exit 2 ;;
esac
case "$ARCH" in
  x86_64|amd64) ARCH_PART="x86_64" ;;
  arm64|aarch64) ARCH_PART="aarch64" ;;
  *) echo "install.sh: unsupported architecture: $ARCH" >&2; exit 2 ;;
esac
TARGET="${ARCH_PART}-${OS_PART}"

resolve_latest() {
  local api="https://api.github.com/repos/${REPO}/releases/latest"
  local json
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    json="$(curl -fsSL -H "Authorization: Bearer ${GITHUB_TOKEN}" "$api")"
  else
    json="$(curl -fsSL "$api")"
  fi
  # Minimal JSON parsing without jq: "tag_name": "v0.1.0"
  printf '%s' "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

if [ "$VERSION" = "latest" ]; then
  VERSION="$(resolve_latest)"
  [ -n "$VERSION" ] || { echo "install.sh: could not resolve latest release" >&2; exit 1; }
fi
# Normalize: allow "0.1.0" or "v0.1.0".
case "$VERSION" in
  v*) ;;
  *) VERSION="v$VERSION" ;;
esac

if [ -z "$INSTALL_DIR" ]; then
  if [ -w "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi

ASSET="agentkube-${VERSION}-${TARGET}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Installing AgentKube ${VERSION} (${TARGET}) to ${INSTALL_DIR} ..."
curl -fsSL --proto '=https' --tlsv1.2 \
  -o "${TMPDIR}/${ASSET}" \
  "${BASE_URL}/${ASSET}"

# Verify checksum when possible (non-fatal if tools are missing).
if curl -fsSL --proto '=https' --tlsv1.2 \
    -o "${TMPDIR}/${ASSET}.sha256" \
    "${BASE_URL}/${ASSET}.sha256" 2>/dev/null; then
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$TMPDIR" && sha256sum -c "${ASSET}.sha256")
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$TMPDIR" && shasum -a 256 -c "${ASSET}.sha256")
  else
    echo "install.sh: warning: no sha256 tool found, skipping checksum verification" >&2
  fi
else
  echo "install.sh: warning: checksum file not found, skipping verification" >&2
fi

tar -xzf "${TMPDIR}/${ASSET}" -C "$TMPDIR"
mkdir -p "$INSTALL_DIR"
for bin in $WANTED; do
  if [ ! -f "${TMPDIR}/${bin}" ]; then
    echo "install.sh: asset is missing expected binary: $bin" >&2; exit 1
  fi
  # Use sudo only when needed and available.
  if [ -w "$INSTALL_DIR" ]; then
    install -m 755 "${TMPDIR}/${bin}" "${INSTALL_DIR}/${bin}"
  elif command -v sudo >/dev/null 2>&1; then
    sudo install -m 755 "${TMPDIR}/${bin}" "${INSTALL_DIR}/${bin}"
  else
    echo "install.sh: ${INSTALL_DIR} is not writable and sudo is unavailable" >&2; exit 1
  fi
done

echo "Installed: $WANTED -> $INSTALL_DIR"
if ! command -v akctl >/dev/null 2>&1 && [ "$INSTALL_DIR" != "/usr/local/bin" ]; then
  echo "Add to PATH: export PATH=\"${INSTALL_DIR}:\$PATH\""
fi
if [ "$WANTED" != "${WANTED/akctl/}" ] || [ "$ONLY" = "all" ]; then
  if [ -x "${INSTALL_DIR}/akctl" ]; then
    "${INSTALL_DIR}/akctl" version || true
  fi
fi
