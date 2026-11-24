#!/bin/bash
# design 216 §19: run both example plugins against a scripted host, no engine needed.
# Usage: ./check.sh [path-to-bash]   (default /bin/bash, which is 3.2 on macOS). Needs python3, perl.
set -u
B=${1:-/bin/bash}
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
fail=0
H='{"jsonrpc":"2.0","method":"hello","params":{"protocol":1,"plugin":"ping"}}'
S='{"jsonrpc":"2.0","method":"shutdown","params":{"reason":"disable","grace_ms":3000}}'

# ping_case NAME LINES SECONDS FEEDER: run ping/run.sh under $B with FEEDER on its stdin and
# `timeout` emulated by perl's alarm (macOS has no timeout(1)); expect exit 0, LINES status
# lines, and the script gone after SECONDS (±1).
ping_case() {
  name=$1 want_lines=$2 want_s=$3 feeder=$4
  a=$(date +%s)
  $feeder | (cd "$here/ping" && PING_HOST=127.0.0.1 perl -e 'alarm 40; exec @ARGV' "$B" run.sh \
    > "$tmp/$name.out"; echo "$? $(( $(date +%s) - a ))" > "$tmp/$name.res")
  read -r rc secs < "$tmp/$name.res"
  lines=$(grep -c '"method":"status"' "$tmp/$name.out")
  ok=yes
  [ "$rc" = 0 ] && [ "$lines" = "$want_lines" ] && [ $((secs - want_s)) -le 1 ] && [ $((want_s - secs)) -le 1 ] || ok=NO
  echo $ok > "$tmp/$name.ok"   # cases run in background subshells: report through a file
  echo "ping $name: exit=$rc status_lines=$lines gone_after=${secs}s (want 0, $want_lines, ${want_s}s) $ok"
}
no_input() { :; }
hello_then_eof() { echo "$H"; }
quiet_25s_then_eof() { echo "$H"; sleep 25; }
quiet_12s_then_shutdown() { echo "$H"; sleep 12; echo "$S"; sleep 8; }

ping_case no_input 0 0 no_input &
ping_case hello_then_eof 1 0 hello_then_eof &
ping_case quiet_12s_then_shutdown 2 12 quiet_12s_then_shutdown &
ping_case quiet_25s_then_eof 3 25 quiet_25s_then_eof &
wait
grep -q NO "$tmp"/*.ok && fail=1

# turns: hello, one TurnFinished (TURNS_LIMIT=1), a reset press, a render pull, a tools/call,
# a request it does not handle, an unknown notification, shutdown.
export THEUTH_PLUGIN_DATA=$tmp/turns-data TURNS_LIMIT=1
sid=01a20e56-84d4-775f-ae0c-65af2ba8c55b
cat > "$tmp/turns.in" <<EOF
{"jsonrpc":"2.0","method":"hello","params":{"protocol":1,"plugin":"turns","version":"0.2.0"}}
{"jsonrpc":"2.0","method":"event","params":{"topic":"agent.events","version":1,"type":"TurnFinished","session":"$sid","at_ms":1795530420113,"data":{"type":"TurnFinished","session":"$sid","turn":"t1","stop_reason":"end_turn","usage":{"prompt":1200,"completion":34,"cached":1000},"context":1234}}}
{"jsonrpc":"2.0","id":7,"method":"action","params":{"placement":"turns","action":"reset","args":{},"client":"desktop","activation":"act-1","form":null}}
{"jsonrpc":"2.0","id":8,"method":"render","params":{"placement":"turns","client":"desktop"}}
{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"turns_today","arguments":{}}}
{"jsonrpc":"2.0","id":10,"method":"query","params":{"placement":"turns","query":"history","args":{}}}
{"jsonrpc":"2.0","method":"granted","params":{"grants":{}}}
{"jsonrpc":"2.0","method":"shutdown","params":{"reason":"disable","grace_ms":3000}}
EOF
(cd "$here/turns" && perl -e 'alarm 20; exec @ARGV' python3 main.py < "$tmp/turns.in" > "$tmp/turns.out")
trc=$?
python3 - "$tmp/turns.out" "$tmp/turns-data/state.json" "$trc" <<'EOF' || fail=1
import json, sys
out = [json.loads(l) for l in open(sys.argv[1])]
state = json.load(open(sys.argv[2]))
seq = [m.get("method") or ("result" if "result" in m else "error") + f"#{m['id']}" for m in out]
ui = [m["params"] for m in out if m.get("method") == "ui"]
checks = {
    "exit 0": sys.argv[3] == "0",
    "sequence": seq == ["status", "ui",                       # start
                        "status", "ui", "emit",               # TurnFinished at the limit
                        "status", "ui", "result#7",           # reset press
                        "status", "ui", "result#8",           # render pull
                        "result#9",                           # tools/call
                        "error#10"],                          # unhandled request answered
    "every ui has text": all(u.get("text") for u in ui),
    "emit payload": [m["params"] for m in out if m.get("method") == "emit"]
        == [{"event": "limit_reached", "data": {"day": state["day"], "turns": 1}}],
    "reset result": out[7] == {"jsonrpc": "2.0", "id": 7, "result": {"ok": True}},
    "tool result": out[11]["result"]["content"][0]["text"].startswith("0 of 1 turns today"),
    "-32601 for query": out[12]["error"]["code"] == -32601,
    "state after reset": state["turns"] == 0 and state["tokens"] == 0,
}
for k, v in checks.items():
    print(f"turns {k}: {'yes' if v else 'NO'}")
sys.exit(0 if all(checks.values()) else 1)
EOF
rm -rf "$tmp"
echo "check.sh with $B ($("$B" -c 'echo $BASH_VERSION')): $([ $fail = 0 ] && echo PASS || echo FAIL)"
exit $fail
