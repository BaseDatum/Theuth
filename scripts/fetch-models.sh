#!/usr/bin/env bash
# Fetch every model file the daemon loads (the REQUIRED list — the same list release-models.sh
# publishes) that is missing or empty under <data_dir>/models/. Per file: local copy → documented
# origin → the already-published release bucket (the URL the desktop downloader uses; the copy is
# checked against the bucket's models.json size + sha256). "Latest" = the origin's current `main`;
# the log line records the revision / etag the origin reports. Files already on disk are never
# re-downloaded or overwritten. Downloads land as `<file>.part` and are renamed when complete. No
# AWS credentials involved.
#
#   scripts/fetch-models.sh              fetch what is missing (also `just fetch-models`)
#   release-models.sh sources this file for REQUIRED / fetch_missing / PUB_MODELS and runs it first.
#
# Origins (43 §5, 45, 54 §7, meetings 04 §0.1 + 04-post-nemotron-as-built, 57 §4.1 + §12.8):
#   hf:<owner>/<repo>/<path>   https://huggingface.co/<owner>/<repo>/resolve/main/<path>;
#                              HF_TOKEN (Authorization: Bearer) is sent when set — kyutai/pocket-tts is gated
#   https://…                  verbatim (pyannote-rs GitHub release assets: wespeaker)
#   none:<why>                 no documented download (parakeet-tdt's hub id was never recorded; the voice
#                              is generated): only the bucket copy
#
# Env:
#   THEUTH_MODELS_DIR / MODELS_DIR   destination (default: ${THEUTH_DATA_DIR:-~/.theuth}/models)
#   THEUTH_S3_DRY_RUN / DRY_RUN=1    print what would be fetched (origin, then bucket), download nothing
#   HF_TOKEN                         Hugging Face token for gated repos
#   THEUTH_S3_ENDPOINT / THEUTH_S3_BUCKET / THEUTH_S3_PREFIX / THEUTH_S3_PUBLIC_BASE_URL  bucket, as release-s3.sh
set -euo pipefail

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
  nemotron-diarization/model_quantized.onnx        # meeting/src/post/diarize.rs DIAR_MODEL (graph) …
  nemotron-diarization/model_quantized.onnx_data   # … and its weights (ONNX external data, must sit beside it)
  pyannote/wespeaker_en_voxceleb_CAM++.onnx        # meeting/src/post/diarize.rs EMBEDDING
  decision/model.safetensors                       # decision/src/engine.rs MODEL_FILES (57 §12.8): the Hugging Face
  decision/encoder/config.json                     # convaiinnovations/laya root layout, loaded by candle/mod.rs Loaded::open
  decision/tokenizer/tokenizer.json
  decision/rl_agent_config.json
  decision-browser/model.safetensors               # decision.checkpoints.browser (57 §4.1 item 6): cklxx/laya-browser v17s (mmBERT-base,
  decision-browser/encoder/config.json             # Gemma tokenizer; upstream removed v10 on 2026-09-27), same four-file layout
  decision-browser/tokenizer/tokenizer.json
  decision-browser/rl_agent_config.json
)

# Where each REQUIRED file comes from. Only origins a design doc records — never a guessed URL.
origin_of() {
  case "$1" in
    pocket-tts/voices/*)      echo "none:made locally — ptts create_voice from kyutai/tts-voices voice-zero/peter_yearsley.wav (43 §5)" ;;
    pocket-tts/*)             echo "hf:kyutai/pocket-tts/${1#pocket-tts/}" ;;
    parakeet-tdt/*)           echo "none:parakeet-tdt-0.6b-v3-int8-smoothquant (Olicorne's requantisation of istupakov's export, files renamed to parakeet-rs names) — hub id not recorded (45 §STT)" ;;
    nemotron-streaming-en/*)  echo "hf:lokkju/nemotron-speech-streaming-en-0.6b-int8/${1#*/}" ;;
    nemotron-diarization/*)   echo "hf:onnx-community/Nemotron-3-Diarization-ONNX/onnx/${1#*/}" ;;
    pyannote/*)               echo "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/${1#*/}" ;;
    decision/*)               echo "hf:convaiinnovations/laya/${1#*/}" ;;
    decision-browser/*)       echo "hf:cklxx/laya-browser/v17s/${1#*/}" ;;
    *)                        echo "none:not in origin_of" ;;
  esac
}

# shellcheck source=load-signing-env.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/load-signing-env.sh"   # THEUTH_S3_* overrides; no-op when already loaded
MODELS_DIR="${THEUTH_MODELS_DIR:-${MODELS_DIR:-${THEUTH_DATA_DIR:-$HOME/.theuth}/models}}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-${DRY_RUN:-0}}"
ENDPOINT="${THEUTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${THEUTH_S3_BUCKET:-theuth}"
PREFIX="${THEUTH_S3_PREFIX:-releases}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
PUBLIC_BASE="${THEUTH_S3_PUBLIC_BASE_URL:-${ENDPOINT}/${BUCKET}/${PREFIX}}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
PUB_MODELS="${PUBLIC_BASE}/models"

size_of() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; } # GNU first: BSD `-f %z` is a valid *filesystem* query on GNU and returns garbage

# Download $2 to $1.part (headers to $1.headers), extra curl args after. Prints one line and
# returns 1 on any HTTP/transport failure, leaving nothing behind.
download() { # dst url [curl args...]
  local dst="$1" url="$2" code why=""; shift 2
  mkdir -p "$(dirname "$dst")"
  code="$(curl -sSL --retry 3 --progress-bar -D "$dst.headers" -o "$dst.part" -w '%{http_code}' "$@" "$url")" || code="curl failed"
  [[ "$code" == 2* && -s "$dst.part" ]] && return 0
  case "$code:$url" in
    40[13]:*huggingface.co/*) why=" — gated repo (set HF_TOKEN) or a proxy blocking the host" ;;
    40[13]:*)                 why=" — not published there (or not public)" ;;
    404:*)                    why=" — the file moved or was renamed at the origin; fix origin_of in scripts/fetch-models.sh" ;;
  esac
  grep -qi zscaler "$dst.part" 2>/dev/null && why=" — blocked by the Zscaler proxy on this network"
  rm -f "$dst.part" "$dst.headers"
  echo "    HTTP $code from $url$why"
  return 1
}
header() { tr -d '\r' <"$1" | awk -v k="$2" 'tolower($1)==k {v=$2} END {print v}'; }

# The bucket's models.json, fetched once (empty list when unreachable).
BUCKET_MANIFEST=""
bucket_manifest() {
  [[ -n "$BUCKET_MANIFEST" ]] || BUCKET_MANIFEST="$(curl -fsSL --retry 3 "$PUB_MODELS/models.json" 2>/dev/null || echo '{"files":[]}')"
}
# "size sha256" of $1 as the bucket lists it ("" when not listed).
bucket_row() { jq -r --arg p "$1" '.files[] | select(.path == $p and (.platform | not)) | "\(.size) \(.sha256)"' <<<"$BUCKET_MANIFEST"; }

# Put $1 (a REQUIRED path) under $MODELS_DIR: documented origin first, then the release bucket.
fetch_one() {
  local f="$1" dst="$MODELS_DIR/$1" origin url="" auth=() fallback="$PUB_MODELS/$1" row
  origin="$(origin_of "$f")"
  case "$origin" in
    none:*) ;;
    hf:*)   local rest="${origin#hf:}" owner repo; owner="${rest%%/*}"; rest="${rest#*/}"; repo="${rest%%/*}"
            url="https://huggingface.co/$owner/$repo/resolve/main/${rest#*/}"
            [[ -n "${HF_TOKEN:-}" ]] && auth=(-H "Authorization: Bearer $HF_TOKEN") ;;
    *)      url="$origin" ;;
  esac
  if [[ "$DRY_RUN" == "1" ]]; then
    if [[ -n "$url" ]]; then echo "    DRY_RUN: fetch $f ← $url, else ← $fallback"
    else echo "    DRY_RUN: fetch $f ← $fallback (no documented origin: ${origin#none:})"; fi
    return 0
  fi
  if [[ -n "$url" ]]; then
    echo "    $f: fetching $url"
    if download "$dst" "$url" ${auth[@]+"${auth[@]}"}; then
      local rev etag
      rev="$(header "$dst.headers" x-repo-commit:)"; etag="$(header "$dst.headers" etag:)"
      rm -f "$dst.headers"; mv "$dst.part" "$dst"
      echo "    $f: fetched $(size_of "$dst") bytes${rev:+ (rev $rev)}${etag:+ (etag $etag)}"
      return 0
    fi
    echo "    $f: origin failed; trying the release bucket"
  else
    echo "    $f: no documented origin (${origin#none:}); trying the release bucket"
  fi
  download "$dst" "$fallback" || { echo "error: $f: not at its origin and not on the bucket; put it in $MODELS_DIR/$f" >&2; return 1; }
  rm -f "$dst.headers"
  bucket_manifest
  row="$(bucket_row "$f")"
  if [[ "$row" != "$(size_of "$dst.part") $(shasum -a 256 "$dst.part" | awk '{print $1}')" ]]; then
    rm -f "$dst.part"
    echo "error: $f: bucket copy does not match $PUB_MODELS/models.json (${row:-not listed})" >&2
    return 1
  fi
  mv "$dst.part" "$dst"
  echo "    $f: fetched $(size_of "$dst") bytes from the bucket (size + sha256 match models.json)"
}

# Every REQUIRED file missing or empty under $MODELS_DIR → fetch_one. The first failure stops the run.
fetch_missing() {
  echo "==> models missing under $MODELS_DIR (dry run: $DRY_RUN; bucket $PUB_MODELS/)"
  local f n=0
  for f in "${REQUIRED[@]}"; do
    [[ -s "$MODELS_DIR/$f" ]] && continue
    n=$((n + 1))
    fetch_one "$f" || exit 1
  done
  [[ $n -eq 0 ]] && echo "    all ${#REQUIRED[@]} files present"
  return 0
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  command -v curl >/dev/null 2>&1 || { echo "error: curl required" >&2; exit 1; }
  command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }
  fetch_missing
fi
