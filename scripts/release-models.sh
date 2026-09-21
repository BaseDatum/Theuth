#!/usr/bin/env bash
# Publish the model files the sidecar loads from <data_dir>/models/ to the release bucket
# (43 §4.6, 45 §4.6, 54 §9, meetings 04 §0.1, 57 §2), and the ONNX Runtime they run on
# (57 §6 step 2c), one copy per platform. The desktop app fetches what is missing at start
# (crates/desktop/src/models.rs), the runtime only for its own platform.
#
#   upload      each REQUIRED file → s3://$BUCKET/$PREFIX/models/<model>/<file>
#               (skipped when the object is already there with the same size)
#   runtime     for each PLATFORMS entry: Microsoft's release archive (cached in target/,
#               sha256-checked against scripts/ort-version.env) → the bare dylib →
#               s3://…/models/onnxruntime/<rust target triple>/<lib>, plus VERSION
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
#   THEUTH_MODELS_DIR           source (default: ${THEUTH_DATA_DIR:-~/.theuth}/models)
#   THEUTH_S3_ENDPOINT / THEUTH_S3_BUCKET / THEUTH_S3_PREFIX / THEUTH_S3_PUBLIC_BASE_URL  as release-s3.sh
#   THEUTH_S3_DRY_RUN=1         no writes
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
# shellcheck source=ort-version.env
source "$ROOT/scripts/ort-version.env"

need() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 required${2:+ ($2)}" >&2; exit 1; }; }
need aws "brew install awscli"
need jq
need curl
need shasum
[[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] \
  || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset (see .env.signing.example)" >&2; exit 1; }

# The files each loader opens; nothing optional (config.json, nemo128.onnx, READMEs, HF caches).
REQUIRED=(
  pocket-tts/tts_b6369a24.safetensors              # voice/src/model.rs WEIGHTS
  pocket-tts/tokenizer.model                       # voice/src/model.rs TOKENIZERS
  pocket-tts/voices/peter_yearsley.safetensors     # voice/src/model.rs VOICES_DIR
  parakeet-tdt/encoder-model.onnx                  # voice/src/stt.rs Engine::load
  parakeet-tdt/decoder_joint-model.onnx
  parakeet-tdt/vocab.txt
  nemotron-streaming-en/encoder.onnx               # voice/src/stt.rs Stream::open
  nemotron-streaming-en/decoder_joint.onnx
  nemotron-streaming-en/tokenizer.model
  pyannote/segmentation-3.0.onnx                   # meeting/src/post/diarize.rs
  pyannote/wespeaker_en_voxceleb_CAM++.onnx
  decision/model.safetensors                       # decision/src/engine.rs MODEL_FILES (57 §12.8): the Hugging Face
  decision/encoder/config.json                     # convaiinnovations/laya root layout, loaded by candle/mod.rs Loaded::open
  decision/tokenizer/tokenizer.json
  decision/rl_agent_config.json
)

# Runtime platforms: Microsoft's archive name → rust target triple, archive member, on-disk name,
# and the sha256 variable in ort-version.env. Design 58 adds linux-x64 / win-x64 rows.
PLATFORMS=(osx-arm64)
triple_of()  { case "$1" in osx-arm64) echo aarch64-apple-darwin ;; linux-x64) echo x86_64-unknown-linux-gnu ;; win-x64) echo x86_64-pc-windows-msvc ;; *) echo "error: unknown platform $1" >&2; exit 1 ;; esac; }
lib_of()     { case "$1" in osx-*) echo libonnxruntime.dylib ;; linux-*) echo libonnxruntime.so ;; win-*) echo onnxruntime.dll ;; esac; }
member_of()  { case "$1" in osx-*) echo "onnxruntime-$1-${ORT_VERSION}/lib/libonnxruntime.${ORT_VERSION}.dylib" ;; linux-*) echo "onnxruntime-$1-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION}" ;; win-*) echo "onnxruntime-$1-${ORT_VERSION}/lib/onnxruntime.dll" ;; esac; }
sha_of()     { case "$1" in osx-arm64) echo "$ORT_SHA256_OSX_ARM64" ;; linux-x64) echo "${ORT_SHA256_LINUX_X64:-}" ;; win-x64) echo "${ORT_SHA256_WIN_X64:-}" ;; esac; }

MODELS_DIR="${THEUTH_MODELS_DIR:-${THEUTH_DATA_DIR:-$HOME/.theuth}/models}"
ENDPOINT="${THEUTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${THEUTH_S3_BUCKET:-theuth}"
PREFIX="${THEUTH_S3_PREFIX:-releases}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
PUBLIC_BASE="${THEUTH_S3_PUBLIC_BASE_URL:-${ENDPOINT}/${BUCKET}/${PREFIX}}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-0}"
S3_MODELS="s3://${BUCKET}/${PREFIX}/models"
PUB_MODELS="${PUBLIC_BASE}/models"

# rustfs: path-style, no mandatory checksums, no IMDS probe (release-s3.sh).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true
AWS=(aws --endpoint-url "$ENDPOINT")

for f in "${REQUIRED[@]}"; do
  [[ -s "$MODELS_DIR/$f" ]] || { echo "error: $MODELS_DIR/$f missing" >&2; exit 1; }
done

echo "==> models from $MODELS_DIR → $S3_MODELS/ (public $PUB_MODELS/, dry run: $DRY_RUN)"

size_of() { stat -f %z "$1" 2>/dev/null || stat -c %s "$1"; }
remote_size() { "${AWS[@]}" s3api head-object --bucket "$BUCKET" --key "$PREFIX/models/$1" --query ContentLength --output text 2>/dev/null || true; }
s3_cp() { # src key content-type
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 cp $1 $S3_MODELS/$2"; return 0; fi
  "${AWS[@]}" s3 cp "$1" "$S3_MODELS/$2" --content-type "$3" >/dev/null
}

MANIFEST='{"files":[]}'
# path key platform size sha
manifest_add() {
  local extra='{}'
  [[ "$2" != "$1" ]] && extra="$(jq --arg k "$2" '. + {key: $k}' <<<"$extra")"
  [[ -n "$3" ]] && extra="$(jq --arg p "$3" '. + {platform: $p}' <<<"$extra")"
  MANIFEST="$(jq --arg p "$1" --argjson s "$4" --arg h "$5" --argjson x "$extra" '.files += [{path: $p, size: $s, sha256: $h} + $x]' <<<"$MANIFEST")"
}
# local remote-key content-type
upload() {
  local size; size="$(size_of "$1")"
  if [[ "$(remote_size "$2")" == "$size" ]]; then
    echo "    $2: already uploaded ($size bytes)"
  else
    echo "    $2: upload ($size bytes)"
    s3_cp "$1" "$2" "$3"
  fi
}
for f in "${REQUIRED[@]}"; do
  src="$MODELS_DIR/$f"
  upload "$src" "$f" application/octet-stream
  manifest_add "$f" "$f" "" "$(size_of "$src")" "$(shasum -a 256 "$src" | awk '{print $1}')"
done

echo "==> ONNX Runtime ${ORT_VERSION} → $S3_MODELS/onnxruntime/<triple>/"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$ROOT/target"
RUNTIME_KEYS=()
for plat in "${PLATFORMS[@]}"; do
  triple="$(triple_of "$plat")"; lib="$(lib_of "$plat")"; sha="$(sha_of "$plat")"
  [[ -n "$sha" ]] || { echo "error: no sha256 for $plat in scripts/ort-version.env" >&2; exit 1; }
  tgz="$ROOT/target/onnxruntime-${plat}-${ORT_VERSION}.tgz"
  if [[ ! -f "$tgz" ]] || ! echo "$sha  $tgz" | shasum -a 256 -c --status; then
    echo "    fetching onnxruntime-${plat}-${ORT_VERSION}.tgz"
    curl -sSfL -o "$tgz" "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-${plat}-${ORT_VERSION}.tgz"
    echo "$sha  $tgz" | shasum -a 256 -c --status || { echo "error: onnxruntime-${plat} tarball sha256 mismatch" >&2; exit 1; }
  fi
  mkdir -p "$WORK/$triple"
  tar -xzf "$tgz" -O "$(member_of "$plat")" > "$WORK/$triple/$lib"
  printf '%s\n' "$ORT_VERSION" > "$WORK/$triple/VERSION"
  for name in "$lib" VERSION; do
    key="onnxruntime/$triple/$name"
    upload "$WORK/$triple/$name" "$key" application/octet-stream
    manifest_add "onnxruntime/$name" "$key" "$triple" "$(size_of "$WORK/$triple/$name")" "$(shasum -a 256 "$WORK/$triple/$name" | awk '{print $1}')"
    RUNTIME_KEYS+=("$key:$WORK/$triple/$name")
  done
done

echo "==> verify via $PUB_MODELS/"
if [[ "$DRY_RUN" == "1" ]]; then
  echo "    DRY_RUN: nothing to verify"
else
  for f in "${REQUIRED[@]}"; do
    got="$(curl -fsSIL --retry 3 "$PUB_MODELS/$f" | tr -d '\r' | awk 'tolower($1)=="content-length:" {print $2}' | tail -n1)"
    want="$(size_of "$MODELS_DIR/$f")"
    [[ "$got" == "$want" ]] || { echo "error: $f: public Content-Length '$got' != $want (policy or upload)" >&2; exit 1; }
    echo "    $f ok"
  done
  for entry in "${RUNTIME_KEYS[@]}"; do
    key="${entry%%:*}"; local_file="${entry#*:}"
    got="$(curl -fsSIL --retry 3 "$PUB_MODELS/$key" | tr -d '\r' | awk 'tolower($1)=="content-length:" {print $2}' | tail -n1)"
    want="$(size_of "$local_file")"
    [[ "$got" == "$want" ]] || { echo "error: $key: public Content-Length '$got' != $want (policy or upload)" >&2; exit 1; }
    echo "    $key ok"
  done
fi

echo "==> models.json"
jq . <<<"$MANIFEST" | tee "$WORK/models.json"
s3_cp "$WORK/models.json" models.json application/json
echo "==> done: $PUB_MODELS/models.json"
