#!/usr/bin/env bash
# Package theuth.ipa for iOS/iPadOS TestFlight.
#
# Pipeline:
#   1. toolchain (full Xcode, Apple Distribution, rust iOS target)
#   2. cargo tauri ios build --export-method app-store-connect
#      → crates/desktop/gen/apple/build/arm64/theuth.ipa
#
# The Xcode project (crates/desktop/gen/apple) is committed. Caps live in
# desktop_iOS/desktop_iOS.entitlements. Do not `tauri ios init` over it.
# Signing matches jarvis: automatic + team ID. Export-method app-store-connect
# re-signs with Apple Distribution. Do not pass CODE_SIGN_IDENTITY — that
# conflicts with automatic development signing.
#
# Env:
#   APPLE_DEVELOPMENT_TEAM     overwrite bundle.iOS.developmentTeam
#   THEUTH_IOS_BUILD_NUMBER       CFBundleVersion (default: git commit count)
#   THEUTH_SKIP_BUN_INSTALL=1     skip `bun install`
#   THEUTH_IOS_EXPORT_METHOD      default app-store-connect
# Optional local defaults: repo-root `.env.signing`.
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

DESKTOP="$ROOT/crates/desktop"
APPLE_DIR="$DESKTOP/gen/apple"
TEAM_ID="${APPLE_DEVELOPMENT_TEAM:-8H548FVL95}"
EXPORT_METHOD="${THEUTH_IOS_EXPORT_METHOD:-app-store-connect}"
VERSION="$(awk '/^\[workspace.package\]/{p=1} p&&/^version =/{gsub(/"/,""); print $3; exit}' Cargo.toml)"
BUILD_NUMBER="${THEUTH_IOS_BUILD_NUMBER:-$(git -C "$ROOT" rev-list --count HEAD)}"

fail() { echo "package-ios: $*" >&2; exit 1; }

echo "==> [1/3] toolchain"
dev_dir="$(xcode-select -p 2>/dev/null || true)"
if [[ -z "$dev_dir" || "$dev_dir" != *"Xcode.app"* ]]; then
  fail "full Xcode required. xcode-select -p = '${dev_dir:-<none>}'.
        Install Xcode and run: sudo xcode-select -s /Applications/Xcode.app/Contents/Developer"
fi
echo "    ok: Xcode at $dev_dir"

if [[ "$EXPORT_METHOD" == "app-store-connect" || "$EXPORT_METHOD" == "release-testing" ]]; then
  if ! security find-identity -v -p codesigning \
      | grep -F "Apple Distribution: " | grep -F "$TEAM_ID" | grep -v REVOKED >/dev/null; then
    fail "no valid 'Apple Distribution' identity for team $TEAM_ID.
        Developer ID Application is macOS-only. Create/install an Apple
        Distribution certificate (developer.apple.com → Certificates)."
  fi
  echo "    ok: Apple Distribution identity for team $TEAM_ID"
fi

if ! rustup target list --installed 2>/dev/null | grep -qx 'aarch64-apple-ios'; then
  echo "    rustup target add aarch64-apple-ios"
  rustup target add aarch64-apple-ios
fi
echo "    ok: rust target aarch64-apple-ios"

echo "==> [2/3] Xcode project (gen/apple)"
[[ -d "$APPLE_DIR/desktop.xcodeproj" ]] || fail "missing committed $APPLE_DIR/desktop.xcodeproj
    (the Xcode project is in git; do not tauri ios init over it)"
if [[ "${THEUTH_SKIP_BUN_INSTALL:-0}" != "1" ]]; then
  (cd "$DESKTOP/ui" && bun install)
fi
just ts-desktop
echo "    ok: $APPLE_DIR"

echo "==> [3/3] tauri ios build (export-method=$EXPORT_METHOD, version=$VERSION, build=$BUILD_NUMBER)"
export APPLE_DEVELOPMENT_TEAM="$TEAM_ID"
# API key is for `just testflight` (altool) only. If it is set here, Tauri
# skips keychain signing and exports with a dummy "Tauri (unset)" cert.
unset APPLE_API_KEY APPLE_API_KEY_ID APPLE_API_ISSUER APPLE_API_KEY_PATH
(cd "$DESKTOP" && TAURI_FRONTEND_PATH=ui cargo tauri ios build \
  --config tauri.ios.conf.json \
  --export-method "$EXPORT_METHOD" \
  --build-number "$BUILD_NUMBER")

IPA="$(find "$APPLE_DIR/build" -name '*.ipa' -type f | head -1 || true)"
[[ -n "$IPA" && -f "$IPA" ]] || fail "no .ipa produced under $APPLE_DIR/build
    (signing / provisioning failed? check Xcode account + App ID + App Store Connect app)"

echo
echo "done:"
echo "  ipa:     $IPA"
echo "  version: $VERSION  build: $BUILD_NUMBER"
echo "  team:    $TEAM_ID"
echo "  method:  $EXPORT_METHOD"
echo "  next:    just testflight     # upload to App Store Connect / TestFlight"
