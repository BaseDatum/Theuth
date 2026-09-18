# shellcheck shell=bash
# Source from packaging scripts after `cd "$ROOT"`:  source "$ROOT/scripts/app-parser-env.sh"
#
# The app parser helper (design doc 46 §2.1) ships beside `wth` as
# `wth-app-parser-<library sha256>`: one immutable file per library hash, so an
# update can drop the new helper next to the old one before swapping `wth`, and
# an older `wth` still running keeps finding its own. The hash is the committed
# `packages/apps/generated/library-hash.txt`, the value `wth` and the helper both
# embed at build time; the updater accepts only `wth-app-parser-[0-9a-f]{64}`.
#
# Sets: HELPER_HASH  64 lowercase hex
#       HELPER       wth-app-parser-$HELPER_HASH
#       HELPER_SRC   packages/apps/dist/wth-app-parser  (output of `bun run build:helper`)
HELPER_HASH="$(tr -d '[:space:]' <packages/apps/generated/library-hash.txt)"
[[ "$HELPER_HASH" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "error: packages/apps/generated/library-hash.txt is not a sha256 hex (got '$HELPER_HASH'; run bun run generate in packages/apps)" >&2; exit 1; }
HELPER="wth-app-parser-$HELPER_HASH"
HELPER_SRC="packages/apps/dist/wth-app-parser"

# build_helper: standalone `bun build --compile` executable for the host, no runtime Bun/Node.
# WTH_SKIP_BUN_INSTALL=1 skips `bun install`.
build_helper() {
  command -v bun >/dev/null 2>&1 || { echo "error: bun required to build $HELPER (brew install oven-sh/bun/bun)" >&2; exit 1; }
  (
    cd packages/apps
    export OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1
    [[ "${WTH_SKIP_BUN_INSTALL:-0}" == "1" ]] || bun install --frozen-lockfile
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
