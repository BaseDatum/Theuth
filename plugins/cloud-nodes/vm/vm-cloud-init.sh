#!/bin/bash
# A native theuth node on a VM (design doc 224 §9): no Docker. Rendered into the provider's
# user-data by scripts/cloud-node-do-test.sh and the cloud-nodes plugin, which fill in the
# double-underscore placeholders below: the node NAME (mesh.name), DATA_DEV (the block device for
# /data), TICKET (the one-time pairing ticket, or empty), MANIFEST (the release manifest URL),
# PUBKEY (crates/update/pubkey.txt: Tauri's base64 of the minisign public key file) and
# INSTALL_SH (base64 of deploy/computer/theuth-install.sh, shared with the engine image).
# User `engine` (uid 1000) owns /data, so the engine's updater replaces /data/bin/t in place; systemd
# restarts it (Restart=always) after the updater's exit 75. No `set -x`: the ticket must not reach
# a log.
set -euo pipefail
exec >>/var/log/theuth-node.log 2>&1
echo "theuth-node: boot $(date -u +%FT%TZ)"
name='__NAME__'
dev='__DATA_DEV__'
ticket='__TICKET__'

# Keys only, whatever the image defaults to (the firewall has no inbound rule anyway).
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\n' >/etc/ssh/sshd_config.d/00-theuth.conf
systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true

# 1 GiB swap: a 1 GiB VM and a 1.2 GB engine binary. Not fatal: a host may refuse swap.
if [ ! -f /swapfile ]; then
	if fallocate -l 1G /swapfile && chmod 600 /swapfile && mkswap -q /swapfile && swapon /swapfile; then
		echo '/swapfile none swap sw 0 0' >>/etc/fstab
	else
		echo "theuth-node: no swap"
	fi
fi

# The engine's user: uid 1000 (an image's default `ubuntu` user gives the uid up).
if ! id -u engine >/dev/null 2>&1; then
	userdel -r ubuntu 2>/dev/null || true
	useradd -m -u 1000 -s /bin/bash engine
fi

# Agents on a node are meant to have root (owner decision, design doc 224 §9): passwordless sudo
# for `engine`. Written to a temp file and checked with visudo before it goes live, so a bad line
# can never lock sudo out.
if [ ! -e /etc/sudoers.d/90-theuth-engine ]; then
	printf 'engine ALL=(ALL) NOPASSWD:ALL\n' >/etc/sudoers.d/.90-theuth-engine.tmp
	chmod 0440 /etc/sudoers.d/.90-theuth-engine.tmp
	if visudo -cf /etc/sudoers.d/.90-theuth-engine.tmp; then
		mv /etc/sudoers.d/.90-theuth-engine.tmp /etc/sudoers.d/90-theuth-engine
		visudo -c >/dev/null || { rm -f /etc/sudoers.d/90-theuth-engine; echo "theuth-node: sudoers rejected"; }
	else
		rm -f /etc/sudoers.d/.90-theuth-engine.tmp
		echo "theuth-node: sudoers drop-in invalid, not installed"
	fi
fi

# The volume at /data: formatted once (never over an existing filesystem).
for _ in $(seq 1 90); do [ -e "$dev" ] && break; sleep 2; done
[ -e "$dev" ] || { echo "theuth-node: volume $dev missing"; exit 1; }
blkid "$dev" >/dev/null || mkfs.ext4 -q -L theuth-data "$dev"
mkdir -p /data
grep -q ' /data ' /etc/fstab || echo "$dev /data ext4 defaults,nofail,discard,noatime 0 2" >>/etc/fstab
mountpoint -q /data || mount /data
chown engine:engine /data

# What the Linux `t` needs: the minisign check, and the shared libraries it links.
apt-get update -q
DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends \
	ca-certificates curl git python3 ripgrep tar zstd minisign \
	libgbm1 libwayland-client0 libxcb1 libasound2t64 libpipewire-0.3-0t64

mkdir -p /usr/local/share/theuth
printf '%s' '__PUBKEY__' >/usr/local/share/theuth/pubkey.txt
echo '__INSTALL_SH__' | base64 -d >/usr/local/lib/theuth-install.sh
chmod 755 /usr/local/lib/theuth-install.sh
# shellcheck disable=SC2016 # written literally, expanded at login
echo 'case ":$PATH:" in *:/data/bin:*) ;; *) PATH="/data/bin:$PATH" ;; esac' >/etc/profile.d/theuth.sh

if [ -n "$ticket" ] && [ ! -e /data/.theuth-paired ]; then
	(umask 077 && printf '%s\n' "$ticket" >/data/pair-ticket)
	chown engine:engine /data/pair-ticket
fi
runuser -u engine -- env THEUTH_DATA_DIR=/data /usr/local/lib/theuth-install.sh config "$name" server

cat >/etc/systemd/system/theuth-engine.service <<'EOF'
[Unit]
Description=theuth engine (design doc 224)
Wants=network-online.target
After=network-online.target
RequiresMountsFor=/data
StartLimitIntervalSec=0

[Service]
User=engine
Group=engine
Environment=THEUTH_DATA_DIR=/data
Environment=THEUTH_MANIFEST=__MANIFEST__
# The newest signed release into /data/bin (download + minisign on first start).
ExecStartPre=/usr/local/lib/theuth-install.sh install
ExecStart=/data/bin/t --headless --data-dir /data
Restart=always
RestartSec=5
TimeoutStartSec=1200

[Install]
WantedBy=multi-user.target
EOF

cat >/etc/systemd/system/theuth-join.service <<'EOF'
[Unit]
Description=theuth: join the mesh with /data/pair-ticket (until paired)
After=theuth-engine.service
RequiresMountsFor=/data
ConditionPathExists=!/data/.theuth-paired
StartLimitIntervalSec=0

[Service]
User=engine
Group=engine
Environment=THEUTH_DATA_DIR=/data
ExecStart=/usr/local/lib/theuth-install.sh join
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now --no-block theuth-engine.service theuth-join.service
echo "theuth-node: units started $(date -u +%FT%TZ)"
