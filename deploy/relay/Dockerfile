# syntax=docker/dockerfile:1
# theuth-relay + browser UI (design doc 31 §5.4 / §7.6) + theuth.io page. Build from the repo root:
#   docker build -f deploy/relay/Dockerfile -t theuth-relay .
# Static musl binary, rustls (no OpenSSL), `FROM scratch`. UI is Vite `dist/` at `/web` (port 8787);
# the marketing page is Vite `dist/` at `/site` (port 8788, `--site`).
#
# Why BUILDPLATFORM on the UI stage?
#   Bun SIGILL/segfaults under QEMU (classic arm64 host → linux/amd64). The Vite
#   output is arch-independent, so the SPA always compiles on the native builder.

# ── UI (always native) ────────────────────────────────────────────────────────
FROM --platform=$BUILDPLATFORM oven/bun:1.4-alpine AS web
# Layout matches the repo: ui tsconfig/vite resolve `@theuth/apps` and the
# patched `@openuidev/*` install at ../../../packages/apps (46), and
# `@theuth/documents` from source (59). vite.config.ts also reads
# ../tauri.conf.json for __APP_VERSION__. Documents is consumed from source
# against the UI's `@blocknote/core` (vite `documentsDeps`); no bun install.
WORKDIR /src/crates/desktop/ui
COPY crates/desktop/tauri.conf.json /src/crates/desktop/tauri.conf.json
COPY packages/apps /src/packages/apps
RUN bun install --frozen-lockfile --cwd /src/packages/apps
COPY packages/documents /src/packages/documents
COPY crates/desktop/ui/package.json crates/desktop/ui/bun.lock ./
RUN bun install --frozen-lockfile
COPY crates/desktop/ui/ ./
RUN bun run build

# ── theuth.io page (always native) ────────────────────────────────────────────
# vite.config.ts reads ../crates/desktop/tauri.conf.json for __APP_VERSION__ (copied above).
FROM --platform=$BUILDPLATFORM oven/bun:1.4-alpine AS site
WORKDIR /src/site
COPY crates/desktop/tauri.conf.json /src/crates/desktop/tauri.conf.json
COPY site/package.json site/bun.lock ./
RUN bun install --frozen-lockfile
COPY site/ ./
RUN bun run build

# ── relay ─────────────────────────────────────────────────────────────────────
# Copy relay and its only in-repo dependency (`platform`: SIGTERM stop).
# The workspace [patch.crates-io] block pulls git forks (btls-sys → boringssl
# submodule, anydoc, brush, …) that -p relay never uses; resolving them under
# buildx is what broke the amd64 build. `platform` is path-only (tokio, anyhow,
# dirs, libc) and has no further path deps, so it does not reintroduce those forks.
FROM rust:1-alpine AS build
ARG TARGETPLATFORM
# build-base/cmake/perl/linux-headers: aws-lc-sys (rustls' provider) compiles C.
RUN apk add --no-cache build-base cmake perl linux-headers
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/relay ./crates/relay
COPY crates/platform ./crates/platform
RUN sed -i 's|members = \["crates/\*"\]|members = ["crates/relay", "crates/platform"]|' Cargo.toml \
 && sed -i '/^\[patch\.crates-io\]/,$d' Cargo.toml
ENV RUSTFLAGS="-C target-feature=+crt-static"
# Cache ids are per-platform: buildx runs amd64+arm64 in parallel and a shared
# registry cache races on `.cargo-ok` (`File exists`).
RUN --mount=type=cache,id=theuth-relay-cargo-registry-${TARGETPLATFORM},target=/usr/local/cargo/registry \
    --mount=type=cache,id=theuth-relay-cargo-target-${TARGETPLATFORM},target=/src/target \
    target="$(rustc -vV | sed -n 's/^host: //p')" \
 && cargo build --release -p relay --target "$target" \
 && cp "target/$target/release/theuth-relay" /theuth-relay

# ── runtime ───────────────────────────────────────────────────────────────────
FROM scratch
COPY --from=build /theuth-relay /theuth-relay
COPY --from=web --chown=65532:65532 /src/crates/desktop/ui/dist /web
COPY --from=site --chown=65532:65532 /src/site/dist /site
USER 65532
EXPOSE 8787 8788
ENTRYPOINT ["/theuth-relay"]
CMD ["--listen", "0.0.0.0:8787", "--web", "/web", "--site", "/site", "--site-listen", "0.0.0.0:8788"]
