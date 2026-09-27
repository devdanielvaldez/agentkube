#!/usr/bin/env bash
# Regenerates Formula/agentkube.rb for a release tag.
#
# Usage:
#   ./scripts/bump-formula.sh v0.1.0
#   ./scripts/bump-formula.sh v0.1.0 --checksums-dir ./dist
#   ./scripts/bump-formula.sh v0.1.0 --formula Formula/agentkube.rb
#
# By default the per-asset .sha256 files are downloaded from the GitHub
# Release matching the tag. With --checksums-dir they are read from a local
# directory instead (useful for testing: it must contain
# agentkube-<tag>-<target>.tar.gz.sha256 for the four unix targets).

set -euo pipefail

REPO="devdanielvaldez/agentkube"
TAG=""
CHECKSUMS_DIR=""
FORMULA="Formula/agentkube.rb"

usage() {
  cat <<'EOF'
Usage: bump-formula.sh vX.Y.Z [--checksums-dir DIR] [--formula PATH] [--help]
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --checksums-dir) CHECKSUMS_DIR="${2:?--checksums-dir requires a directory}"; shift 2 ;;
    --checksums-dir=*) CHECKSUMS_DIR="${1#--checksums-dir=}"; shift ;;
    --formula) FORMULA="${2:?--formula requires a path}"; shift 2 ;;
    --formula=*) FORMULA="${1#--formula=}"; shift ;;
    -*) echo "bump-formula.sh: unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) TAG="$1"; shift ;;
  esac
done

[ -n "$TAG" ] || { echo "bump-formula.sh: missing tag (e.g. v0.1.0)" >&2; usage >&2; exit 2; }
case "$TAG" in
  v*) BARE="${TAG#v}" ;;
  *) echo "bump-formula.sh: tag must start with v (got $TAG)" >&2; exit 2 ;;
esac

TARGETS=(
  "aarch64-apple-darwin"
  "x86_64-apple-darwin"
  "aarch64-unknown-linux-gnu"
  "x86_64-unknown-linux-gnu"
)

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT
if [ -z "$CHECKSUMS_DIR" ]; then
  CHECKSUMS_DIR="$TMPDIR"
  for target in "${TARGETS[@]}"; do
    asset="agentkube-${TAG}-${target}.tar.gz"
    curl -fsSL --proto '=https' --tlsv1.2 \
      -o "${CHECKSUMS_DIR}/${asset}.sha256" \
      "https://github.com/${REPO}/releases/download/${TAG}/${asset}.sha256"
  done
fi

sha_for() {
  case "$1" in
    aarch64-apple-darwin) printf '%s' "$SHA_AARCH64_DARWIN" ;;
    x86_64-apple-darwin) printf '%s' "$SHA_X86_64_DARWIN" ;;
    aarch64-unknown-linux-gnu) printf '%s' "$SHA_AARCH64_LINUX" ;;
    x86_64-unknown-linux-gnu) printf '%s' "$SHA_X86_64_LINUX" ;;
  esac
}

for target in "${TARGETS[@]}"; do
  asset="agentkube-${TAG}-${target}.tar.gz"
  file="${CHECKSUMS_DIR}/${asset}.sha256"
  [ -f "$file" ] || { echo "bump-formula.sh: missing checksum file: $file" >&2; exit 1; }
  # Format: "<sha256>  <filename>"
  sha="$(awk '{print $1}' "$file")"
  case "$sha" in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f])
      ;;
    *) echo "bump-formula.sh: invalid sha256 in $file: $sha" >&2; exit 1 ;;
  esac
  case "$target" in
    aarch64-apple-darwin) SHA_AARCH64_DARWIN="$sha" ;;
    x86_64-apple-darwin) SHA_X86_64_DARWIN="$sha" ;;
    aarch64-unknown-linux-gnu) SHA_AARCH64_LINUX="$sha" ;;
    x86_64-unknown-linux-gnu) SHA_X86_64_LINUX="$sha" ;;
  esac
done

mkdir -p "$(dirname "$FORMULA")"
cat > "$FORMULA" <<EOF
# Documentation: https://docs.brew.sh/Formula-Cookbook
# This file is updated automatically by scripts/bump-formula.sh on every
# release (see .github/workflows/release.yml). Do not edit version/sha256
# by hand; run \`./scripts/bump-formula.sh vX.Y.Z\` instead.
class Agentkube < Formula
  desc "Orchestration layer for autonomous AI agents"
  homepage "https://github.com/${REPO}"
  version "${BARE}"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/${REPO}/releases/download/${TAG}/agentkube-${TAG}-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha_for aarch64-apple-darwin)"
    end
    on_intel do
      url "https://github.com/${REPO}/releases/download/${TAG}/agentkube-${TAG}-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha_for x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/${REPO}/releases/download/${TAG}/agentkube-${TAG}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "$(sha_for aarch64-unknown-linux-gnu)"
    end
    on_intel do
      url "https://github.com/${REPO}/releases/download/${TAG}/agentkube-${TAG}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "$(sha_for x86_64-unknown-linux-gnu)"
    end
  end

  def install
    bin.install "akctl", "agentkube-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akctl version")
  end
end
EOF

if command -v ruby >/dev/null 2>&1; then
  ruby -c "$FORMULA"
fi
echo "Wrote $FORMULA for $TAG"
