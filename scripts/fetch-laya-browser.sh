#!/usr/bin/env bash
# Fetch the browser decision checkpoint — cklxx/laya-browser v10 (design doc 57 §4.1 item 6) —
# into <data_dir>/models/decision-browser/, the layout `decision::engine::MODEL_FILES` loads
# (`decision.checkpoints.browser`). Interim path until `release-models.sh` publishes the same
# four files under `decision-browser/` and the desktop downloader fetches them (54 §9).
#
#   scripts/fetch-laya-browser.sh [DEST]      DEST default: ${THEUTH_DATA_DIR:-~/.theuth}/models/decision-browser
#
# Files already present with the expected size are kept; anything else is re-downloaded with
# `curl -L` and its size checked against the hub's listing (sizes below, read 2026-09-21).
set -euo pipefail

REPO="https://huggingface.co/cklxx/laya-browser/resolve/main"
REV="v10"
DEST="${1:-${THEUTH_DATA_DIR:-$HOME/.theuth}/models/decision-browser}"

# <remote path under $REV>  <expected bytes>
FILES=(
  "model.safetensors 842609220"
  "encoder/config.json 2084"
  "rl_agent_config.json 633"
  "tokenizer/tokenizer.json 3583228"
  "tokenizer/tokenizer_config.json 337"
)

command -v curl >/dev/null 2>&1 || { echo "error: curl required" >&2; exit 1; }
size_of() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; } # GNU first: BSD `-f %z` is a valid *filesystem* query on GNU and returns garbage

mkdir -p "$DEST"
for entry in "${FILES[@]}"; do
  path="${entry% *}"; want="${entry##* }"
  out="$DEST/$path"
  if [ -f "$out" ] && [ "$(size_of "$out")" = "$want" ]; then
    echo "ok        $path ($want B)"
    continue
  fi
  mkdir -p "$(dirname "$out")"
  echo "fetching  $path"
  curl -L --fail --progress-bar -o "$out.part" "$REPO/$REV/$path"
  got="$(size_of "$out.part")"
  if [ "$got" != "$want" ]; then
    echo "error: $path is $got B, expected $want B" >&2
    rm -f "$out.part"
    exit 1
  fi
  mv "$out.part" "$out"
  echo "ok        $path ($want B)"
done
echo "checkpoint in $DEST"
