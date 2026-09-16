#!/usr/bin/env bash
# Upload a packaged wth.ipa to App Store Connect (TestFlight).
#
# NOT run by `just package-ios` — upload needs an App Store Connect API key.
# Run after a signed app-store-connect IPA:
#
#   just package-ios
#   just testflight
#
# Auth (first match wins):
#   APPLE_API_KEY_ID + APPLE_API_ISSUER
#     key file: $APPLE_API_KEY_PATH, or AuthKey_<id>.p8 in
#     ~/.appstoreconnect/private_keys (altool's search path)
#   APPLE_API_KEY is accepted as an alias of APPLE_API_KEY_ID (Tauri name)
#
# Optional local defaults: repo-root `.env.signing`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

APPLE_DIR="$ROOT/crates/desktop/gen/apple"
IPA="${1:-}"
if [[ -z "$IPA" ]]; then
  IPA="$(find "$APPLE_DIR/build" -name '*.ipa' -type f -print0 2>/dev/null \
    | xargs -0 ls -t 2>/dev/null | head -1 || true)"
fi
[[ -n "$IPA" && -f "$IPA" ]] || {
  echo "error: no IPA found (run 'just package-ios' first or pass a path)" >&2
  exit 1
}
IPA="$(cd "$(dirname "$IPA")" && pwd)/$(basename "$IPA")"

KEY_ID="${APPLE_API_KEY_ID:-${APPLE_API_KEY:-}}"
ISSUER="${APPLE_API_ISSUER:-}"
[[ -n "$KEY_ID" && -n "$ISSUER" ]] || {
  echo "error: set APPLE_API_KEY_ID (or APPLE_API_KEY) and APPLE_API_ISSUER" >&2
  echo "       in .env.signing" >&2
  exit 1
}

# altool only searches fixed directories. If the user pointed APPLE_API_KEY_PATH
# elsewhere, copy it into the canonical location without overwriting a newer file.
ASC_DIR="${HOME}/.appstoreconnect/private_keys"
CANONICAL="$ASC_DIR/AuthKey_${KEY_ID}.p8"
if [[ ! -f "$CANONICAL" ]]; then
  SRC="${APPLE_API_KEY_PATH:-}"
  [[ -n "$SRC" && -f "$SRC" ]] || {
    echo "error: missing $CANONICAL" >&2
    echo "       put AuthKey_${KEY_ID}.p8 in $ASC_DIR or set APPLE_API_KEY_PATH" >&2
    exit 1
  }
  mkdir -p "$ASC_DIR"
  cp "$SRC" "$CANONICAL"
  chmod 600 "$CANONICAL"
  echo "==> installed API key at $CANONICAL"
fi

BYTES="$(stat -f%z "$IPA" 2>/dev/null || stat -c%s "$IPA")"
if (( BYTES >= 1048576 )); then
  SIZE_H="$(awk -v b="$BYTES" 'BEGIN { printf "%.1f MB", b/1048576 }')"
else
  SIZE_H="${BYTES} B"
fi

echo "==> IPA: $IPA ($SIZE_H)"
echo "==> uploading to App Store Connect (key $KEY_ID)"
xcrun altool --upload-app \
  --type ios \
  --file "$IPA" \
  --apiKey "$KEY_ID" \
  --apiIssuer "$ISSUER"

echo
echo "done. Apple will process the build (often 5–20 min), then it appears"
echo "under App Store Connect → wth → TestFlight. Add testers there."
echo "  https://appstoreconnect.apple.com/apps"
