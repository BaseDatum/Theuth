#!/bin/sh
# First-boot bootstrap for a ghcr.io/basedatum/theuth-vm-base guest (the `base`
# stage of deploy/vm/Dockerfile: debian:trixie-slim + systemd, getty masked;
# design doc 79 §6.1 "bootstrap image"): the same packages, user, units and
# enable+start as the `desktop` stage, from Debian's mirrors only.
# Idempotent: exits at once when the marker exists; every step below is safe to
# rerun if a first run died halfway. Run as root.
# `t` is not fetched here — bring-up `put`s a verified binary at
# /usr/local/bin/t afterwards (crates/sandbox/src/release.rs); theuth.service
# restarts every 2 s until it appears.
set -eu

MARKER=/var/lib/theuth-vm/bootstrapped
[ -e "$MARKER" ] && { echo "theuth-vm: already bootstrapped"; exit 0; }

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  xfce4 xfce4-terminal xfce4-session dbus-x11 tigervnc-standalone-server tigervnc-tools \
  novnc websockify fonts-dejavu-core firefox-esr ca-certificates curl git \
  python3 ripgrep tar zstd minisign xdg-utils xauth x11-utils \
  x11-xserver-utils procps libgbm1 libpipewire-0.3-0t64
curl -fsSL https://deb.nodesource.com/setup_lts.x | bash -
apt-get install -y --no-install-recommends nodejs
curl -fsSL https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin UV_NO_MODIFY_PATH=1 sh
apt-get clean
rm -rf /var/lib/apt/lists/*

id engine >/dev/null 2>&1 || useradd -m -u 1000 -s /bin/bash engine
mkdir -p /data /home/engine/.vnc
if [ ! -s /home/engine/.vnc/passwd ]; then
  printf changeme | vncpasswd -f > /home/engine/.vnc/passwd
  chmod 600 /home/engine/.vnc/passwd
fi
chown -R engine:engine /data /home/engine/.vnc
printf 'd /run/user/1000 0700 engine engine -\n' > /etc/tmpfiles.d/theuth-vm.conf
systemd-tmpfiles --create /etc/tmpfiles.d/theuth-vm.conf

# Units: keep in step with deploy/vm/units/*.service.
cat > /etc/systemd/system/xvnc.service <<'EOF'
[Unit]
Description=Xtigervnc :1 (RFB on localhost:5901, VncAuth from /home/engine/.vnc/passwd)
After=systemd-tmpfiles-setup.service

[Service]
User=engine
Group=engine
Environment=HOME=/home/engine
ExecStart=/usr/bin/Xtigervnc :1 -rfbauth /home/engine/.vnc/passwd -geometry 1440x900 -depth 24 -localhost -AcceptSetDesktopSize -ac
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF

cat > /etc/systemd/system/desktop.service <<'EOF'
[Unit]
Description=XFCE session on DISPLAY=:1
After=xvnc.service
Requires=xvnc.service

[Service]
User=engine
Group=engine
Environment=DISPLAY=:1 XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/engine
WorkingDirectory=/home/engine
ExecStartPre=/bin/sh -c 'for n in $$(seq 100); do xdpyinfo -display :1 >/dev/null 2>&1 && exit 0; sleep 0.1; done; exit 1'
ExecStart=/usr/bin/dbus-launch --exit-with-session startxfce4
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

cat > /etc/systemd/system/novnc.service <<'EOF'
[Unit]
Description=websockify on 6080 serving noVNC, bridging to Xtigervnc :1
After=xvnc.service

[Service]
ExecStart=/usr/bin/websockify --web=/usr/share/novnc 0.0.0.0:6080 localhost:5901
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF

cat > /etc/systemd/system/theuth.service <<'EOF'
[Unit]
Description=theuth engine (t --headless) on DISPLAY=:1
After=network-online.target desktop.service
Wants=network-online.target

[Service]
User=engine
Group=engine
Environment=DISPLAY=:1 THEUTH_DATA_DIR=/data THEUTH_LAUNCH=1 HOME=/home/engine XDG_RUNTIME_DIR=/run/user/1000
WorkingDirectory=/home/engine
ExecStart=/usr/local/bin/t --headless
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now xvnc.service desktop.service novnc.service theuth.service
# serial-getty@hvc0 / getty@tty1 are masked in the base image (79 S1 #8).

mkdir -p "$(dirname "$MARKER")"
date -u +%FT%TZ > "$MARKER"
echo "theuth-vm: bootstrapped"
