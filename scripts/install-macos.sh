#!/usr/bin/env bash
# Install (or overwrite) wth.app into /Applications (or $WTH_INSTALL_DIR).
#
# Uses the packaged app from the last `just package-macos`, or an explicit
# path via $WTH_APP / first argument.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-${WTH_APP:-$ROOT/target/release/bundle/macos/wth.app}}"
DEST_DIR="${WTH_INSTALL_DIR:-/Applications}"
DEST="$DEST_DIR/wth.app"

[[ -d "$APP" ]] || {
  echo "error: $APP not found — run 'just package-macos' first" >&2
  exit 1
}
# The sidecar spawns wth-app-parser-<hash> from its own directory (46 §2.1); a bundle
# without it is a pre-46 or half-packaged app, not something to install.
compgen -G "$APP/Contents/MacOS/wth-app-parser-*" >/dev/null || {
  echo "error: $APP has no Contents/MacOS/wth-app-parser-<hash> — rerun 'just package-macos'" >&2
  exit 1
}

echo "==> installing $APP → $DEST"
mkdir -p "$DEST_DIR"
if [[ -d "$DEST" ]]; then
  echo "    overwriting existing $DEST"
  rm -rf "$DEST"
fi
ditto "$APP" "$DEST"

echo
echo "done: $DEST"
echo "  open: open \"$DEST\""
