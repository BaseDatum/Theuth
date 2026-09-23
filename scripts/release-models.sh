#!/usr/bin/env bash
# Publish the model files the sidecar loads from <data_dir>/models/ to the release bucket
# (43 §4.6, 45 §4.6, 54 §9, meetings 04 §0.1, 57 §2), the ONNX Runtime they run on (57 §6 step 2c)
# and its WebGPU plugin EP (58 step 13), one copy per platform, from one machine of any OS. The
# desktop app fetches what is missing at start (crates/desktop/src/models.rs), the runtime and
# plugin only for its own platform.
#
#   fetch       scripts/fetch-models.sh: every REQUIRED file missing under $MODELS_DIR is downloaded
#               from its documented origin (HF_TOKEN for gated repos), else from the already-published
#               bucket copy (checked against the bucket's models.json); a file found nowhere stops the
#               run and names itself. Local files are never re-downloaded.
#   upload      each REQUIRED file → s3://$BUCKET/$PREFIX/models/<model>/<file>
#               (every upload is skipped when the object is already there with the same size)
#   runtime     for each PLATFORMS entry: Microsoft's release archive (tgz; zip for win-*; cached in
#               target/, sha256-checked against scripts/ort-version.env) → the bare dylib/so/dll →
#               s3://…/models/onnxruntime/<rust target triple>/<lib>, plus VERSION
#   plugins     the WebGPU plugin EP nupkg (Microsoft.ML.OnnxRuntime.EP.WebGpu, same cache and pin) →
#               runtimes/<rid>/native/* → s3://…/models/onnxruntime/<triple>/plugins/<file>
#               for win-x64 (three DLLs) and linux-*; macOS's runtime has WebGPU built in (no plugin);
#               win-arm64 is in the nupkg but we ship no such build
#   verify      anonymous HEAD of every URL: 200 and Content-Length == local size
#   models.json {files: [{path, size, sha256, platform?, key?}]} — written last.
#               `platform` = rust target triple the entry is for (absent: every platform);
#               `key` = remote key relative to models/ when it differs from `path`
#               (the runtime is stored per platform but lands at <data_dir>/models/<path>).
#
# The .app does not need the bucket copy: package-macos.sh bundles the same dylib into
# Contents/Frameworks, which `models::onnx::ort_lib` tries first.
#
# Env (repo-root .env.signing is loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   THEUTH_MODELS_DIR (or MODELS_DIR)   source (default: ${THEUTH_DATA_DIR:-~/.theuth}/models)
#   THEUTH_S3_ENDPOINT / THEUTH_S3_BUCKET / THEUTH_S3_PREFIX / THEUTH_S3_PUBLIC_BASE_URL  as release-s3.sh
#   THEUTH_S3_DRY_RUN=1 (or DRY_RUN=1)  print every fetch and every `aws s3 cp`; download and write nothing
#   HF_TOKEN                            Hugging Face token for gated repos (kyutai/pocket-tts)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
# shellcheck source=ort-version.env
source "$ROOT/scripts/ort-version.env"
# shellcheck source=fetch-models.sh
source "$ROOT/scripts/fetch-models.sh"   # REQUIRED, origin_of, fetch_missing, MODELS_DIR, DRY_RUN, size_of

need() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 required${2:+ ($2)}" >&2; exit 1; }; }
need aws "brew install awscli"
need jq
need curl
need shasum
need unzip
[[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] \
  || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset (see .env.signing.example)" >&2; exit 1; }

# Runtime platforms: Microsoft's archive name → rust target triple, archive kind and member, on-disk
# name, sha256 variable in ort-version.env, and the nupkg rid whose plugin files go beside it.
PLATFORMS=(osx-arm64 linux-x64 linux-aarch64 win-x64)
triple_of()  { case "$1" in osx-arm64) echo aarch64-apple-darwin ;; linux-x64) echo x86_64-unknown-linux-gnu ;; linux-aarch64) echo aarch64-unknown-linux-gnu ;; win-x64) echo x86_64-pc-windows-msvc ;; *) echo "error: unknown platform $1" >&2; exit 1 ;; esac; }
lib_of()     { case "$1" in osx-*) echo libonnxruntime.dylib ;; linux-*) echo libonnxruntime.so ;; win-*) echo onnxruntime.dll ;; esac; }
ext_of()     { case "$1" in win-*) echo zip ;; *) echo tgz ;; esac; }
member_of()  { case "$1" in osx-*) echo "onnxruntime-$1-${ORT_VERSION}/lib/libonnxruntime.${ORT_VERSION}.dylib" ;; linux-*) echo "onnxruntime-$1-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION}" ;; win-*) echo "onnxruntime-$1-${ORT_VERSION}/lib/onnxruntime.dll" ;; esac; }
sha_of()     { case "$1" in osx-arm64) echo "$ORT_SHA256_OSX_ARM64" ;; linux-x64) echo "$ORT_SHA256_LINUX_X64" ;; linux-aarch64) echo "$ORT_SHA256_LINUX_AARCH64" ;; win-x64) echo "$ORT_SHA256_WIN_X64" ;; esac; }
plugin_rid_of()   { case "$1" in linux-x64) echo linux-x64 ;; linux-aarch64) echo linux-arm64 ;; win-x64) echo win-x64 ;; *) echo "" ;; esac; }
plugin_files_of() { case "$1" in win-*) echo "onnxruntime_providers_webgpu.dll dxcompiler.dll dxil.dll" ;; linux-*) echo libonnxruntime_providers_webgpu.so ;; esac; }
ORT_URL="https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}"
NUPKG="microsoft.ml.onnxruntime.ep.webgpu.${WEBGPU_EP_VERSION}.nupkg"
NUPKG_URL="https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.ep.webgpu/${WEBGPU_EP_VERSION}/${NUPKG}"

# ENDPOINT / BUCKET / PREFIX / PUBLIC_BASE / PUB_MODELS come from fetch-models.sh (same env, same defaults).
S3_MODELS="s3://${BUCKET}/${PREFIX}/models"

# rustfs: path-style, no mandatory checksums, no IMDS probe (release-s3.sh).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true
AWS=(aws --endpoint-url "$ENDPOINT")

fetch_missing
for f in "${REQUIRED[@]}"; do
  [[ -s "$MODELS_DIR/$f" || "$DRY_RUN" == "1" ]] || { echo "error: $MODELS_DIR/$f missing" >&2; exit 1; }
done

echo "==> models from $MODELS_DIR → $S3_MODELS/ (public $PUB_MODELS/, dry run: $DRY_RUN)"

remote_size() { "${AWS[@]}" s3api head-object --bucket "$BUCKET" --key "$PREFIX/models/$1" --query ContentLength --output text 2>/dev/null || true; }
s3_cp() { # src key content-type
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 cp $1 $S3_MODELS/$2"; return 0; fi
  "${AWS[@]}" s3 cp "$1" "$S3_MODELS/$2" --content-type "$3" >/dev/null
}

MANIFEST='{"files":[]}'
PUBLISHED=()   # key:local, for verify
# Upload $1 as models/$2 unless already there with the same size, add its manifest row
# (path $3, platform $4 — empty: every platform). A file not on disk is only allowed in a dry run
# (it would be fetched first): the cp line is printed, the row skipped.
publish() { # local key path platform
  local size
  if [[ ! -s "$1" ]]; then
    [[ "$DRY_RUN" == "1" ]] || { echo "error: $1 missing" >&2; exit 1; }
    echo "    $2: not on disk yet — DRY_RUN: aws s3 cp $1 $S3_MODELS/$2 (manifest row omitted)"; return 0
  fi
  size="$(size_of "$1")"
  if [[ "$(remote_size "$2")" == "$size" ]]; then
    echo "    $2: already uploaded ($size bytes)"
  else
    echo "    $2: upload ($size bytes)"
    s3_cp "$1" "$2" application/octet-stream
  fi
  local extra='{}'
  [[ "$2" != "$3" ]] && extra="$(jq --arg k "$2" '. + {key: $k}' <<<"$extra")"
  [[ -n "$4" ]] && extra="$(jq --arg p "$4" '. + {platform: $p}' <<<"$extra")"
  MANIFEST="$(jq --arg p "$3" --argjson s "$size" --arg h "$(shasum -a 256 "$1" | awk '{print $1}')" --argjson x "$extra" \
    '.files += [{path: $p, size: $s, sha256: $h} + $x]' <<<"$MANIFEST")"
  PUBLISHED+=("$2:$1")
}
for f in "${REQUIRED[@]}"; do
  publish "$MODELS_DIR/$f" "$f" "$f" ""
done

# Microsoft's archive $1 (file name) from $2 (URL) into target/, sha256 $3; in a dry run a missing
# archive is only announced. Prints the cached path, or nothing when it is not there.
cached_archive() {
  local file="$ROOT/target/$1"
  if [[ ! -f "$file" ]] || ! echo "$3  $file" | shasum -a 256 -c --status; then
    if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: fetch $1 ← $2" >&2; return 0; fi
    echo "    fetching $1" >&2
    curl -sSfL -o "$file.part" "$2" || { echo "error: $2: download failed" >&2; exit 1; }
    echo "$3  $file.part" | shasum -a 256 -c --status || { echo "error: $1 sha256 mismatch (scripts/ort-version.env)" >&2; exit 1; }
    mv "$file.part" "$file"
  fi
  echo "$file"
}
# Member $2 of archive $1 (tgz or zip) to stdout.
extract() { case "$1" in *.tgz) tar -xzf "$1" -O "$2" ;; *) unzip -p "$1" "$2" ;; esac; }

echo "==> ONNX Runtime ${ORT_VERSION} + WebGPU EP ${WEBGPU_EP_VERSION} → $S3_MODELS/onnxruntime/<triple>/"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$ROOT/target"
nupkg="$(cached_archive "$NUPKG" "$NUPKG_URL" "$WEBGPU_EP_SHA256")"
for plat in "${PLATFORMS[@]}"; do
  triple="$(triple_of "$plat")"; lib="$(lib_of "$plat")"; sha="$(sha_of "$plat")"; ext="$(ext_of "$plat")"
  [[ -n "$sha" ]] || { echo "error: no sha256 for $plat in scripts/ort-version.env" >&2; exit 1; }
  archive="$(cached_archive "onnxruntime-${plat}-${ORT_VERSION}.${ext}" "$ORT_URL/onnxruntime-${plat}-${ORT_VERSION}.${ext}" "$sha")"
  mkdir -p "$WORK/$triple/plugins"
  if [[ -n "$archive" ]]; then
    extract "$archive" "$(member_of "$plat")" > "$WORK/$triple/$lib"
    printf '%s\n' "$ORT_VERSION" > "$WORK/$triple/VERSION"
  fi
  for name in "$lib" VERSION; do
    publish "$WORK/$triple/$name" "onnxruntime/$triple/$name" "onnxruntime/$name" "$triple"
  done
  rid="$(plugin_rid_of "$plat")"
  [[ -n "$rid" ]] || continue
  for name in $(plugin_files_of "$plat"); do
    [[ -n "$nupkg" ]] && unzip -p "$nupkg" "runtimes/$rid/native/$name" > "$WORK/$triple/plugins/$name"
    publish "$WORK/$triple/plugins/$name" "onnxruntime/$triple/plugins/$name" "onnxruntime/plugins/$name" "$triple"
  done
done

echo "==> verify via $PUB_MODELS/"
if [[ "$DRY_RUN" == "1" ]]; then
  echo "    DRY_RUN: nothing to verify"
else
  for entry in "${PUBLISHED[@]}"; do
    key="${entry%%:*}"; local_file="${entry#*:}"
    got="$(curl -fsSIL --retry 3 "$PUB_MODELS/$key" | tr -d '\r' | awk 'tolower($1)=="content-length:" {print $2}' | tail -n1)"
    want="$(size_of "$local_file")"
    [[ "$got" == "$want" ]] || { echo "error: $key: public Content-Length '$got' != $want (policy or upload)" >&2; exit 1; }
    echo "    $key ok"
  done
fi

echo "==> models.json (${#PUBLISHED[@]} rows)"
jq . <<<"$MANIFEST" | tee "$WORK/models.json"
s3_cp "$WORK/models.json" models.json application/json
echo "==> done: $PUB_MODELS/models.json"
