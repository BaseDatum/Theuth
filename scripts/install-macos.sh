#!/usr/bin/env bash
# Install (or overwrite) theuth.app into /Applications (or $THEUTH_INSTALL_DIR).
#
# Uses the packaged app from the last `just package-macos`, or an explicit
# path via $THEUTH_APP / first argument.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-${THEUTH_APP:-$ROOT/target/release/bundle/macos/theuth.app}}"
DEST_DIR="${THEUTH_INSTALL_DIR:-/Applications}"
DEST="$DEST_DIR/theuth.app"

[[ -d "$APP" ]] || {
  echo "error: $APP not found — run 'just package-macos' first" >&2
  exit 1
}
# The sidecar spawns theuth-app-parser-<hash> from its own directory (46 §2.1); a bundle
# without it is a pre-46 or half-packaged app, not something to install.
compgen -G "$APP/Contents/MacOS/theuth-app-parser-*" >/dev/null || {
  echo "error: $APP has no Contents/MacOS/theuth-app-parser-<hash> — rerun 'just package-macos'" >&2
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
