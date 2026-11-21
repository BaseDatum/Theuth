# shellcheck shell=bash
# Source from packaging scripts after `cd "$ROOT"`:  source "$ROOT/scripts/app-parser-env.sh"
#
# The files that ship beside `theuth`:
#
# The app parser entry (design doc 46 §2.1; apps-inproc-as-built-P1.md) ships beside `theuth` as
# `theuth-app-parser-<library sha256>`. `theuth` validates apps in-process (crate app-js) and never
# runs it: since the in-process parser it is a tiny compatibility stub (crates/app-parser-stub,
# `cargo build`, no Bun) that prints the hash for `--version`, kept only because installed
# updaters refuse a payload without exactly one `theuth-app-parser-[0-9a-f]{64}` entry. The hash is
# the committed `packages/apps/generated/library-hash.txt`, the value `theuth` and the stub both
# embed at build time. A later release may drop the entry once no supported updater requires it.
#
# The documents helper (design doc 59 §3.2) ships as `theuth-documents-<protocol version>`
# (`crates/document/src/helper.rs::helper_path`), the version being PROTOCOL_VERSION of
# `packages/documents/src/protocol.ts`, which `theuth` reads at compile time; the updater
# accepts `theuth-documents-[0-9.]+`.
#
# Sets: HELPER_HASH     64 lowercase hex
#       HELPER          theuth-app-parser-$HELPER_HASH
#       HELPER_SRC      target/release/theuth-app-parser-stub  (output of `cargo build --release -p app-parser-stub`)
#       DOC_VERSION     e.g. 59.5
#       DOC_HELPER      theuth-documents-$DOC_VERSION
#       DOC_HELPER_SRC  packages/documents/dist/theuth-documents  (output of `bun run build:helper`)
HELPER_HASH="$(tr -d '[:space:]' <packages/apps/generated/library-hash.txt)"
[[ "$HELPER_HASH" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "error: packages/apps/generated/library-hash.txt is not a sha256 hex (got '$HELPER_HASH'; run bun run generate in packages/apps)" >&2; exit 1; }
HELPER="theuth-app-parser-$HELPER_HASH"
HELPER_SRC="target/release/theuth-app-parser-stub"

# build_helper: the compatibility stub for the host (cargo, no Bun). Cargo honours CARGO_TARGET_DIR;
# the stub lands in target/release unless the caller overrides HELPER_SRC accordingly.
build_helper() {
  cargo build --release -p app-parser-stub
  [[ -x "$HELPER_SRC" ]] || { echo "error: $HELPER_SRC not produced by cargo build --release -p app-parser-stub" >&2; exit 1; }
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
