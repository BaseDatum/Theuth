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
# theuth-app-parser-<hash> (46 §2.1) is a compatibility stub for installed updaters (the parser and the
# documents helper run inside `t`) but ships in every bundle; a bundle without it is a half-packaged app,
# not something to install.
for helper in theuth-app-parser; do
  compgen -G "$APP/Contents/MacOS/$helper-*" >/dev/null || {
    echo "error: $APP has no Contents/MacOS/$helper-* — rerun 'just package-macos'" >&2
    exit 1
  }
done

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
