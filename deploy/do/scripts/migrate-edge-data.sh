#!/usr/bin/env bash
# Move the share edge's data dir (PVC theuth-share/theuth-share-edge-data) from the cluster to the droplet and start
# the edge there. The data dir holds iroh.key (the endpoint id shipped clients pin), acme/letsencrypt.json (the
# production ACME account the tun.zip CAA records pin) and share-edge.sqlite (names, meshes, DNS ledger).
#
#   deploy/do/scripts/migrate-edge-data.sh <reserved-ip>
#
# Order, by design (the iroh key must never be live in two places, and the SQLite file must be copied closed):
#   1. scale the cluster edge to 0 and wait for its pod to exit (it drains tunnels for up to 30 s)
#   2. mount the PVC READ-ONLY in a throwaway busybox pod (the edge image is FROM scratch: no tar to exec)
#   3. tar it over ssh into /srv/theuth/edge (owner 65532), verify every file's sha256 on both sides
#   4. delete the helper pod, start the droplet's edge (`docker compose --profile edge up -d share-edge`)
# The cluster edge is NOT scaled back up by this script. If anything fails before step 4 it prints the rollback.
# Run deploy.sh first (secrets, config and the edge image must already be on the droplet).
# Env: KUBE_CONTEXT (optional), SSH_USER (default root), FORCE=1 to move an existing /srv/theuth/edge aside.
# Remote commands are single-quoted on purpose: they expand on the droplet.
# shellcheck disable=SC2016,SC2029
set -euo pipefail
set +x

[[ $# -eq 1 ]] || { echo "usage: $0 <reserved-ip>" >&2; exit 2; }
ip=$1
ns=theuth-share
deploy=theuth-share-edge
pvc=theuth-share-edge-data
helper=edge-data-export
target="${SSH_USER:-root}@$ip"
ssh_opts=(-o StrictHostKeyChecking=accept-new -o ServerAliveInterval=15)
kc=(kubectl -n "$ns")
[[ -n "${KUBE_CONTEXT:-}" ]] && kc+=(--context "$KUBE_CONTEXT")

remote() { ssh "${ssh_opts[@]}" "$target" "$@"; }

echo "==> preflight"
"${kc[@]}" get "deploy/$deploy" "pvc/$pvc" >/dev/null
replicas=$("${kc[@]}" get "deploy/$deploy" -o jsonpath='{.spec.replicas}')
remote 'set -e
  test -f /srv/theuth/app/compose.yaml || { echo "no /srv/theuth/app: run deploy.sh first" >&2; exit 1; }
  test -s /srv/theuth/secrets/tunzip-dns.json || { echo "no DNS credential: run deploy.sh first" >&2; exit 1; }
  cd /srv/theuth/app && img=$(docker compose --profile edge config --images | grep share-edge) &&
    docker image inspect "$img" >/dev/null || { echo "edge image not pulled: run deploy.sh first" >&2; exit 1; }
  ! docker compose --profile edge ps --status running --services | grep -qx share-edge ||
    { echo "share-edge already running on the droplet" >&2; exit 1; }'
if remote 'test -n "$(ls -A /srv/theuth/edge 2>/dev/null)"'; then
  if [[ "${FORCE:-}" == 1 ]]; then
    remote 'mv /srv/theuth/edge "/srv/theuth/edge.bak-$(date +%Y%m%dT%H%M%S)"'
  else
    echo "/srv/theuth/edge is not empty; FORCE=1 moves it aside" >&2
    exit 1
  fi
fi
remote 'install -d -m 0700 -o 65532 -g 65532 /srv/theuth/edge'

rollback_hint() {
  echo >&2
  echo "FAILED before the droplet edge started. Roll back with:" >&2
  echo "  ${kc[*]} scale deploy/$deploy --replicas=$replicas" >&2
}
cleanup() { "${kc[@]}" delete pod "$helper" --ignore-not-found --wait=false >/dev/null 2>&1 || true; }
trap 'rollback_hint; cleanup' ERR
trap cleanup EXIT

start=$(date +%s)
echo "==> [1/4] scaling cluster edge to 0 (was $replicas)"
"${kc[@]}" scale "deploy/$deploy" --replicas=0
for _ in $(seq 1 60); do
  [[ -z "$("${kc[@]}" get pod -l "app.kubernetes.io/name=$deploy" -o name)" ]] && break
  sleep 2
done
[[ -z "$("${kc[@]}" get pod -l "app.kubernetes.io/name=$deploy" -o name)" ]] ||
  { echo "cluster edge pod still present after 120 s" >&2; false; }

echo "==> [2/4] read-only helper pod on $pvc"
"${kc[@]}" run "$helper" --image=busybox:1.37 --restart=Never --overrides="$(cat <<JSON
{"spec": {
  "automountServiceAccountToken": false,
  "securityContext": {"runAsUser": 65532, "runAsGroup": 65532, "runAsNonRoot": true},
  "containers": [{
    "name": "$helper", "image": "busybox:1.37", "command": ["sleep", "3600"],
    "securityContext": {"allowPrivilegeEscalation": false, "readOnlyRootFilesystem": true, "capabilities": {"drop": ["ALL"]}},
    "volumeMounts": [{"name": "data", "mountPath": "/data", "readOnly": true}]
  }],
  "volumes": [{"name": "data", "persistentVolumeClaim": {"claimName": "$pvc", "readOnly": true}}]
}}
JSON
)" >/dev/null
"${kc[@]}" wait --for=condition=Ready "pod/$helper" --timeout=180s
echo "files on the PVC:"
"${kc[@]}" exec "$helper" -- sh -c 'cd /data && find . ! -type d | sort | xargs ls -ln' | awk '{print "  " $1, $3":"$4, $5, $NF}'
"${kc[@]}" exec "$helper" -- sh -c 'test -s /data/iroh.key && test -s /data/acme/letsencrypt.json && test -s /data/share-edge.sqlite' ||
  { echo "PVC lacks iroh.key, acme/letsencrypt.json or share-edge.sqlite" >&2; false; }

echo "==> [3/4] copying to $ip:/srv/theuth/edge"
"${kc[@]}" exec "$helper" -- tar -C /data -cf - . |
  remote 'tar -C /srv/theuth/edge -xpf - --numeric-owner && chown -R 65532:65532 /srv/theuth/edge && chmod 0700 /srv/theuth/edge'
src=$("${kc[@]}" exec "$helper" -- sh -c 'cd /data && find . -type f | sort | xargs sha256sum')
dst=$(remote 'cd /srv/theuth/edge && find . -type f | sort | xargs sha256sum')
if [[ "$src" != "$dst" ]]; then
  echo "checksum mismatch between PVC and droplet copy" >&2
  diff <(awk '{print $2}' <<<"$src") <(awk '{print $2}' <<<"$dst") >&2 || true
  false
fi
echo "  $(wc -l <<<"$src" | tr -d ' ') files, sha256 identical"
cleanup

echo "==> [4/4] starting the droplet edge"
trap cleanup EXIT
trap - ERR
remote 'cd /srv/theuth/app && docker compose --profile edge up -d share-edge'
sleep 10
remote 'cd /srv/theuth/app && docker compose --profile edge logs --tail 40 share-edge
  docker compose --profile edge exec -T share-edge /theuth-share-edge --config /etc/theuth-share-edge/config.yaml admin status' || true
echo "==> edge down in the cluster for $(( $(date +%s) - start ))s so far. Now switch DNS (README step 6)."
echo "    Cluster edge left at 0 replicas; do not scale it back while the droplet edge runs."
