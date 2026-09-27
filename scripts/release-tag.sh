#!/usr/bin/env bash
# Helper to create and push a release tag, triggering the release pipeline.
#
# Usage:
#   ./scripts/release-tag.sh v0.1.0
#   ./scripts/release-tag.sh v0.1.0 --dry-run
#
# The tag must match v* (e.g. v0.1.0). Pushing it triggers
# .github/workflows/release.yml, which builds all platform installers and
# creates the GitHub Release.

set -euo pipefail

DRY_RUN=0
TAG=""

usage() {
  cat <<'EOF'
Usage: release-tag.sh vX.Y.Z [--dry-run]

  Creates an annotated tag and pushes it to origin.
EOF
}

for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    --dry-run) DRY_RUN=1 ;;
    v*) TAG="$arg" ;;
    *) echo "release-tag.sh: unexpected argument: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

if [ -z "$TAG" ]; then
  echo "release-tag.sh: missing tag (e.g. v0.1.0)" >&2; usage >&2; exit 2
fi

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "release-tag.sh: not inside a git repository" >&2; exit 1
fi

if [ "$(git status --porcelain | head -n 1)" != "" ]; then
  echo "release-tag.sh: working tree is dirty, commit or stash first" >&2; exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "release-tag.sh: tag already exists locally: $TAG" >&2; exit 1
fi

# Ensure release-critical checks pass before tagging.
echo "Running pre-tag checks (fmt, clippy, test, doc) ..."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo doc --workspace --no-deps

if [ "$DRY_RUN" = "1" ]; then
  echo "[dry-run] would create and push tag: $TAG"
  exit 0
fi

git tag -a "$TAG" -m "release $TAG"
git push origin "$TAG"
echo "Pushed $TAG. Watch the release pipeline under Actions > release."
