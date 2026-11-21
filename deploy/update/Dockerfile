# syntax=docker/dockerfile:1
# Linux `theuth` + `theuth-app-parser-<hash>` + `theuth-documents-<ver>` for the update bucket (design docs 39 §5.1, 46 §2.1, 59 §3.2). Build from the repo root:
#   docker build -f deploy/update/Dockerfile --output type=local,dest=target/linux-out .
# The output stage is `FROM scratch` with three files, so `--output type=local` yields
# `target/linux-out/t`, `target/linux-out/theuth-app-parser-<hash>` and `target/linux-out/theuth-documents-<ver>`.
#
# The app-parser entry is the compatibility stub (crates/app-parser-stub; `t` validates apps in-process,
# docs/design/apps-inproc-as-built-P1.md), built by cargo with `t`. The documents helper is a
# `bun build --compile` executable: Bun is pinned by version + sha256 and used at build time only.
# The hash is packages/apps/generated/library-hash.txt, the value `theuth` embeds (scripts/app-parser-env.sh).
#
# Why ubuntu:24.04 and not musl/alpine?
#   glibc 2.39 is the oldest we support (Ubuntu 24.04+, Debian 13+, Fedora 40+, Homebrew on
#   Linux). musl is out: V8 (deno_core) ships no musl build, BoringSSL/btls-sys and aws-lc-sys
#   assume glibc, and keyring's dbus-secret-service dlopens system libraries at runtime.
#
# Why native only (no --platform, no QEMU)?
#   V8 and BoringSSL under emulation take hours. Run this on a box of the target arch
#   (arm64 Mac with Docker Desktop → linux-aarch64, x86_64 Linux → linux-x86_64).
#   `scripts/stage-linux.sh` uploads the result to staging/<ver>/ for release-s3.sh.

# ── build ─────────────────────────────────────────────────────────────────────
FROM ubuntu:24.04 AS build
ARG TARGETARCH
ARG DEBIAN_FRONTEND=noninteractive
# Toolchain: obscura/btls/boringssl/aws-lc need clang+cmake+perl; libspa-sys runs bindgen (libclang).
# X11/xcb: enigo + xcap.  xdo/xkbcommon: enigo.  dbus: keyring (dbus-secret-service).
# pipewire/drm/gbm/wayland: xcap's Wayland capture path.  libssl-dev: openssl-probe stragglers.
# egl/gl: khronos-egl (wgpu/xcap GPU paths).  protoc: signal's spqr (prost-build); nasm: aws-lc-sys / boringssl assembly.
# asound/pulse/udev: meeting capture off macOS (flexaudio: cpal/ALSA mic, PipeWire + Pulse loopback; 58 step 10).
RUN apt-get update && apt-get install -y --no-install-recommends \
      curl ca-certificates git build-essential clang cmake llvm libclang-dev pkg-config perl unzip \
      protobuf-compiler nasm \
      libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev libxcb-randr0-dev \
      libx11-dev libxtst-dev libxdo-dev libxkbcommon-dev \
      libdbus-1-dev \
      libpipewire-0.3-dev libspa-0.2-dev libdrm-dev libgbm-dev libwayland-dev libegl1-mesa-dev libgl-dev \
      libasound2-dev libpulse-dev libudev-dev \
      libssl-dev libcap-ng-dev \
 && rm -rf /var/lib/apt/lists/*

# Bun, pinned (SHASUMS256.txt of the GitHub release). Bump both sums with the version.
ARG BUN_VERSION=1.4.2
ARG BUN_SHA256_X64=36368faef7527875d5ffa52e53cd48021741f2a83eb6208a8dd64068d422a913
ARG BUN_SHA256_AARCH64=54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7
RUN case "$TARGETARCH" in \
      amd64) bun_arch=x64; bun_sha="$BUN_SHA256_X64" ;; \
      arm64) bun_arch=aarch64; bun_sha="$BUN_SHA256_AARCH64" ;; \
      *) echo "unsupported TARGETARCH $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && curl -fsSLo /tmp/bun.zip "https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/bun-linux-${bun_arch}.zip" \
 && echo "$bun_sha  /tmp/bun.zip" | sha256sum -c - \
 && unzip -q /tmp/bun.zip -d /tmp \
 && install -m 755 "/tmp/bun-linux-${bun_arch}/bun" /usr/local/bin/bun \
 && rm -rf /tmp/bun.zip "/tmp/bun-linux-${bun_arch}" \
 && bun --version | grep -qx "$BUN_VERSION"
ENV OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
WORKDIR /src
# Toolchain channel comes from the repo, so this layer only rebuilds when rust-toolchain.toml changes.
COPY rust-toolchain.toml ./
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none \
 && rustup toolchain install "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)" --profile minimal

COPY . /src
# Cache ids are per-arch (deploy/relay/Dockerfile:32-35). Cache mounts are not part of the
# layer, so the binary is copied out before the mount goes away.
RUN --mount=type=cache,id=theuth-update-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=theuth-update-cargo-git-${TARGETARCH},target=/usr/local/cargo/git \
    --mount=type=cache,id=theuth-update-cargo-target-${TARGETARCH},target=/src/target \
    cargo build --release -p theuth -p app-parser-stub \
 && cp target/release/t /t \
 && /t --version \
 && hash="$(tr -d '[:space:]' <packages/apps/generated/library-hash.txt)" \
 && echo "$hash" | grep -Eqx '[0-9a-f]{64}' \
 && install -m 755 target/release/theuth-app-parser-stub "/theuth-app-parser-$hash" \
 && "/theuth-app-parser-$hash" --version | grep -q "$hash" \
 && printf '' | "/theuth-app-parser-$hash"

# Documents helper (59 §3.2): named by PROTOCOL_VERSION of packages/documents/src/protocol.ts,
# which `theuth` embeds at compile time; `--version` must echo it.
RUN --mount=type=cache,id=theuth-update-bun-${TARGETARCH},target=/root/.bun/install/cache \
    cd packages/documents \
 && bun install --frozen-lockfile \
 && bun run build:helper \
 && ver="$(grep 'PROTOCOL_VERSION = ' src/protocol.ts | tr -dc '0-9.')" \
 && echo "$ver" | grep -Eqx '[0-9.]+' \
 && install -m 755 dist/theuth-documents "/theuth-documents-$ver" \
 && "/theuth-documents-$ver" --version | grep -qx "theuth-documents $ver" \
 && printf '' | "/theuth-documents-$ver"

# ── output ────────────────────────────────────────────────────────────────────
FROM scratch
COPY --from=build /t /theuth-app-parser-* /theuth-documents-* /
