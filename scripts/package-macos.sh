#!/usr/bin/env bash
# Package theuth.app + theuth.dmg for macOS (jarvis pipeline, stripped).
#
#   1. cargo build --release -p theuth  (daemon sidecar, host arch)
#      + packages/apps: bun build --compile → theuth-app-parser (46 §2.1; scripts/app-parser-env.sh)
#   2. check_sidecar (LC_RPATH /usr/lib/swift + `t --version`; 27 Revision 2026-10-07) and
#      stage sidecar as binaries/t-<host-triple>
#   3. cargo tauri build --config tauri.bundle.conf.json  (app bundle only)
#      + libonnxruntime.dylib (Microsoft's release, pinned) into Contents/Frameworks (45 §2.1)
#      + theuth-app-parser-<hash> into Contents/MacOS beside the sidecar (the documents helper is in `t`)
#      + Theuth Companion.app (130 D11; scripts/bundle-companion.sh when crates/desktop/companion exists)
#        into Contents/Library/LoginItems — by hand, not `bundle.macOS.files`: the bundler copies those
#        but never signs them (spike SA §3), and this script's sign order is already manual
#   4. codesign inside-out (hardened runtime): $THEUTH_SIGN_IDENTITY, else ad-hoc "-"
#      both helpers get scripts/entitlements-app-parser.plist (Bun JIT); check_sidecar again on
#      the signed sidecar (library validation must still load the system Swift runtime)
#   5. hdiutil DMG (after signing — Tauri's dmg target would snapshot pre-signature)
#
# Arm64-only: built on/for the host. Universal binaries are out of scope.
#
# Env:
#   THEUTH_SIGN_IDENTITY      codesign identity (default: "-" ad-hoc)
#   THEUTH_SKIP_BUN_INSTALL=1 skip `bun install` (desktop UI and packages/apps)
# Optional local defaults: repo-root `.env.signing`.
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

HOST_TRIPLE="$(rustc -vV | awk '/^host:/ {print $2}')"
IDENTITY="${THEUTH_SIGN_IDENTITY:--}"
BUNDLE_ID="io.theuth.mac"
BUNDLE_DIR="target/release/bundle/macos"
APP="$BUNDLE_DIR/theuth.app"
# ONNX Runtime for every ONNX model (45 §2.1, 57 §6 step 2c): dlopened by the sidecar from
# Contents/Frameworks — first in `models::onnx::ort_lib`'s search order, because a hardened-runtime
# app must load the copy we codesign. The pin lives in scripts/ort-version.env.
# shellcheck source=ort-version.env
source "$ROOT/scripts/ort-version.env"
ORT_SHA256="$ORT_SHA256_OSX_ARM64"
ORT_TGZ="target/onnxruntime-osx-arm64-${ORT_VERSION}.tgz"
ENTITLEMENTS="crates/desktop/entitlements.plist"
HELPER_ENTITLEMENTS="scripts/entitlements-app-parser.plist"
# 130 D11: the SwiftPM product of scripts/bundle-companion.sh; absent until W1 lands, then nested as a
# login item (SMAppService needs Contents/Library/LoginItems) and signed before the outer app.
COMPANION_SRC="target/release/companion/Theuth Companion.app"
COMPANION_DST="$APP/Contents/Library/LoginItems/Theuth Companion.app"
COMPANION_ENTITLEMENTS="crates/desktop/companion.entitlements.plist"

# check_sidecar <path>: the daemon links ScreenCaptureKit through Swift bridges (design 27
# Revision 2026-10-07; spike 121 §6), so its Mach-O references @rpath/libswift_Concurrency.dylib
# and must carry LC_RPATH /usr/lib/swift (emitted by crates/theuth/build.rs) or dyld aborts it at
# launch: `Library not loaded: @rpath/libswift_Concurrency.dylib … no LC_RPATH's found`. Checked on
# the plain build and again after codesign, where the hardened runtime's library validation must
# still accept the system Swift dylibs.
check_sidecar() {
  local out
  if otool -L "$1" | grep -q 'libswift'; then
    otool -l "$1" | grep -A2 LC_RPATH | grep -q '/usr/lib/swift' \
      || { echo "error: $1 links the Swift runtime but carries no LC_RPATH /usr/lib/swift (crates/theuth/build.rs missing or not applied?)" >&2; exit 1; }
  fi
  out="$("$1" --version 2>&1)" || { echo "error: $1 --version failed: $out" >&2; exit 1; }
  [[ "$out" == t\ * ]] || { echo "error: $1 --version printed '$out', expected 't <version>'" >&2; exit 1; }
}

echo "==> [1/5] TypeScript bindings"
just ts-desktop

echo "==> [1/5] building theuth (release, $HOST_TRIPLE)"
# 202: mlx-sys compiles MLX's Metal kernels into $MLX_RS_METAL_PATH/mlx.metallib; pinned here so the
# copy into Contents/Resources below does not have to guess mlx-sys's ~/.mlx/lib/<key>/ directory.
MLX_METAL_DIR="$ROOT/target/release/mlx-metal"
MLX_RS_METAL_PATH="$MLX_METAL_DIR" cargo build --release -p theuth

echo "==> [1/5] building $HELPER (bun build --compile)"
build_helper

if [[ -f crates/desktop/companion/Package.swift ]]; then
  echo "==> [1/5] building Theuth Companion.app (swift build + bundle)"
  scripts/bundle-companion.sh
else
  echo "==> [1/5] no crates/desktop/companion/Package.swift; skipping the companion build"
fi

echo "==> [2/5] checking + staging sidecar"
check_sidecar target/release/t
mkdir -p crates/desktop/binaries
cp target/release/t "crates/desktop/binaries/t-$HOST_TRIPLE"

echo "==> [3/5] tauri build (release app bundle)"
if [[ "${THEUTH_SKIP_BUN_INSTALL:-0}" != "1" ]]; then
  (cd crates/desktop/ui && bun install)
fi
(cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri build --config tauri.bundle.conf.json)
[[ -d "$APP" ]] || { echo "error: $APP not produced" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/t" ]] || { echo "error: sidecar missing at $APP/Contents/MacOS/t" >&2; exit 1; }
[[ -x "$APP/Contents/MacOS/theuth-desktop" ]] || { echo "error: app binary missing at $APP/Contents/MacOS/theuth-desktop" >&2; exit 1; }

echo "==> [3/5] libonnxruntime ${ORT_VERSION} → Contents/Frameworks"
# Three sources, in order: the cached archive, Microsoft's release, our model bucket (the same
# dylib, extracted from that archive by release-models.sh). github.com/microsoft/onnxruntime
# has answered 404 since 2026-11-18, so the bucket is what works today. Whatever the source,
# the dylib itself must match its pin.
mkdir -p "$APP/Contents/Frameworks"
ORT_DYLIB="$APP/Contents/Frameworks/libonnxruntime.dylib"
ORT_MIRROR="${THEUTH_S3_PUBLIC_BASE_URL:-https://s3.rwx.dev/theuth/releases}/models/onnxruntime/aarch64-apple-darwin/libonnxruntime.dylib"
ort_tgz_ok() { [[ -f "$ORT_TGZ" ]] && echo "$ORT_SHA256  $ORT_TGZ" | shasum -a 256 -c --status; }
if ! ort_tgz_ok; then
  curl -sSfL -o "$ORT_TGZ" "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-osx-arm64-${ORT_VERSION}.tgz" 2>/dev/null || rm -f "$ORT_TGZ"
fi
if ort_tgz_ok; then
  tar -xzf "$ORT_TGZ" -O "onnxruntime-osx-arm64-${ORT_VERSION}/lib/libonnxruntime.${ORT_VERSION}.dylib" > "$ORT_DYLIB"
else
  echo "    Microsoft's release archive is unavailable; taking the dylib from $ORT_MIRROR"
  curl -sSfL -o "$ORT_DYLIB" "$ORT_MIRROR" || { echo "error: libonnxruntime ${ORT_VERSION}: neither Microsoft's archive nor $ORT_MIRROR could be fetched" >&2; exit 1; }
fi
echo "$ORT_DYLIB_SHA256_OSX_ARM64  $ORT_DYLIB" | shasum -a 256 -c --status || { echo "error: libonnxruntime.dylib sha256 mismatch (scripts/ort-version.env)" >&2; exit 1; }
chmod 755 "$ORT_DYLIB"

# 202: the MLX speech model's Metal kernels. The sidecar (Contents/MacOS/t) finds them at
# ../Resources/mlx.metallib (crates/voice-mlx/src/metal.rs); without the file the MLX engine reports
# itself unavailable and dictation stays on ONNX.
echo "==> [3/5] mlx.metallib → Contents/Resources"
[[ -f "$MLX_METAL_DIR/mlx.metallib" ]] || { echo "error: $MLX_METAL_DIR/mlx.metallib not built (mlx-sys)" >&2; exit 1; }
install -m 644 "$MLX_METAL_DIR/mlx.metallib" "$APP/Contents/Resources/mlx.metallib"

echo "==> [3/5] $HELPER → Contents/MacOS"
rm -f "$APP/Contents/MacOS"/theuth-app-parser-*
install -m 755 "$HELPER_SRC" "$APP/Contents/MacOS/$HELPER"

if [[ -d "$COMPANION_SRC" ]]; then
  echo "==> [3/5] Theuth Companion.app → Contents/Library/LoginItems"
  rm -rf "$COMPANION_DST"
  mkdir -p "$(dirname "$COMPANION_DST")"
  ditto "$COMPANION_SRC" "$COMPANION_DST"
else
  echo "==> [3/5] no $COMPANION_SRC; packaging without the companion"
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
DMG="$BUNDLE_DIR/theuth_${VERSION}_$(uname -m).dmg"

echo "==> [4/5] codesign (identity: $IDENTITY, hardened runtime)"
TIMESTAMP_FLAG="--timestamp"
[[ "$IDENTITY" == "-" ]] && TIMESTAMP_FLAG="--timestamp=none"
# Nested helpers get $BUNDLE_ID.* so macOS 15+ Local Network Privacy attributes
# to the app instead of an orphan bare-binary row.
codesign --force $TIMESTAMP_FLAG --sign "$IDENTITY" "$APP/Contents/Frameworks/libonnxruntime.dylib"
codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" \
  -i "${BUNDLE_ID}.app-parser" \
  --entitlements "$HELPER_ENTITLEMENTS" "$APP/Contents/MacOS/$HELPER"
check_helper "$APP/Contents/MacOS/$HELPER"
codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" \
  -i "${BUNDLE_ID}.t" \
  --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/t"
# The companion bundle: `codesign` on a bundle signs its CFBundleExecutable and seals Resources/Info.plist;
# it has no nested code of its own. Hardened runtime + timestamp are what notarization requires of it.
if [[ -d "$COMPANION_DST" ]]; then
  codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" \
    -i "${BUNDLE_ID}.companion" \
    --entitlements "$COMPANION_ENTITLEMENTS" "$COMPANION_DST"
fi
codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" \
  --entitlements "$ENTITLEMENTS" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
check_sidecar "$APP/Contents/MacOS/t"
echo "--- spctl assessment (fails until notarized — expected for ad-hoc):"
spctl --assess --type execute -vv "$APP" || true

echo "==> [5/5] building DMG"
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
ditto "$APP" "$STAGING/theuth.app"
ln -s /Applications "$STAGING/Applications"
rm -f "$DMG"
hdiutil create -volname "theuth" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null
if [[ "$IDENTITY" != "-" ]]; then
  codesign --force $TIMESTAMP_FLAG --sign "$IDENTITY" "$DMG"
fi

echo
echo "done:"
echo "  app: $APP"
echo "  helper: $APP/Contents/MacOS/$HELPER"
[[ -d "$COMPANION_DST" ]] && echo "  companion: $COMPANION_DST"
echo "  dmg: $DMG"
if [[ "$IDENTITY" == "-" ]]; then
  echo "  signed: ad-hoc (set THEUTH_SIGN_IDENTITY for a Developer ID build," \
       "then run just notarize-macos)"
fi
