#!/usr/bin/env bash
# Probe every public name against one IP, ignoring public DNS (curl --resolve / openssl -connect).
#   deploy/do/scripts/smoke.sh <ip> [share-name]
# share-name: a tun.zip name with a live tunnel (e.g. sky.tun.zip) to check the edge end to end; optional.
set -uo pipefail

[[ $# -ge 1 ]] || { echo "usage: $0 <ip> [share-name]" >&2; exit 2; }
ip=$1
share=${2:-}
fail=0

check() { # name expected-status-regex path
  local code
  code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 --resolve "$1:443:$ip" "https://$1$3") || code=ERR
  if [[ "$code" =~ ^($2)$ ]]; then echo "ok   https://$1$3 -> $code"; else echo "FAIL https://$1$3 -> $code (want $2)"; fail=1; fi
}

cert() { # name: print issuer and SANs of what $ip serves for SNI name, verified against the name
  local out
  out=$(openssl s_client -connect "$ip:443" -servername "$1" -verify_hostname "$1" -verify_return_error </dev/null 2>/dev/null) ||
    { echo "FAIL tls $1 (handshake or verification)"; fail=1; return; }
  echo "ok   tls $1: $(openssl x509 -noout -issuer <<<"$out" | sed 's/^issuer=//') | $(openssl x509 -noout -ext subjectAltName <<<"$out" | tail -n +2 | tr -d ' ')"
}

echo "--- theuth.io via Caddy"
check theuth.io 200 /
check www.theuth.io 301 /
check relay.theuth.io 200 /healthz
check app.theuth.io 200 /
for n in theuth.io www.theuth.io relay.theuth.io app.theuth.io; do cert "$n"; done
code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 --resolve "theuth.io:80:$ip" http://theuth.io/x) || code=ERR
if [[ "$code" == 301 ]]; then echo "ok   http://theuth.io/x -> 301"; else echo "FAIL http://theuth.io/x -> $code"; fail=1; fi

echo "--- iroh-relay (TLS in-process)"
cert iroh.relay.theuth.io
check iroh.relay.theuth.io '200' /
check iroh.relay.theuth.io 200 /ping

echo "--- share edge (*.tun.zip, TLS passed through)"
if [[ -n "$share" ]]; then
  cert "$share"
  check "$share" '[1-5][0-9][0-9]' /
else
  # No live tunnel named: only TCP reachability can be checked (HAProxy also accepts before reading the SNI), so
  # pass a live share name for a real end-to-end check.
  if openssl s_client -connect "$ip:443" -servername "smoke-$RANDOM.tun.zip" </dev/null 2>&1 | grep -q 'CONNECTED'; then
    echo "ok   443 reachable for *.tun.zip (weak check: pass a live share name for end to end)"
  else
    echo "FAIL no TCP connect for *.tun.zip"; fail=1
  fi
fi

echo "--- unknown SNI is refused"
if openssl s_client -connect "$ip:443" -servername example.org </dev/null 2>/dev/null | grep -q 'BEGIN CERTIFICATE'; then
  echo "FAIL a certificate was served for example.org"; fail=1
else
  echo "ok   example.org refused"
fi

exit $fail
