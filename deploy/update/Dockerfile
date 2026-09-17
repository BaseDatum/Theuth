# syntax=docker/dockerfile:1
# Linux `wth` for the update bucket (design doc 39 §5.1). Build from the repo root:
#   docker build -f deploy/update/Dockerfile --output type=local,dest=target/linux-out .
# The output stage is `FROM scratch` with one file, so `--output type=local` yields `target/linux-out/wth`.
#
# Why ubuntu:22.04 and not musl/alpine?
#   glibc 2.35 is the oldest we support (Ubuntu 22.04+, Debian 12+, Fedora 36+, Homebrew on
#   Linux). musl is out: V8 (deno_core) ships no musl build, BoringSSL/btls-sys and aws-lc-sys
#   assume glibc, and keyring's dbus-secret-service dlopens system libraries at runtime.
#
# Why native only (no --platform, no QEMU)?
#   V8 and BoringSSL under emulation take hours. Run this on a box of the target arch
#   (arm64 Mac with Docker Desktop → linux-aarch64, x86_64 Linux → linux-x86_64).
#   `scripts/stage-linux.sh` uploads the result to staging/<ver>/ for release-s3.sh.

# ── build ─────────────────────────────────────────────────────────────────────
FROM ubuntu:22.04 AS build
ARG TARGETARCH
ARG DEBIAN_FRONTEND=noninteractive
# Toolchain: obscura/btls/boringssl/aws-lc need clang+cmake+perl; libspa-sys runs bindgen (libclang).
# X11/xcb: enigo + xcap.  xdo/xkbcommon: enigo.  dbus: keyring (dbus-secret-service).
# pipewire/drm/gbm/wayland: xcap's Wayland capture path.  libssl-dev: openssl-probe stragglers.
RUN apt-get update && apt-get install -y --no-install-recommends \
      curl ca-certificates git build-essential clang cmake llvm libclang-dev pkg-config perl \
      libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev libxcb-randr0-dev \
      libx11-dev libxtst-dev libxdo-dev libxkbcommon-dev \
      libdbus-1-dev \
      libpipewire-0.3-dev libdrm-dev libgbm-dev libwayland-dev \
      libssl-dev \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
WORKDIR /src
# Toolchain channel comes from the repo, so this layer only rebuilds when rust-toolchain.toml changes.
COPY rust-toolchain.toml ./
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none \
 && rustup toolchain install "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)" --profile minimal

COPY . /src
# Cache ids are per-arch (deploy/relay/Dockerfile:32-35). Cache mounts are not part of the
# layer, so the binary is copied out before the mount goes away.
RUN --mount=type=cache,id=wth-update-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=wth-update-cargo-git-${TARGETARCH},target=/usr/local/cargo/git \
    --mount=type=cache,id=wth-update-cargo-target-${TARGETARCH},target=/src/target \
    cargo build --release -p wth \
 && cp target/release/wth /wth \
 && /wth --version

# ── output ────────────────────────────────────────────────────────────────────
FROM scratch
COPY --from=build /wth /wth
