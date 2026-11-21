#!/usr/bin/env bash
# Package the theuth desktop app for Linux: .deb, .rpm and .AppImage, staged for release-s3.sh.
#
#   1. cargo build --release -p theuth  (daemon sidecar, host arch)
#      + packages/apps: bun build --compile → theuth-app-parser (46 §2.1; scripts/app-parser-env.sh)
#   2. stage both as externalBin: binaries/t-<triple>, binaries/theuth-app-parser-<hash>-<triple>
#      (the helper must sit beside `t`, so it rides the same mechanism)
#   3. cargo tauri build --config <inline json>  (targets from tauri.linux.conf.json: deb, rpm, appimage)
#   4. copy as theuth_<ver>_linux-<arch>.{deb,rpm,AppImage} → target/release/dist
#   5. aws s3 cp → s3://$THEUTH_S3_BUCKET/staging/<ver>/  (private; release-s3.sh publishes)
#
# Native only: run on a box of the target arch; the bundles bind to the host's glibc and
# webkit2gtk, so build on the oldest distro you support (the CLI tarball uses ubuntu:24.04,
# deploy/update/Dockerfile). No libonnxruntime in the bundle — the app downloads it from the
# model bucket at start (release-models.sh publishes the linux rows; `t` finds it under
# ~/.theuth/models/onnxruntime/).
#
# Host packages (Debian/Ubuntu): HOST_PKGS below = the daemon's from deploy/update/Dockerfile:28-34 plus
# the webkit2gtk/gtk/appindicator/rsvg dev packages and patchelf for the Tauri bundler,
# and: rustup (rust-toolchain.toml), cargo-tauri, bun, jq, just, aws CLI v2 (upload only).
# `scripts/package-linux.sh --check` prints what is missing and exits 2 (build-remote.sh runs it first; nothing is installed).
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   THEUTH_S3_ENDPOINT / THEUTH_S3_BUCKET   as release-s3.sh
#   THEUTH_S3_DRY_RUN=1                     print the upload instead of running it
#   THEUTH_SKIP_UPLOAD=1                    stop after dist/ (local install / smoke test; release-s3.sh reads dist/ first)
#   THEUTH_SKIP_BUN_INSTALL=1               skip `bun install` (desktop UI and packages/apps)
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
# Tauri runs linuxdeploy + its plugins as AppImages, which mount themselves through FUSE 2; Ubuntu ≥ 24.10 and
# most containers have only fuse3 (or no /dev/fuse). Extract-and-run needs neither and costs a few seconds.
export APPIMAGE_EXTRACT_AND_RUN=1

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

[[ "$(uname -s)" == "Linux" ]] || { echo "error: builds Linux bundles on Linux only (got $(uname -s))" >&2; exit 1; }

HOST_PKGS=(
  curl ca-certificates git build-essential clang cmake llvm libclang-dev pkg-config perl unzip
  protobuf-compiler nasm   # signal's spqr (prost-build); aws-lc-sys / boringssl assembly
  libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev libxcb-randr0-dev
  libx11-dev libxtst-dev libxdo-dev libxkbcommon-dev
  libdbus-1-dev
  libpipewire-0.3-dev libspa-0.2-dev libdrm-dev libgbm-dev libwayland-dev libegl1-mesa-dev libgl-dev
  libasound2-dev libpulse-dev libudev-dev   # meeting capture (flexaudio: ALSA mic, PipeWire/Pulse loopback)
  libssl-dev
  libcap-ng-dev   # microsandbox → msb_krun_devices → capng links libcap-ng (sandbox VMs, 79)
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf
  xdg-utils file   # AppImage bundler copies xdg-open into the image and linuxdeploy runs file(1)
)
# check_prereqs: every HOST_PKGS entry installed (dpkg hosts only) and every tool present; exit 2 listing the gaps.
check_prereqs() {
  local missing=() p t
  if command -v dpkg-query >/dev/null 2>&1; then
    for p in "${HOST_PKGS[@]}"; do
      # shellcheck disable=SC2016  # ${Status} is dpkg's format string, not shell
      [[ "$(dpkg-query -W -f='${Status}' "$p" 2>/dev/null)" == "install ok installed" ]] || missing+=("$p")
    done
  else
    echo "    (no dpkg-query: host packages not checked; see HOST_PKGS in scripts/package-linux.sh)"
  fi
  for t in cargo rustc bun jq just; do command -v "$t" >/dev/null 2>&1 || missing+=("$t"); done
  cargo tauri --version >/dev/null 2>&1 || missing+=("cargo-tauri (cargo install tauri-cli)")
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "error: missing on $(hostname): ${missing[*]}" >&2
    echo "       apt install: ${missing[*]}" >&2
    return 2
  fi
  echo "    prerequisites ok: $(rustc -vV | awk '/^host:/ {print $2}'), $(cargo tauri --version 2>/dev/null), bun $(bun --version), just $(just --version | awk '{print $2}')"
}
if [[ "${1:-}" == "--check" ]]; then
  echo "==> prerequisites"
  check_prereqs
  exit 0
fi
echo "==> prerequisites"
check_prereqs

# Cap cargo's parallelism to the RAM: one rustc per 2.5 GiB of MemAvailable, never more than the
# cores. The release build of the microsandbox crates (sandbox VMs, 79) peaks well above 1.5 GiB per
# rustc, and 12 jobs on the 8 GiB build-A container tripped the OOM killer ("signal: 9, SIGKILL" on
# `microsandbox (lib)`, 2026-10-03). CARGO_BUILD_JOBS set by the caller wins.
if [[ -z "${CARGO_BUILD_JOBS:-}" && -r /proc/meminfo ]]; then
  mem_kib="$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)"
  cores="$(nproc)"
  jobs=$(( mem_kib / 2621440 ))
  (( jobs > cores )) && jobs=$cores
  (( jobs < 1 )) && jobs=1
  if (( jobs < cores )); then
    export CARGO_BUILD_JOBS="$jobs"
    echo "    cargo jobs capped at $jobs ($cores cores, $(( mem_kib / 1048576 )) GiB available)"
  fi
fi

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

echo "==> [2/5] staging sidecars (t, $HELPER)"
mkdir -p crates/desktop/binaries
rm -f crates/desktop/binaries/theuth-app-parser-*
cp target/release/t "crates/desktop/binaries/t-$HOST_TRIPLE"
cp "$HELPER_SRC" "crates/desktop/binaries/$HELPER-$HOST_TRIPLE"

echo "==> [3/5] tauri build (deb, rpm, appimage)"
if [[ "${THEUTH_SKIP_BUN_INSTALL:-0}" != "1" ]]; then
  (cd crates/desktop/ui && bun install)
fi
# externalBin is not in tauri.conf.json (tauri-build would fail `cargo check` without the files);
# the helper name carries the library hash, so the overlay is rendered here, not committed.
BUNDLE_CONF="$(jq -cn --arg h "binaries/$HELPER" '{bundle: {externalBin: ["binaries/t", $h]}}')"
rm -rf "$BUNDLE_DIR/deb" "$BUNDLE_DIR/rpm" "$BUNDLE_DIR/appimage"
# linuxdeploy aborts when `ldd` fails on any executable in the AppDir; on aarch64 `ldd` exits 1 with no output on
# Bun-compiled binaries (the app parser helper) although `ld.so --verify` accepts them and they need only glibc, which
# linuxdeploy never bundles. A shim turns "failed silently" into "no dependencies"; every other case passes through.
LDD_SHIM="$(mktemp -d)"
cat >"$LDD_SHIM/ldd" <<'SH'
#!/usr/bin/env bash
out="$(/usr/bin/ldd "$@" 2>&1)"; rc=$?
if [[ $rc -ne 0 && -z "$out" ]]; then exit 0; fi
printf '%s\n' "$out"; exit $rc
SH
chmod +x "$LDD_SHIM/ldd"
(cd crates/desktop && PATH="$LDD_SHIM:$PATH" TAURI_FRONTEND_PATH=ui cargo tauri build --config "$BUNDLE_CONF")
rm -rf "$LDD_SHIM"

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
# The deb must carry the sidecar and the helper in /usr/bin (`t` looks for it beside itself).
listed="$(dpkg-deb -c "$DEB" | awk '{print $6}' | sed 's|^\./||' | sort)"
for f in usr/bin/t "usr/bin/$HELPER" usr/bin/theuth-desktop; do
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
