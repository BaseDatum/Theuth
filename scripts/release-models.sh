#!/usr/bin/env bash
# Publish the speech model files the sidecar loads from <data_dir>/models/ to the release
# bucket (43 §4.6, 45 §4.6, 54 §9, meetings 04 §0.1). The desktop app fetches what is missing
# at start (crates/desktop/src/models.rs).
#
#   upload      each REQUIRED file → s3://$BUCKET/$PREFIX/models/<model>/<file>
#               (skipped when the object is already there with the same size)
#   verify      anonymous HEAD of every URL: 200 and Content-Length == local size
#   models.json {files: [{path, size, sha256}]} — written last
#
# libonnxruntime is not a model: package-macos.sh ships it in Contents/Frameworks.
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
  decision/model.onnx                              # decision/src/laya.rs Engine::load (57 §6 step 2b)
  decision/tokenizer.json
  decision/rl_agent_config.json
)

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
for f in "${REQUIRED[@]}"; do
  src="$MODELS_DIR/$f"
  size="$(size_of "$src")"
  sha="$(shasum -a 256 "$src" | awk '{print $1}')"
  if [[ "$(remote_size "$f")" == "$size" ]]; then
    echo "    $f: already uploaded ($size bytes)"
  else
    echo "    $f: upload ($size bytes)"
    s3_cp "$src" "$f" application/octet-stream
  fi
  MANIFEST="$(jq --arg p "$f" --argjson s "$size" --arg h "$sha" '.files += [{path: $p, size: $s, sha256: $h}]' <<<"$MANIFEST")"
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
fi

echo "==> models.json"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
jq . <<<"$MANIFEST" | tee "$WORK/models.json"
s3_cp "$WORK/models.json" models.json application/json
echo "==> done: $PUB_MODELS/models.json"
