#!/usr/bin/env bash
# Bump the product version (major | minor | patch | x.y.z).
#
# One version, two files kept in lockstep:
#   Cargo.toml                       [workspace.package].version — the binary
#                                    crates only (theuth, desktop, relay:
#                                    version.workspace = true), `t --version`,
#                                    macOS bundle, release scripts. Library
#                                    crates keep a frozen version and read the
#                                    product version at runtime
#                                    (platform::version, set by each binary), so
#                                    a bump rebuilds the binaries, not the workspace.
#   crates/desktop/tauri.conf.json   version — iOS CFBundleShortVersionString
#                                    (tauri ios build has no Cargo.toml fallback)
#                                    and the UI badge (vite define)
#   Cargo.lock                       via `cargo update --workspace`
#
# Usage:
#   just bump-version patch | minor | major | 1.2.3
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "error: cargo not on PATH (export PATH=\"\$(just toolchain):\$PATH\")" >&2; exit 1; }

PART="${1:-}"
[[ -n "$PART" ]] || { echo "usage: $0 major|minor|patch|x.y.z" >&2; exit 1; }

CONF="crates/desktop/tauri.conf.json"
CURRENT="$(awk '/^\[workspace.package\]/{p=1} p&&/^version =/{gsub(/"/,""); print $3; exit}' Cargo.toml)"
CONF_CURRENT="$(jq -r .version "$CONF")"
[[ "$CURRENT" == "$CONF_CURRENT" ]] \
  || { echo "error: Cargo.toml ($CURRENT) and $CONF ($CONF_CURRENT) disagree; fix by hand first" >&2; exit 1; }

IFS=. read -r MAJ MIN PAT <<<"$CURRENT"
case "$PART" in
  major) NEW="$((MAJ + 1)).0.0" ;;
  minor) NEW="$MAJ.$((MIN + 1)).0" ;;
  patch) NEW="$MAJ.$MIN.$((PAT + 1))" ;;
  *)
    [[ "$PART" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: '$PART' is not major|minor|patch|x.y.z" >&2; exit 1; }
    NEW="$PART"
    ;;
esac

echo "==> version $CURRENT → $NEW"
# First `version = "…"` after [workspace.package] only; dependency tables below are untouched.
awk -v new="$NEW" '/^\[workspace.package\]/{p=1} p&&/^version =/{$0="version = \"" new "\""; p=0} {print}' Cargo.toml > Cargo.toml.tmp \
  && mv Cargo.toml.tmp Cargo.toml
# sed, not jq: jq reflows the arrays and the diff stops being one line.
sed -i '' -E "s/^(  \"version\": \")[^\"]+(\",)$/\1$NEW\2/" "$CONF"
[[ "$(jq -r .version "$CONF")" == "$NEW" ]] || { echo "error: $CONF version line not rewritten" >&2; exit 1; }

echo "==> cargo update --workspace"
cargo update --workspace --quiet

echo "done: $NEW"
echo "  git commit -am 'release: v$NEW'  &&  just release / just release-ios"
