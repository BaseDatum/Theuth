# The sandbox VM image (design doc 79 §6.1, contract §2.12): a Debian 13
# (trixie, glibc 2.41) systemd guest for microsandbox that boots into
# Xtigervnc :1 → XFCE → websockify/noVNC on 6080 → `t --headless` with DISPLAY=:1.
#
#   just vm-image-base                 # stage `base` only → ghcr.io/basedatum/theuth-vm-base:trixie (bootstrap route)
#   just vm-image tag=2.3.3            # buildx amd64+arm64, local
#   just vm-image tag=2.3.3 --push     # publish ghcr.io/basedatum/theuth-vm:2.3.3
#   just vm-image-dev                  # stage `desktop` + a local Linux `t` (Dockerfile.dev)
#
# Why trixie: the released Linux `t` needs glibc ≥ 2.39 (bookworm has 2.36);
# Debian 14 (forky) is testing and not a base for a shipped image.
#
# `t` comes from the release bucket's latest.json (platforms["cli-linux-<arch>"])
# and is minisign-verified with the same key as deploy/computer/Dockerfile.
# THEUTH_T_URL overrides the tarball URL (its .sig must sit beside it) for a
# release that has no asset for the target arch.

# --- base: the smallest systemd guest `msb run --init auto` can boot. Also the
# bootstrap route's base image (deploy/vm/bootstrap.sh installs the rest at first boot).
FROM debian:trixie-slim AS base

# No console login under microsandbox: serial-getty@hvc0 waits on dev-hvc0.device
# for minutes and keeps `is-system-running` in `starting` (79 S1 #8); tty1 likewise.
# libkrun's guest kernel has no loadable modules ("Failed to initialize kmod
# context"), so systemd-modules-load fails every boot and leaves `degraded`.
RUN set -eu; \
    export DEBIAN_FRONTEND=noninteractive; \
    apt-get update; \
    apt-get install -y --no-install-recommends systemd systemd-sysv dbus ca-certificates; \
    apt-get clean; rm -rf /var/lib/apt/lists/*; \
    systemctl mask serial-getty@hvc0.service getty@tty1.service systemd-modules-load.service

CMD ["/lib/systemd/systemd"]

# --- desktop: packages, Node LTS, uv, user `engine`, the four units. No `t` yet.
FROM base AS desktop

LABEL io.theuth.vm.schema=2

# One layer: apt set, Node LTS (nodesource), uv. Firefox ESR is ~250 MB (79 §12.9).
# libpipewire-0.3-0t64 is trixie's name for bookworm's libpipewire-0.3-0 (time64 ABI rename).
RUN set -eu; \
    export DEBIAN_FRONTEND=noninteractive; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      xfce4 xfce4-terminal xfce4-session dbus-x11 tigervnc-standalone-server tigervnc-tools \
      novnc websockify fonts-dejavu-core firefox-esr curl git \
      python3 ripgrep tar zstd minisign xdg-utils xauth x11-utils \
      x11-xserver-utils procps libgbm1 libpipewire-0.3-0t64; \
    curl -fsSL https://deb.nodesource.com/setup_lts.x | bash -; \
    apt-get install -y --no-install-recommends nodejs; \
    curl -fsSL https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin UV_NO_MODIFY_PATH=1 sh; \
    apt-get clean; rm -rf /var/lib/apt/lists/*

# uid 1000: files the host puts into /data are the engine's. A placeholder VNC
# password so xvnc.service starts before bring-up replaces it with the real one.
RUN set -eu; \
    useradd -m -u 1000 -s /bin/bash engine; \
    mkdir -p /data /home/engine/.vnc; \
    printf changeme | vncpasswd -f > /home/engine/.vnc/passwd; \
    chmod 600 /home/engine/.vnc/passwd; \
    chown -R engine:engine /data /home/engine/.vnc; \
    printf 'd /run/user/1000 0700 engine engine -\n' > /etc/tmpfiles.d/theuth-vm.conf

COPY units/ /etc/systemd/system/
RUN systemctl enable xvnc.service desktop.service novnc.service theuth.service

ENV THEUTH_DATA_DIR=/data
# CMD ["/lib/systemd/systemd"] inherited from `base`; `msb run --init auto` finds it.

# --- release: the minisign-verified `t` from the release manifest.
FROM desktop

# crates/update/pubkey.txt: Tauri's base64 of the minisign public key file.
ARG THEUTH_PUBKEY=dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEVBNTk5MEMwMkQ0RjcwRjEKUldUeGNFOHR3SkJaNmxORnpCUHlRUHYwMlV5bHhlUnoxbTQxRmROOWhlbis1ZVlNMTFOakMxU0sK
ARG THEUTH_MANIFEST=https://s3.rwx.dev/theuth/releases/latest.json
ARG THEUTH_T_URL=

RUN set -eu; \
    case "$(dpkg --print-architecture)" in amd64) a=x86_64 ;; arm64) a=aarch64 ;; *) echo "unsupported arch" >&2; exit 1 ;; esac; \
    if [ -n "$THEUTH_T_URL" ]; then \
      url="$THEUTH_T_URL"; curl -fsSL --retry 3 -o /tmp/t.sig "$url.sig"; \
    else \
      url="$(curl -fsSL --retry 3 "$THEUTH_MANIFEST" | python3 -c 'import json,sys; m=json.load(sys.stdin); k="cli-linux-'"$a"'"; p=m["platforms"].get(k) or sys.exit(f"release {m[\"version\"]} has no {k} asset; pass --build-arg THEUTH_T_URL="); open("/tmp/t.sig","w").write(p["signature"]); print(p["url"])')"; \
    fi; \
    curl -fsSL --retry 3 -o /tmp/t.tar.gz "$url"; \
    pub="$(printf '%s' "$THEUTH_PUBKEY" | base64 -d | sed -n 2p)"; \
    base64 -d /tmp/t.sig > /tmp/t.minisig; \
    minisign -Vq -P "$pub" -m /tmp/t.tar.gz -x /tmp/t.minisig; \
    tar -xzf /tmp/t.tar.gz -C /usr/local/bin; \
    rm -f /tmp/t.tar.gz /tmp/t.sig /tmp/t.minisig; \
    /usr/local/bin/t --version
