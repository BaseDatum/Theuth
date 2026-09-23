#!/usr/bin/env bash
# Fetch every model file the daemon loads (the REQUIRED list — the same list release-models.sh
# publishes) that is missing or empty under <data_dir>/models/, from where each one was taken.
# "Latest" = the origin's current `main`; the log line records the revision / etag the origin
# reports. Files already on disk are never re-downloaded or overwritten. Downloads land as
# `<file>.part` and are renamed when complete. No AWS credentials involved.
#
#   scripts/fetch-models.sh              fetch what is missing (also `just fetch-models`)
#   release-models.sh sources this file for REQUIRED / fetch_missing and runs it first.
#
# Origins (43 §5, 45, 54 §7, meetings 04 §0.1, 57 §4.1 + §12.8):
#   hf:<owner>/<repo>/<path>   https://huggingface.co/<owner>/<repo>/resolve/main/<path>;
#                              HF_TOKEN (Authorization: Bearer) is sent when set — kyutai/pocket-tts is gated
#   https://…                  verbatim (pyannote-rs GitHub release assets)
#   none:<why>                 no documented download: the script names the file and stops
#
# Env:
#   THEUTH_MODELS_DIR / MODELS_DIR   destination (default: ${THEUTH_DATA_DIR:-~/.theuth}/models)
#   THEUTH_S3_DRY_RUN / DRY_RUN=1    print what would be fetched, download nothing
#   HF_TOKEN                         Hugging Face token for gated repos
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
  pyannote/segmentation-3.0.onnx                   # meeting/src/post/diarize.rs
  pyannote/wespeaker_en_voxceleb_CAM++.onnx
  decision/model.safetensors                       # decision/src/engine.rs MODEL_FILES (57 §12.8): the Hugging Face
  decision/encoder/config.json                     # convaiinnovations/laya root layout, loaded by candle/mod.rs Loaded::open
  decision/tokenizer/tokenizer.json
  decision/rl_agent_config.json
  decision-browser/model.safetensors               # decision.checkpoints.browser (57 §4.1 item 6): cklxx/laya-browser v10, same layout
  decision-browser/encoder/config.json             # (scripts/fetch-laya-browser.sh fetches the same four files + tokenizer_config.json)
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
    pyannote/*)               echo "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/${1#*/}" ;;
    decision/*)               echo "hf:convaiinnovations/laya/${1#*/}" ;;
    decision-browser/*)       echo "hf:cklxx/laya-browser/v10/${1#*/}" ;;
    *)                        echo "none:not in origin_of" ;;
  esac
}

MODELS_DIR="${THEUTH_MODELS_DIR:-${MODELS_DIR:-${THEUTH_DATA_DIR:-$HOME/.theuth}/models}}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-${DRY_RUN:-0}}"

size_of() { stat -f %z "$1" 2>/dev/null || stat -c %s "$1"; }

# Download $1 (a REQUIRED path) into $MODELS_DIR. Returns 1 when there is nothing to fetch it from.
fetch_one() {
  local f="$1" dst="$MODELS_DIR/$1" origin url auth=() hdr code rev etag
  origin="$(origin_of "$f")"
  case "$origin" in
    none:*) echo "    $f: MISSING, no known source (${origin#none:}); put it in $MODELS_DIR/$f" >&2; return 1 ;;
    hf:*)   local rest="${origin#hf:}" owner repo; owner="${rest%%/*}"; rest="${rest#*/}"; repo="${rest%%/*}"
            url="https://huggingface.co/$owner/$repo/resolve/main/${rest#*/}"
            [[ -n "${HF_TOKEN:-}" ]] && auth=(-H "Authorization: Bearer $HF_TOKEN") ;;
    *)      url="$origin" ;;
  esac
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: fetch $f ← $url"; return 0; fi
  echo "    $f: fetching $url"
  mkdir -p "$(dirname "$dst")"
  hdr="$dst.headers"
  code="$(curl -sSL --retry 3 --progress-bar -D "$hdr" -o "$dst.part" -w '%{http_code}' "${auth[@]}" "$url")" || code="curl failed"
  if [[ "$code" != 2* ]] || [[ ! -s "$dst.part" ]]; then
    local why=""
    case "$code" in
      401|403) why=" — gated repo (set HF_TOKEN) or a proxy blocking the host"
               grep -qi zscaler "$dst.part" 2>/dev/null && why=" — blocked by the Zscaler proxy on this network" ;;
      404)     why=" — the file moved or was renamed at the origin; fix origin_of in scripts/fetch-models.sh" ;;
    esac
    rm -f "$dst.part" "$hdr"
    echo "error: $f: HTTP $code from $url$why" >&2
    return 1
  fi
  rev="$(tr -d '\r' <"$hdr" | awk 'tolower($1)=="x-repo-commit:" {r=$2} END {print r}')"
  etag="$(tr -d '\r' <"$hdr" | awk 'tolower($1)=="etag:" {e=$2} END {print e}')"
  rm -f "$hdr"
  mv "$dst.part" "$dst"
  echo "    $f: fetched $(size_of "$dst") bytes${rev:+ (rev $rev)}${etag:+ (etag $etag)}"
}

# Every REQUIRED file missing or empty under $MODELS_DIR → fetch_one. One failure stops the run.
fetch_missing() {
  echo "==> models missing under $MODELS_DIR (dry run: $DRY_RUN)"
  local f n=0 unsourced=()
  for f in "${REQUIRED[@]}"; do
    [[ -s "$MODELS_DIR/$f" ]] && continue
    n=$((n + 1))
    fetch_one "$f" || { [[ "$(origin_of "$f")" == none:* ]] && unsourced+=("$f") || exit 1; }
  done
  [[ $n -eq 0 ]] && echo "    all ${#REQUIRED[@]} files present"
  if [[ ${#unsourced[@]} -gt 0 && "$DRY_RUN" != "1" ]]; then
    echo "error: ${#unsourced[@]} file(s) have no documented source: ${unsourced[*]}" >&2
    exit 1
  fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  command -v curl >/dev/null 2>&1 || { echo "error: curl required" >&2; exit 1; }
  fetch_missing
fi
