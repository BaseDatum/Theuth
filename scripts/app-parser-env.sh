# shellcheck shell=bash
# Source from packaging scripts after `cd "$ROOT"`:  source "$ROOT/scripts/app-parser-env.sh"
#
# Both bun-compiled helpers that ship beside `theuth`:
#
# The app parser helper (design doc 46 §2.1) ships beside `theuth` as
# `theuth-app-parser-<library sha256>`: one immutable file per library hash, so an
# update can drop the new helper next to the old one before swapping `theuth`, and
# an older `theuth` still running keeps finding its own. The hash is the committed
# `packages/apps/generated/library-hash.txt`, the value `theuth` and the helper both
# embed at build time; the updater accepts only `theuth-app-parser-[0-9a-f]{64}`.
#
# The documents helper (design doc 59 §3.2) ships as `theuth-documents-<protocol version>`
# (`crates/document/src/helper.rs::helper_path`), the version being PROTOCOL_VERSION of
# `packages/documents/src/protocol.ts`, which `theuth` reads at compile time; the updater
# accepts `theuth-documents-[0-9.]+`.
#
# Sets: HELPER_HASH     64 lowercase hex
#       HELPER          theuth-app-parser-$HELPER_HASH
#       HELPER_SRC      packages/apps/dist/theuth-app-parser  (output of `bun run build:helper`)
#       DOC_VERSION     e.g. 59.5
#       DOC_HELPER      theuth-documents-$DOC_VERSION
#       DOC_HELPER_SRC  packages/documents/dist/theuth-documents  (output of `bun run build:helper`)
HELPER_HASH="$(tr -d '[:space:]' <packages/apps/generated/library-hash.txt)"
[[ "$HELPER_HASH" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "error: packages/apps/generated/library-hash.txt is not a sha256 hex (got '$HELPER_HASH'; run bun run generate in packages/apps)" >&2; exit 1; }
HELPER="theuth-app-parser-$HELPER_HASH"
HELPER_SRC="packages/apps/dist/theuth-app-parser"

# build_helper: standalone `bun build --compile` executable for the host, no runtime Bun/Node.
# THEUTH_SKIP_BUN_INSTALL=1 skips `bun install`.
build_helper() {
  command -v bun >/dev/null 2>&1 || { echo "error: bun required to build $HELPER (brew install oven-sh/bun/bun)" >&2; exit 1; }
  (
    cd packages/apps
    export OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1
    [[ "${THEUTH_SKIP_BUN_INSTALL:-0}" == "1" ]] || bun install --frozen-lockfile
    bun run build:helper
  )
  [[ -x "$HELPER_SRC" ]] || { echo "error: $HELPER_SRC not produced by bun run build:helper" >&2; exit 1; }
  check_helper "$HELPER_SRC"
}

# check_helper <path>: the executable reports the hash this checkout embeds and
# exits 0 on stdin EOF — the same probe release-s3.sh runs on every tarball.
check_helper() {
  local out
  out="$("$1" --version 2>&1)" || { echo "error: $1 --version failed: $out" >&2; exit 1; }
  [[ "$out" == *"$HELPER_HASH"* ]] || { echo "error: $1 --version printed '$out', expected library hash $HELPER_HASH (stale build? rerun bun run generate && build:helper)" >&2; exit 1; }
  printf '' | "$1" >/dev/null || { echo "error: $1 did not exit 0 on stdin EOF" >&2; exit 1; }
}

DOC_VERSION="$(sed -n 's/^export const PROTOCOL_VERSION = "\([0-9.]*\)";.*/\1/p' packages/documents/src/protocol.ts)"
[[ "$DOC_VERSION" =~ ^[0-9.]+$ ]] \
  || { echo "error: packages/documents/src/protocol.ts declares no PROTOCOL_VERSION (got '$DOC_VERSION')" >&2; exit 1; }
DOC_HELPER="theuth-documents-$DOC_VERSION"
DOC_HELPER_SRC="packages/documents/dist/theuth-documents"

# build_doc_helper: same as build_helper for packages/documents.
build_doc_helper() {
  command -v bun >/dev/null 2>&1 || { echo "error: bun required to build $DOC_HELPER (brew install oven-sh/bun/bun)" >&2; exit 1; }
  (
    cd packages/documents
    [[ "${THEUTH_SKIP_BUN_INSTALL:-0}" == "1" ]] || bun install --frozen-lockfile
    bun run build:helper
  )
  [[ -x "$DOC_HELPER_SRC" ]] || { echo "error: $DOC_HELPER_SRC not produced by bun run build:helper" >&2; exit 1; }
  check_doc_helper "$DOC_HELPER_SRC"
}

# check_doc_helper <path>: `--version` prints `theuth-documents $DOC_VERSION`; exits 0 on stdin EOF.
check_doc_helper() {
  local out
  out="$("$1" --version 2>&1)" || { echo "error: $1 --version failed: $out" >&2; exit 1; }
  [[ "$out" == "theuth-documents $DOC_VERSION" ]] || { echo "error: $1 --version printed '$out', expected 'theuth-documents $DOC_VERSION' (stale build? rerun bun run build:helper in packages/documents)" >&2; exit 1; }
  printf '' | "$1" >/dev/null || { echo "error: $1 did not exit 0 on stdin EOF" >&2; exit 1; }
}
