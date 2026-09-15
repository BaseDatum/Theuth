#!/usr/bin/env bash
# Notarize + staple a packaged wth DMG. Not run by `just package-macos`.
#
#   WTH_SIGN_IDENTITY="Developer ID Application: …" just package-macos
#   just notarize-macos
#
# Auth (first match):
#   WTH_NOTARY_KEYCHAIN_PROFILE   notarytool store-credentials profile
#   WTH_NOTARY_APPLE_ID + WTH_NOTARY_TEAM_ID + WTH_NOTARY_PASSWORD
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
BUNDLE_DIR="$ROOT/target/release/bundle/macos"
DMG="${1:-}"
if [[ -z "$DMG" ]]; then
  DMG="$(ls -t "$BUNDLE_DIR"/wth_*.dmg 2>/dev/null | head -1 || true)"
fi
[[ -n "$DMG" && -f "$DMG" ]] || { echo "error: no DMG found (run 'just package-macos' first or pass a path)" >&2; exit 1; }
DMG="$(cd "$(dirname "$DMG")" && pwd)/$(basename "$DMG")"

if [[ -n "${WTH_NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  AUTH=(--keychain-profile "$WTH_NOTARY_KEYCHAIN_PROFILE")
else
  : "${WTH_NOTARY_APPLE_ID:?set WTH_NOTARY_APPLE_ID (or WTH_NOTARY_KEYCHAIN_PROFILE)}"
  : "${WTH_NOTARY_TEAM_ID:?set WTH_NOTARY_TEAM_ID}"
  : "${WTH_NOTARY_PASSWORD:?set WTH_NOTARY_PASSWORD (app-specific password)}"
  AUTH=(--apple-id "$WTH_NOTARY_APPLE_ID" --team-id "$WTH_NOTARY_TEAM_ID" --password "$WTH_NOTARY_PASSWORD")
fi

echo "==> submitting $DMG"
xcrun notarytool submit "$DMG" "${AUTH[@]}" --wait
echo "==> stapling ticket"
xcrun stapler staple "$DMG"
echo "==> gatekeeper assessment"
spctl --assess --type open --context context:primary-signature -vv "$DMG"
echo "done: $DMG is notarized and stapled"
