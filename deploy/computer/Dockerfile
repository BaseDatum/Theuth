# An engine in a container (design doc 64 §4.3, kept by 67 §13; hosted nodes 224 §5): `t --headless`
# from the signed release tarball, no display, no published ports — it reaches the mesh over the
# relay / LAN like any engine.
#
#   docker build --build-arg THEUTH_VERSION=2.31.2 --build-arg THEUTH_PUBKEY="$(cat crates/update/pubkey.txt)" \
#     -t theuth-engine:2.31.2 deploy/computer
#
# `t mesh add --docker` builds it with this binary's version and pubkey (context: this file,
# entrypoint.sh and theuth-install.sh). The baked binary lives in /usr/local/lib/theuth;
# theuth-install.sh (shared with the native VM install, 224 §9) copies the newest signed `t` into
# /data/bin on every start and the entrypoint runs that one, so the engine's own updater can
# replace it in place (uid 1000 owns /data) and a recreated container keeps the newer version.
FROM ubuntu:24.04

ARG THEUTH_VERSION
# crates/update/pubkey.txt: Tauri's base64 of the minisign public key file.
ARG THEUTH_PUBKEY
# The manifest theuth-install.sh checks at start (release.yml passes latest-prerelease.json for prereleases).
ARG THEUTH_MANIFEST=https://s3.rwx.dev/theuth/releases/latest.json

# libgbm1 … libpipewire: the shared libraries the Linux `t` links (`ldd` on 2.31.2: without them it
# exits 127 before main).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git python3 ripgrep tar zstd minisign tini util-linux \
      libgbm1 libwayland-client0 libxcb1 libasound2t64 libpipewire-0.3-0t64 \
 && rm -rf /var/lib/apt/lists/*

# theuth_<version>_linux-<arch>.tar.gz + .sig from the release bucket, verified
# with minisign before anything is extracted (39 §5.1). The key stays in the
# image: theuth-install.sh verifies later releases against it.
RUN set -eu; \
    case "$(dpkg --print-architecture)" in amd64) a=x86_64 ;; arm64) a=aarch64 ;; *) echo "unsupported arch" >&2; exit 1 ;; esac; \
    f="theuth_${THEUTH_VERSION}_linux-${a}.tar.gz"; \
    base="https://s3.rwx.dev/theuth/releases/${THEUTH_VERSION}"; \
    curl -fsSL --retry 3 -o "/tmp/$f" "$base/$f"; \
    curl -fsSL --retry 3 -o /tmp/t.sig "$base/$f.sig"; \
    mkdir -p /usr/local/share/theuth /usr/local/lib/theuth; \
    printf '%s' "$THEUTH_PUBKEY" > /usr/local/share/theuth/pubkey.txt; \
    pub="$(base64 -d /usr/local/share/theuth/pubkey.txt | sed -n 2p)"; \
    base64 -d /tmp/t.sig > /tmp/t.minisig; \
    minisign -Vq -P "$pub" -m "/tmp/$f" -x /tmp/t.minisig; \
    tar --no-same-owner -xzf "/tmp/$f" -C /usr/local/lib/theuth; \
    ln -s /usr/local/lib/theuth/t /usr/local/bin/t; \
    rm -f "/tmp/$f" /tmp/t.sig /tmp/t.minisig

COPY entrypoint.sh /usr/local/bin/theuth-entrypoint
COPY theuth-install.sh /usr/local/lib/theuth-install.sh
RUN chmod 755 /usr/local/bin/theuth-entrypoint /usr/local/lib/theuth-install.sh

# uid 1000 so files the parent uploads (owned 1000) are the engine's.
RUN userdel -r ubuntu 2>/dev/null || true; \
    useradd -m -u 1000 engine \
 && mkdir -p /data /workspace \
 && chown engine:engine /data /workspace

USER engine
ENV THEUTH_DATA_DIR=/data \
    THEUTH_MANIFEST=${THEUTH_MANIFEST} \
    PATH=/data/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
VOLUME ["/data", "/workspace"]
# Arguments after the image name are passed on to `t --headless --data-dir /data`.
ENTRYPOINT ["theuth-entrypoint"]
