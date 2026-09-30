#!/usr/bin/env bash
# Set up an Ubuntu machine and build the Linux CLI release tarball Homebrew installs.
#
# One machine, one arch (never QEMU). Run this on Ubuntu 24.04 x86_64 and, separately,
# on Ubuntu 24.04 aarch64. 24.04 is the glibc 2.39 floor the release supports
# (deploy/update/Dockerfile, Homebrew 6.0's Linux baseline). A newer Ubuntu links a
# newer glibc and the tarball will not run on 24.04.
#
# Assumes `gh` is already installed and signed in as a user who can read skymoore/wth.
# Builds only. Does not upload, tag, or touch the tap.
#
#   scp scripts/setup-ubuntu-build.sh box:
#   ssh box ./setup-ubuntu-build.sh
#
# Or, from a checkout on the box: ./scripts/setup-ubuntu-build.sh
#
# Env:
#   THEUTH_SRC          clone destination when not already in a checkout (default: ~/theuth)
#   THEUTH_REF          tag, branch, or sha to build (default: the clone's default branch)
#   THEUTH_ALLOW_GLIBC=1  build on a newer Ubuntu anyway
set -euo pipefail

REPO="skymoore/wth"
BUN_VERSION="1.4.0"
BUN_SHA256_X64="2d03fb5fb83ac8b567aca0a281b2ce1a1a19d488f56c2968d88c3f25e92fe452"
BUN_SHA256_AARCH64="4b1a332ee861983eb93bcfe6f770fff94e3e31b2c388bdaea3c8ed35e58eed0e"

die() { echo "error: $*" >&2; exit 1; }

[[ "$(id -u)" -ne 0 ]] || die "run as your user (the one \`gh\` is signed in as), not root"
[[ "$(uname -s)" == "Linux" ]] || die "Linux only (got $(uname -s))"
[[ -r /etc/os-release ]] || die "no /etc/os-release"
# shellcheck disable=SC1091
. /etc/os-release
[[ "${ID:-}" == "ubuntu" ]] || die "Ubuntu only (got ${ID:-unknown}); the tarball's glibc floor is Ubuntu 24.04"
if [[ "${VERSION_ID:-}" != "24.04" && "${THEUTH_ALLOW_GLIBC:-0}" != "1" ]]; then
  die "Ubuntu ${VERSION_ID:-?} links a newer glibc than the 2.39 floor. Use 24.04, or THEUTH_ALLOW_GLIBC=1 if you accept that."
fi

ARCH="$(uname -m | sed s/arm64/aarch64/)"
case "$ARCH" in
  x86_64) BUN_ARCH=x64; BUN_SHA="$BUN_SHA256_X64" ;;
  aarch64) BUN_ARCH=aarch64; BUN_SHA="$BUN_SHA256_AARCH64" ;;
  *) die "unsupported arch '$ARCH' (x86_64 | aarch64)" ;;
esac

command -v gh >/dev/null 2>&1 || die "gh required (already signed in)"
gh auth status >/dev/null 2>&1 || die "gh is not signed in (gh auth login)"
sudo -v

echo "==> apt packages (same set as deploy/update/Dockerfile)"
sudo apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  curl ca-certificates git build-essential clang cmake llvm libclang-dev pkg-config perl unzip \
  protobuf-compiler nasm \
  libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev libxcb-randr0-dev \
  libx11-dev libxtst-dev libxdo-dev libxkbcommon-dev \
  libdbus-1-dev \
  libpipewire-0.3-dev libspa-0.2-dev libdrm-dev libgbm-dev libwayland-dev libegl1-mesa-dev libgl-dev \
  libasound2-dev libpulse-dev libudev-dev \
  libssl-dev libcap-ng-dev \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf xdg-utils file

# Ubuntu's llvm package ships versioned binaries (llvm-objcopy-14). btls-sys wants the short names.
mkdir -p "$HOME/.local/bin"
shopt -s nullglob
for tool in llvm-objcopy llvm-nm; do
  command -v "$tool" >/dev/null 2>&1 && continue
  found=(/usr/bin/"$tool"-*)
  [[ ${#found[@]} -gt 0 ]] || die "$tool not installed (apt package llvm)"
  ln -sfn "${found[0]}" "$HOME/.local/bin/$tool"
done
shopt -u nullglob
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

if ! command -v bun >/dev/null 2>&1 || [[ "$(bun --version)" != "$BUN_VERSION" ]]; then
  echo "==> bun $BUN_VERSION"
  tmp="$(mktemp -d)"
  curl -fsSLo "$tmp/bun.zip" "https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/bun-linux-${BUN_ARCH}.zip"
  echo "$BUN_SHA  $tmp/bun.zip" | sha256sum -c -
  unzip -q "$tmp/bun.zip" -d "$tmp"
  install -m 755 "$tmp/bun-linux-${BUN_ARCH}/bun" "$HOME/.local/bin/bun"
  rm -rf "$tmp"
fi
[[ "$(bun --version)" == "$BUN_VERSION" ]] || die "bun $(bun --version), want $BUN_VERSION"
export OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1

if ! command -v rustup >/dev/null 2>&1; then
  echo "==> rustup"
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:$PATH"
command -v rustup >/dev/null 2>&1 || die "rustup not on PATH after install"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$SCRIPT_DIR/../Cargo.toml" && -f "$SCRIPT_DIR/app-parser-env.sh" ]]; then
  ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
  echo "==> checkout $ROOT"
else
  ROOT="${THEUTH_SRC:-$HOME/theuth}"
  if [[ ! -f "$ROOT/Cargo.toml" ]]; then
    echo "==> clone $REPO → $ROOT"
    gh repo clone "$REPO" "$ROOT"
  else
    echo "==> existing checkout $ROOT"
  fi
fi

if [[ -n "${THEUTH_REF:-}" ]]; then
  echo "==> checkout $THEUTH_REF"
  git -C "$ROOT" fetch --tags origin "$THEUTH_REF"
  git -C "$ROOT" checkout "$THEUTH_REF"
fi

cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

echo "==> rust toolchain"
rustup show active-toolchain >/dev/null

VERSION="$(awk '/^\[workspace.package\]/{p=1} p&&/^version =/{gsub(/"/,""); print $3; exit}' Cargo.toml)"
[[ -n "$VERSION" ]] || die "no version in Cargo.toml"
DIST="$ROOT/target/release/dist"
TGZ="$DIST/theuth_${VERSION}_linux-${ARCH}.tar.gz"

echo "==> cargo build --release -p theuth ($ARCH, $VERSION)"
cargo build --release -p theuth
[[ -x "$ROOT/target/release/t" ]] || die "target/release/t not produced"

echo "==> $HELPER"
build_helper
echo "==> $DOC_HELPER"
build_doc_helper

echo "==> $TGZ"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
install -m 755 "$ROOT/target/release/t" "$stage/t"
install -m 755 "$HELPER_SRC" "$stage/$HELPER"
install -m 755 "$DOC_HELPER_SRC" "$stage/$DOC_HELPER"
mkdir -p "$DIST"
rm -f "$TGZ"
tar --no-xattrs -czf "$TGZ" -C "$stage" t "$HELPER" "$DOC_HELPER"

listed="$(tar -tzf "$TGZ" | sed 's|^\./||' | sort | tr '\n' ' ')"
[[ "$listed" == "t $HELPER $DOC_HELPER " ]] || die "$TGZ lists '$listed', expected 't $HELPER $DOC_HELPER'"
ver_out="$("$stage/t" --version 2>&1)"
[[ "$ver_out" == *"$VERSION"* ]] || die "t --version printed '$ver_out', expected $VERSION"

echo
echo "done: $TGZ"
echo "  $(sha256sum "$TGZ")"
echo "  $listed"
echo "not uploaded. copy it to the release Mac's target/release/dist/ before just release,"
echo "or drop it at s3://theuth/staging/$VERSION/ for release-s3.sh to sign."
