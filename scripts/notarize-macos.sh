#!/usr/bin/env bash
# Notarize + staple a packaged theuth DMG and its sibling theuth.app. Not run by `just package-macos`.
#
#   THEUTH_SIGN_IDENTITY="Developer ID Application: …" just package-macos
#   just notarize-macos
#
# Auth (first match):
#   THEUTH_NOTARY_KEYCHAIN_PROFILE   notarytool store-credentials profile
#   THEUTH_NOTARY_KEY_PATH + THEUTH_NOTARY_KEY_ID + THEUTH_NOTARY_ISSUER   App Store Connect API key (.p8)
#   THEUTH_NOTARY_APPLE_ID + THEUTH_NOTARY_TEAM_ID + THEUTH_NOTARY_PASSWORD
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
BUNDLE_DIR="$ROOT/target/release/bundle/macos"
DMG="${1:-}"
if [[ -z "$DMG" ]]; then
  VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$BUNDLE_DIR/theuth.app/Contents/Info.plist" 2>/dev/null || true)"
  if [[ -n "$VERSION" && -f "$BUNDLE_DIR/theuth_${VERSION}_$(uname -m).dmg" ]]; then
    DMG="$BUNDLE_DIR/theuth_${VERSION}_$(uname -m).dmg"
  else
    DMG="$(ls -t "$BUNDLE_DIR"/theuth_*.dmg 2>/dev/null | head -1 || true)"
  fi
fi
[[ -n "$DMG" && -f "$DMG" ]] || { echo "error: no DMG found (run 'just package-macos' first or pass a path)" >&2; exit 1; }
DMG="$(cd "$(dirname "$DMG")" && pwd)/$(basename "$DMG")"

if [[ -n "${THEUTH_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  AUTH=(--keychain-profile "$THEUTH_NOTARY_KEYCHAIN_PROFILE")
elif [[ -n "${THEUTH_NOTARY_KEY_PATH:-}" ]]; then
  [[ -f "$THEUTH_NOTARY_KEY_PATH" ]] || { echo "error: THEUTH_NOTARY_KEY_PATH does not exist: $THEUTH_NOTARY_KEY_PATH" >&2; exit 1; }
  : "${THEUTH_NOTARY_KEY_ID:?set THEUTH_NOTARY_KEY_ID}"
  : "${THEUTH_NOTARY_ISSUER:?set THEUTH_NOTARY_ISSUER}"
  AUTH=(--key "$THEUTH_NOTARY_KEY_PATH" --key-id "$THEUTH_NOTARY_KEY_ID" --issuer "$THEUTH_NOTARY_ISSUER")
else
  : "${THEUTH_NOTARY_APPLE_ID:?set THEUTH_NOTARY_APPLE_ID (or THEUTH_NOTARY_KEYCHAIN_PROFILE / THEUTH_NOTARY_KEY_PATH)}"
  : "${THEUTH_NOTARY_TEAM_ID:?set THEUTH_NOTARY_TEAM_ID}"
  : "${THEUTH_NOTARY_PASSWORD:?set THEUTH_NOTARY_PASSWORD (app-specific password)}"
  AUTH=(--apple-id "$THEUTH_NOTARY_APPLE_ID" --team-id "$THEUTH_NOTARY_TEAM_ID" --password "$THEUTH_NOTARY_PASSWORD")
fi

echo "==> submitting $DMG"
xcrun notarytool submit "$DMG" "${AUTH[@]}" --wait
echo "==> stapling ticket (DMG)"
xcrun stapler staple "$DMG"
echo "==> gatekeeper assessment (DMG)"
spctl --assess --type open --context context:primary-signature -vv "$DMG"

# Apple's notary service registers tickets for both the DMG and the signed
# theuth.app inside it (by CDHash), so stapling the sibling .app here attaches
# its notarization ticket before package-update-macos.sh archives .app.tar.gz —
# no second notary submission needed. Do NOT staple nested LoginItems
# (Theuth Companion.app) inside theuth.app after signing, as modifying a nested
# bundle would invalidate the outer app's sealed signature.
APP="$(dirname "$DMG")/theuth.app"
if [[ -d "$APP" ]]; then
  echo "==> stapling ticket ($APP)"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  echo "==> verifying signature + gatekeeper assessment ($APP)"
  codesign --verify --deep --strict "$APP"
  spctl --assess --type execute -vv "$APP"
  echo "done: $DMG and $APP are notarized and stapled"
else
  echo "done: $DMG is notarized and stapled"
fi
