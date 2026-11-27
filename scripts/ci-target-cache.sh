#!/usr/bin/env bash
# Pruned Cargo target/ cache backed by Wasabi S3 (Linux & macOS CI runners).
#
# Why this exists alongside sccache:
#   sccache caches Rust `.rlib` crates (~98% hit rate) and C/C++ objects via
#   CMAKE_*_COMPILER_LAUNCHER, but it cannot cache proc-macro crates (~155) or
#   binary crates / build.rs scripts (~288) on an empty target/. Restoring a
#   pruned target/ (.fingerprint/, build/, deps/, plus mlx-metal/ on macOS)
#   lets Cargo treat external deps, proc-macros, and CMake build/ outputs as
#   Fresh on disk while still recompiling changed workspace crates.
#
# Cargo mtime safety note:
#   - actions/checkout gives workspace source files current timestamps (newer
#     than restored target/ artifacts), so workspace crates always rebuild cleanly.
#   - Cargo 1.69+ treats $CARGO_HOME/{registry,git} as immutable for both Rust
#     compilation and `cargo:rerun-if-changed` paths inside $CARGO_HOME/{registry,git},
#     so restoring ~/.cargo/registry/cache + ~/.cargo/git/db via actions/cache
#     does not invalidate external crates in the restored target/.
#
# Keying & storage strategy:
#   - RUSTC_KEY = first 12 hex chars of sha256(`rustc --version --verbose`)
#     so a Rust compiler upgrade never restores an incompatible target/.
#   - LOCK_HASH = first 16 hex chars of sha256(Cargo.lock).
#   - Restore probes exact `${RUSTC_KEY}-${LOCK_HASH}.tar.zst` first, then falls
#     back to `${RUSTC_KEY}-latest.tar.zst` (so version bumps or small lockfile
#     diffs still get ~99% Fresh crates).
#   - Save uploads a single `${RUSTC_KEY}-latest.tar.zst` object per platform
#     and rustc fingerprint (1 streaming upload, zero temp disk overhead, and no
#     unbounded Cargo.lock-keyed object bloat / Wasabi 90-day storage fees).
#
# Usage:
#   scripts/ci-target-cache.sh restore <platform> <profile-dir> [<profile-dir> ...]
#   scripts/ci-target-cache.sh save    <platform> <profile-dir> [<profile-dir> ...]
#   scripts/ci-target-cache.sh pack    <out-archive.tar.zst> <profile-dir> [<profile-dir> ...]
#
# All restore/save failures are 100% non-fatal (log ::warning:: and exit 0).
set -uo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <restore|save|pack> <platform|out-archive> <profile-dir> [<profile-dir> ...]" >&2
  exit 2
fi

ACTION="$1"
TARGET_ARG="$2"
shift 2

ENDPOINT="${WASABI_S3_ENDPOINT:-https://s3.us-central-1.wasabisys.com}"
ENDPOINT="${ENDPOINT%/}"
REGION="${WASABI_S3_REGION:-us-central-1}"
BUCKET="${WASABI_CACHE_BUCKET:-theuth-build-cache}"
PREFIX="${WASABI_TARGET_CACHE_PREFIX:-target}"
PREFIX="${PREFIX#/}"
PREFIX="${PREFIX%/}"

ACCESS_KEY="${WASABI_ACCESS_KEY_ID:-${AWS_ACCESS_KEY_ID:-}}"
SECRET_KEY="${WASABI_SECRET_ACCESS_KEY:-${AWS_SECRET_ACCESS_KEY:-}}"

# Exclusions applied inside the whitelisted subdirectories (.fingerprint/, build/, deps/, mlx-metal/).
# Note: *.exe is intentionally NOT excluded so Windows build/**/build-script-build*.exe are preserved;
# top-level binaries (t, theuth*, desktop*) and top-level dirs (dist/, bundle/, incremental/,
# .microsandbox-runtime-cache/, onnxruntime-*) are already excluded by only archiving the whitelisted subdirs.
TAR_EXCLUDES=(
  --exclude='*.pdb'
  --exclude='*.dSYM'
  --exclude='*.dmg'
  --exclude='*.AppImage'
  --exclude='*.deb'
  --exclude='*.rpm'
  --exclude='*.msi'
  --exclude='*.ipa'
  --exclude='*.xcarchive'
)

sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | awk '{print $1}'
  else
    echo "::warning::Neither sha256sum nor shasum found" >&2
    return 1
  fi
}

wasabi_aws() {
  AWS_ACCESS_KEY_ID="$ACCESS_KEY" \
  AWS_SECRET_ACCESS_KEY="$SECRET_KEY" \
  AWS_DEFAULT_REGION="$REGION" \
  AWS_EC2_METADATA_DISABLED=true \
  AWS_REQUEST_CHECKSUM_CALCULATION=when_required \
  AWS_RESPONSE_CHECKSUM_VALIDATION=when_required \
    aws --endpoint-url "$ENDPOINT" --region "$REGION" "$@"
}

head_object_size() {
  local key="$1"
  local size
  size="$(wasabi_aws s3api head-object \
    --bucket "$BUCKET" \
    --key "$key" \
    --query ContentLength \
    --output text 2>/dev/null || true)"
  if [[ "$size" =~ ^[0-9]+$ ]]; then
    printf '%s' "$size"
  fi
}

compute_keys() {
  local platform="$1"
  local rustc_ver
  rustc_ver="$(rustc --version --verbose)" || return 1
  RUSTC_KEY="$(printf '%s\n' "$rustc_ver" | sha256_stdin | cut -c1-12)"
  if [[ -f Cargo.lock ]]; then
    LOCK_HASH="$(sha256_stdin < Cargo.lock | cut -c1-16)"
  else
    LOCK_HASH="nolock"
  fi
  [[ -n "$RUSTC_KEY" && -n "$LOCK_HASH" ]] || return 1
  PRIMARY_KEY="${PREFIX}/${platform}/${RUSTC_KEY}-${LOCK_HASH}.tar.zst"
  FALLBACK_KEY="${PREFIX}/${platform}/${RUSTC_KEY}-latest.tar.zst"
}

check_s3_prereqs() {
  if [[ -z "$ACCESS_KEY" || -z "$SECRET_KEY" ]]; then
    echo "Skipping target/ cache ${ACTION}: WASABI_ACCESS_KEY_ID / WASABI_SECRET_ACCESS_KEY not set"
    return 1
  fi
  local tool
  for tool in aws zstd tar rustc; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "::warning::Skipping target/ cache ${ACTION}: required tool '${tool}' not found on PATH"
      return 1
    fi
  done
  return 0
}

collect_include_paths() {
  INCLUDE_PATHS=()
  local raw pdir sub candidate
  for raw in "$@"; do
    pdir="${raw%/}"
    # Whitelist strictly what Cargo needs (+ mlx-metal on macOS for mlx.metallib when mlx-sys is Fresh).
    # Top-level dist/, bundle/, incremental/, .microsandbox-runtime-cache/, companion/, onnxruntime-*,
    # and final top-level binaries (t, theuth*, desktop*) are never included.
    for sub in .fingerprint build deps mlx-metal; do
      candidate="${pdir}/${sub}"
      if [[ -d "$candidate" ]]; then
        INCLUDE_PATHS+=("$candidate")
      fi
    done
  done
}

has_nonempty_deps() {
  local raw pdir
  for raw in "$@"; do
    pdir="${raw%/}"
    if [[ -d "${pdir}/deps" ]] && [[ -n "$(ls -A "${pdir}/deps" 2>/dev/null)" ]]; then
      return 0
    fi
  done
  return 1
}

tar_pack_stream() {
  # On macOS, flush disk cache before bsdtar reads newly linked .dylib/proc-macro binaries
  # to avoid the SEEK_HOLE sparse-file corruption bug (rust-lang/cargo#8603 / actions/cache#403).
  if [[ "$(uname -s)" == "Darwin" ]]; then
    sudo -n /usr/sbin/purge 2>/dev/null || true
    COPYFILE_DISABLE=1 tar "${TAR_EXCLUDES[@]}" -cf - "$@"
  else
    tar "${TAR_EXCLUDES[@]}" -cf - "$@"
  fi
}

run_restore() {
  local platform="$1"
  shift
  check_s3_prereqs || return 0
  compute_keys "$platform" || return 1

  local selected_key="" size="" kind=""
  size="$(head_object_size "$PRIMARY_KEY")"
  if [[ -n "$size" ]]; then
    selected_key="$PRIMARY_KEY"
    kind="exact"
  else
    size="$(head_object_size "$FALLBACK_KEY")"
    if [[ -n "$size" ]]; then
      selected_key="$FALLBACK_KEY"
      kind="latest fallback"
    fi
  fi

  if [[ -z "$selected_key" ]]; then
    echo "Wasabi target/ cache miss for ${platform} (rustc=${RUSTC_KEY}, lock=${LOCK_HASH})"
    return 0
  fi

  local uri="s3://${BUCKET}/${selected_key}"
  local size_mib=$(( (size + 524288) / 1048576 ))
  echo "Restoring target/ cache (${kind}, ~${size_mib} MiB) from ${uri} ..."
  SECONDS=0
  if wasabi_aws s3 cp "$uri" - --only-show-errors --cli-connect-timeout 30 --cli-read-timeout 300 \
      | zstd -dc -q \
      | tar -xf -; then
    echo "Restored target/ cache from ${uri} in ${SECONDS}s"
    return 0
  fi

  echo "::warning::Failed to restore target/ cache from ${uri}; cleaning partial cache dirs and continuing with cold build"
  local raw pdir
  for raw in "$@"; do
    pdir="${raw%/}"
    rm -rf "${pdir}/.fingerprint" "${pdir}/build" "${pdir}/deps" "${pdir}/mlx-metal" 2>/dev/null || true
  done
  return 0
}

run_save() {
  local platform="$1"
  shift
  check_s3_prereqs || return 0
  if ! has_nonempty_deps "$@"; then
    echo "Skipping target/ cache save for ${platform}: no non-empty deps/ directory under: $*"
    return 0
  fi

  compute_keys "$platform" || return 1
  collect_include_paths "$@"
  if [[ ${#INCLUDE_PATHS[@]} -eq 0 ]]; then
    echo "Skipping target/ cache save for ${platform}: no cacheable subdirectories found"
    return 0
  fi

  local latest_uri="s3://${BUCKET}/${FALLBACK_KEY}"
  echo "Saving pruned target/ cache (${INCLUDE_PATHS[*]}) to ${latest_uri} ..."
  SECONDS=0
  if tar_pack_stream "${INCLUDE_PATHS[@]}" \
      | zstd -T0 -3 -q \
      | wasabi_aws s3 cp - "$latest_uri" --only-show-errors --cli-connect-timeout 30 --cli-read-timeout 300; then
    local saved_size
    saved_size="$(head_object_size "$FALLBACK_KEY")"
    if [[ -n "$saved_size" ]]; then
      local saved_mib=$(( (saved_size + 524288) / 1048576 ))
      echo "Saved target/ cache (~${saved_mib} MiB) to ${latest_uri} in ${SECONDS}s"
    else
      echo "Saved target/ cache to ${latest_uri} in ${SECONDS}s"
    fi
    return 0
  fi

  echo "::warning::Failed to upload pruned target/ cache to ${latest_uri} (non-fatal)"
  return 0
}

run_pack() {
  local out_archive="$1"
  shift
  if ! has_nonempty_deps "$@"; then
    echo "error: no non-empty deps/ directory under: $*" >&2
    return 1
  fi
  collect_include_paths "$@"
  if [[ ${#INCLUDE_PATHS[@]} -eq 0 ]]; then
    echo "error: no cacheable subdirectories found under: $*" >&2
    return 1
  fi
  tar_pack_stream "${INCLUDE_PATHS[@]}" | zstd -T0 -3 -q -f -o "$out_archive"
}

case "$ACTION" in
  restore)
    if ! run_restore "$TARGET_ARG" "$@"; then
      echo "::warning::target/ cache restore failed unexpectedly (non-fatal); continuing with cold build"
    fi
    exit 0
    ;;
  save)
    if ! run_save "$TARGET_ARG" "$@"; then
      echo "::warning::target/ cache save failed unexpectedly (non-fatal)"
    fi
    exit 0
    ;;
  pack)
    run_pack "$TARGET_ARG" "$@"
    ;;
  *)
    echo "error: unknown action '${ACTION}' (expected restore, save, or pack)" >&2
    exit 2
    ;;
esac
