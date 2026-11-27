#!/usr/bin/env bash
# Look up the Porkbun record ids of the static A records deploy/do/dns manages and write them to
# deploy/do/dns/import-ids.auto.tfvars.json, so `terraform -chdir=deploy/do/dns plan` imports them.
# Read-only: only calls /dns/retrieve. Credentials from the environment, never printed or put on argv:
#   PORKBUN_TUNZIP_APIKEY  PORKBUN_TUNZIP_SECRET   (the porkbun.tunzip.* key; must be allowed on both zones)
# Usage: deploy/do/scripts/porkbun-import-ids.sh [--dry-run]
# shellcheck disable=SC2001
set -euo pipefail
set +x

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
out="$here/../dns/import-ids.auto.tfvars.json"
dry_run=false
[[ "${1:-}" == "--dry-run" ]] && dry_run=true

: "${PORKBUN_TUNZIP_APIKEY:?set PORKBUN_TUNZIP_APIKEY}"
: "${PORKBUN_TUNZIP_SECRET:?set PORKBUN_TUNZIP_SECRET}"
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }

# Must match local.static_records in deploy/do/dns/main.tf.
static=(
  theuth.io www.theuth.io relay.theuth.io app.theuth.io iroh.theuth.io iroh.relay.theuth.io '*.theuth.io'
  tun.zip '*.tun.zip' edge.tun.zip iroh.tun.zip relay.tun.zip
)
static_json=$(printf '%s\n' "${static[@]}" | jq -R . | jq -s .)

retrieve() {
  # Body built from the environment by jq (no secret on any command line), sent on stdin.
  jq -n '{apikey: env.PORKBUN_TUNZIP_APIKEY, secretapikey: env.PORKBUN_TUNZIP_SECRET}' |
    curl -fsS --retry 3 -H 'Content-Type: application/json' --data @- \
      "https://api.porkbun.com/api/json/v3/dns/retrieve/$1"
}

all='[]'
for zone in theuth.io tun.zip; do
  resp=$(retrieve "$zone")
  if [[ "$(jq -r .status <<<"$resp")" != SUCCESS ]]; then
    echo "porkbun retrieve $zone failed: $(jq -r '.message // "unknown error"' <<<"$resp")" >&2
    exit 1
  fi
  all=$(jq --arg zone "$zone" --argjson acc "$all" '$acc + [.records[] | . + {zone: $zone}]' <<<"$resp")
done

echo "A records in theuth.io and tun.zip:"
jq -r '.[] | select(.type == "A") | [.name, .content, "ttl=\(.ttl)", "id=\(.id)"] | @tsv' <<<"$all" | column -t -s $'\t'

dups=$(jq -r --argjson s "$static_json" '[.[] | select(.type == "A" and (.name as $n | $s | index($n)))]
  | group_by(.name) | map(select(length > 1) | .[0].name) | .[]' <<<"$all")
if [[ -n "$dups" ]]; then
  echo "ERROR: more than one A record for: $dups -- resolve by hand at Porkbun first" >&2
  exit 1
fi

others=$(jq -r --argjson s "$static_json" '.[] | select(.type == "A" and ((.name as $n | $s | index($n)) | not)) | .name' <<<"$all")
if [[ -n "$others" ]]; then
  echo "WARNING: A records NOT managed by deploy/do/dns (point them by hand, or add them to main.tf and this script):" >&2
  sed 's/^/  /' <<<"$others" >&2
fi

missing=$(jq -r --argjson s "$static_json" '[.[] | select(.type == "A") | .name] as $have | $s - $have | .[]' <<<"$all")
if [[ -n "$missing" ]]; then
  echo "Static names without their own A record (wildcard-covered or absent; left unmanaged):"
  sed 's/^/  /' <<<"$missing"
fi

ids=$(jq --argjson s "$static_json" '{record_ids: ([.[] | select(.type == "A" and (.name as $n | $s | index($n)))
  | {key: .name, value: (.id | tostring)}] | from_entries)}' <<<"$all")
if $dry_run; then
  echo "$ids"
else
  echo "$ids" >"$out"
  echo "wrote $out ($(jq '.record_ids | length' <<<"$ids") records)"
fi
