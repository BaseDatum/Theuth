# syntax=docker/dockerfile:1
# theuth-share-edge (design doc 210 §8.4). Build from the repo root:
#   docker build -f deploy/share-edge/Dockerfile -t theuth-share-edge .
# Static musl binary, rustls (no OpenSSL), `FROM scratch`, as deploy/relay/Dockerfile.

# Copy share-edge and its only in-repo dependencies (`share-proto`, `platform`). The workspace
# [patch.crates-io] block pulls git forks the edge never uses, so it is cut, as for the relay.
FROM rust:1-alpine AS build
ARG TARGETPLATFORM
# build-base/cmake/perl/linux-headers: aws-lc-sys (rustls' provider) compiles C.
RUN apk add --no-cache build-base cmake perl linux-headers
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/share-edge ./crates/share-edge
COPY crates/share-proto ./crates/share-proto
COPY crates/platform ./crates/platform
RUN sed -i 's|members = \["crates/\*"\]|members = ["crates/share-edge", "crates/share-proto", "crates/platform"]|' Cargo.toml \
 && sed -i '/^\[patch\.crates-io\]/,$d' Cargo.toml
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN --mount=type=cache,id=theuth-share-edge-cargo-registry-${TARGETPLATFORM},target=/usr/local/cargo/registry \
    --mount=type=cache,id=theuth-share-edge-cargo-target-${TARGETPLATFORM},target=/src/target \
    target="$(rustc -vV | sed -n 's/^host: //p')" \
 && cargo build --release -p share-edge --target "$target" \
 && cp "target/$target/release/theuth-share-edge" /theuth-share-edge

FROM scratch
COPY --from=build /theuth-share-edge /theuth-share-edge
USER 65532:65532
# 8443/tcp visitors (the Service or host maps 443), 7850/udp iroh, 9090/tcp /healthz and /metrics.
EXPOSE 8443/tcp 7850/udp 9090/tcp
ENTRYPOINT ["/theuth-share-edge"]
CMD ["run", "--config", "/etc/theuth-share-edge/config.yaml"]
