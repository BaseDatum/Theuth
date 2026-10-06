#!/usr/bin/env bash
# 98c live proof: a scratch theuth engine against a local telegram-mock-ai.
#
#   scripts/telegram-live-proof/proof.sh <label> [--no-build] [--keep]
#
# <label> names the run (main | fix | …): data dir /tmp/98c-proof/data-<label>,
# evidence in /tmp/98c-proof/out-<label>/. Never touches ~/.theuth except to
# copy config.yaml, acp/agents.yaml, skills/ (the vault is per-data_dir keyed: see below).
# Prints PASS/FAIL per step with timings; exit 0 only when every step passed.
set -u -o pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
ENGINE_SRC=${ENGINE_SRC:-$ROOT}   # checkout to build the engine from (another worktree, e.g. the fix branch)
LABEL=${1:?label (main|fix)}; shift || true
BUILD=1; KEEP=0
for a in "$@"; do case $a in --no-build) BUILD=0;; --keep) KEEP=1;; esac; done

export PATH=/Users/Sky.Moore/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT-target}
export BOT_TOKEN=${BOT_TOKEN:-123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11}
export OWNER=${OWNER:-777000111} CHAT=${CHAT:-777000111}
export MOCK_ADMIN=http://127.0.0.1:8082
MOCK_API=http://127.0.0.1:8081
SCRATCH=/tmp/98c-proof
MOCK_SRC=$SCRATCH/telegram-mock-ai
MOCK_BIN=$SCRATCH/mock-bin
DD=$SCRATCH/data-$LABEL
OUT=$SCRATCH/out-$LABEL
SRC_HOME=${THEUTH_SRC_HOME:-$HOME/.theuth}
T_BIN=$CARGO_TARGET_DIR/debug/t
MODEL_TIMEOUT=${MODEL_TIMEOUT:-120}; STEP_TIMEOUT=${STEP_TIMEOUT:-10}
RPC="bun $HERE/rpc.ts"; INJ="bun $HERE/inject.ts"

now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f\n", time*1000'; }
log() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" | tee -a "$OUT/proof.log" >&2; }
RESULTS=()
result() { # step status ms detail
  RESULTS+=("$1|$2|$3|$4"); log "$2 $1 (${3} ms) $4"
}
rpc() { RPC_TIMEOUT_S=${RPC_TIMEOUT_S:-$STEP_TIMEOUT} $RPC call "$DD" "$@"; }
engine_pids() { pgrep -f -- "--data-dir $DD" || true; }
stop_engine() {
  local pids; pids=$(engine_pids); [ -n "$pids" ] || return 0
  kill -TERM $pids 2>/dev/null
  for _ in $(seq 1 50); do [ -z "$(engine_pids)" ] && return 0; sleep 0.2; done
  log "engine did not exit on SIGTERM; SIGKILL"; kill -KILL $(engine_pids) 2>/dev/null || true
}
start_engine() { # $1 = log suffix
  env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH \
    THEUTH_RPC__LISTEN=127.0.0.1:0 THEUTH_SLACK__ENABLED=false \
    THEUTH_MESH__DISCOVERY=false THEUTH_MESH__RELAY=none \
    THEUTH_SECRETS__KEY_STORE=file THEUTH_AGENT__MODEL="$PROOF_MODEL" \
    RUST_LOG=info,telegram=debug,tgbot=debug \
    nohup "$T_BIN" --headless --data-dir "$DD" >"$OUT/engine-$1.out" 2>&1 &
  $RPC wait "$DD" 60 >/dev/null || { log "engine failed to start; see $OUT/engine-$1.out"; return 1; }
}
dump_threads() { # macOS `sample`: where every engine thread is parked right now
  local pid; pid=$(engine_pids | head -1); [ -n "$pid" ] || return 0
  sample "$pid" 2 -mayDie -file "$OUT/threads-$1.txt" >/dev/null 2>&1 && log "thread sample → $OUT/threads-$1.txt"
}
status_ms() { # prints "<ms> <state-type> <bot-username>" or "<ms> ERR"
  local t0 t1 s; t0=$(now_ms)
  if s=$(RPC_TIMEOUT_S=5 $RPC call "$DD" telegram_status 2>&1); then
    t1=$(now_ms)
    echo "$((t1 - t0)) $(echo "$s" | bun -e 'const s=JSON.parse(await Bun.stdin.text()); console.log((s.state?.type??"null")+" "+(s.bot?.username??"-")+" polling="+s.polling)')"
  else
    t1=$(now_ms); echo "$((t1 - t0)) ERR $s"
  fi
}
cleanup() {
  stop_engine
  [ -n "${LAT_PID:-}" ] && kill "$LAT_PID" 2>/dev/null
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null
  [ "$KEEP" = 1 ] || rm -rf "$DD"
}
trap cleanup EXIT

mkdir -p "$SCRATCH" "$OUT"; : >"$OUT/proof.log"
log "run=$LABEL root=$ROOT HEAD=$(git -C "$ROOT" rev-parse --short HEAD) out=$OUT"

# ---- prerequisites ----------------------------------------------------------
for b in go bun perl sqlite3 cargo; do command -v $b >/dev/null || { echo "missing $b"; exit 2; }; done
[ -d "$MOCK_SRC/.git" ] || git clone --quiet https://github.com/skrashevich/telegram-mock-ai "$MOCK_SRC"
if git -C "$MOCK_SRC" apply --check "$HERE/mock.patch" 2>/dev/null; then
  git -C "$MOCK_SRC" apply "$HERE/mock.patch" && log "mock.patch applied"
elif ! git -C "$MOCK_SRC" apply --check --reverse "$HERE/mock.patch" 2>/dev/null; then
  echo "mock.patch neither applies nor is applied in $MOCK_SRC"; exit 2
fi
log "mock: $(git -C "$MOCK_SRC" rev-parse --short HEAD) + mock.patch"
(cd "$MOCK_SRC" && go build -o "$MOCK_BIN" ./cmd/telegram-mock-ai) || exit 2
if [ "$BUILD" = 1 ]; then
  log "cargo build -p theuth in $ENGINE_SRC ($(git -C "$ENGINE_SRC" rev-parse --short HEAD)) …"
  (cd "$ENGINE_SRC" && cargo build -p theuth 2>&1 | tail -3 >>"$OUT/proof.log") || exit 2
fi
[ -x "$T_BIN" ] || { echo "no engine binary at $T_BIN"; exit 2; }

# ---- leftovers --------------------------------------------------------------
stop_engine
pkill -f "$MOCK_BIN -config" 2>/dev/null; sleep 0.3

# ---- mock -------------------------------------------------------------------
TELEGRAM_MOCK_LLM_ENABLED=false TELEGRAM_MOCK_PROACTIVE_ENABLED=false TELEGRAM_MOCK_WELCOME_ENABLED=false \
  nohup "$MOCK_BIN" -config "$HERE/mock-config.yaml" >"$OUT/mock.out" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do curl -sf $MOCK_ADMIN/api/health >/dev/null && break; sleep 0.1; done
curl -sf $MOCK_ADMIN/api/health >/dev/null || { log "mock did not come up"; exit 2; }
curl -s -X DELETE $MOCK_ADMIN/api/calls >/dev/null

# ---- scratch data dir -------------------------------------------------------
# The vault key is a per-data_dir Keychain item, so a copied ~/.theuth/secrets.sqlite
# cannot open here ("the mesh key does not open with this engine's key"). The proof
# seeds its own file-keyed vault from the environment instead (credentials.yaml is
# imported and unlinked by the engine on first open) and runs a model that key serves.
: "${ANTHROPIC_API_KEY:?ANTHROPIC_API_KEY must be set (model key for the scratch engine)}"
PROOF_MODEL=${PROOF_MODEL:-anthropic/claude-haiku-4-5}
rm -rf "$DD"; mkdir -p "$DD/acp"
cp "$SRC_HOME/config.yaml" "$DD/config.yaml"
(umask 077; printf 'ANTHROPIC_API_KEY: "%s"\n' "$ANTHROPIC_API_KEY" >"$DD/credentials.yaml")
cp "$SRC_HOME/acp/agents.yaml" "$DD/acp/agents.yaml"
[ -d "$SRC_HOME/skills" ] && cp -R "$SRC_HOME/skills" "$DD/skills"
grep -q '^telegram:' "$DD/config.yaml" && { echo "config.yaml already has a telegram: section"; exit 2; }
cat >>"$DD/config.yaml" <<EOF

# 98c live proof (appended by proof.sh)
telegram:
  enabled: true
  api_host: "$MOCK_API"
  owner: $OWNER
EOF

# ---- engine + latency sampler -----------------------------------------------
start_engine boot || exit 2
: >"$OUT/latency.csv"
$RPC latency "$DD" "$OUT/latency.csv" 2 &
LAT_PID=$!

# ---- P0 ---------------------------------------------------------------------
T0=$(now_ms); SEQ=$($INJ seq)
rpc set_secret "[\"telegram_bot\",\"$BOT_TOKEN\",\"98c live proof\"]" >/dev/null || log "set_secret failed"
if $INJ await getMe "$SEQ" $STEP_TIMEOUT >"$OUT/p0-getme.json" && $INJ await setMyCommands "$SEQ" $STEP_TIMEOUT >"$OUT/p0-setmycommands.json"; then
  sleep 1
  read -r ms st <<<"$(status_ms)"
  if [ "$ms" -lt 1000 ] && [[ "$st" == Identity* || "$st" == Connected* ]]; then
    result P0 PASS $(($(now_ms) - T0)) "getMe+setMyCommands seen; telegram_status ${ms} ms → $st"
  else
    result P0 FAIL $(($(now_ms) - T0)) "telegram_status ${ms} ms → $st"
  fi
else
  result P0 FAIL $(($(now_ms) - T0)) "mock saw no getMe/setMyCommands within ${STEP_TIMEOUT}s"
fi

# A model turn: inject text, expect reaction(👀) → sendChatAction → draft/rich/plain send
# → closing reaction (👌 done / 😱 failed / 🤷 cancelled). Args: step text
summary() { bun -e 'const e=JSON.parse(await Bun.stdin.text());console.log(e.method+" @"+e.waited_ms+"ms")'; }
turn() {
  local step=$1 text=$2 t0 seq react send final closed
  t0=$(now_ms); seq=$($INJ seq)
  $INJ message "$text" >"$OUT/$step-inject.json" || { result "$step" FAIL 0 "inject failed"; return; }
  if ! react=$($INJ await setMessageReaction "$seq" $MODEL_TIMEOUT --chat "$CHAT"); then
    dump_threads "$step"
    result "$step" FAIL $(($(now_ms) - t0)) "no setMessageReaction (👀) within ${MODEL_TIMEOUT}s: the message was never dispatched; telegram_status now: $(status_ms)"; return
  fi
  $INJ await sendChatAction "$seq" $STEP_TIMEOUT --chat "$CHAT" >/dev/null || log "$step: no sendChatAction (continuing)"
  if send=$($INJ await sendRichMessageDraft,sendRichMessage,sendMessage "$seq" $MODEL_TIMEOUT --chat "$CHAT"); then
    final=$($INJ await sendRichMessage,sendMessage "$seq" $MODEL_TIMEOUT --chat "$CHAT" | summary) || final="(no final send)"
    # the turn is over when the worker swaps 👀 for the closing reaction
    closed=$($INJ await setMessageReaction "$(echo "$react" | bun -e 'console.log(JSON.parse(await Bun.stdin.text()).seq)')" 30 --chat "$CHAT" --match '👌|😱|🤷' | summary) || closed="(no closing reaction)"
    result "$step" PASS $(($(now_ms) - t0)) "👀 → $(echo "$send" | summary), final $final, closed by $closed"
  else
    dump_threads "$step"
    result "$step" FAIL $(($(now_ms) - t0)) "👀 seen, no send within ${MODEL_TIMEOUT}s"
  fi
}

# ---- P1 ---------------------------------------------------------------------
turn P1 "hello, reply with one word"

# ---- P2 ---------------------------------------------------------------------
T0=$(now_ms); SEQ=$($INJ seq); P2_START=$(now_ms)
$INJ message "/sessions" >"$OUT/p2-inject.json"
if panel=$($INJ await sendRichMessage,sendMessage "$SEQ" $MODEL_TIMEOUT --chat "$CHAT" --match 'ession|title'); then
  PANEL_ID=$(echo "$panel" | bun -e 'const e=JSON.parse(await Bun.stdin.text());console.log(e.response?.result?.message_id??"")')
  result P2 PASS $(($(now_ms) - T0)) "$(echo "$panel" | bun -e 'const e=JSON.parse(await Bun.stdin.text());console.log(e.method+" message_id="+(e.response?.result?.message_id??"?")+" @"+e.waited_ms+"ms: "+JSON.stringify(e.params.rich_message??e.params.text).slice(0,120))')"
else
  PANEL_ID=""
  dump_threads P2
  result P2 FAIL $(($(now_ms) - T0)) "no sessions panel within ${MODEL_TIMEOUT}s; telegram_status now: $(status_ms)"
fi

# ---- P3 ---------------------------------------------------------------------
T0=$(now_ms); SEQ=$($INJ seq)
if [ -n "$PANEL_ID" ]; then
  DATA=$($INJ messages | ID=$PANEL_ID bun -e 'const id=Number(process.env.ID);const m=(JSON.parse(await Bun.stdin.text())).find(m=>m.message_id===id);const b=(m?.reply_markup?.inline_keyboard??[]).flat().find(b=>b.callback_data?.startsWith("cmd:"));console.log(b?.callback_data??"")')
  if [ -z "$DATA" ]; then
    result P3 FAIL 0 "panel $PANEL_ID has no cmd: button"
  else
    $INJ callback "$PANEL_ID" "$DATA" >"$OUT/p3-callback-id.txt"
    if ans=$($INJ await answerCallbackQuery "$SEQ" $STEP_TIMEOUT); then
      if ed=$($INJ await editMessageText,editMessageReplyMarkup "$SEQ" $MODEL_TIMEOUT --chat "$CHAT"); then
        result P3 PASS $(($(now_ms) - T0)) "pressed $DATA → answerCallbackQuery, $(echo "$ed" | summary)"
      else
        result P3 FAIL $(($(now_ms) - T0)) "pressed $DATA → answerCallbackQuery but no edit within ${MODEL_TIMEOUT}s"
      fi
    else
      result P3 FAIL $(($(now_ms) - T0)) "pressed $DATA → no answerCallbackQuery within ${STEP_TIMEOUT}s"
    fi
  fi
else
  result P3 SKIP 0 "no panel from P2"
fi

# ---- P3b: a force_reply prompt whose continuation hangs must not block the chat ---------
# /skills add (no argument) → prompt; reply with a git URL whose clone stalls (non-routable
# host) → immediately a plain message, which must still be answered.
T0=$(now_ms); SEQ=$($INJ seq)
$INJ message "/skills add" >"$OUT/p3b-inject-cmd.json"
if prompt=$($INJ await sendRichMessage,sendMessage "$SEQ" $STEP_TIMEOUT --chat "$CHAT" --match 'force_reply'); then
  PROMPT_ID=$(echo "$prompt" | bun -e 'const e=JSON.parse(await Bun.stdin.text());console.log(e.response?.result?.message_id??"")')
  SEQ=$($INJ seq)
  $INJ message "${HANG_URL:-http://10.255.255.1/hang.git}" --reply-to "$PROMPT_ID" >"$OUT/p3b-inject-reply.json"
  turn P3b "still there? one word"
  # the bounded continuation answers the prompt on its own (ASK_TIMEOUT + 1 s)
  if to=$($INJ await sendRichMessage,sendMessage "$SEQ" 40 --chat "$CHAT" --match 'did not answer|error:'); then
    log "P3b: prompt answered by $(echo "$to" | summary): $(echo "$to" | bun -e 'const e=JSON.parse(await Bun.stdin.text());console.log(JSON.stringify(e.params.rich_message??e.params.text).slice(0,120))')"
  else
    log "P3b: no bounded error reply to the prompt within 40 s"
  fi
else
  result P3b FAIL $(($(now_ms) - T0)) "no force_reply prompt for /skills add within ${STEP_TIMEOUT}s"
fi

# ---- P4 ---------------------------------------------------------------------
turn P4 "hello again, one word"
P4_END=$(now_ms)

# ---- P5 ---------------------------------------------------------------------
kill "$LAT_PID" 2>/dev/null; LAT_PID=
read -r P5_STATUS P5_DETAIL <<<"$(P2=$P2_START P4=$P4_END CSV=$OUT/latency.csv bun -e '
const p2=Number(process.env.P2), p4=Number(process.env.P4);
const rows=(await Bun.file(process.env.CSV).text()).trim().split("\n").filter(Boolean).map(l=>l.split(","));
const ok=rows.filter(r=>!r[2].startsWith("ERR"));
const max=Math.max(...ok.map(r=>Number(r[1])));
const win=rows.filter(r=>Number(r[0])>=p2&&Number(r[0])<=p4);
const winErr=win.filter(r=>r[2].startsWith("ERR")).length;
const errs=rows.length-ok.length;
const pass=rows.length>0&&errs===0&&max<1000&&win.length>0&&winErr===0;
console.log((pass?"PASS":"FAIL")+" samples="+rows.length+" max="+max+"ms errors="+errs+" during_P2-P4="+win.length+" (errors "+winErr+")");
')"
result P5 "$P5_STATUS" 0 "$P5_DETAIL"

# ---- evidence snapshot before the restart -----------------------------------
$INJ ledger 0 "$OUT/ledger-before-restart.txt" >/dev/null
$INJ ledger 0 "$OUT/ledger-before-restart-with-getupdates.txt" --all >/dev/null
$INJ messages >"$OUT/messages-before-restart.json"
rpc telegram_status >"$OUT/status-before-restart.json" 2>&1 || true

# ---- P6 ---------------------------------------------------------------------
T0=$(now_ms); SEQ=$($INJ seq)
stop_engine
rm -f "$DD/engine.json"
if start_engine restart; then
  T1=$(now_ms); P6_STATUS=FAIL; st="(never answered)"; ms=0; SEEN=""
  while [ $(($(now_ms) - T1)) -lt 5000 ]; do
    read -r ms st <<<"$(status_ms)"
    SEEN="$SEEN${SEEN:+ → }${st%% *}"
    if [[ "$st" == *theuth_proof_bot* ]]; then P6_STATUS=PASS; break; fi
    sleep 0.2
  done
  result P6 "$P6_STATUS" $(($(now_ms) - T0)) "restart; telegram_status ${ms} ms, $(($(now_ms) - T1)) ms after engine.json → $st; states seen: $SEEN"
  # the channel re-reads the token at boot: wait for getMe, then P1 again
  $INJ await getMe "$SEQ" $STEP_TIMEOUT >/dev/null || log "P6: no getMe after restart"
  sleep 2
  turn P6b "hello after restart, one word"
else
  result P6 FAIL $(($(now_ms) - T0)) "engine did not come back"
  result P6b SKIP 0 "engine down"
fi

# ---- evidence + table -------------------------------------------------------
$INJ ledger 0 "$OUT/ledger.txt" >/dev/null
$INJ ledger 0 "$OUT/ledger-with-getupdates.txt" --all >/dev/null
$INJ messages >"$OUT/messages.json"
cp "$DD"/logs/theuth.log.* "$OUT/" 2>/dev/null || true
{
  echo; echo "| step | status | ms | detail |"; echo "|---|---|---:|---|"
  for r in "${RESULTS[@]}"; do IFS='|' read -r s st ms d <<<"$r"; echo "| $s | $st | $ms | $d |"; done
} | tee "$OUT/table.md"
log "evidence: $OUT"
for r in "${RESULTS[@]}"; do case $r in *"|FAIL|"*) exit 1;; esac; done
exit 0
