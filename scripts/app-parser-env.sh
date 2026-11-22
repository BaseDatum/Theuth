# shellcheck shell=bash
# Source from packaging scripts after `cd "$ROOT"`:  source "$ROOT/scripts/app-parser-env.sh"
#
# The file that ships beside `theuth` (the documents helper is compiled into `theuth` itself:
# crates/js-runtime):
#
# The app parser entry (design doc 46 §2.1; apps-inproc-as-built-P1.md) ships beside `theuth` as
# `theuth-app-parser-<library sha256>`. `theuth` validates apps in-process (crate js-runtime) and never
# runs it: since the in-process parser it is a tiny compatibility stub (crates/app-parser-stub,
# `cargo build`, no Bun) that prints the hash for `--version`, kept only because installed
# updaters refuse a payload without exactly one `theuth-app-parser-[0-9a-f]{64}` entry. The hash is
# the committed `packages/apps/generated/library-hash.txt`, the value `theuth` and the stub both
# embed at build time. A later release may drop the entry once no supported updater requires it.
#
# Sets: HELPER_HASH     64 lowercase hex
#       HELPER          theuth-app-parser-$HELPER_HASH
#       HELPER_SRC      target/release/theuth-app-parser-stub  (output of `cargo build --release -p app-parser-stub`)
HELPER_HASH="$(tr -d '[:space:]' <packages/apps/generated/library-hash.txt)"
[[ "$HELPER_HASH" =~ ^[0-9a-f]{64}$ ]] \
  || { echo "error: packages/apps/generated/library-hash.txt is not a sha256 hex (got '$HELPER_HASH'; run bun run generate in packages/apps)" >&2; exit 1; }
HELPER="theuth-app-parser-$HELPER_HASH"
HELPER_SRC="${CARGO_TARGET_DIR:-target}/release/theuth-app-parser-stub"

# check_apps_bundle: the committed in-process bundles (crates/js-runtime/js) match their inputs and the library
# hash `t` embeds (review M1). shasum only, no bun; `just apps-bundle` fixes a failure.
check_apps_bundle() {
  diff <(scripts/apps-bundle-hash.sh) crates/js-runtime/js/apps.sha256 >/dev/null \
    || { echo "error: crates/js-runtime/js (app bundles) is stale against packages/apps (run just apps-bundle and commit)" >&2; exit 1; }
  grep -q "$HELPER_HASH" crates/js-runtime/js/theuth-apps-inproc.js \
    || { echo "error: crates/js-runtime/js/theuth-apps-inproc.js does not embed library hash $HELPER_HASH (run just apps-bundle)" >&2; exit 1; }
}

# build_helper: check the bundles, then build the compatibility stub for the host (cargo, no Bun) into
# $HELPER_SRC (honours CARGO_TARGET_DIR).
build_helper() {
  check_apps_bundle
  cargo build --release -p app-parser-stub
  [[ -x "$HELPER_SRC" ]] || { echo "error: $HELPER_SRC not produced by cargo build --release -p app-parser-stub" >&2; exit 1; }
  check_helper "$HELPER_SRC"
}

# check_helper <path>: the executable reports the hash this checkout embeds and
# exits 0 on stdin EOF — the same probe release-s3.sh runs on every tarball.
check_helper() {
  local out
  out="$("$1" --version 2>&1)" || { echo "error: $1 --version failed: $out" >&2; exit 1; }
  [[ "$out" == *"$HELPER_HASH"* ]] || { echo "error: $1 --version printed '$out', expected library hash $HELPER_HASH (stale build? rerun cargo build --release -p app-parser-stub)" >&2; exit 1; }
  printf '' | "$1" >/dev/null || { echo "error: $1 did not exit 0 on stdin EOF" >&2; exit 1; }
}
