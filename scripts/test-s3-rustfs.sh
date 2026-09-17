#!/usr/bin/env bash
# Probe the release bucket: credentials, write, public read, private staging (39 §12.1).
#
# Verifies the ops setup release-s3.sh and stage-linux.sh rely on:
#   1. authenticated access (head/list bucket)
#   2. authenticated PutObject under releases/ and staging/
#   3. anonymous GET 200 under releases/ (bucket policy scripts/s3/wth-bucket-policy.json)
#   4. anonymous GET 403 under staging/ (unsigned Linux tarballs stay private)
#   5. anonymous PUT rejected
# Cleans up its probe objects on exit (unless WTH_S3_KEEP_PROBE=1).
#
# Apply the policy first (admin creds; once per bucket):
#   aws --endpoint-url "$WTH_S3_ENDPOINT" --region us-east-1 \
#     s3api put-bucket-policy --bucket wth --policy file://scripts/s3/wth-bucket-policy.json
#
# Env (same knobs and defaults as release-s3.sh; .env.signing loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   WTH_S3_ENDPOINT (https://s3.rwx.dev)  WTH_S3_BUCKET (wth)  WTH_S3_PREFIX (releases)
#   WTH_S3_PUBLIC_BASE_URL (${ENDPOINT}/${BUCKET})   WTH_S3_DRY_RUN=1   WTH_S3_KEEP_PROBE=1
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

command -v aws >/dev/null 2>&1 || { echo "error: aws CLI v2 required (brew install awscli)" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "error: curl required" >&2; exit 1; }

ENDPOINT="${WTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${WTH_S3_BUCKET:-wth}"
PREFIX="${WTH_S3_PREFIX:-releases}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
# Bucket root, not the releases prefix: the probe also GETs staging/.
PUBLIC_BASE="${WTH_S3_PUBLIC_BASE_URL:-${ENDPOINT}/${BUCKET}/${PREFIX}}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
PUBLIC_ROOT="${PUBLIC_BASE%/"$PREFIX"}"
DRY_RUN="${WTH_S3_DRY_RUN:-0}"
KEEP_PROBE="${WTH_S3_KEEP_PROBE:-0}"

[[ "$DRY_RUN" == "1" || ( -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ) ]] \
  || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset (see .env.signing.example)" >&2; exit 1; }

# rustfs: path-style, no mandatory checksums, no IMDS probe (jarvis release-s3.sh:164-174).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true
AWS=(aws --endpoint-url "$ENDPOINT")

PASS=0; FAIL=0; SKIP=0
ok()   { echo "  PASS: $1"; PASS=$((PASS + 1)); }
bad()  { echo "  FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
skip() { echo "  SKIP: $1"; SKIP=$((SKIP + 1)); }

PROBE_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
PROBE_BODY="wth rustfs probe ${PROBE_ID}"
PROBE_FILE="$(mktemp)"
printf '%s\n' "$PROBE_BODY" >"$PROBE_FILE"
REL_KEY="${PREFIX}/.wth-s3-probe/${PROBE_ID}.txt"
STG_KEY="staging/.wth-s3-probe/${PROBE_ID}.txt"
UPLOADED=()

cleanup() {
  rm -f "$PROBE_FILE"
  [[ "$KEEP_PROBE" == "1" || "$DRY_RUN" == "1" ]] && return 0
  local k
  for k in ${UPLOADED[@]+"${UPLOADED[@]}"}; do
    echo "==> cleanup s3://${BUCKET}/${k}"
    "${AWS[@]}" s3 rm "s3://${BUCKET}/${k}" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

echo "==> rustfs / S3 probe"
echo "    endpoint:     $ENDPOINT"
echo "    bucket:       $BUCKET"
echo "    prefix:       $PREFIX"
echo "    public root:  $PUBLIC_ROOT"
echo

# Anonymous GET; prints the HTTP status (000 on transport failure). Body → $2 when given.
anon_get() { curl -sS -o "${2:-/dev/null}" -w '%{http_code}' --max-time 30 "$1" || echo 000; }

echo "==> [1/5] authenticated access"
if [[ "$DRY_RUN" == "1" ]]; then
  skip "head-bucket (dry-run)"
elif "${AWS[@]}" s3api head-bucket --bucket "$BUCKET" 2>/dev/null; then
  ok "head-bucket $BUCKET"
elif "${AWS[@]}" s3 ls "s3://${BUCKET}" >/dev/null 2>&1; then
  ok "list bucket $BUCKET (head-bucket unsupported; list ok)"
else
  bad "cannot head/list bucket $BUCKET with these credentials"
fi

echo "==> [2/5] authenticated write (PutObject under $PREFIX/ and staging/)"
for k in "$REL_KEY" "$STG_KEY"; do
  if [[ "$DRY_RUN" == "1" ]]; then
    skip "put s3://${BUCKET}/${k} (dry-run)"
  elif "${AWS[@]}" s3 cp "$PROBE_FILE" "s3://${BUCKET}/${k}" --content-type text/plain >/dev/null; then
    UPLOADED+=("$k"); ok "put s3://${BUCKET}/${k}"
  else
    bad "put s3://${BUCKET}/${k} failed"
  fi
done

echo "==> [3/5] public read (anonymous GET under $PREFIX/ → 200, body match)"
if [[ "$DRY_RUN" == "1" ]]; then
  skip "anonymous GET (dry-run)"
elif [[ " ${UPLOADED[*]-} " != *" $REL_KEY "* ]]; then
  skip "anonymous GET (probe not uploaded)"
else
  BODY="$(mktemp)"
  CODE="$(anon_get "${PUBLIC_ROOT}/${REL_KEY}" "$BODY")"
  if [[ "$CODE" == "200" && "$(tr -d '\r' <"$BODY" | head -n1)" == "$PROBE_BODY" ]]; then
    ok "anonymous GET ${PUBLIC_ROOT}/${REL_KEY}"
  else
    bad "anonymous GET HTTP $CODE for ${PUBLIC_ROOT}/${REL_KEY} (expected 200 + body match)"
    echo "    hint: aws --endpoint-url $ENDPOINT s3api put-bucket-policy --bucket $BUCKET --policy file://scripts/s3/wth-bucket-policy.json" >&2
  fi
  rm -f "$BODY"
fi

echo "==> [4/5] staging stays private (anonymous GET under staging/ → 403)"
if [[ "$DRY_RUN" == "1" ]]; then
  skip "anonymous GET staging (dry-run)"
elif [[ " ${UPLOADED[*]-} " != *" $STG_KEY "* ]]; then
  skip "anonymous GET staging (probe not uploaded)"
else
  CODE="$(anon_get "${PUBLIC_ROOT}/${STG_KEY}")"
  case "$CODE" in
    403 | 401) ok "anonymous GET staging/ rejected (HTTP $CODE)" ;;
    *) bad "anonymous GET staging/ returned HTTP $CODE (expected 403); the policy must cover ${PREFIX}/* only" ;;
  esac
fi

echo "==> [5/5] public write rejected (anonymous PUT)"
if [[ "$DRY_RUN" == "1" ]]; then
  skip "anonymous PUT (dry-run)"
else
  DENY_KEY="${PREFIX}/.wth-s3-probe/should-not-write-${PROBE_ID}.txt"
  CODE="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 -X PUT -H 'Content-Type: text/plain' \
    --data should-fail "${PUBLIC_ROOT}/${DENY_KEY}" || echo 000)"
  case "$CODE" in
    200 | 201 | 204)
      bad "anonymous PUT succeeded (HTTP $CODE) — bucket allows unauthenticated writes"
      "${AWS[@]}" s3 rm "s3://${BUCKET}/${DENY_KEY}" >/dev/null 2>&1 || true ;;
    000) bad "anonymous PUT did not complete (network/TLS?)" ;;
    *) ok "anonymous PUT rejected (HTTP $CODE)" ;;
  esac
fi

echo
echo "==> summary: $PASS passed, $FAIL failed, $SKIP skipped"
((FAIL == 0)) || exit 1
echo "done: $ENDPOINT/$BUCKET is ready for release-s3.sh"
