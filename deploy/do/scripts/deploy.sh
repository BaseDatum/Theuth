#!/usr/bin/env bash
# Ship deploy/do/host to the droplet and (re)start the compose project.
#   deploy/do/scripts/deploy.sh <reserved-ip> [--with-edge]
# Without --with-edge the share edge (compose profile `edge`) is left alone: it is started by
# migrate-edge-data.sh once the cluster's data dir is in place. --with-edge refuses unless /srv/theuth/edge/iroh.key
# exists, so a deploy can never mint a new edge key.
#
# Secrets come from the caller's environment only, never from files in the repo, never echoed, never on argv:
#   PORKBUN_TUNZIP_APIKEY, PORKBUN_TUNZIP_SECRET   one Porkbun key for theuth.io (Caddy DNS-01) and tun.zip (edge)
#   THEUTH_RELAY_COOKIE_SECRET                     the cluster Secret theuth/theuth-relay key cookie-secret
#   GHCR_PULL_USER, GHCR_PULL_TOKEN                optional: only if the images are private (they are public since 2026-11-26)
# Non-secret pins (RELAY_IMAGE, …) default to deploy/do/host/.env.example; the same names in the environment win.
# SSH_USER (default root).
# Remote commands are single-quoted on purpose: they expand on the droplet.
# shellcheck disable=SC2016,SC2029
set -euo pipefail
set +x

usage() { echo "usage: $0 <reserved-ip> [--with-edge]" >&2; exit 2; }
[[ $# -ge 1 ]] || usage
ip=$1; shift
with_edge=false
for a in "$@"; do
  case "$a" in
    --with-edge) with_edge=true ;;
    *) usage ;;
  esac
done
[[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not an IPv4 address: $ip" >&2; exit 2; }

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
host_dir="$here/../host"
target="${SSH_USER:-root}@$ip"
ssh_opts=(-o StrictHostKeyChecking=accept-new -o ServerAliveInterval=15)
secrets=(PORKBUN_TUNZIP_APIKEY PORKBUN_TUNZIP_SECRET THEUTH_RELAY_COOKIE_SECRET)

for v in "${secrets[@]}"; do
  [[ -n "${!v:-}" ]] || { echo "missing env $v" >&2; exit 1; }
done
for c in jq rsync; do command -v "$c" >/dev/null || { echo "$c required" >&2; exit 1; }; done

remote() { ssh "${ssh_opts[@]}" "$target" "$@"; }

echo "==> waiting for cloud-init on $ip"
remote 'cloud-init status --wait >/dev/null; mountpoint -q /srv/theuth && docker compose version >/dev/null' ||
  { echo "host not ready: /srv/theuth not mounted or docker compose missing" >&2; exit 1; }

echo "==> rendering and syncing $host_dir -> /srv/theuth/app"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
rsync -a --exclude .env "$host_dir/" "$stage/"
sed -i.bak "s/__PUBLIC_IP__/$ip/g" "$stage/share-edge/config.yaml" && rm "$stage/share-edge/config.yaml.bak"
rsync -a --delete --exclude .env -e "ssh ${ssh_opts[*]}" "$stage/" "$target:/srv/theuth/app/"

echo "==> writing /srv/theuth/app/.env (values not shown)"
render_env() {
  local line key val
  while IFS= read -r line; do
    [[ "$line" =~ ^([A-Z0-9_]+)=(.*)$ ]] || continue
    key=${BASH_REMATCH[1]}
    val=${BASH_REMATCH[2]}
    printf '%s=%s\n' "$key" "${!key:-$val}"
  done <"$host_dir/.env.example"
}
render_env | remote 'umask 077; cat >/srv/theuth/app/.env.tmp && mv /srv/theuth/app/.env.tmp /srv/theuth/app/.env'

echo "==> writing /srv/theuth/secrets/tunzip-dns.json (values not shown)"
jq -n '{key: env.PORKBUN_TUNZIP_APIKEY, secret: env.PORKBUN_TUNZIP_SECRET}' |
  remote 'umask 077; f=/srv/theuth/secrets/tunzip-dns.json; cat >"$f.tmp" && chown 65532:65532 "$f.tmp" && chmod 0400 "$f.tmp" && mv "$f.tmp" "$f"'

if [[ -n "${GHCR_PULL_TOKEN:-}" ]]; then
  echo "==> ghcr.io login as ${GHCR_PULL_USER:?GHCR_PULL_USER required with GHCR_PULL_TOKEN}"
  printf '%s' "$GHCR_PULL_TOKEN" | remote "docker login ghcr.io -u '$GHCR_PULL_USER' --password-stdin >/dev/null"
fi

echo "==> pull, build, up"
remote 'set -e; cd /srv/theuth/app
  docker compose --profile edge pull --ignore-buildable --quiet
  docker compose build --quiet caddy
  docker compose up -d'

if $with_edge; then
  echo "==> share edge"
  remote 'set -e; test -s /srv/theuth/edge/iroh.key || { echo "refusing: /srv/theuth/edge/iroh.key missing (run migrate-edge-data.sh)" >&2; exit 1; }
    test -s /srv/theuth/edge/acme/letsencrypt.json || { echo "refusing: ACME account missing" >&2; exit 1; }
    cd /srv/theuth/app && docker compose --profile edge up -d share-edge'
fi

remote 'cd /srv/theuth/app && docker compose --profile edge ps'
