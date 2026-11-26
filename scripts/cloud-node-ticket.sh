#!/usr/bin/env bash
# Mint a one-time pairing ticket for a remote engine (design doc 224 §4) on the machine whose
# engine the new node should join, and print it on stdout (nothing else goes to stdout).
#
#   THEUTH_PAIR_TICKET="$(scripts/cloud-node-ticket.sh)" scripts/cloud-node-do-test.sh create
#
# The ticket is `t mesh pair`'s string without `vault_key`/`vault_key_id`: the same one-time
# token (valid 5 minutes, consumed by the first engine that uses it), this engine's id, relay
# and addresses. A `t` that has `t mesh pair --remote` does this itself; for an older `t` the
# string is decoded and re-encoded here (`1.` + base32 of the JSON payload).
#
# THEUTH_T: the command that runs `t` (default `t`; e.g. `docker exec engine t`).
set -euo pipefail

read -r -a T <<<"${THEUTH_T:-t}"

if "${T[@]}" mesh pair --help 2>/dev/null | grep -q -- '--remote'; then
	exec "${T[@]}" mesh pair --remote
fi

# shellcheck disable=SC2016 # Python source, not shell
"${T[@]}" mesh pair | python3 -c '
import base64, json, sys
raw = sys.stdin.read().strip()
s = json.loads(raw) if raw.startswith("\"") else raw   # `t mesh pair` prints a JSON string
version, body = s.strip().split(".", 1)
if version != "1":
    sys.exit("cloud-node-ticket: unknown pairing string version " + version)
b = body.upper()
p = json.loads(base64.b32decode(b + "=" * (-len(b) % 8)))
p.pop("vault_key", None)
p.pop("vault_key_id", None)
enc = base64.b32encode(json.dumps(p, separators=(",", ":")).encode()).decode().rstrip("=")
print("1." + enc)
'
