#!/usr/bin/env bash
# Package the theuth desktop app for Linux: .deb, .rpm and .AppImage, staged for release-s3.sh.
#
#   1. cargo build --release -p theuth  (daemon sidecar, host arch)
#      + packages/apps: bun build --compile → theuth-app-parser (46 §2.1; scripts/app-parser-env.sh)
#      + packages/documents: bun build --compile → theuth-documents (59 §3.2; same env script)
#   2. stage all as externalBin: binaries/t-<triple>, binaries/theuth-app-parser-<hash>-<triple>,
#      binaries/theuth-documents-<ver>-<triple> (the helpers must sit beside `t` — helper_path — so
#      they ride the same mechanism)
#   3. cargo tauri build --config <inline json>  (targets from tauri.linux.conf.json: deb, rpm, appimage)
#   4. copy as theuth_<ver>_linux-<arch>.{deb,rpm,AppImage} → target/release/dist
#   5. aws s3 cp → s3://$THEUTH_S3_BUCKET/staging/<ver>/  (private; release-s3.sh publishes)
#
# Native only: run on a box of the target arch; the bundles bind to the host's glibc and
# webkit2gtk, so build on the oldest distro you support (the CLI tarball uses ubuntu:22.04,
# deploy/update/Dockerfile). No libonnxruntime in the bundle — the app downloads it from the
# model bucket at start (release-models.sh publishes the linux rows; `t` finds it under
# ~/.theuth/models/onnxruntime/).
#
# Host packages (Debian/Ubuntu): the daemon's from deploy/update/Dockerfile:28-34 plus
#   libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf
# and: rustup (rust-toolchain.toml), cargo-tauri, bun, jq, aws CLI v2.
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   THEUTH_S3_ENDPOINT / THEUTH_S3_BUCKET   as release-s3.sh
#   THEUTH_S3_DRY_RUN=1                     print the upload instead of running it
#   THEUTH_SKIP_UPLOAD=1                    stop after dist/ (local install / smoke test)
#   THEUTH_SKIP_BUN_INSTALL=1               skip `bun install` (desktop UI and packages/apps)
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

[[ "$(uname -s)" == "Linux" ]] || { echo "error: builds Linux bundles on Linux only (got $(uname -s))" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 required${2:+ ($2)}" >&2; exit 1; }; }
need cargo "rustup; rust-toolchain.toml picks the channel"
need bun "https://bun.sh"
need jq
cargo tauri --version >/dev/null 2>&1 || { echo "error: cargo tauri required (cargo install tauri-cli)" >&2; exit 1; }

HOST_TRIPLE="$(rustc -vV | awk '/^host:/ {print $2}')"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64 | aarch64) ;;
  *) echo "error: unsupported arch '$ARCH' (x86_64 | aarch64)" >&2; exit 1 ;;
esac
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="theuth") | .version')"
BUNDLE_DIR="target/release/bundle"
DIST="target/release/dist"
ENDPOINT="${THEUTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${THEUTH_S3_BUCKET:-theuth}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-0}"
DEST="s3://${BUCKET}/staging/${VERSION}/"

echo "==> [1/5] TypeScript bindings"
just ts-desktop

echo "==> [1/5] building theuth (release, $HOST_TRIPLE)"
cargo build --release -p theuth

echo "==> [1/5] building $HELPER (bun build --compile)"
build_helper

echo "==> [1/5] building $DOC_HELPER (bun build --compile)"
build_doc_helper

echo "==> [2/5] staging sidecars (t, $HELPER, $DOC_HELPER)"
mkdir -p crates/desktop/binaries
rm -f crates/desktop/binaries/theuth-app-parser-* crates/desktop/binaries/theuth-documents-*
cp target/release/t "crates/desktop/binaries/t-$HOST_TRIPLE"
cp "$HELPER_SRC" "crates/desktop/binaries/$HELPER-$HOST_TRIPLE"
cp "$DOC_HELPER_SRC" "crates/desktop/binaries/$DOC_HELPER-$HOST_TRIPLE"

echo "==> [3/5] tauri build (deb, rpm, appimage)"
if [[ "${THEUTH_SKIP_BUN_INSTALL:-0}" != "1" ]]; then
  (cd crates/desktop/ui && bun install)
fi
# externalBin is not in tauri.conf.json (tauri-build would fail `cargo check` without the files);
# the helper names carry the library hash / protocol version, so the overlay is rendered here, not committed.
BUNDLE_CONF="$(jq -cn --arg h "binaries/$HELPER" --arg d "binaries/$DOC_HELPER" '{bundle: {externalBin: ["binaries/t", $h, $d]}}')"
rm -rf "$BUNDLE_DIR/deb" "$BUNDLE_DIR/rpm" "$BUNDLE_DIR/appimage"
(cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri build --config "$BUNDLE_CONF")

echo "==> [4/5] → $DIST"
mkdir -p "$DIST"
one() { # dir glob → exactly one file
  local hits=("$1"/$2)
  [[ ${#hits[@]} -eq 1 && -f "${hits[0]}" ]] || { echo "error: expected one $2 in $1, got: ${hits[*]}" >&2; exit 1; }
  printf '%s' "${hits[0]}"
}
DEB="$(one "$BUNDLE_DIR/deb" "theuth_${VERSION}_*.deb")"
RPM="$(one "$BUNDLE_DIR/rpm" "theuth-${VERSION}-*.rpm")"
APPIMAGE="$(one "$BUNDLE_DIR/appimage" "theuth_${VERSION}_*.AppImage")"
OUT=()
for src in "$DEB" "$RPM" "$APPIMAGE"; do
  dst="$DIST/theuth_${VERSION}_linux-${ARCH}.${src##*.}"
  cp -f "$src" "$dst"
  OUT+=("$dst")
done
# The deb must carry the sidecar and both helpers in /usr/bin (helper_path looks beside `t`).
listed="$(dpkg-deb -c "$DEB" | awk '{print $6}' | sed 's|^\./||' | sort)"
for f in usr/bin/t "usr/bin/$HELPER" "usr/bin/$DOC_HELPER" usr/bin/theuth-desktop; do
  grep -qx "$f" <<<"$listed" || { echo "error: $DEB lacks $f" >&2; exit 1; }
done

echo "==> [5/5] upload → $DEST"
if [[ "${THEUTH_SKIP_UPLOAD:-0}" == "1" ]]; then
  echo "    THEUTH_SKIP_UPLOAD=1: not uploaded"
elif [[ "$DRY_RUN" == "1" ]]; then
  for f in "${OUT[@]}"; do echo "    DRY_RUN: aws --endpoint-url $ENDPOINT s3 cp $f $DEST"; done
else
  need aws "aws CLI v2"
  [[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset" >&2; exit 1; }
  # rustfs: path-style, no mandatory checksums, no IMDS probe (release-s3.sh).
  export AWS_S3_FORCE_PATH_STYLE=true
  export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
  export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
  export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
  export AWS_EC2_METADATA_DISABLED=true
  for f in "${OUT[@]}"; do
    aws --endpoint-url "$ENDPOINT" s3 cp "$f" "$DEST" --content-type application/octet-stream
  done
fi

echo
echo "done:"
for f in "${OUT[@]}"; do echo "  $f"; done
echo "  install locally: sudo apt install $DIST/theuth_${VERSION}_linux-${ARCH}.deb"
