#!/usr/bin/env bash
# Package wth.app + wth.dmg for macOS (jarvis pipeline, stripped).
#
#   1. cargo build --release -p wth  (daemon sidecar, host arch)
#   2. stage sidecar as binaries/wth-<host-triple>
#   3. cargo tauri build --config tauri.bundle.conf.json  (app bundle only)
#      + libonnxruntime.dylib (Microsoft's release, pinned) into Contents/Frameworks (45 §2.1)
#   4. codesign inside-out (hardened runtime): $WTH_SIGN_IDENTITY, else ad-hoc "-"
#   5. hdiutil DMG (after signing — Tauri's dmg target would snapshot pre-signature)
#
# Arm64-only: built on/for the host. Universal binaries are out of scope.
#
# Env:
#   WTH_SIGN_IDENTITY      codesign identity (default: "-" ad-hoc)
#   WTH_SKIP_BUN_INSTALL=1 skip `bun install`
# Optional local defaults: repo-root `.env.signing`.
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

HOST_TRIPLE="$(rustc -vV | awk '/^host:/ {print $2}')"
IDENTITY="${WTH_SIGN_IDENTITY:--}"
BUNDLE_ID="wth.rwx.dev"
BUNDLE_DIR="target/release/bundle/macos"
APP="$BUNDLE_DIR/wth.app"
# ONNX Runtime for speech-to-text (45 §2.1): dlopened by the sidecar from Contents/Frameworks.
ORT_VERSION="1.27.0"
ORT_SHA256="545e81c58152353acb0d1e8bd6ce4b62f830c0961f5b3acfedc790ffd76e477a"
ORT_TGZ="target/onnxruntime-osx-arm64-${ORT_VERSION}.tgz"
ENTITLEMENTS="crates/desktop/entitlements.plist"

echo "==> [1/5] TypeScript bindings"
just ts-desktop

echo "==> [1/5] building wth (release, $HOST_TRIPLE)"
cargo build --release -p wth

echo "==> [2/5] staging sidecar"
mkdir -p crates/desktop/binaries
cp target/release/wth "crates/desktop/binaries/wth-$HOST_TRIPLE"

echo "==> [3/5] tauri build (release app bundle)"
if [[ "${WTH_SKIP_BUN_INSTALL:-0}" != "1" ]]; then
  (cd crates/desktop/ui && bun install)
fi
(cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri build --config tauri.bundle.conf.json)
[[ -d "$APP" ]] || { echo "error: $APP not produced" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/wth" ]] || { echo "error: sidecar missing at $APP/Contents/MacOS/wth" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/wth-desktop" ]] || { echo "error: app binary missing at $APP/Contents/MacOS/wth-desktop" >&2; exit 1; }

echo "==> [3/5] libonnxruntime ${ORT_VERSION} → Contents/Frameworks"
if [[ ! -f "$ORT_TGZ" ]] || ! echo "$ORT_SHA256  $ORT_TGZ" | shasum -a 256 -c --status; then
  curl -sSfL -o "$ORT_TGZ" "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-osx-arm64-${ORT_VERSION}.tgz"
  echo "$ORT_SHA256  $ORT_TGZ" | shasum -a 256 -c --status || { echo "error: onnxruntime tarball sha256 mismatch" >&2; exit 1; }
fi
mkdir -p "$APP/Contents/Frameworks"
tar -xzf "$ORT_TGZ" -O "onnxruntime-osx-arm64-${ORT_VERSION}/lib/libonnxruntime.${ORT_VERSION}.dylib" > "$APP/Contents/Frameworks/libonnxruntime.dylib"
chmod 755 "$APP/Contents/Frameworks/libonnxruntime.dylib"

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
DMG="$BUNDLE_DIR/wth_${VERSION}_$(uname -m).dmg"

echo "==> [4/5] codesign (identity: $IDENTITY, hardened runtime)"
TIMESTAMP_FLAG="--timestamp"
[[ "$IDENTITY" == "-" ]] && TIMESTAMP_FLAG="--timestamp=none"
# Nested helpers get $BUNDLE_ID.* so macOS 15+ Local Network Privacy attributes
# to the app instead of an orphan bare-binary row.
codesign --force $TIMESTAMP_FLAG --sign "$IDENTITY" "$APP/Contents/Frameworks/libonnxruntime.dylib"
codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" \
  -i "${BUNDLE_ID}.wth" \
  --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/wth"
codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" \
  --entitlements "$ENTITLEMENTS" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "--- spctl assessment (fails until notarized — expected for ad-hoc):"
spctl --assess --type execute -vv "$APP" || true

echo "==> [5/5] building DMG"
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
ditto "$APP" "$STAGING/wth.app"
ln -s /Applications "$STAGING/Applications"
rm -f "$DMG"
hdiutil create -volname "wth" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null
if [[ "$IDENTITY" != "-" ]]; then
  codesign --force $TIMESTAMP_FLAG --sign "$IDENTITY" "$DMG"
fi

echo
echo "done:"
echo "  app: $APP"
echo "  dmg: $DMG"
if [[ "$IDENTITY" == "-" ]]; then
  echo "  signed: ad-hoc (set WTH_SIGN_IDENTITY for a Developer ID build," \
       "then run just notarize-macos)"
fi
