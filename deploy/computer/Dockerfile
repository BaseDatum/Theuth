# An engine in a container (design doc 64 §4.3, kept by 67 §13): `t --headless`
# from the signed release tarball, no display, no published ports — it reaches
# the mesh over the relay / LAN like any engine.
#
#   docker build --build-arg THEUTH_VERSION=1.16.0 --build-arg THEUTH_PUBKEY="$(cat crates/update/pubkey.txt)" \
#     -t theuth-engine:1.16.0 deploy/computer
#
# `t mesh add --docker` builds it with this binary's version and pubkey.
FROM ubuntu:24.04

ARG THEUTH_VERSION
# crates/update/pubkey.txt: Tauri's base64 of the minisign public key file.
ARG THEUTH_PUBKEY

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git python3 ripgrep tar zstd minisign \
 && rm -rf /var/lib/apt/lists/*

# theuth_<version>_linux-<arch>.tar.gz + .sig from the release bucket, verified
# with minisign before anything is extracted (39 §5.1).
RUN set -eu; \
    case "$(dpkg --print-architecture)" in amd64) a=x86_64 ;; arm64) a=aarch64 ;; *) echo "unsupported arch" >&2; exit 1 ;; esac; \
    f="theuth_${THEUTH_VERSION}_linux-${a}.tar.gz"; \
    base="https://s3.rwx.dev/theuth/releases/${THEUTH_VERSION}"; \
    curl -fsSL --retry 3 -o "/tmp/$f" "$base/$f"; \
    curl -fsSL --retry 3 -o /tmp/t.sig "$base/$f.sig"; \
    pub="$(printf '%s' "$THEUTH_PUBKEY" | base64 -d | sed -n 2p)"; \
    base64 -d /tmp/t.sig > /tmp/t.minisig; \
    minisign -Vq -P "$pub" -m "/tmp/$f" -x /tmp/t.minisig; \
    tar -xzf "/tmp/$f" -C /usr/local/bin; \
    rm -f "/tmp/$f" /tmp/t.sig /tmp/t.minisig

# uid 1000 so files the parent uploads (owned 1000) are the engine's.
RUN userdel -r ubuntu 2>/dev/null || true; \
    useradd -m -u 1000 engine \
 && mkdir -p /data /workspace \
 && chown engine:engine /data /workspace

USER engine
ENV THEUTH_DATA_DIR=/data
VOLUME ["/data", "/workspace"]
ENTRYPOINT ["t", "--headless", "--data-dir", "/data"]
