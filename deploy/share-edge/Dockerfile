# syntax=docker/dockerfile:1
# theuth-share-edge (design doc 210 §8.4). Build from the repo root:
#   docker build -f deploy/share-edge/Dockerfile -t theuth-share-edge .
#   just share-edge-docker-push            # linux/amd64 + linux/arm64, pushed to ghcr
# Static musl binary, rustls (no OpenSSL), `FROM scratch`, as deploy/relay/Dockerfile.
#
# Cross-compiled: the build stage runs on $BUILDPLATFORM and cargo-zigbuild (zig as the C compiler
# and linker for aws-lc-sys and the bundled SQLite) targets $TARGETARCH's musl triple, so no Rust or
# C runs under QEMU when an arm64 Mac builds the amd64 image (or the reverse).
#
# Reproducibility: the build starts from the repo's committed Cargo.lock. Only share-edge and its
# in-repo path dependencies (`share-proto`, `platform`) are copied, and the workspace
# [patch.crates-io] block is cut (it pulls git forks and `vendor/` crates the edge never uses, as for
# the relay). The one patch the edge does reach is `vendor/libsqlite3-sys` (SQLCipher over BoringSSL,
# for the Signal store); without it the lock's path entry is orphaned, so `cargo metadata` re-resolves
# once (cargo keeps every existing lock entry and adds only the missing crates.io `libsqlite3-sys`), and
# `cargo update --precise` pins that to 0.36.0, the version the vendored copy forks, with its `bundled`
# SQLite (sqlx's feature). The edge does not use SQLCipher. The build then runs `--locked`.

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
ARG TARGETARCH
# Cargo features of share-edge, e.g. `--build-arg FEATURES=k8s` (the edge manages its Ingresses; rwx-dev overlay).
ARG FEATURES=""
# Zig + cargo-zigbuild from PyPI, pinned.
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3-pip \
 && rm -rf /var/lib/apt/lists/* \
 && pip3 install --break-system-packages --no-cache-dir ziglang==0.13.0 cargo-zigbuild==0.24.1
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "unsupported TARGETARCH $TARGETARCH" >&2; exit 1 ;; \
    esac > /target \
 && rustup target add "$(cat /target)"
COPY crates/share-edge ./crates/share-edge
COPY crates/share-proto ./crates/share-proto
COPY crates/platform ./crates/platform
# Trim the workspace, drop the patches, re-resolve and pin libsqlite3-sys (see the header).
RUN --mount=type=cache,id=theuth-share-edge-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    sed -i 's|members = \["crates/\*"\]|members = ["crates/share-edge", "crates/share-proto", "crates/platform"]|' Cargo.toml \
 && sed -i '/^\[patch\.crates-io\]/,$d' Cargo.toml \
 && cargo metadata --format-version 1 >/dev/null \
 && cargo update -p libsqlite3-sys --precise 0.36.0
RUN --mount=type=cache,id=theuth-share-edge-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=theuth-share-edge-cargo-target-${TARGETARCH},target=/src/target \
    target="$(cat /target)" \
 && cargo zigbuild --locked --release -p share-edge ${FEATURES:+--features "$FEATURES"} --target "$target" \
 && cp "target/$target/release/theuth-share-edge" /theuth-share-edge

FROM scratch
COPY --from=build /theuth-share-edge /theuth-share-edge
USER 65532:65532
# 8443/tcp visitors (the Service or host maps 443), 7850/udp iroh, 9090/tcp /healthz and /metrics.
EXPOSE 8443/tcp 7850/udp 9090/tcp
ENTRYPOINT ["/theuth-share-edge"]
CMD ["run", "--config", "/etc/theuth-share-edge/config.yaml"]
