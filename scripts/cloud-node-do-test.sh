#!/usr/bin/env bash
# A hosted theuth node on DigitalOcean, without the plugin (design doc 224 §9): a tagged Droplet
# running the engine container, its /data and /workspace on an attached block-storage Volume, a
# Cloud Firewall with no inbound rule (unless SSH_CIDR), restart=always.
#
#   export DIGITALOCEAN_TOKEN=…                       # read from the environment, never printed
#   THEUTH_PAIR_TICKET="$(scripts/cloud-node-ticket.sh)" scripts/cloud-node-do-test.sh create
#   scripts/cloud-node-do-test.sh status
#   scripts/cloud-node-do-test.sh ticket              # SSH mode: deliver a fresh ticket
#   scripts/cloud-node-do-test.sh destroy [--keep-volume]
#   scripts/cloud-node-do-test.sh list
#   scripts/cloud-node-do-test.sh user-data           # print the cloud-init script, ticket redacted
#
# Environment (defaults in brackets):
#   NAME [theuth-test]  REGION [lon1]  SIZE [s-1vcpu-1gb]  VOLUME_GB [10]
#   DROPLET_IMAGE [ubuntu-24-04-x64]  (a Docker 1-click slug skips the docker.io install)
#   ENGINE_IMAGE [build]  `build` = build deploy/computer on the Droplet (the entrypoint this branch
#                         adds is not in a published image yet); else an image ref to pull
#   THEUTH_VERSION [latest.json's]  the release `build` bakes in
#   SSH_KEY  a DO SSH key id or fingerprint; SSH_CIDR  e.g. 203.0.113.7/32 → inbound 22 from it only
#   THEUTH_T [t]  how to run the local `t` for the join check (`create` waits for the peer)
#
# Cost (DigitalOcean list prices, 2026): s-1vcpu-1gb $6/mo + 10 GiB volume $1/mo ≈ $7/mo, billed
# hourly while it exists (≈ $0.01/h). `destroy` stops all of it; `--keep-volume` keeps $1/mo.
set -euo pipefail

NAME="${NAME:-theuth-test}"
REGION="${REGION:-lon1}"
SIZE="${SIZE:-s-1vcpu-1gb}"
VOLUME_GB="${VOLUME_GB:-10}"
DROPLET_IMAGE="${DROPLET_IMAGE:-ubuntu-24-04-x64}"
ENGINE_IMAGE="${ENGINE_IMAGE:-build}"
MANIFEST="https://s3.rwx.dev/theuth/releases/latest.json"
API="https://api.digitalocean.com/v2"
TAG_MANAGED="theuth-managed"
TAG_NODE="theuth-node:${NAME}"
VOLUME="theuth-${NAME}-data"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

die() {
	printf 'cloud-node-do-test: %s\n' "$*" >&2
	exit 1
}
say() { printf '%s\n' "$*" >&2; }

case "$NAME" in
*[!a-z0-9-]* | -* | *-) die "NAME must be [a-z0-9-], not starting or ending with -" ;;
esac
[ "${#NAME}" -le 40 ] || die "NAME: at most 40 characters"

TMP="$(mktemp -d)"
chmod 700 "$TMP"
trap 'rm -rf "$TMP"' EXIT

# The token goes to curl in a 0600 header file, never on a command line (`ps` shows those).
auth() {
	[ -n "${DIGITALOCEAN_TOKEN:-}" ] || die "DIGITALOCEAN_TOKEN is not set"
	if [ ! -f "$TMP/auth" ]; then
		(umask 077 && printf 'Authorization: Bearer %s\n' "$DIGITALOCEAN_TOKEN" >"$TMP/auth")
	fi
}

# api METHOD PATH [BODY_FILE] → the response body on stdout; an HTTP error is fatal.
api() {
	auth
	local args=(-sS -X "$1" -H "@$TMP/auth" -H 'Content-Type: application/json' -o "$TMP/resp" -w '%{http_code}')
	[ $# -ge 3 ] && args+=(--data-binary "@$3")
	local code
	code="$(curl "${args[@]}" "$API$2")"
	if [ "$code" -ge 400 ]; then
		[ -n "${API_SOFT:-}" ] && return 1
		die "$1 $2 → HTTP $code: $(head -c 400 "$TMP/resp")"
	fi
	cat "$TMP/resp"
}

# Fails the script itself (not a `$(…)` subshell) on a bad token or no network.
check_token() { api GET /account >/dev/null; }

# json EXPR: evaluate a Python expression over the JSON on stdin (`d`), print the result.
json() { python3 -c "import json,sys; d=json.load(sys.stdin); r=$1; print('' if r is None else r)"; }

urlenc() { python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }

droplet_json() { api GET "/droplets?tag_name=$(urlenc "$TAG_NODE")"; }
droplet_id() { droplet_json | json "d['droplets'][0]['id'] if d['droplets'] else None"; }
droplet_ip() { droplet_json | json "next((n['ip_address'] for d0 in d['droplets'] for n in d0['networks']['v4'] if n['type']=='public'), None)"; }
volume_id() { api GET "/volumes?name=$VOLUME&region=$REGION" | json "d['volumes'][0]['id'] if d['volumes'] else None"; }
firewall_id() { api GET "/firewalls?per_page=200" | json "next((f['id'] for f in d['firewalls'] if f['name']=='theuth-$NAME'), None)"; }

# The cloud-init script. $1: the ticket (or a placeholder for `user-data`).
user_data() {
	local ticket="$1" version build="" pair=""
	if [ -n "$ticket" ]; then
		pair="if [ ! -e /mnt/theuth/data/.theuth-paired ]; then
	(umask 077 && printf '%s\\n' '${ticket}' >/mnt/theuth/data/pair-ticket)
	chown 1000:1000 /mnt/theuth/data/pair-ticket
fi"
	fi
	if [ "$ENGINE_IMAGE" = build ]; then
		version="${THEUTH_VERSION:-$(curl -fsSL "$MANIFEST" | json "d['version']")}"
		[ -n "$version" ] || die "could not read the release version from $MANIFEST"
		build="$(
			cat <<EOF
mkdir -p /opt/theuth-engine
echo '$(base64 <"$ROOT/deploy/computer/Dockerfile" | tr -d '\n')' | base64 -d >/opt/theuth-engine/Dockerfile
echo '$(base64 <"$ROOT/deploy/computer/entrypoint.sh" | tr -d '\n')' | base64 -d >/opt/theuth-engine/entrypoint.sh
docker build -q --build-arg THEUTH_VERSION='$version' --build-arg THEUTH_PUBKEY='$(tr -d '\n' <"$ROOT/crates/update/pubkey.txt")' -t theuth-engine:local /opt/theuth-engine
IMAGE=theuth-engine:local
EOF
		)"
	else
		build="docker pull '$ENGINE_IMAGE'
IMAGE='$ENGINE_IMAGE'"
	fi
	cat <<EOF
#!/bin/bash
# theuth hosted node (design doc 224 §9). No \`set -x\`: the ticket must not reach a log.
set -euo pipefail
exec >>/var/log/theuth-node.log 2>&1
echo "theuth-node: boot \$(date -u +%FT%TZ)"

# Keys only, whatever the image defaults to (the firewall has no inbound rule anyway).
printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\n' >/etc/ssh/sshd_config.d/00-theuth.conf
systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true

# 1 GiB swap: the 1 GiB Droplet plus a 1.2 GB engine binary.
if [ ! -f /swapfile ]; then
	fallocate -l 1G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
	echo '/swapfile none swap sw 0 0' >>/etc/fstab
fi

# The Volume: formatted once (never over an existing filesystem), mounted at /mnt/theuth.
dev=/dev/disk/by-id/scsi-0DO_Volume_${VOLUME}
for _ in \$(seq 1 90); do [ -e "\$dev" ] && break; sleep 2; done
[ -e "\$dev" ] || { echo "theuth-node: volume \$dev missing"; exit 1; }
blkid "\$dev" >/dev/null || mkfs.ext4 -q -L theuth-data "\$dev"
mkdir -p /mnt/theuth
grep -q ' /mnt/theuth ' /etc/fstab || echo "\$dev /mnt/theuth ext4 defaults,nofail,discard,noatime 0 2" >>/etc/fstab
mountpoint -q /mnt/theuth || mount /mnt/theuth
mkdir -p /mnt/theuth/data /mnt/theuth/workspace
chown 1000:1000 /mnt/theuth/data /mnt/theuth/workspace

# The one-time ticket, for the entrypoint's join loop (it deletes the file once joined).
${pair:-# none: \`cloud-node-do-test.sh ticket\` delivers one over ssh}

if ! command -v docker >/dev/null; then
	apt-get update -q
	DEBIAN_FRONTEND=noninteractive apt-get install -y -q docker.io
fi
systemctl enable --now docker

${build}

docker rm -f theuth-engine 2>/dev/null || true
# --init: docker-init is PID 1, so \`t\` counts as managed and an applied update exits for a restart.
docker run -d --name theuth-engine --restart always --init --hostname '${NAME}' \\
	-e THEUTH_NODE_NAME='${NAME}' \\
	-v /mnt/theuth/data:/data -v /mnt/theuth/workspace:/workspace \\
	--log-opt max-size=10m --log-opt max-file=3 \\
	"\$IMAGE"
echo "theuth-node: container started \$(date -u +%FT%TZ)"
EOF
}

ssh_root() {
	local ip
	ip="$(droplet_ip)"
	[ -n "$ip" ] || die "no Droplet $NAME"
	ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "root@$ip" "$@"
}

cmd_create() {
	[ -z "$(droplet_id)" ] || die "a Droplet tagged $TAG_NODE exists; \`$0 destroy\` first"
	local ticket="${THEUTH_PAIR_TICKET:-}"
	if [ -z "$ticket" ] && [ -z "${SSH_CIDR:-}" ]; then
		die "THEUTH_PAIR_TICKET is not set (mint it: scripts/cloud-node-ticket.sh), and without SSH_CIDR there is no way to deliver one later"
	fi
	case "$ticket" in
	'' | 1.[A-Za-z2-7]*) ;;
	*) die "THEUTH_PAIR_TICKET does not look like a pairing string (1.<base32>)" ;;
	esac
	case "$ticket" in *[!A-Za-z2-7.]*) die "THEUTH_PAIR_TICKET has characters a ticket never has" ;; esac
	local start=$SECONDS

	say "plan: Droplet theuth-$NAME ($SIZE, $DROPLET_IMAGE) in $REGION, Volume $VOLUME (${VOLUME_GB} GiB), firewall theuth-$NAME"
	say "      ≈ \$$(python3 -c "print(round({'s-1vcpu-1gb':6,'s-1vcpu-2gb':12,'s-2vcpu-2gb':18,'s-2vcpu-4gb':24}.get('$SIZE',0) + 0.10*$VOLUME_GB, 2))")/month (unknown sizes count \$0)"

	# An existing tag answers 422 here; either way it exists afterwards.
	for t in "$TAG_MANAGED" "$TAG_NODE"; do
		printf '{"name":%s}' "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$t")" >"$TMP/body"
		API_SOFT=1 api POST /tags "$TMP/body" >/dev/null || true
	done

	# Firewall first, on the tag, so the Droplet is covered from its first packet.
	python3 - "$NAME" "$TAG_NODE" "${SSH_CIDR:-}" >"$TMP/body" <<'EOF'
import json, sys
name, tag, ssh = sys.argv[1:4]
out = {"addresses": ["0.0.0.0/0", "::/0"]}
print(json.dumps({
    "name": "theuth-" + name,
    "tags": [tag],
    "inbound_rules": [{"protocol": "tcp", "ports": "22", "sources": {"addresses": [ssh]}}] if ssh else [],
    "outbound_rules": [
        {"protocol": "tcp", "ports": "all", "destinations": out},
        {"protocol": "udp", "ports": "all", "destinations": out},
        {"protocol": "icmp", "destinations": out},
    ],
}))
EOF
	[ -n "$(firewall_id)" ] || api POST /firewalls "$TMP/body" >/dev/null
	say "firewall: theuth-$NAME (inbound: ${SSH_CIDR:+tcp/22 from $SSH_CIDR}${SSH_CIDR:-none})"

	local vid
	vid="$(volume_id)"
	if [ -n "$vid" ]; then
		say "volume: reusing $VOLUME ($vid)"
	else
		python3 - "$VOLUME" "$REGION" "$VOLUME_GB" "$TAG_MANAGED" "$TAG_NODE" >"$TMP/body" <<'EOF'
import json, sys
v, region, gb, *tags = sys.argv[1:]
print(json.dumps({"name": v, "region": region, "size_gigabytes": int(gb), "filesystem_type": "ext4",
                  "filesystem_label": "theuth-data", "tags": tags,
                  "description": "theuth node /data and /workspace (design doc 224)"}))
EOF
		vid="$(api POST /volumes "$TMP/body" | json "d['volume']['id']")"
		say "volume: $VOLUME ($vid)"
	fi

	(umask 077 && user_data "$ticket" >"$TMP/user-data")
	python3 - "$NAME" "$REGION" "$SIZE" "$DROPLET_IMAGE" "$vid" "$TAG_MANAGED" "$TAG_NODE" "${SSH_KEY:-}" "$TMP/user-data" >"$TMP/body" <<'EOF'
import json, sys
name, region, size, image, vid, t1, t2, key, ud = sys.argv[1:]
body = {"name": "theuth-" + name, "region": region, "size": size, "image": image, "volumes": [vid],
        "tags": [t1, t2], "ipv6": True, "monitoring": False, "user_data": open(ud).read()}
if key:
    body["ssh_keys"] = [int(key) if key.isdigit() else key]
print(json.dumps(body))
EOF
	chmod 600 "$TMP/body"
	local did
	did="$(api POST /droplets "$TMP/body" | json "d['droplet']['id']")"
	rm -f "$TMP/body" "$TMP/user-data"
	say "droplet: theuth-$NAME ($did), waiting for it to be active"
	local status=""
	for _ in $(seq 1 60); do
		status="$(api GET "/droplets/$did" | json "d['droplet']['status']")"
		[ "$status" = active ] && break
		sleep 5
	done
	[ "$status" = active ] || die "Droplet $did is still $status"
	say "droplet: active at $(droplet_ip) after $((SECONDS - start)) s; cloud-init now installs docker, builds/pulls the image and starts the engine"

	read -r -a T <<<"${THEUTH_T:-t}"
	if [ -z "$ticket" ]; then
		say "no ticket given: once the container runs, deliver one with:"
		say "  THEUTH_PAIR_TICKET=\"\$(scripts/cloud-node-ticket.sh)\" $0 ticket"
	elif command -v "${T[0]}" >/dev/null; then
		say "waiting up to 10 min for '$NAME' in \`${T[*]} mesh status\` (the ticket expires 5 min after it was minted)"
		for _ in $(seq 1 60); do
			if "${T[@]}" mesh status 2>/dev/null | python3 -c '
import json, sys
d = json.load(sys.stdin)
sys.exit(0 if any(p["name"] == sys.argv[1] for p in d["peers"]) else 1)' "$NAME"; then
				say "joined: '$NAME' is a peer after $((SECONDS - start)) s"
				verify_hint
				return 0
			fi
			sleep 10
		done
		say "not joined after $((SECONDS - start)) s."
		say "  With SSH_CIDR+SSH_KEY: \`$0 status\` shows the node's log; \`$0 ticket\` delivers a fresh ticket."
		say "  Without: the Droplet console (cloud.digitalocean.com → Droplets → theuth-$NAME → Console) → /var/log/theuth-node.log, docker logs theuth-engine."
		exit 1
	fi
	verify_hint
}

verify_hint() {
	say "verify:"
	say "  t mesh status            # peer '$NAME', reachable: true (via the relay)"
	say "  $0 status"
	say "  $0 destroy [--keep-volume]"
}

cmd_ticket() {
	[ -n "${SSH_CIDR:-}" ] || say "note: without SSH_CIDR the firewall blocks ssh"
	local ticket="${THEUTH_PAIR_TICKET:-}"
	[ -n "$ticket" ] || die "THEUTH_PAIR_TICKET is not set"
	printf '%s\n' "$ticket" | ssh_root 'umask 077; cat >/mnt/theuth/data/pair-ticket && chown 1000:1000 /mnt/theuth/data/pair-ticket && echo delivered'
}

cmd_status() {
	local did
	did="$(droplet_id)"
	if [ -z "$did" ]; then
		say "no Droplet tagged $TAG_NODE"
	else
		api GET "/droplets/$did" | json "'droplet %s %s %s %s ip=%s created=%s' % (d['droplet']['name'], d['droplet']['status'], d['droplet']['size_slug'], d['droplet']['region']['slug'], next((n['ip_address'] for n in d['droplet']['networks']['v4'] if n['type']=='public'), '-'), d['droplet']['created_at'])"
	fi
	local vid
	vid="$(volume_id)"
	[ -z "$vid" ] || api GET "/volumes/$vid" | json "'volume %s %s GiB attached=%s' % (d['volume']['name'], d['volume']['size_gigabytes'], d['volume']['droplet_ids'])"
	local fid
	fid="$(firewall_id)"
	[ -z "$fid" ] || api GET "/firewalls/$fid" | json "'firewall %s %s inbound=%s' % (d['firewall']['name'], d['firewall']['status'], [r['ports'] for r in d['firewall']['inbound_rules']])"
	if [ -n "$did" ] && [ -n "${SSH_CIDR:-}" ]; then
		ssh_root 'tail -n 5 /var/log/theuth-node.log; docker ps --filter name=theuth-engine --format "{{.Status}} {{.Image}}"; docker logs --tail 40 theuth-engine 2>&1 | grep -v pw.conf | tail -n 15'
	fi
}

cmd_destroy() {
	local keep=false
	[ "${1:-}" = --keep-volume ] && keep=true
	local did vid fid
	did="$(droplet_id)"
	if [ -n "$did" ]; then
		api DELETE "/droplets/$did" >/dev/null
		say "droplet $did: deleted"
	fi
	fid="$(firewall_id)"
	if [ -n "$fid" ]; then
		api DELETE "/firewalls/$fid" >/dev/null
		say "firewall $fid: deleted"
	fi
	vid="$(volume_id)"
	if [ -n "$vid" ] && ! $keep; then
		for _ in $(seq 1 60); do
			[ "$(api GET "/volumes/$vid" | json "len(d['volume']['droplet_ids'])")" = 0 ] && break
			sleep 5
		done
		api DELETE "/volumes/$vid" >/dev/null
		say "volume $VOLUME: deleted"
	elif [ -n "$vid" ]; then
		say "volume $VOLUME: kept (\$0.10/GiB/month); \`$0 create\` reattaches it, \`$0 destroy\` deletes it"
	fi
	api DELETE "/tags/$(urlenc "$TAG_NODE")" >/dev/null 2>&1 || true
	say "take '$NAME' out of the mesh: t mesh status → t mesh remove <id>"
}

cmd_list() {
	api GET "/droplets?tag_name=$TAG_MANAGED&per_page=200" | json "'\n'.join('droplet %s %s %s' % (x['name'], x['status'], x['region']['slug']) for x in d['droplets']) or 'no droplets'"
	api GET "/volumes?per_page=200" | json "'\n'.join('volume %s %s GiB attached=%s' % (x['name'], x['size_gigabytes'], x['droplet_ids']) for x in d['volumes'] if '$TAG_MANAGED' in (x.get('tags') or [])) or 'no volumes'"
}

case "${1:-}" in
create) check_token && cmd_create ;;
status) check_token && cmd_status ;;
ticket) check_token && cmd_ticket ;;
destroy) check_token && cmd_destroy "${2:-}" ;;
list) check_token && cmd_list ;;
user-data) user_data '<ticket>' ;;
*) die "usage: $0 create|status|ticket|destroy [--keep-volume]|list|user-data" ;;
esac
