#!/usr/bin/env bash
# Build the Linux CLI tarball natively in Docker and stage it in the bucket (39 §5.1).
#
# Pipeline (run once per arch, on a box of that arch — never QEMU):
#   1. docker build -f deploy/update/Dockerfile --output type=local  → target/linux-out/{t,theuth-app-parser-<hash>}
#      THEUTH_LINUX_NATIVE=1: no Docker — cargo build --release -p theuth + the bun app parser helper on this
#      Linux host (scripts/build-remote.sh on the Ubuntu 24.04 build host), same two files in target/linux-out
#   2. tar → target/release/dist/theuth_<ver>_linux-<arch>.tar.gz  (entries `t` + the helper, unsigned)
#   3. aws s3 cp → s3://$THEUTH_S3_BUCKET/staging/<ver>/  (private; release-s3.sh signs, publishes, deletes)
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   THEUTH_S3_ENDPOINT   API endpoint (default: https://s3.rwx.dev)
#   THEUTH_S3_BUCKET     bucket (default: theuth)
#   THEUTH_LINUX_ARCH    x86_64 | aarch64 (default: uname -m, arm64→aarch64; Docker Desktop
#                     builds the Mac's native arch, so the host spelling is the container's)
#   THEUTH_LINUX_NATIVE=1  build on this Linux host instead of Docker (arch = uname -m)
#   THEUTH_SKIP_UPLOAD=1   stop after dist/ (release-s3.sh reads dist/ first, staging/ is the fallback)
#   THEUTH_S3_DRY_RUN=1  print the upload instead of running it
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

NATIVE="${THEUTH_LINUX_NATIVE:-0}"
if [[ "$NATIVE" == "1" ]]; then
  [[ "$(uname -s)" == "Linux" ]] || { echo "error: THEUTH_LINUX_NATIVE=1 builds on Linux only (got $(uname -s))" >&2; exit 1; }
  command -v bun >/dev/null 2>&1 || { echo "error: bun required (https://bun.sh)" >&2; exit 1; }
else
  command -v docker >/dev/null 2>&1 || { echo "error: docker required (Docker Desktop on macOS, docker-ce on Linux; or THEUTH_LINUX_NATIVE=1 on a Linux host)" >&2; exit 1; }
fi
command -v cargo >/dev/null 2>&1 || { echo "error: cargo not on PATH (export PATH=\"\$(just toolchain):\$PATH\")" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "error: jq required" >&2; exit 1; }

ENDPOINT="${THEUTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${THEUTH_S3_BUCKET:-theuth}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-0}"
# rustfs: path-style, no mandatory checksums, no IMDS probe (jarvis release-s3.sh:164-174).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="theuth") | .version')"
ARCH="${THEUTH_LINUX_ARCH:-$(uname -m | sed s/arm64/aarch64/)}"
[[ "$NATIVE" != "1" ]] || ARCH="$(uname -m)"
case "$ARCH" in
  x86_64 | aarch64) ;;
  *) echo "error: unsupported arch '$ARCH' (x86_64 | aarch64)" >&2; exit 1 ;;
esac
OUT="target/linux-out"
DIST="target/release/dist"
TGZ="$DIST/theuth_${VERSION}_linux-${ARCH}.tar.gz"
DEST="s3://${BUCKET}/staging/${VERSION}/"

rm -rf "$OUT"
if [[ "$NATIVE" == "1" ]]; then
  echo "==> [1/3] native build (linux-$ARCH): cargo build --release -p theuth, $HELPER"
  cargo build --release -p theuth
  [[ "$(target/release/t --version 2>&1)" == *"$VERSION"* ]] || { echo "error: target/release/t --version did not print $VERSION" >&2; exit 1; }
  build_helper
  mkdir -p "$OUT"
  install -m 755 target/release/t "$OUT/t"
  install -m 755 "$HELPER_SRC" "$OUT/$HELPER"
else
  echo "==> [1/3] docker build (native, linux-$ARCH)"
  docker build -f deploy/update/Dockerfile --output "type=local,dest=$OUT" .
fi
[[ -f "$OUT/t" ]] || { echo "error: $OUT/t not produced" >&2; exit 1; }
[[ -f "$OUT/$HELPER" ]] || { echo "error: $OUT/$HELPER not produced (image built from another library hash?)" >&2; exit 1; }
chmod 755 "$OUT/t" "$OUT/$HELPER"

echo "==> [2/3] $TGZ"
mkdir -p "$DIST"
rm -f "$TGZ"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$TGZ" -C "$OUT" t "$HELPER"

echo "==> [3/3] upload → $DEST"
if [[ "${THEUTH_SKIP_UPLOAD:-0}" == "1" ]]; then
  echo "    THEUTH_SKIP_UPLOAD=1: not uploaded (release-s3.sh reads $TGZ from dist/)"
elif [[ "$DRY_RUN" == "1" ]]; then
  echo "    DRY_RUN: aws --endpoint-url $ENDPOINT s3 cp $TGZ $DEST"
else
  command -v aws >/dev/null 2>&1 || { echo "error: aws CLI v2 required (brew install awscli)" >&2; exit 1; }
  [[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset" >&2; exit 1; }
  aws --endpoint-url "$ENDPOINT" s3 cp "$TGZ" "$DEST" --content-type application/gzip
fi

echo
echo "done: staged $(basename "$TGZ") for release-s3.sh"
