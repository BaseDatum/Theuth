#!/usr/bin/env bash
# Pack the notarized theuth.app into the update payloads (39 §5.1).
#
# Pipeline (after `just package-macos` + `just notarize-macos`):
#   1. dist/theuth_<ver>_macos-aarch64.app.tar.gz   theuth.app        (tauri-plugin-updater payload)
#   2. dist/theuth_<ver>_macos-aarch64.tar.gz       `t` + `theuth-app-parser-<hash>` + `theuth-documents-<ver>` (CLI payload, formula, self-update)
#   3. dist/theuth_<ver>_macos-aarch64.dmg          the packaged DMG, renamed to the manifest key
#
# dist/ = target/release/dist. No signing here: release-s3.sh signs every
# tarball in one place after the Linux ones are pulled from staging.
#
# COPYFILE_DISABLE=1 + --no-xattrs: BSD tar would otherwise emit AppleDouble
# "._theuth.app" entries, which the updater plugin fails to unpack.
#
# Env: none. Optional local defaults: repo-root `.env.signing`.
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

command -v cargo >/dev/null 2>&1 || { echo "error: cargo not on PATH (export PATH=\"\$(just toolchain):\$PATH\")" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="theuth") | .version')"
# The one place the shell arch spelling is mapped to Rust's std::env::consts pair.
KEY="$(uname -s | tr '[:upper:]' '[:lower:]' | sed s/darwin/macos/)-$(uname -m | sed s/arm64/aarch64/)"
BUNDLE_DIR="target/release/bundle/macos"
APP="$BUNDLE_DIR/theuth.app"
DMG="$BUNDLE_DIR/theuth_${VERSION}_$(uname -m).dmg"
DIST="target/release/dist"

[[ -d "$APP" ]] || { echo "error: $APP not found — run just package-macos first" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/t" ]] || { echo "error: sidecar missing at $APP/Contents/MacOS/t" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/$HELPER" ]] || { echo "error: $HELPER missing in $APP/Contents/MacOS — the app was packaged from another library hash; rerun just package-macos" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/$DOC_HELPER" ]] || { echo "error: $DOC_HELPER missing in $APP/Contents/MacOS — the app was packaged from another protocol version; rerun just package-macos" >&2; exit 1; }
[[ -f "$DMG" ]] || { echo "error: $DMG not found — run just package-macos first" >&2; exit 1; }

mkdir -p "$DIST"
APP_TGZ="$DIST/theuth_${VERSION}_${KEY}.app.tar.gz"
CLI_TGZ="$DIST/theuth_${VERSION}_${KEY}.tar.gz"
DIST_DMG="$DIST/theuth_${VERSION}_${KEY}.dmg"
rm -f "$APP_TGZ" "$CLI_TGZ" "$DIST_DMG"

echo "==> [1/3] $APP_TGZ"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$APP_TGZ" -C "$BUNDLE_DIR" theuth.app

echo "==> [2/3] $CLI_TGZ"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$CLI_TGZ" -C "$APP/Contents/MacOS" t "$HELPER" "$DOC_HELPER"

echo "==> [3/3] $DIST_DMG"
cp "$DMG" "$DIST_DMG"

echo
echo "done:"
ls -lh "$APP_TGZ" "$CLI_TGZ" "$DIST_DMG"
