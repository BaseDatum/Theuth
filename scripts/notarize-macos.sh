#!/usr/bin/env bash
# Notarize + staple a packaged theuth DMG. Not run by `just package-macos`.
#
#   THEUTH_SIGN_IDENTITY="Developer ID Application: …" just package-macos
#   just notarize-macos
#
# Auth (first match):
#   THEUTH_NOTARY_KEYCHAIN_PROFILE   notarytool store-credentials profile
#   THEUTH_NOTARY_APPLE_ID + THEUTH_NOTARY_TEAM_ID + THEUTH_NOTARY_PASSWORD
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
BUNDLE_DIR="$ROOT/target/release/bundle/macos"
DMG="${1:-}"
if [[ -z "$DMG" ]]; then
  DMG="$(ls -t "$BUNDLE_DIR"/t_*.dmg 2>/dev/null | head -1 || true)"
fi
[[ -n "$DMG" && -f "$DMG" ]] || { echo "error: no DMG found (run 'just package-macos' first or pass a path)" >&2; exit 1; }
DMG="$(cd "$(dirname "$DMG")" && pwd)/$(basename "$DMG")"

if [[ -n "${THEUTH_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  AUTH=(--keychain-profile "$THEUTH_NOTARY_KEYCHAIN_PROFILE")
else
  : "${THEUTH_NOTARY_APPLE_ID:?set THEUTH_NOTARY_APPLE_ID (or THEUTH_NOTARY_KEYCHAIN_PROFILE)}"
  : "${THEUTH_NOTARY_TEAM_ID:?set THEUTH_NOTARY_TEAM_ID}"
  : "${THEUTH_NOTARY_PASSWORD:?set THEUTH_NOTARY_PASSWORD (app-specific password)}"
  AUTH=(--apple-id "$THEUTH_NOTARY_APPLE_ID" --team-id "$THEUTH_NOTARY_TEAM_ID" --password "$THEUTH_NOTARY_PASSWORD")
fi

echo "==> submitting $DMG"
xcrun notarytool submit "$DMG" "${AUTH[@]}" --wait
echo "==> stapling ticket"
xcrun stapler staple "$DMG"
echo "==> gatekeeper assessment"
spctl --assess --type open --context context:primary-signature -vv "$DMG"
echo "done: $DMG is notarized and stapled"
