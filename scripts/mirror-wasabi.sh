#!/usr/bin/env bash
# Mirror a signed theuth release (binaries, .sig files, SHA256SUMS, legal texts,
# and rewritten latest.json / latest-prerelease.json) to the Wasabi S3 bucket.
#
# Modes:
#   ./scripts/mirror-wasabi.sh
#       Mirror from local target/release/dist (default, run right after scripts/release-s3.sh).
#   ./scripts/mirror-wasabi.sh --from-s3 <version>
#       Download a live release from self-hosted S3 (THEUTH_S3_PUBLIC_BASE_URL),
#       verify SHA256SUMS, and mirror it to Wasabi (useful for backfilling).
#
# Env (optional local defaults loaded from repo-root .env.signing):
#   WASABI_ACCESS_KEY_ID / WASABI_SECRET_ACCESS_KEY   required credentials
#   WASABI_S3_ENDPOINT         default: https://s3.us-central-1.wasabisys.com
#   WASABI_S3_REGION           default: us-central-1
#   WASABI_S3_BUCKET           default: theuth-releases
#   WASABI_S3_PREFIX           default: "" (bucket root: <ver>/ and latest.json)
#   WASABI_S3_PUBLIC_BASE_URL  default: ${ENDPOINT}/${BUCKET}[/${PREFIX}]
#   THEUTH_VERSION             release version (default: from Cargo.toml or --from-s3 arg)
#   THEUTH_DIST_DIR            default: target/release/dist
#   THEUTH_RELEASE_PRERELEASE  1 → write latest-prerelease.json instead of latest.json
#   THEUTH_S3_PUBLIC_BASE_URL  source base URL for --from-s3 (default: https://s3.rwx.dev/theuth/releases)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

FROM_S3=0
ARG_VER=""
if [[ "${1:-}" == "--from-s3" ]]; then
  FROM_S3=1
  ARG_VER="${2:-}"
  [[ -n "$ARG_VER" ]] || { echo "usage: $0 --from-s3 <version>" >&2; exit 2; }
fi

ENDPOINT="${WASABI_S3_ENDPOINT:-https://s3.us-central-1.wasabisys.com}"; ENDPOINT="${ENDPOINT%/}"
REGION="${WASABI_S3_REGION:-us-central-1}"
BUCKET="${WASABI_S3_BUCKET:-theuth-releases}"
PREFIX="${WASABI_S3_PREFIX:-}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
PRERELEASE="${THEUTH_RELEASE_PRERELEASE:-0}"
MANIFEST_NAME="latest.json"
[[ "$PRERELEASE" == "1" ]] && MANIFEST_NAME="latest-prerelease.json"

if [[ -n "$ARG_VER" ]]; then
  VERSION="$ARG_VER"
elif [[ -n "${THEUTH_VERSION:-}" ]]; then
  VERSION="$THEUTH_VERSION"
else
  VERSION="$(awk '/^\[workspace.package\]/{p=1} p&&/^version =/{gsub(/"/,""); print $3; exit}' Cargo.toml)"
fi
[[ -n "$VERSION" ]] || { echo "error: could not determine release version" >&2; exit 1; }

KEY_ID="${WASABI_ACCESS_KEY_ID:-}"
SECRET_KEY="${WASABI_SECRET_ACCESS_KEY:-}"
[[ -n "$KEY_ID" && -n "$SECRET_KEY" ]] || {
  echo "error: WASABI_ACCESS_KEY_ID and WASABI_SECRET_ACCESS_KEY must be set" >&2
  exit 1
}

if [[ -n "$PREFIX" ]]; then
  S3_ROOT="s3://${BUCKET}/${PREFIX}"
  DEFAULT_PUB="${ENDPOINT}/${BUCKET}/${PREFIX}"
else
  S3_ROOT="s3://${BUCKET}"
  DEFAULT_PUB="${ENDPOINT}/${BUCKET}"
fi
PUBLIC_BASE="${WASABI_S3_PUBLIC_BASE_URL:-$DEFAULT_PUB}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
S3_VER="${S3_ROOT}/${VERSION}"
PUB_VER="${PUBLIC_BASE}/${VERSION}"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/theuth-wasabi.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# Isolated AWS env for Wasabi (never inherits self-hosted S3 credentials).
wasabi_aws() {
  AWS_ACCESS_KEY_ID="$KEY_ID" \
  AWS_SECRET_ACCESS_KEY="$SECRET_KEY" \
  AWS_DEFAULT_REGION="$REGION" \
  AWS_EC2_METADATA_DISABLED=true \
  AWS_REQUEST_CHECKSUM_CALCULATION=when_required \
  AWS_RESPONSE_CHECKSUM_VALIDATION=when_required \
    aws --endpoint-url "$ENDPOINT" --region "$REGION" "$@"
}

wasabi_key_of() {
  local rel="$1"
  if [[ -n "$PREFIX" ]]; then
    printf '%s/%s' "$PREFIX" "$rel"
  else
    printf '%s' "$rel"
  fi
}

wasabi_object_matches_size() {
  local local_file="$1" rel_key="$2"
  local local_size remote_size
  local_size="$(stat -f%z "$local_file" 2>/dev/null || stat -c%s "$local_file")"
  remote_size="$(wasabi_aws s3api head-object --bucket "$BUCKET" --key "$(wasabi_key_of "$rel_key")" \
    --query ContentLength --output text 2>/dev/null || true)"
  [[ -n "$remote_size" && "$remote_size" == "$local_size" ]]
}

wasabi_cp() {
  local src="$1" dst="$2" ct
  case "$dst" in
    *.json) ct=application/json ;;
    *.dmg) ct=application/x-apple-diskimage ;;
    *.exe) ct=application/vnd.microsoft.portable-executable ;;
    *.tar.gz) ct=application/gzip ;;
    *.zip) ct=application/zip ;;
    *.sig | *.deb | *.rpm | *.AppImage) ct=application/octet-stream ;;
    *) ct=text/plain ;;
  esac
  local n=1
  until wasabi_aws s3 cp "$src" "$dst" --content-type "$ct" \
      --cli-read-timeout 300 --cli-connect-timeout 30 >/dev/null; do
    n=$(( n + 1 ))
    [[ $n -le 3 ]] || { echo "error: $dst: upload failed after 3 attempts" >&2; return 1; }
    echo "    retry $n: $dst" >&2
    sleep $(( n * 5 ))
  done
  echo "    $dst"
}

if [[ "$FROM_S3" == "1" ]]; then
  SRC_BASE="${THEUTH_S3_PUBLIC_BASE_URL:-https://s3.rwx.dev/theuth/releases}"; SRC_BASE="${SRC_BASE%/}"
  DIST="$WORK/dist"
  mkdir -p "$DIST"
  echo "==> [wasabi] fetching $VERSION from $SRC_BASE/"
  curl -fsSL --retry 3 "$SRC_BASE/$VERSION/SHA256SUMS" -o "$DIST/SHA256SUMS"
  curl -fsSL --retry 3 "$SRC_BASE/$MANIFEST_NAME" -o "$DIST/$MANIFEST_NAME"
  LIVE_VER="$(jq -r '.version' "$DIST/$MANIFEST_NAME")"
  [[ "$LIVE_VER" == "$VERSION" ]] || {
    echo "error: $SRC_BASE/$MANIFEST_NAME is version $LIVE_VER, expected $VERSION" >&2
    exit 1
  }
  while read -r _sha fname; do
    [[ -n "$fname" ]] || continue
    curl -fsSL --retry 3 "$SRC_BASE/$VERSION/$fname" -o "$DIST/$fname"
  done < "$DIST/SHA256SUMS"
  (cd "$DIST" && shasum -a 256 -c SHA256SUMS)
  # Download every .sig file referenced by the manifest platforms
  while IFS= read -r url; do
    [[ -n "$url" ]] || continue
    base="${url##*/}"
    curl -fsSL --retry 3 "$SRC_BASE/$VERSION/${base}.sig" -o "$DIST/${base}.sig"
  done < <(jq -r '.platforms[].url' "$DIST/$MANIFEST_NAME" | sort -u)
else
  DIST="${THEUTH_DIST_DIR:-$ROOT/target/release/dist}"
  [[ -d "$DIST" ]] || { echo "error: dist dir not found: $DIST" >&2; exit 1; }
  [[ -f "$DIST/SHA256SUMS" ]] || { echo "error: missing $DIST/SHA256SUMS" >&2; exit 1; }
  [[ -f "$DIST/$MANIFEST_NAME" ]] || { echo "error: missing $DIST/$MANIFEST_NAME" >&2; exit 1; }
fi

echo "==> [wasabi] mirroring theuth $VERSION → $S3_VER/"
echo "    endpoint: $ENDPOINT (region $REGION)"
echo "    public:   $PUBLIC_BASE"

shopt -s nullglob
FILES=()
for f in "$DIST"/theuth_"${VERSION}"_*; do
  FILES+=("$(basename "$f")")
done
for extra in SHA256SUMS LICENSE NOTICE THIRD-PARTY-NOTICES; do
  if [[ -f "$DIST/$extra" ]]; then
    FILES+=("$extra")
  elif [[ -f "$ROOT/$extra" ]]; then
    cp "$ROOT/$extra" "$WORK/$extra"
    FILES+=("$extra")
  fi
done

[[ ${#FILES[@]} -gt 0 ]] || { echo "error: no release files found for $VERSION in $DIST" >&2; exit 1; }

for fname in "${FILES[@]}"; do
  src="$DIST/$fname"
  [[ -f "$src" ]] || src="$WORK/$fname"
  if [[ "$fname" != "SHA256SUMS" ]] && wasabi_object_matches_size "$src" "${VERSION}/${fname}"; then
    echo "    present ${S3_VER}/${fname} (skipped)"
    continue
  fi
  wasabi_cp "$src" "${S3_VER}/${fname}"
done

echo "==> [wasabi] writing rewritten $MANIFEST_NAME → ${S3_ROOT}/${MANIFEST_NAME}"
jq --arg base "$PUB_VER" '
  .platforms |= with_entries(
    .value.url = ($base + "/" + (.value.url | split("/") | last))
  )
' "$DIST/$MANIFEST_NAME" > "$WORK/$MANIFEST_NAME"
wasabi_cp "$WORK/$MANIFEST_NAME" "${S3_ROOT}/${MANIFEST_NAME}"

echo "==> [wasabi] verifying public reads at $PUBLIC_BASE/"
FETCHED_VER="$(curl -fsSL --retry 3 "${PUBLIC_BASE}/${MANIFEST_NAME}" | jq -r '.version')"
[[ "$FETCHED_VER" == "$VERSION" ]] || {
  echo "error: ${PUBLIC_BASE}/${MANIFEST_NAME} returned version '$FETCHED_VER', expected '$VERSION'" >&2
  exit 1
}
curl -fsSL --retry 3 "${PUB_VER}/SHA256SUMS" -o "$WORK/live-SHA256SUMS"
cmp -s "$DIST/SHA256SUMS" "$WORK/live-SHA256SUMS" || {
  echo "error: ${PUB_VER}/SHA256SUMS differs from local SHA256SUMS" >&2
  exit 1
}
PLATFORM_COUNT="$(curl -fsSL --retry 3 "${PUBLIC_BASE}/${MANIFEST_NAME}" | jq '.platforms | length')"
echo "    ok: ${PUBLIC_BASE}/${MANIFEST_NAME} (version=$FETCHED_VER, platforms=$PLATFORM_COUNT)"
echo "    ok: ${PUB_VER}/SHA256SUMS verified"
