# syntax=docker/dockerfile:1
# wth-relay (design doc 31 §5.4). Build from the repo root:
#   docker build -f deploy/relay/Dockerfile -t wth-relay .
# Static musl binary, rustls (no OpenSSL), `FROM scratch`, ≈ 10 MB.

FROM rust:1-alpine AS build
# build-base/cmake/perl/linux-headers: aws-lc-sys (rustls' provider) compiles C.
RUN apk add --no-cache build-base cmake perl linux-headers git
WORKDIR /src
COPY . .
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    target="$(rustc -vV | sed -n 's/^host: //p')" \
 && cargo build --release --locked -p relay --target "$target" \
 && cp "target/$target/release/wth-relay" /wth-relay

FROM scratch
COPY --from=build /wth-relay /wth-relay
USER 65532
EXPOSE 8787
ENTRYPOINT ["/wth-relay"]
CMD ["--listen", "0.0.0.0:8787"]
