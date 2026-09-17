#!/usr/bin/env bash
# Annotated git tag v<ver> for a release, pushed to origin (39 §5.1).
# Last step of `just release`, after release-s3.sh has published latest.json.
#
# Refuses when the tag exists or the tree is dirty: the tag must name the
# commit the artifacts were built from.
#
# Env:
#   WTH_RELEASE_NO_PUSH=1   create the tag locally, skip `git push`
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

command -v cargo >/dev/null 2>&1 || { echo "error: cargo not on PATH (export PATH=\"\$(just toolchain):\$PATH\")" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="wth") | .version')"
TAG="v${VERSION}"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree is dirty; commit or stash before tagging $TAG" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  echo "error: tag $TAG already exists (bump [workspace.package] version)" >&2
  exit 1
fi

echo "==> git tag -a $TAG"
git tag -a "$TAG" -m "wth $VERSION"

if [[ "${WTH_RELEASE_NO_PUSH:-0}" == "1" ]]; then
  echo "    WTH_RELEASE_NO_PUSH=1: not pushed"
else
  echo "==> git push origin $TAG"
  git push origin "refs/tags/$TAG"
fi

echo "done: $TAG"
