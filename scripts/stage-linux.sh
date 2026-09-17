#!/usr/bin/env bash
# Build the Linux CLI tarball natively in Docker and stage it in the bucket (39 §5.1).
#
# Pipeline (run once per arch, on a box of that arch — never QEMU):
#   1. docker build -f deploy/update/Dockerfile --output type=local  → target/linux-out/wth
#   2. tar → target/release/dist/wth_<ver>_linux-<arch>.tar.gz  (one entry `wth`, unsigned)
#   3. aws s3 cp → s3://$WTH_S3_BUCKET/staging/<ver>/  (private; release-s3.sh signs, publishes, deletes)
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   WTH_S3_ENDPOINT   API endpoint (default: https://s3.rwx.dev)
#   WTH_S3_BUCKET     bucket (default: wth)
#   WTH_LINUX_ARCH    x86_64 | aarch64 (default: uname -m, arm64→aarch64; Docker Desktop
#                     builds the Mac's native arch, so the host spelling is the container's)
#   WTH_S3_DRY_RUN=1  print the upload instead of running it
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"

command -v docker >/dev/null 2>&1 || { echo "error: docker required (Docker Desktop on macOS, docker-ce on Linux)" >&2; exit 1; }
command -v aws >/dev/null 2>&1 || { echo "error: aws CLI v2 required (brew install awscli)" >&2; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "error: cargo not on PATH (export PATH=\"\$(just toolchain):\$PATH\")" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }

ENDPOINT="${WTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${WTH_S3_BUCKET:-wth}"
DRY_RUN="${WTH_S3_DRY_RUN:-0}"
# rustfs: path-style, no mandatory checksums, no IMDS probe (jarvis release-s3.sh:164-174).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="wth") | .version')"
ARCH="${WTH_LINUX_ARCH:-$(uname -m | sed s/arm64/aarch64/)}"
case "$ARCH" in
  x86_64 | aarch64) ;;
  *) echo "error: unsupported arch '$ARCH' (x86_64 | aarch64)" >&2; exit 1 ;;
esac
OUT="target/linux-out"
DIST="target/release/dist"
TGZ="$DIST/wth_${VERSION}_linux-${ARCH}.tar.gz"
DEST="s3://${BUCKET}/staging/${VERSION}/"

echo "==> [1/3] docker build (native, linux-$ARCH)"
rm -rf "$OUT"
docker build -f deploy/update/Dockerfile --output "type=local,dest=$OUT" .
[[ -f "$OUT/wth" ]] || { echo "error: $OUT/wth not produced" >&2; exit 1; }
chmod 755 "$OUT/wth"

echo "==> [2/3] $TGZ"
mkdir -p "$DIST"
rm -f "$TGZ"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$TGZ" -C "$OUT" wth

echo "==> [3/3] upload → $DEST"
if [[ "$DRY_RUN" == "1" ]]; then
  echo "    DRY_RUN: aws --endpoint-url $ENDPOINT s3 cp $TGZ $DEST"
else
  [[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset" >&2; exit 1; }
  aws --endpoint-url "$ENDPOINT" s3 cp "$TGZ" "$DEST" --content-type application/gzip
fi

echo
echo "done: staged $(basename "$TGZ") for release-s3.sh"
