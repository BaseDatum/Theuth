# theuth plugin SDK for bash 3.2+ (design 216), v0.1.0. Vendored: copy beside run.sh and source it.
# Write a theuth plugin in a few lines:
#
#   . "$(dirname "$0")/theuth-plugin.sh"
#   theuth_action inc 'theuth_state_add n 1'
#   theuth_run
#
# plugin.yaml is the contract: the host never runs this file to learn tools or subscriptions.
# `bash run.sh --theuth-manifest` prints what the code registered, to compare.
# Wire: newline-delimited JSON-RPC 2.0. Protocol lines go to fd 3 (a dup of the script's stdout, made when
# this file is sourced). theuth_run moves stdin to fd 4; handlers run with stdin from /dev/null.
# Handlers are strings eval'd in the current shell (functions and globals stay visible), one at a time.
# A request handler's stdout is its result (see theuth_action, theuth_tool, ...); stdout of hooks and
# event handlers goes to stderr (the plugin log). `return N` works in a handler string; `exit` ends the
# plugin. Do not run plugins under `set -e`.
# theuth_request (and call/ping/input/catalog) blocks and reads stdin itself, stacking other host messages
# for the loop to replay afterwards, so it works inside handlers. It is not re-entrant: never call it
# inside $(...) (a subshell loses the stacked messages and the request id).
# JSON is parsed with jq when on PATH, else python3 (THEUTH_JSON=jq|python3 forces one); one is required.
# No drive() test harness: test bash plugins through sdk/conformance/host.py.
# Raw hatch: theuth_send, theuth_request, theuth_on_request, theuth_on_notify.

THEUTH_SDK_PROTOCOL=1
exec 3>&1
_th_manifest=0
for _th_a in "$@"; do if [ "$_th_a" = --theuth-manifest ]; then _th_manifest=1; fi; done
_th_json=${THEUTH_JSON:-}
if [ -z "$_th_json" ]; then
  if command -v jq >/dev/null 2>&1; then _th_json=jq; elif command -v python3 >/dev/null 2>&1; then _th_json=python3; fi
fi
_th_k=() _th_n=() _th_c=()   # registry as parallel arrays (kind, name, code): bash 3.2 has no maps
_th_q=() _th_qh=0 _th_qt=0   # host messages stacked while a request waited for its response
_th_rid=0 _th_eof=0 _th_running=0 _th_fail= _th_npl=0 _th_pl0= _th_sdir= _th_tmp=
THEUTH_HELLO='{}' THEUTH_GRANTS='{}' THEUTH_CLIENTS='{}' THEUTH_DATA_DIR= THEUTH_CANCELLED=
THEUTH_RESULT= THEUTH_ERROR=

# ---- JSON backends: every op exists in jq and python3 with the same output ----
_th_jqp='def s: if . == null then "" elif type == "string" then . else tojson end;
def o: if type == "object" then . else {} end;
if length != 1 or (.[0] | type) != "object" then "_tp_ok=0" else .[0] |
(.params | o) as $p | (.method | s) as $m | ($p.grants | o) as $g |
($g.placements | if type == "array" then . else [] end) as $pls |
@sh "_tp_ok=1 _tp_m=\($m) _tp_hasm=\(has("method")) _tp_hasid=\(has("id"))
_tp_id=\(if has("id") then .id | tojson else "" end) _tp_p=\(.params // {} | tojson)
_tp_res=\(.result | tojson) _tp_haserr=\(has("error")) _tp_err=\(.error | tojson)
_tp_name=\($p[if $m == "action" then "action" elif $m == "query" then "query" else "name" end] | s)
_tp_args=\(if $m == "tools/call" then $p.arguments // {} else $p.args end | tojson)
_tp_pl=\($p.placement | s) _tp_topic=\($p.topic | s) _tp_type=\($p.type | s) _tp_data=\($p.data | tojson)
_tp_sess=\(if $p | has("session") then $p.session else ($p._meta | o | .theuth | o | .session) end | s)
_tp_cid=\($p.requestId | tojson) _tp_proto=\($p.protocol | tojson) _tp_grants=\($g | tojson)
_tp_npl=\($pls | length) _tp_pl0=\($pls[0] | s) _tp_ddir=\($p.data_dir | s) _tp_clients=\($p.clients | o | tojson)"
end'
_th_py='import json, re, shlex, sys
op, a = sys.argv[1], sys.argv[2:]
def d(v): return json.dumps(v, ensure_ascii=False, separators=(",", ":"))
def s(v): return "" if v is None else v if isinstance(v, str) else d(v)
def o(v): return v if isinstance(v, dict) else {}
try: v = json.loads(sys.stdin.read())
except ValueError: sys.exit(1)
if op == "parse":
    if not isinstance(v, dict): print("_tp_ok=0"); sys.exit()
    p, m = o(v.get("params")), s(v.get("method"))
    g = o(p.get("grants")); pls = g.get("placements") if isinstance(g.get("placements"), list) else []
    out = dict(ok=1, m=m, hasm="method" in v, hasid="id" in v, id=d(v["id"]) if "id" in v else "",
        p=d(v.get("params") or {}), res=d(v.get("result")), haserr="error" in v, err=d(v.get("error")),
        name=s(p.get({"action": "action", "query": "query"}.get(m, "name"))),
        args=d(p.get("arguments") or {}) if m == "tools/call" else d(p.get("args")),
        pl=s(p.get("placement")), topic=s(p.get("topic")), type=s(p.get("type")), data=d(p.get("data")),
        sess=s(p["session"] if "session" in p else o(o(p.get("_meta")).get("theuth")).get("session")),
        cid=d(p.get("requestId")), proto=d(p.get("protocol")), grants=d(g), npl=len(pls),
        pl0=s(pls[0]) if pls else "", ddir=s(p.get("data_dir")), clients=d(o(p.get("clients"))))
    print(" ".join("_tp_%s=%s" % (k, shlex.quote(str(x).lower() if isinstance(x, bool) else str(x)))
                   for k, x in out.items()))
elif op == "get":
    path = a[0].strip(); ln = re.search(r"\|\s*length\s*$", path)
    path = path[:ln.start()].strip() if ln else path
    T = r"\.?\[(-?\d+)\]|\.?\[\"([^\"]*)\"\]|\.([A-Za-z_]\w*)|\.\"([^\"]*)\"|\."
    if not re.fullmatch("(?:%s)*" % T, path):
        sys.stderr.write("theuth_get: the python3 fallback supports .a.b, .[0], .[\"k\"] and |length\n"); sys.exit(2)
    for i, k, n, q in re.findall(T, path):
        if i: i = int(i); v = v[i] if isinstance(v, list) and -len(v) <= i < len(v) else None
        elif k or n or q: v = v.get(k or n or q) if isinstance(v, dict) else None
    if ln: v = len(v) if isinstance(v, (list, dict, str)) else 0 if v is None else abs(v)
    print(v if isinstance(v, str) else d(v))
elif op == "compact": print(d(v))
elif op == "toolres": sys.exit(0 if isinstance(v, dict) and ("content" in v or "isError" in v) and print(d(v)) is None else 1)
elif op == "obj": print(d(v)) if isinstance(v, dict) else sys.exit(1)
elif op == "sget": print(d(v.get(a[0])))
elif op == "sint": x = v.get(a[0]); print(x if isinstance(x, int) and not isinstance(x, bool) else 0)
elif op == "sset":
    try: v[a[0]] = json.loads(a[1])
    except ValueError: sys.exit(1)
    print(d(v))'
_th_j() { # OP [ARG...] <JSON
  if [ "$_th_json" = jq ]; then
    case $1 in
      parse) jq -rs "$_th_jqp" 2>/dev/null ;;
      compact) jq -cs 'if length == 1 then .[0] else error("expected one JSON value") end' ;;
      toolres) jq -cse 'if length == 1 and (.[0] | type) == "object" and (.[0] | has("content") or has("isError")) then .[0] else null end' 2>/dev/null ;;
      obj) jq -cs 'if length == 1 and (.[0] | type) == "object" then .[0] else error("not an object") end' 2>/dev/null ;;
      get) jq -rc "$2" ;;
      sget) jq -c --arg k "$2" '.[$k]' ;;
      sint) jq --arg k "$2" '.[$k] | if type == "number" and . == floor then . else 0 end' ;;
      sset) jq -c --arg k "$2" --argjson v "$3" '.[$k] = $v' ;;
    esac
  else PYTHONIOENCODING=utf-8 python3 -c "$_th_py" "$@"; fi
}
_th_parse() { local o; _tp_ok=0 _tp_m=; o=$(_th_j parse <<<"$1") && eval "$o"; }

# _th_str S: JSON-encode a string into $_th_s (no ASCII/HTML escaping), in pure bash.
_th_str() {
  local s=$1 o= c i
  case $s in *[\\\"]*) s=${s//\\/\\\\}; s=${s//\"/\\\"} ;; esac
  case $s in *[$'\001'-$'\037']*)
    s=${s//$'\n'/\\n}; s=${s//$'\r'/\\r}; s=${s//$'\t'/\\t}
    case $s in *[$'\001'-$'\037']*)
      for ((i = 0; i < ${#s}; i++)); do
        c=${s:i:1}
        case $c in [$'\001'-$'\037']) printf -v c '\\u%04x' "'$c" ;; esac
        o=$o$c
      done
      s=$o ;;
    esac ;;
  esac
  _th_s="\"$s\""
}

# _th_obj PAIRS...: build a JSON object into $_th_o. `key=value` is a string (`badge` digits -> number),
# `key:=json` is raw JSON. A repeated key keeps its first position and its last value.
_th_obj() {
  local a k v i n
  _th_ok=() _th_ov=()
  for a in "$@"; do
    case $a in *=*) ;; *) echo "theuth: expected key=value or key:=json, got: $a" >&2; return 1 ;; esac
    k=${a%%=*} v=${a#*=}
    case $k in
      *:) k=${k%:}; [ -n "$v" ] || v=null ;;
      badge) case $v in '' | *[!0-9]*) _th_str "$v"; v=$_th_s ;; *) v=$((10#$v)) ;; esac ;;
      *) _th_str "$v"; v=$_th_s ;;
    esac
    n=${#_th_ok[@]} i=0
    while [ $i -lt $n ] && [ "${_th_ok[$i]}" != "$k" ]; do i=$((i + 1)); done
    _th_ok[$i]=$k _th_ov[$i]=$v
  done
  _th_o= i=0
  while [ $i -lt ${#_th_ok[@]} ]; do _th_str "${_th_ok[$i]}"; _th_o="$_th_o,$_th_s:${_th_ov[$i]}"; i=$((i + 1)); done
  _th_o="{${_th_o#,}}"
}

_th_write() { printf '%s\n' "$1" >&3; }
_th_result() { _th_write "{\"jsonrpc\":\"2.0\",\"id\":$1,\"result\":$2}"; }
_th_error() { _th_str "$3"; _th_write "{\"jsonrpc\":\"2.0\",\"id\":$1,\"error\":{\"code\":$2,\"message\":$_th_s}}"; }

# ---- registration: CODE is a string eval'd in the current shell ----
_th_add() { local i=${#_th_k[@]}; _th_k[$i]=$1; _th_n[$i]=$2; _th_c[$i]=$3; }
_th_reg() {
  local i=0
  while [ $i -lt ${#_th_k[@]} ]; do
    if [ "${_th_k[$i]}" = "$1" ] && [ "${_th_n[$i]}" = "$2" ]; then _th_c[$i]=$3; return 0; fi
    i=$((i + 1))
  done
  _th_add "$@"
}
_th_find() { # KIND NAME -> $_th_f (code); status 1 when none
  local i=0
  while [ $i -lt ${#_th_k[@]} ]; do
    if [ "${_th_k[$i]}" = "$1" ] && [ "${_th_n[$i]}" = "$2" ]; then _th_f=${_th_c[$i]}; return 0; fi
    i=$((i + 1))
  done
  return 1
}
# theuth_on NAME CODE: event handler. NAME: `agent.events#TurnFinished`, `agent.events`,
# `turns.limit_reached` (a plugins.events type) or `*`; first match wins. Context: THEUTH_TOPIC,
# THEUTH_TYPE, THEUTH_SESSION, THEUTH_DATA (JSON), THEUTH_PARAMS.
theuth_on() { _th_reg event "$1" "$2"; }
# theuth_action NAME CODE: stdout JSON is the result (empty -> {"ok":true}). Args: THEUTH_ARGS (JSON).
theuth_action() { _th_reg action "$1" "$2"; }
# theuth_query NAME CODE: OpenUI Query(name). Stdout JSON is the value (empty -> null).
theuth_query() { _th_reg query "$1" "$2"; }
# theuth_command NAME CODE: `/plugin:name`. Stdout text -> {"text": …}; a JSON object is sent as-is; empty -> {}.
theuth_command() { _th_reg command "$1" "$2"; }
# theuth_tool NAME CODE: tools/call of a tool declared in plugin.yaml. Stdout (trailing newlines cut) is the text
# content, unless it is a JSON object with `content` or `isError`: that CallToolResult is sent as-is (structuredContent).
# A non-zero status answers isError with the stdout text (or "failed (status N)").
theuth_tool() { _th_reg tool "$1" "$2"; }
# theuth_on_request METHOD CODE: raw hatch for a host request; stdout JSON is the result (empty -> null).
theuth_on_request() { _th_reg request "$1" "$2"; }
# theuth_on_notify METHOD CODE: raw hatch for a host notification (params in THEUTH_PARAMS).
theuth_on_notify() { _th_reg notify "$1" "$2"; }
# Hooks (several allowed, run in order; stdout goes to the log). on_start runs after hello and, when no
# on_render hook exists, on `render`. on_granted: THEUTH_GRANTS is already replaced. on_lagged default:
# a warn log. on_cancel: the id is already in THEUTH_CANCELLED.
theuth_on_start() { _th_add start "" "$1"; }
theuth_on_shutdown() { _th_add shutdown "" "$1"; }
theuth_on_granted() { _th_add granted "" "$1"; }
theuth_on_lagged() { _th_add lagged "" "$1"; }
theuth_on_cancel() { _th_add cancel "" "$1"; }
theuth_on_render() { _th_add render "" "$1"; }

# ---- plugin -> host notifications: arguments are key=value / key:=json pairs ----
# theuth_send METHOD [PAIRS...]: raw notification.
theuth_send() {
  local m=$1; shift
  _th_obj "$@" || return 1
  _th_str "$m"; _th_write "{\"jsonrpc\":\"2.0\",\"method\":$_th_s,\"params\":$_th_o}"
}
_th_place() { # sets $_th_pa to the default placement=… pair unless the arguments name one
  local a; _th_pa=
  for a in "$@"; do case $a in placement=* | placement:=*) return 0 ;; esac; done
  if [ "$_th_npl" != 1 ]; then echo "theuth: pass placement=…: the grant holds $_th_npl placements" >&2; return 1; fi
  _th_pa=placement=$_th_pl0
}
# theuth_status dot=ok|busy|warn|error|off|none [badge=N|text] [label=…] [tooltip=…] [placement=…]
theuth_status() { _th_place "$@" || return 1; theuth_send status ${_th_pa:+"$_th_pa"} "$@"; }
# theuth_ui text=… [button=Label:action[:style]]... [source=…] [format=…] [placement=…]
# Without source=, format openui (the default) sends an OpenUI card of text + buttons; another format sends text.
theuth_ui() {
  local a text= fmt= src= has=0 pl= muts= calls= i=0 rest label act style
  for a in "$@"; do
    case $a in
      text=*) text=${a#text=} ;;
      format=*) fmt=${a#format=} ;;
      source=*) src=${a#source=} has=1 ;;
      placement=*) pl=$a ;;
      button=*)
        rest=${a#button=}; label=${rest%%:*}; rest=${rest#*:}; act=${rest%%:*} style=secondary
        case $rest in *:*) style=${rest#*:} ;; esac
        _th_str "$act"; muts="$muts"$'\n'"a$i = Mutation($_th_s, {})"
        _th_str "$label"; label=$_th_s; _th_str "$style"
        calls="$calls, Button($label, @Run(a$i), $_th_s)" i=$((i + 1)) ;;
      *) echo "theuth_ui: unknown argument: $a" >&2; return 1 ;;
    esac
  done
  if [ $has = 0 ] && [ "${fmt:-openui}" = openui ]; then
    _th_str "$text"; fmt=openui src="root = Card([body])"$'\n'"body = TextContent($_th_s)"
    [ $i = 0 ] || src="root = Card([body, controls])"$'\n'"body = TextContent($_th_s)$muts"$'\n'"controls = Buttons([${calls#, }])"
  elif [ $has = 0 ]; then src=$text; fi
  _th_place $pl || return 1
  theuth_send ui ${_th_pa:+"$_th_pa"} ${pl:+"$pl"} format="${fmt:-openui}" text="$text" source="$src"
}
# theuth_emit EVENT [PAIRS...]: publish a declared event with data built from PAIRS. The host drops an
# undeclared name (-32003 in the plugin log); the SDK cannot see that. Session: theuth_send emit … session=….
theuth_emit() { local e=$1; shift; _th_obj "$@" || return 1; theuth_send emit event="$e" data:="$_th_o"; }
# theuth_notify title=… [body=…] [level=info] [open:=json]: inbox item (capability `notify`).
theuth_notify() { theuth_send notify title= body= level=info "$@"; }
# theuth_invalidate [placement=…]: clients re-run the placement's OpenUI Queries.
theuth_invalidate() { _th_place "$@" || return 1; theuth_send invalidate ${_th_pa:+"$_th_pa"} "$@"; }
# theuth_log MESSAGE [level=info] [PAIRS...]: PAIRS become `fields`.
theuth_log() {
  local m=$1 lv=info a n; shift; n=$#
  for a in "$@"; do case $a in level=*) lv=${a#level=} ;; *) set -- "$@" "$a" ;; esac; done
  shift $n
  if [ $# = 0 ]; then theuth_send log level="$lv" message="$m"; return; fi
  _th_obj "$@" || return 1; theuth_send log level="$lv" message="$m" fields:="$_th_o"
}

# ---- plugin -> host requests: status 0 + THEUTH_RESULT, or status 1 + THEUTH_ERROR (JSON error) ----
# theuth_request METHOD [PARAMS_JSON]: raw request. Reads stdin until its response; on EOF -> -32006.
theuth_request() {
  local _th_my _th_p=${2:-}
  [ -n "$_th_p" ] || _th_p='{}'
  THEUTH_RESULT= THEUTH_ERROR=
  if [ "$_th_running" != 1 ]; then
    THEUTH_ERROR='{"code":-32603,"message":"theuth_request outside theuth_run"}'
    echo "theuth_request $1: only inside theuth_run handlers" >&2; return 1
  fi
  _th_rid=$((_th_rid + 1)); _th_my=$_th_rid
  _th_str "$1"; _th_write "{\"jsonrpc\":\"2.0\",\"id\":$_th_my,\"method\":$_th_s,\"params\":$_th_p}"
  while _th_fresh; do
    [ "$_tp_ok" = 1 ] || continue
    if [ "$_tp_hasm" = true ]; then _th_q[$_th_qt]=$_th_line _th_qt=$((_th_qt + 1)); continue; fi
    [ "$_tp_hasid" = true ] && [ "$_tp_id" = "$_th_my" ] || continue
    if [ "$_tp_haserr" = true ]; then THEUTH_ERROR=$_tp_err; return 1; fi
    THEUTH_RESULT=$_tp_res; return 0
  done
  THEUTH_ERROR='{"code":-32006,"message":"host closed stdin"}'; return 1
}
# theuth_call TOOL [ARGS_JSON]: host-brokered tool call; THEUTH_RESULT is the ToolOutput {content, is_error, structured?}.
theuth_call() { local a=${2:-}; _th_str "$1"; theuth_request call "{\"tool\":$_th_s,\"args\":${a:-"{}"}}"; }
# theuth_ping: THEUTH_RESULT is {now_ms}.
theuth_ping() { theuth_request ping '{}'; }
# theuth_input TEXT [SESSION] [DELIVERY]: send text into a session as the user (capability `input`).
theuth_input() {
  local p; _th_str "$1"; p="{\"text\":$_th_s,\"session\":"
  if [ -n "${2:-}" ]; then _th_str "$2"; p=$p$_th_s; else p=${p}null; fi
  if [ -n "${3:-}" ]; then _th_str "$3"; p="$p,\"delivery\":$_th_s"; fi
  theuth_request input "$p}"
}
# theuth_catalog [TOPIC]
theuth_catalog() { if [ -n "${1:-}" ]; then _th_str "$1"; theuth_request catalog "{\"topic\":$_th_s}"; else theuth_request catalog; fi; }
# theuth_fail CODE MESSAGE [DATA_JSON]: answer the current request with this JSON-RPC error. Returns 1.
theuth_fail() {
  _th_str "${2:-}"; _th_fail="{\"code\":$1,\"message\":$_th_s"
  if [ -n "${3:-}" ]; then _th_fail="$_th_fail,\"data\":$3"; fi
  _th_fail=$_th_fail'}'; return 1
}
# theuth_cancelled [ID_JSON]: status 0 when the host cancelled that request (default: the current one).
theuth_cancelled() { case $'\n'$THEUTH_CANCELLED in *$'\n'"${1-$THEUTH_ID}"$'\n'*) return 0 ;; esac; return 1; }
# theuth_get PATH [JSON]: the value at a jq path in JSON (default $THEUTH_PARAMS); strings raw, else JSON.
# With jq any filter works; the python3 fallback supports `.a.b`, `.[0]`, `.["k"]` and a trailing `|length`.
theuth_get() { _th_j get "$1" <<<"${2-$THEUTH_PARAMS}"; }

# ---- state: an object in <dir>/state.json, dir = $THEUTH_PLUGIN_DATA, hello data_dir, or ./.plugin-data ----
_th_sload() {
  _th_sf=${THEUTH_PLUGIN_DATA:-${_th_sdir:-./.plugin-data}}/state.json _th_st='{}'
  [ -f "$_th_sf" ] || return 0
  _th_st=$(_th_j obj <"$_th_sf") || { _th_st='{}'; theuth_log "state.json unreadable, starting empty" level=warn; }
}
_th_sput() { # KEY JSON: set and save atomically (state.tmp + mv)
  local t d=${_th_sf%/*}
  t=$(_th_j sset "$1" "$2" <<<"$_th_st") || { echo "theuth_state_set $1: invalid JSON: $2" >&2; return 1; }
  _th_st=$t
  mkdir -p "$d" && printf '%s' "$t" >"$d/state.tmp" && mv -f "$d/state.tmp" "$_th_sf"
}
# theuth_state_get KEY: prints the JSON value (null when missing).
theuth_state_get() { _th_sload; _th_j sget "$1" <<<"$_th_st"; }
# theuth_state_int KEY: prints the integer (0 when missing or not an integer).
theuth_state_int() { _th_sload; _th_j sint "$1" <<<"$_th_st"; }
# theuth_state_set KEY JSON
theuth_state_set() { _th_sload; _th_sput "$1" "$2"; }
# theuth_state_add KEY [N=1]: read-modify-write; prints nothing (an action's stdout is its result).
theuth_state_add() { local v; _th_sload; v=$(_th_j sint "$1" <<<"$_th_st") && _th_sput "$1" $((v + ${2:-1})); }

# ---- dispatch ----
_th_eval() { local _th_code=$1; shift; eval "$_th_code"; }
_th_bg() { # CODE WHAT: hook/event handler, stdout to the log; a failure is logged and the loop goes on
  _th_eval "$1" >&2 </dev/null || { local rc=$?; theuth_log "$2 failed: status $rc" level=error; return $rc; }
}
_th_hooks() { # KIND: run every hook of KIND; status of the last failure
  local _th_i=0 _th_bad=0
  while [ $_th_i -lt ${#_th_k[@]} ]; do
    if [ "${_th_k[$_th_i]}" = "$1" ]; then _th_bg "${_th_c[$_th_i]}" "on_$1" || _th_bad=$?; fi
    _th_i=$((_th_i + 1))
  done
  return $_th_bad
}
_th_exec() { # CODE: request handler; stdout -> $_th_out (trailing newlines cut), status -> $_th_rc
  _th_fail=
  _th_eval "$1" >"$_th_tmp" </dev/null
  _th_rc=$? _th_out=
  IFS= read -r -d '' _th_out <"$_th_tmp"
  while :; do case $_th_out in *$'\n') _th_out=${_th_out%$'\n'} ;; *) break ;; esac; done
}
_th_fresh() { # read + parse one host line from fd 4; cancellations are recorded here, at read time
  [ "$_th_eof" = 0 ] || return 1
  _th_line=
  if ! IFS= read -r -u 4 _th_line && [ -z "$_th_line" ]; then _th_eof=1; return 1; fi
  _th_line=${_th_line%$'\r'}
  _th_parse "$_th_line"
  if [ "$_tp_m" = notifications/cancelled ]; then THEUTH_CANCELLED=$THEUTH_CANCELLED$_tp_cid$'\n'; fi
}
_th_next() { # stacked messages first, then stdin
  if [ $_th_qh -lt $_th_qt ]; then
    _th_line=${_th_q[$_th_qh]}; unset "_th_q[$_th_qh]"; _th_qh=$((_th_qh + 1))
    if [ $_th_qh = $_th_qt ]; then _th_qh=0 _th_qt=0; fi
    _th_parse "$_th_line"; return 0
  fi
  _th_fresh
}
_th_ctx() {
  THEUTH_MSG=$_th_line THEUTH_METHOD=$_tp_m THEUTH_ID=$_tp_id THEUTH_NAME=$_tp_name THEUTH_PARAMS=$_tp_p
  THEUTH_ARGS=$_tp_args THEUTH_PLACEMENT=$_tp_pl THEUTH_SESSION=$_tp_sess THEUTH_TOPIC=$_tp_topic
  THEUTH_TYPE=$_tp_type THEUTH_DATA=$_tp_data
}
_th_hello() {
  if [ "$_tp_proto" != "$THEUTH_SDK_PROTOCOL" ]; then
    theuth_log "unsupported protocol $_tp_proto; this SDK speaks $THEUTH_SDK_PROTOCOL" level=error
    rm -f "$_th_tmp"; exit 1
  fi
  THEUTH_HELLO=$_tp_p THEUTH_GRANTS=$_tp_grants THEUTH_CLIENTS=$_tp_clients THEUTH_DATA_DIR=$_tp_ddir
  _th_npl=$_tp_npl _th_pl0=$_tp_pl0
  if [ -z "${THEUTH_PLUGIN_DATA:-}" ]; then _th_sdir=$_tp_ddir; fi
  _th_hooks start
}
_th_request() { # ID METHOD: answer one host request
  local _th_kind= _th_r
  case $2 in action | query | command) _th_kind=$2 ;; tools/call) _th_kind=tool ;; esac
  if [ -n "$_th_kind" ]; then
    _th_find "$_th_kind" "$THEUTH_NAME" || { _th_error "$1" -32601 "$2 not handled: $THEUTH_NAME"; return; }
    _th_exec "$_th_f"
  elif _th_find request "$2"; then _th_kind=request; _th_exec "$_th_f"
  elif [ "$2" = render ] && { _th_find render "" || _th_find start ""; }; then
    if _th_find render ""; then _th_hooks render; else _th_hooks start; fi
    _th_rc=$? _th_out= _th_kind=render _th_fail=
  else _th_error "$1" -32601 "method not found: $2"; return; fi
  if [ -n "$_th_fail" ]; then _th_write "{\"jsonrpc\":\"2.0\",\"id\":$1,\"error\":$_th_fail}"; return; fi
  if [ "$_th_rc" != 0 ]; then
    _th_out=${_th_out:-"failed (status $_th_rc)"}
    theuth_log "$2${THEUTH_NAME:+ $THEUTH_NAME} failed: $_th_out" level=error
    if [ $_th_kind != tool ]; then _th_error "$1" -32603 "$_th_out"; return; fi
    _th_str "$_th_out"; _th_result "$1" "{\"content\":[{\"type\":\"text\",\"text\":$_th_s}],\"isError\":true}"; return
  fi
  if [ $_th_kind = tool ] && case ${_th_out#"${_th_out%%[![:space:]]*}"} in \{*) ;; *) false ;; esac &&
    _th_r=$(_th_j toolres <<<"$_th_out"); then _th_result "$1" "$_th_r"; return; fi
  case $_th_kind:$_th_out in
    tool:*) _th_str "$_th_out"; _th_result "$1" "{\"content\":[{\"type\":\"text\",\"text\":$_th_s}],\"isError\":false}"; return ;;
    render:*) _th_result "$1" '{}'; return ;;
    action:) _th_out='{"ok":true}' ;;
    command:) _th_out='{}' ;;
    command:\{*) ;;
    command:*) _th_str "$_th_out"; _th_result "$1" "{\"text\":$_th_s}"; return ;;
    *:) _th_out=null ;;
  esac
  if _th_r=$(_th_j compact <<<"$_th_out"); then _th_result "$1" "$_th_r"
  else _th_error "$1" -32603 "$2 handler printed invalid JSON: ${_th_out:0:200}"; fi
}
_th_dispatch() { # one host message after hello (context set); status 1 on shutdown
  local _th_key
  if [ "$_tp_hasid" = true ]; then _th_request "$_tp_id" "$_tp_m"; return 0; fi
  case $_tp_m in
    shutdown) return 1 ;;
    event)
      for _th_key in "$THEUTH_TOPIC#$THEUTH_TYPE" "$THEUTH_TOPIC" "$THEUTH_TYPE" '*'; do
        if _th_find event "$_th_key"; then _th_bg "$_th_f" event; break; fi
      done ;;
    granted) THEUTH_GRANTS=$_tp_grants _th_npl=$_tp_npl _th_pl0=$_tp_pl0; _th_hooks granted ;;
    lagged)
      if _th_find lagged ""; then _th_hooks lagged
      else theuth_send log level=warn message="lagged: $(theuth_get .dropped) events dropped" fields:="$THEUTH_PARAMS"; fi ;;
    notifications/cancelled) _th_hooks cancel ;;
    *) if _th_find notify "$_tp_m"; then _th_bg "$_th_f" "$_tp_m"; fi ;;
  esac
  return 0
}
_th_manifest_print() {
  local row i out
  echo "# registered in code (plugin.yaml is the contract)"
  for row in tools:tool actions:action queries:query commands:command on:event; do
    out= i=0
    while [ $i -lt ${#_th_k[@]} ]; do
      if [ "${_th_k[$i]}" = "${row#*:}" ]; then _th_str "${_th_n[$i]}"; out="$out, $_th_s"; fi
      i=$((i + 1))
    done
    echo "${row%%:*}: [${out#, }]"
  done
}
# theuth_run: with --theuth-manifest in the script's arguments print the manifest and exit 0 (stdin unread).
# Otherwise wait for hello, run on_start hooks, dispatch until shutdown or stdin EOF, run on_shutdown
# hooks and exit 0.
theuth_run() {
  if [ "$_th_manifest" = 1 ]; then _th_manifest_print; exit 0; fi
  case $_th_json in jq | python3) command -v "$_th_json" >/dev/null 2>&1 ;; *) false ;; esac ||
    { echo "theuth-plugin.sh: needs jq or python3 (THEUTH_JSON=$_th_json)" >&2; exit 1; }
  _th_tmp=$(mktemp "${TMPDIR:-/tmp}/theuth-plugin.XXXXXX") || exit 1
  exec 4<&0 0</dev/null
  _th_running=1
  local _th_hi=0
  while _th_next; do
    [ "$_tp_ok" = 1 ] || continue
    if [ $_th_hi = 0 ]; then
      if [ "$_tp_m" = hello ]; then _th_hi=1; _th_ctx; _th_hello
      elif [ "$_tp_hasm" = true ] && [ "$_tp_hasid" = true ]; then _th_error "$_tp_id" -32601 "hello first"; fi
    elif [ "$_tp_hasm" = true ]; then _th_ctx; _th_dispatch || break
    fi
  done
  _th_hooks shutdown
  rm -f "$_th_tmp"
  exit 0
}
