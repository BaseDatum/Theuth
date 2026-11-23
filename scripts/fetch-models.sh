#!/usr/bin/env bash
# Keep every model file the daemon loads (the REQUIRED list — the same list release-models.sh
# publishes) current under <data_dir>/models/. Per file: missing → documented origin, else the
# already-published release bucket (checked against the bucket's models.json size + sha256); present
# with an `hf:` origin → HEAD the origin and compare its `x-repo-commit` / etag / size with the local
# record, replace the file when the content changed. "Latest" = the origin's current `main`.
# Downloads land as `<file>.part`, are size- and hash-checked against the response headers (the
# `x-linked-etag` of an LFS file is its sha256; a plain file's etag is its git blob sha1) and renamed
# into place. No AWS credentials involved.
#
# Records: $MODELS_DIR/.origin/<path>.json {repo, path, revision, etag, size, sha256, fetched_at},
# written after every origin download. A file without a record (pre-existing install) is hashed and
# compared with the origin's etag; a match writes the record, a mismatch is stale. Bucket copies get
# no record (no origin identity) and are checked the same way next run. `https:` origins (GitHub
# release assets, pinned by URL) and `none:` files are never re-checked.
#
#   scripts/fetch-models.sh [--check] [--force <path|all>]     also `just fetch-models …`
#     --check          report only (`up to date` / `STALE` / `MISSING` / `cannot check`), write
#                      nothing; exit 2 when anything is stale, missing or unverifiable
#     --force <p|all>  re-download <REQUIRED path> (or every file with a URL origin) regardless
#   release-models.sh sources this file for REQUIRED / fetch_models / PUB_MODELS / origin_of /
#   record_field and runs fetch_models first; the flags are read from its command line.
#
# Per file the run prints one of: `new (rev …)`, `up to date (rev …)`, `updated rev a → b`,
# `cannot check … (kept)` (HEAD failed: gated 401 without HF_TOKEN, proxy 403, moved 404 — the local
# copy stays), `pinned` / `no documented origin`. A failed download stops the run (exit 1).
#
# Origins (43 §5, 45, 54 §7, meetings 04 §0.1 + 04-post-nemotron-as-built, 57 §4.1 + §12.8):
#   hf:<owner>/<repo>/<path>   $HF_BASE/<owner>/<repo>/resolve/main/<path>;
#                              HF_TOKEN (Authorization: Bearer) is sent when set — kyutai/pocket-tts is gated
#   https://…                  verbatim (pyannote-rs GitHub release assets: wespeaker)
#   repo:<path>                a file committed to this repository (<path> from the repo root): copied when missing,
#                              replaced when it differs from the checkout (our Pocket TTS config.json and voices, 43 §5)
#   none:<why>                 no documented download (the voice is generated): only the bucket copy
#
# Env:
#   THEUTH_MODELS_DIR / MODELS_DIR   destination (default: ${THEUTH_DATA_DIR:-~/.theuth}/models)
#   MODELS_DRY_RUN=1 (or THEUTH_S3_DRY_RUN / DRY_RUN)  HEAD the origins and print the plan; download nothing
#   HF_TOKEN                         Hugging Face token for gated repos
#   HF_BASE                          Hugging Face host (default https://huggingface.co; tests point it at a fake)
#   THEUTH_S3_ENDPOINT / THEUTH_S3_BUCKET / THEUTH_S3_PREFIX / THEUTH_S3_PUBLIC_BASE_URL  bucket, as release-s3.sh
set -euo pipefail

# The files each loader opens; nothing optional (config.json, nemo128.onnx, READMEs, HF caches).
REQUIRED=(
  pocket-tts-en-2026-10/config.json                # voice/src/model.rs locate (ptts Checkpoint::open, voice::TTS_MODEL_SUBDIR): ours,
  pocket-tts-en-2026-10/model.safetensors          # Kyutai's english_2026-10 weights + tokenizer from the ungated
  pocket-tts-en-2026-10/tokenizer.json             # kyutai/pocket-tts-without-voice-cloning (CC BY 4.0, 2026-10-21 @ a7056f50)
  pocket-tts-en-2026-10/voices/peter_yearsley.safetensors  # Kyutai's voice states as `emb` files
  pocket-tts-en-2026-10/voices/alba.safetensors            # (crates/voice/build-voices/from_state.py); alba = notebooks' second host
  pocket-tts/tts_b6369a24.safetensors              # remove after the next release: engines from earlier releases
  pocket-tts/tokenizer.model                       # read pocket-tts/ (ptts git 5d50b52, english_2026-01)
  pocket-tts/voices/peter_yearsley.safetensors     # remove after the next release
  parakeet-tdt/encoder-model.onnx                  # voice/src/stt.rs Engine::load; Olicorne/parakeet-tdt-0.6b-v3-optimized-onnx
  parakeet-tdt/decoder_joint-model.onnx            # istupakov_smoothquant/int8/*.int8.onnx: encoder @ c3638803, decoder @ 06d54e30
  parakeet-tdt/vocab.txt                           # (the <unk> ban, joint bias -1e4; 2026-10-06); vocab.txt from the repo root
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
  decision-browser/model.safetensors               # decision.checkpoints.browser (57 §4.1 item 6): cklxx/laya-browser v19s at the repo root (mmBERT-base,
  decision-browser/encoder/config.json             # Gemma tokenizer; upstream removed v10 on 2026-09-27), same four-file layout
  decision-browser/tokenizer/tokenizer.json
  decision-browser/rl_agent_config.json
  decision-gliner/model.safetensors                # decision.checkpoints.gliner { kind: gliner } (57 §GLiNER2.5-Decide checkpoint):
  decision-gliner/tokenizer.json                   # fastino/GLiNER2.5-Decide @ 5a7adf72, decision/src/engine.rs model_files(Kind::Gliner),
  decision-gliner/encoder_config/config.json       # loaded by gliner/candle.rs Loaded::open (the top-level config.json is not read)
)

# Where each REQUIRED file comes from. Only origins a design doc records — never a guessed URL.
origin_of() {
  case "$1" in
    pocket-tts-en-2026-10/config.json | pocket-tts-en-2026-10/voices/*)
                              echo "repo:crates/voice/data/$1" ;;
    pocket-tts-en-2026-10/*)  echo "hf:kyutai/pocket-tts-without-voice-cloning/languages/english_2026-10/${1#*/}" ;;
    pocket-tts/voices/*)      echo "none:made locally — ptts create_voice from kyutai/tts-voices voice-zero/peter_yearsley.wav (43 §5)" ;;
    pocket-tts/*)             echo "hf:kyutai/pocket-tts/${1#pocket-tts/}" ;;
    parakeet-tdt/vocab.txt)   echo "hf:Olicorne/parakeet-tdt-0.6b-v3-optimized-onnx/vocab.txt" ;;
    parakeet-tdt/*)           local f="${1#*/}"; echo "hf:Olicorne/parakeet-tdt-0.6b-v3-optimized-onnx/istupakov_smoothquant/int8/${f%.onnx}.int8.onnx" ;;   # upstream int8 names → parakeet-rs names
    nemotron-streaming-en/*)  echo "hf:lokkju/nemotron-speech-streaming-en-0.6b-int8/${1#*/}" ;;
    nemotron-diarization/*)   echo "hf:onnx-community/Nemotron-3-Diarization-ONNX/onnx/${1#*/}" ;;
    pyannote/*)               echo "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/${1#*/}" ;;
    decision/*)               echo "hf:convaiinnovations/laya/${1#*/}" ;;
    decision-browser/*)       echo "hf:cklxx/laya-browser/${1#*/}" ;;   # v19s lives at the repo root (2026-10-04)
    decision-gliner/*)        echo "hf:fastino/GLiNER2.5-Decide/${1#*/}" ;;
    *)                        echo "none:not in origin_of" ;;
  esac
}

# shellcheck source=load-signing-env.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/load-signing-env.sh"   # THEUTH_S3_* overrides; no-op when already loaded
# shellcheck source=lib/common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/common.sh"   # size_of
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # `repo:` origins
MODELS_DIR="${THEUTH_MODELS_DIR:-${MODELS_DIR:-${THEUTH_DATA_DIR:-$HOME/.theuth}/models}}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-${MODELS_DRY_RUN:-${DRY_RUN:-0}}}"
HF_BASE="${HF_BASE:-https://huggingface.co}"; HF_BASE="${HF_BASE%/}"
# shellcheck source=lib/s3-env.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/s3-env.sh"   # ENDPOINT
BUCKET="${THEUTH_S3_BUCKET:-theuth}"
PREFIX="${THEUTH_S3_PREFIX:-releases}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
PUBLIC_BASE="${THEUTH_S3_PUBLIC_BASE_URL:-${ENDPOINT}/${BUCKET}/${PREFIX}}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
PUB_MODELS="${PUBLIC_BASE}/models"

# Flags — from this script's or, when sourced, release-models.sh's command line.
CHECK=0; FORCE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --check) CHECK=1 ;;
    --force) FORCE="${2:?--force needs <REQUIRED path|all>}"; shift ;;
    *)       echo "usage: $0 [--check] [--force <path|all>]" >&2; exit 64 ;;
  esac
  shift
done

sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
blob_sha1_of() { { printf 'blob %s\0' "$(size_of "$1")"; cat "$1"; } | shasum -a 1 | awk '{print $1}'; }
# $1's identity in the form origin etag $2 uses: sha256 for an LFS file (64 hex), git blob sha1 for a
# plain one (40 hex), nothing when the etag is neither (no comparison possible).
local_etag_of() { case "${#2}" in 64) sha256_of "$1" ;; 40) blob_sha1_of "$1" ;; *) echo "" ;; esac; }
unquote() { local v="${1#W/}"; v="${v%\"}"; echo "${v#\"}"; }

# Header $2 (lowercase, with the colon) from a `curl -D` file holding every hop; the last hop wins.
header() { tr -d '\r' <"$1" | awk -v k="$2" 'tolower($1)==k {v=$2} END {print v}'; }
# The file's identity as the origin reports it. Only the huggingface.co hop carries
# `x-repo-commit` / `x-linked-*`; the CDN hop's `etag` is not the file's, hence the preference.
origin_rev()  { header "$1" x-repo-commit:; }
origin_etag() { local e; e="$(header "$1" x-linked-etag:)"; [[ -n "$e" ]] || e="$(header "$1" etag:)"; unquote "$e"; }
origin_size() { local s; s="$(header "$1" x-linked-size:)"; [[ -n "$s" ]] || s="$(header "$1" content-length:)"; echo "$s"; }

# Why HTTP $1 from $2 (body in $3 when there is one) — appended to the failure line.
explain() {
  if [[ -n "${3:-}" ]] && grep -qi zscaler "$3" 2>/dev/null; then echo " — blocked by the Zscaler proxy on this network"; return; fi
  local code; code="$(header "${4:-/dev/null}" x-error-code:)"
  case "$1:$2" in
    40[13]:"$HF_BASE/"*) echo " — gated repo (set HF_TOKEN) or a proxy blocking the host${code:+ (x-error-code: $code)}" ;;
    40[13]:*)            echo " — not published there (or not public)" ;;
    404:*)               echo " — the file moved or was renamed at the origin; fix origin_of in scripts/fetch-models.sh${code:+ (x-error-code: $code)}" ;;
  esac
}

# Download $2 to $1.part (headers of every hop to $1.headers), extra curl args after. Prints one line
# and returns 1 on any HTTP/transport failure, leaving nothing behind.
download() { # dst url [curl args...]
  local dst="$1" url="$2" code; shift 2
  mkdir -p "$(dirname "$dst")"
  code="$(curl -sSL --retry 3 --progress-bar -D "$dst.headers" -o "$dst.part" -w '%{http_code}' "$@" "$url")" || code="curl failed"
  [[ "$code" == 2* && -s "$dst.part" ]] && return 0
  echo "    HTTP $code from $url$(explain "$code" "$url" "$dst.part" "$dst.headers")"
  rm -f "$dst.part" "$dst.headers"
  return 1
}
probe() { # url headers_file [curl args...]
  local url="$1" hdr="$2" code; shift 2
  code="$(curl -sIL --retry 3 --max-time 60 -D "$hdr" -o /dev/null -w '%{http_code}' "$@" "$url")" || code="curl failed"
  [[ "$code" == 2* ]] && return 0
  if [[ "$code" == 403 ]]; then curl -s --max-time 15 -r 0-4095 -o "$hdr.body" "$@" "$url" || true; fi   # a proxy block page names itself
  echo "    HTTP $code from $url$(explain "$code" "$url" "$hdr.body" "$hdr")"
  rm -f "$hdr.body"
  return 1
}

# The bucket's models.json, fetched once (empty list when unreachable).
BUCKET_MANIFEST=""
bucket_manifest() {
  [[ -n "$BUCKET_MANIFEST" ]] || BUCKET_MANIFEST="$(curl -fsSL --retry 3 "$PUB_MODELS/models.json" 2>/dev/null || echo '{"files":[]}')"
}
# "size sha256" of $1 as the bucket lists it ("" when not listed).
bucket_row() { jq -r --arg p "$1" '.files[] | select(.path == $p and (.platform | not)) | "\(.size) \(.sha256)"' <<<"$BUCKET_MANIFEST"; }

# $MODELS_DIR/.origin/<path>.json — what the origin said when the local copy was fetched.
record_path() { echo "$MODELS_DIR/.origin/$1.json"; }
record_field() { jq -r ".$2 // empty" "$(record_path "$1")" 2>/dev/null || true; } # path field
write_record() { # path repo rpath revision etag size sha256
  local rec; rec="$(record_path "$1")"; mkdir -p "$(dirname "$rec")"
  jq -n --arg repo "$2" --arg path "$3" --arg rev "$4" --arg etag "$5" --argjson size "$6" --arg sha "$7" \
    --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{repo: $repo, path: $path, revision: $rev, etag: $etag, size: $size, sha256: $sha, fetched_at: $at}' >"$rec.tmp"
  mv "$rec.tmp" "$rec"
}

# $1's origin → ORIGIN (the origin_of string), ORIGIN_URL ("" for none:), ORIGIN_REPO / ORIGIN_RPATH
# (hf: owner/repo and the path inside it; the URL itself for https:), ORIGIN_AUTH (curl args).
resolve_origin() {
  local rest
  ORIGIN="$(origin_of "$1")"; ORIGIN_URL=""; ORIGIN_REPO=""; ORIGIN_RPATH=""; ORIGIN_AUTH=()
  case "$ORIGIN" in
    none:* | repo:*) ;;
    hf:*)   rest="${ORIGIN#hf:}"; ORIGIN_REPO="${rest%%/*}"; rest="${rest#*/}"; ORIGIN_REPO="$ORIGIN_REPO/${rest%%/*}"; ORIGIN_RPATH="${rest#*/}"
            ORIGIN_URL="$HF_BASE/$ORIGIN_REPO/resolve/main/$ORIGIN_RPATH"
            if [[ -n "${HF_TOKEN:-}" ]]; then ORIGIN_AUTH=(-H "Authorization: Bearer $HF_TOKEN"); fi ;;
    *)      ORIGIN_URL="$ORIGIN"; ORIGIN_REPO="$ORIGIN"; ORIGIN_RPATH="${ORIGIN##*/}" ;;
  esac
}

# Download $1 from ORIGIN_URL into place: size (and sha256 / blob sha1 when the origin's etag gives
# one) checked against the response headers, atomic rename, record written. Returns 1 with nothing
# left behind on failure. Sets FETCHED_REV.
fetch_origin() {
  local f="$1" dst="$MODELS_DIR/$1" rev etag size got_size got_etag sha
  download "$dst" "$ORIGIN_URL" ${ORIGIN_AUTH[@]+"${ORIGIN_AUTH[@]}"} || return 1
  rev="$(origin_rev "$dst.headers")"; etag="$(origin_etag "$dst.headers")"; size="$(origin_size "$dst.headers")"
  rm -f "$dst.headers"
  got_size="$(size_of "$dst.part")"
  if [[ -n "$size" && "$size" != "$got_size" ]]; then
    rm -f "$dst.part"; echo "    $f: got $got_size bytes, the origin announced $size"; return 1
  fi
  got_etag="$(local_etag_of "$dst.part" "$etag")"
  if [[ -n "$got_etag" && "$got_etag" != "$etag" ]]; then
    rm -f "$dst.part"; echo "    $f: hash $got_etag does not match the origin's etag $etag"; return 1
  fi
  if [[ ${#etag} -eq 64 ]]; then sha="$got_etag"; else sha="$(sha256_of "$dst.part")"; fi
  mv "$dst.part" "$dst"
  write_record "$f" "$ORIGIN_REPO" "$ORIGIN_RPATH" "$rev" "$etag" "$got_size" "$sha"
  FETCHED_REV="$rev"
}

# Put missing $1 under $MODELS_DIR: documented origin first, then the release bucket.
fetch_one() {
  local f="$1" dst="$MODELS_DIR/$1" fallback="$PUB_MODELS/$1" row
  if [[ "$DRY_RUN" == "1" ]]; then
    if [[ -n "$ORIGIN_URL" ]]; then echo "    $f: DRY_RUN: new ← $ORIGIN_URL, else ← $fallback"
    else echo "    $f: DRY_RUN: new ← $fallback (no documented origin: ${ORIGIN#none:})"; fi
    return 0
  fi
  if [[ -n "$ORIGIN_URL" ]]; then
    echo "    $f: fetching $ORIGIN_URL"
    if fetch_origin "$f"; then
      echo "    $f: new ($(size_of "$dst") bytes${FETCHED_REV:+, rev $FETCHED_REV})"
      return 0
    fi
    echo "    $f: origin failed; trying the release bucket"
  else
    echo "    $f: no documented origin (${ORIGIN#none:}); trying the release bucket"
  fi
  download "$dst" "$fallback" || { echo "error: $f: not at its origin and not on the bucket; put it in $MODELS_DIR/$f" >&2; return 1; }
  rm -f "$dst.headers"
  bucket_manifest
  row="$(bucket_row "$f")"
  if [[ "$row" != "$(size_of "$dst.part") $(sha256_of "$dst.part")" ]]; then
    rm -f "$dst.part"
    echo "error: $f: bucket copy does not match $PUB_MODELS/models.json (${row:-not listed})" >&2
    return 1
  fi
  mv "$dst.part" "$dst"
  echo "    $f: new ($(size_of "$dst") bytes from the bucket; size + sha256 match models.json)"
}

# A `repo:` file: the checkout's copy is the truth. Missing → copied, different → replaced (counted
# stale); --check only reports.
sync_repo() {
  local f="$1" dst="$MODELS_DIR/$1" rel="${ORIGIN#repo:}" what
  [[ -f "$REPO_ROOT/$rel" ]] || { echo "error: $f: $rel is not in this checkout" >&2; return 1; }
  if [[ ! -s "$dst" ]]; then
    if [[ "$CHECK" == 1 ]]; then echo "    $f: MISSING"; MISSING=$((MISSING + 1)); return 0; fi
    what=new
  elif cmp -s "$REPO_ROOT/$rel" "$dst"; then
    echo "    $f: up to date (repo)"; return 0
  else
    STALE=$((STALE + 1))
    if [[ "$CHECK" == 1 ]]; then echo "    $f: STALE (differs from $rel)"; return 0; fi
    what=updated
  fi
  if [[ "$DRY_RUN" == 1 ]]; then echo "    $f: DRY_RUN: $what ← $rel"; return 0; fi
  mkdir -p "$(dirname "$dst")"
  cp "$REPO_ROOT/$rel" "$dst.part" && mv "$dst.part" "$dst"
  echo "    $f: $what ($(size_of "$dst") bytes from $rel)"
}

# $1 is on disk: HEAD its origin, compare with the record (or the file's own hash when there is
# none) and replace it when the content changed. --check only reports; a failed HEAD keeps the file.
refresh_one() {
  local f="$1" dst="$MODELS_DIR/$1" hdr rev etag size rec_rev rec_etag local_sha=""
  if [[ "$FORCE" == all || "$FORCE" == "$f" ]] && [[ -n "$ORIGIN_URL" ]]; then
    rec_rev="$(record_field "$f" revision)"
    if [[ "$CHECK" == 1 || "$DRY_RUN" == 1 ]]; then echo "    $f: forced — would re-download ← $ORIGIN_URL"; STALE=$((STALE + 1)); return 0; fi
    echo "    $f: forced; fetching $ORIGIN_URL"
    fetch_origin "$f" || exit 1
    echo "    $f: updated rev ${rec_rev:-?} → ${FETCHED_REV:-?} ($(size_of "$dst") bytes)"
    return 0
  fi
  case "$ORIGIN" in
    none:*)  echo "    $f: no documented origin — not checked"; return 0 ;;
    hf:*)    ;;
    *)       echo "    $f: pinned (${ORIGIN%%:*} origin, no freshness check)"; return 0 ;;
  esac
  hdr="$(mktemp)"
  if ! probe "$ORIGIN_URL" "$hdr" ${ORIGIN_AUTH[@]+"${ORIGIN_AUTH[@]}"}; then
    echo "    $f: cannot check freshness (kept)"; UNVERIFIED=$((UNVERIFIED + 1)); rm -f "$hdr"; return 0
  fi
  rev="$(origin_rev "$hdr")"; etag="$(origin_etag "$hdr")"; size="$(origin_size "$hdr")"; rm -f "$hdr"
  rec_rev="$(record_field "$f" revision)"; rec_etag="$(record_field "$f" etag)"
  if [[ -z "$rec_etag" ]]; then   # pre-existing install: the file's own hash stands in for the record
    rec_etag="$(local_etag_of "$dst" "$etag")"
    [[ ${#etag} -eq 64 ]] && local_sha="$rec_etag"
  fi
  if [[ "$rec_etag" == "$etag" && "$(size_of "$dst")" == "$size" ]]; then
    if [[ "$CHECK" != 1 && "$DRY_RUN" != 1 && ( "$rec_rev" != "$rev" || ! -f "$(record_path "$f")" ) ]]; then
      write_record "$f" "$ORIGIN_REPO" "$ORIGIN_RPATH" "$rev" "$etag" "$(size_of "$dst")" "${local_sha:-$(sha256_of "$dst")}"
    fi
    echo "    $f: up to date (rev ${rev:-?})"
    return 0
  fi
  STALE=$((STALE + 1))
  if [[ "$CHECK" == 1 ]]; then
    echo "    $f: STALE rev ${rec_rev:-?} → ${rev:-?} (etag ${rec_etag:-?} → $etag, $(size_of "$dst") → ${size:-?} bytes)"; return 0
  fi
  if [[ "$DRY_RUN" == 1 ]]; then echo "    $f: DRY_RUN: would replace, rev ${rec_rev:-?} → ${rev:-?} ← $ORIGIN_URL"; return 0; fi
  echo "    $f: stale (rev ${rec_rev:-?} → ${rev:-?}); fetching $ORIGIN_URL"
  fetch_origin "$f" || exit 1
  echo "    $f: updated rev ${rec_rev:-?} → ${FETCHED_REV:-?} ($(size_of "$dst") bytes)"
}

# Every REQUIRED file: missing → fetch_one, present → refresh_one. The first failed download stops
# the run; with --check nothing is written and the exit code is 2 when anything needs attention.
fetch_models() {
  echo "==> models under $MODELS_DIR (check: $CHECK, dry run: $DRY_RUN, force: ${FORCE:-none}; bucket $PUB_MODELS/)"
  local f; STALE=0; UNVERIFIED=0; MISSING=0
  for f in "${REQUIRED[@]}"; do
    resolve_origin "$f"
    if [[ "$ORIGIN" == repo:* ]]; then sync_repo "$f" || exit 1; continue; fi
    if [[ ! -s "$MODELS_DIR/$f" ]]; then
      if [[ "$CHECK" == 1 ]]; then echo "    $f: MISSING"; MISSING=$((MISSING + 1)); continue; fi
      fetch_one "$f" || exit 1
    else
      refresh_one "$f"
    fi
  done
  echo "    ${#REQUIRED[@]} files: $STALE stale, $MISSING missing, $UNVERIFIED not checkable"
  if [[ "$CHECK" == 1 && $((STALE + UNVERIFIED + MISSING)) -gt 0 ]]; then exit 2; fi
  return 0
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  command -v curl >/dev/null 2>&1 || { echo "error: curl required" >&2; exit 1; }
  command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }
  fetch_models
fi
