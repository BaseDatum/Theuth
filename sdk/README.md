# theuth plugin SDKs

A theuth plugin is a folder: `plugin.yaml` (the contract the install card shows) and a program that speaks
newline-delimited JSON-RPC 2.0 on stdin/stdout (design 216 §6). These SDKs make that program a short script.
Each is one vendored file, no dependencies beyond the language's standard library (Rust: `serde_json`).

`theuth plugin new <name> --lang py|js|sh|go|rb|rs` copies `templates/<lang>/` and the SDK file into
`~/.theuth/plugins/<name>/`, renaming `presses`. `--raw` copies `templates/raw-py` or `templates/raw-sh`:
the same plugin without an SDK, the shape of the 216 examples. `py`, `js` and `sh` need no compile step;
`go run .` and `cargo run` compile on first start (the language's cost, not theuth's).

| Lang | SDK file (copied beside your code) | Hello world | `run:` |
|---|---|---|---|
| Python 3.9+ | `python/theuth_plugin.py` | `templates/py/main.py` | `[python3, main.py]` |
| JavaScript (Node 18+, Bun, Deno 2) | `ts/theuth-plugin.js` (+ `.d.ts`) | `templates/js/main.mjs` (+ `package.json`: `"type": "module"`, which Node 18–20 need) | `[node, main.mjs]` |
| bash 3.2+ (jq or python3) | `sh/theuth-plugin.sh` | `templates/sh/run.sh` | `[bash, run.sh]` |
| Go 1.22+ | `go/theuth.go` (`package main`) | `templates/go/main.go` | `[go, run, .]` |
| Ruby | `ruby/theuth_plugin.rb` | `templates/rb/main.rb` | `[ruby, main.rb]` |
| Rust | `rust/src/lib.rs` → `src/theuth.rs` | `templates/rs/src/main.rs` | `[cargo, run, --quiet]` |

Deno: `run: [deno, run, --allow-env, --allow-read, --allow-write, main.mjs]`.

## Quickstart

The hello world: a sidebar dot with a badge, a popover with one button that counts presses, one tool the
model can call. Python (13 lines):

```python
from theuth_plugin import plugin
def paint():
    n = plugin.state.int("n")
    plugin.status(dot="ok", badge=n, label=str(n))
    plugin.ui(text=f"{n} presses", buttons=[("Press", "inc")])
@plugin.action("inc")
def inc(_):
    plugin.state.add("n", 1); paint()
@plugin.tool("presses")
def presses(_):
    return str(plugin.state.int("n"))
plugin.on_start(paint)
plugin.run()
```

The same program in every language is in `templates/<lang>/`. Its `plugin.yaml` declares the placement
`presses` (sidebar, popover, openui, status dot) and the tool `presses`.

## Verbs

Same names everywhere. Go capitalizes (`plugin.OnStart`), bash prefixes (`theuth_on_start`), JavaScript
also accepts camelCase (`plugin.onStart`).

| Verb | What it does |
|---|---|
| `on(name, fn)` | Event handler. `name` is `agent.events#TurnFinished`, `agent.events`, a plugin event `turns.limit_reached`, or `*`. First match wins in that order. Names: [catalog.md](catalog.md). |
| `action(name, fn)` | A press on an OpenUI `Mutation(name, …)`. Return value → `result`; nothing → `{"ok": true}`. |
| `query(name, fn)` | An OpenUI `Query(name, …)`. Return value → the query's value. |
| `command(name, fn)` | `/plugin:name`. A string → `{"text": …}`; an object as-is; nothing → `{}`. |
| `tool(name, fn)` | `tools/call` for a tool declared in `plugin.yaml`. A string → text content; an MCP `CallToolResult` (has `content`/`isError`) as-is; any other value → its JSON as text (+ `structuredContent` for objects). |
| `on_start(fn)` | After `hello`. Paint here. |
| `on_render(fn)` | `render` request (a client opens a placement the host has no content for). Without it, `on_start` hooks re-run. The SDK replies `{}`. |
| `on_shutdown(fn)` | `shutdown` or stdin EOF; then the process exits 0. |
| `on_granted(fn)` / `on_lagged(fn)` / `on_cancel(fn)` | `granted` (the SDK has already replaced `plugin.grants`), `lagged` (default: a warn log), `notifications/cancelled` (the id is in `plugin.cancelled`). |
| `status(dot, badge, label, tooltip, placement)` | Dot `ok│busy│warn│error│off│none`, badge (int, 0 hides; or ≤ 4 chars). |
| `ui(text, buttons, source, format, placement)` | Replace a placement's content. `text` is always sent (TUI and channels show it). `buttons` = `[(label, action[, style])]` writes the OpenUI card below; `source` is sent unchanged. |
| `emit(event, data, session)` | A declared event. An undeclared name is dropped by the host (`-32003` in the plugin log), which the SDK cannot see. |
| `notify(title, body, level, open)` | Inbox item; needs capability `notify`. |
| `invalidate(placement)` | Clients re-run the placement's `Query`s without remounting. |
| `log(message, level, fields)` | A line in the plugin log. Not stdout. |
| `call(tool, args)` | Host-brokered tool call (capability `call`). Returns the `ToolOutput` `{content, is_error, structured?}` — snake_case, unlike `tools/call` results. |
| `input(text, session, delivery)` / `catalog(topic)` / `ping()` | The other host requests. |
| `state` | A dict in `$THEUTH_PLUGIN_DATA/state.json`: `get`, `int`, `set`, `add` (each write is atomic), `save`. |
| `send(method, params)` / `request(method, params)` | Raw notification / raw request. |
| `on_request(method, fn)` / `on_notify(method, fn)` | Raw handlers for host methods the helpers do not wrap. |
| `run()` | Read `hello`, call `on_start`, dispatch until `shutdown` or EOF. |
| `drive(lines, responder)` | Test harness (not bash): feed host messages, get back what the plugin sent. |

Placement defaults to the only id in `grants.placements`; with more than one, pass it (omitting it is a local
error, nothing is sent).

The card `ui(text, buttons)` writes, byte-for-byte the same in every SDK (so equal state never remounts the
popover):

```
root = Card([body, controls])
body = TextContent("3 presses")
a0 = Mutation("inc", {})
controls = Buttons([Button("Press", @Run(a0), "secondary")])
```

Errors: raise (or return) `RpcError(code, message, data)` to answer with that JSON-RPC error. Any other
exception answers `-32603` with its message — except in a tool, where it becomes `isError: true` with the
message so the model sees it. The process stays up. `call`/`request` raise `RpcError` on an error answer;
`-32003` (not granted), `-32004` (denied) and `-32005` (rate limited, `data.retry_after_ms`) are normal
answers. Requests nobody handles get `-32601` at once.

### Per language

- **Python**: handlers are decorators or `plugin.action("inc", fn)`; requests and events are dicts with
  attribute access (`req.args`, `ev.data`). `drive()` returns a list.
- **JavaScript**: handlers may be `async`; `status`/`ui`/`notify` take an options object (or positional
  arguments); `drive()` returns a Promise. Types in `theuth-plugin.d.ts`.
- **Go**: `plugin.Action("inc", func(Action) any {…})`; return a `*RpcError` for that error, any other
  `error` (or a panic) for `-32603`. No default arguments: `Log(msg, fields...)`, `LogAt(level, msg, fields...)`,
  `Notify(title, body, level, open)`, `Emit(event, data, session...)`, `Input(text, session, delivery)` (empty =
  unset). `Status{Dot, Badge, Label, Tooltip, Placement}`, `UI{Text, Buttons, Source, Format, Placement}`.
  gofmt makes `theuth.go` the longest SDK file.
- **Rust**: `mod theuth; use theuth::*;`, then `plugin().action("inc", |req| …)`. Handlers return `()`,
  strings, `serde_json::Value` or `Result<_, RpcError>`; panics become `-32603` / tool errors.
- **Ruby**: `plugin.action("inc") { |req| … }`; keyword arguments as in Python (`plugin.status(dot:, badge:)`).
  `Plugin#send` is the protocol `send`; use `__send__` for Ruby's.
- **bash**: handlers are strings run in the current shell; a request handler's stdout is its result (an
  action's JSON, a tool's text, or a tool's whole `CallToolResult` object). `theuth_fail CODE MESSAGE [DATA]`
  answers an error. Output helpers take `key=value` (string) and `key:=json` pairs; `button=Label:action[:style]`.
  Context: `$THEUTH_METHOD`, `$THEUTH_NAME`, `$THEUTH_ARGS`, `$THEUTH_PARAMS`, `$THEUTH_DATA`, `theuth_get .path`.
  `theuth_request` sets `$THEUTH_RESULT` or `$THEUTH_ERROR`; never call it inside `$(…)`. No `drive()`: test
  through `conformance/host.py`.

## yaml wins

The host learns tools, subscriptions, emits, placements and capabilities from `plugin.yaml` before any process
starts. It never runs your code to find out: that is what the install card and the grant protect. Registering
a handler grants nothing.

To compare code and manifest, run the program with `--theuth-manifest`. It prints what the code registered and
exits 0 without reading stdin:

```
$ python3 main.py --theuth-manifest
# registered in code (plugin.yaml is the contract)
tools: ["presses"]
actions: ["inc"]
queries: []
commands: []
on: []
```

`manifest.schema.json` is the JSON Schema of `plugin.yaml` v1; the templates point editors at it with a
`# yaml-language-server: $schema=../../sdk/manifest.schema.json` line (the path `theuth` extracts the SDK to,
`~/.theuth/sdk/`, seen from `~/.theuth/plugins/<name>/`). The templates' `watch:` lists the SDK file so the
code hash (§10.2) covers the SDK the plugin runs.

## Escape hatch

The helpers are not a sandbox and hide nothing. Events and requests carry `raw` (the whole message) and `data`
is exactly what the host sent. `send`, `request`, `on_request` and `on_notify` speak any method, so a host
method added after this SDK works without an SDK release. Or skip the SDK: `templates/raw-py` and
`templates/raw-sh` are complete plugins in plain Python and bash.

Stdout is the protocol. Python and Ruby turn stray prints into `log` lines and JavaScript routes `console.log`
there while `run()` is active; in Go and Rust, `fmt.Println`/`println!` write to the protocol stream — use
`plugin.Log`/`plugin().log` or stderr. A flood of non-JSON lines gets the plugin restarted (§6.1).

## Concurrency

The stdin reader is never the thread that runs your handlers, so `call()`/`request()` from inside a handler
gets its answer (the conformance suite checks this). Handlers run one at a time, so `state` needs no lock. A
thread you start yourself must not touch `state` without its own lock. bash is single-threaded:
`theuth_request` reads ahead for its answer and replays the other lines afterwards; it is not re-entrant.

## Testing

`drive(lines, responder)` runs the dispatcher on a list of host messages and returns the outbound list:

```python
out = plugin.drive([hello, press], responder=lambda method, params: {"now_ms": 1})
assert out[0]["method"] == "status"
```

The conformance suite runs every SDK against one scripted host (`conformance/host.py`):

```
bash sdk/conformance/check.sh            # all languages; or: check.sh py js sh go rb rs raw
```

It stages each fixture (`conformance/plugins/<lang>/`) and template with its SDK file the way `plugin new`
does, and checks: paint after `hello`, a `ping` from `on_start`, action/tool/command/render/query replies,
`-32601`, non-JSON and unknown lines ignored, `lagged`/`cancelled`/`granted`, `request("ping")` from inside an
action (the deadlock test), `call` results and errors, handler exceptions, the raw hatch, `shutdown` → exit 0
with `state.json` written, stdin EOF → exit 0, and `--theuth-manifest`. JavaScript runs under Node, Bun and
Deno; bash under `/bin/bash` 3.2 and a newer bash, each with jq and with python3. A missing runtime is
reported as SKIP. `docs/design/216-examples/check.sh` still checks the raw protocol without any SDK.

## Event catalog

[catalog.md](catalog.md) lists every subscribable name with its version and first description sentence,
generated from `bindings/openrpc.json`. After a wire schema change: `python3 sdk/gen/catalog.py`.
`python3 sdk/gen/catalog.py --check` exits 1 when the committed file is stale. The SDKs are untyped (dicts,
objects, `map[string]any`, `serde_json::Value`); the JSON Schemas live in `bindings/openrpc.json`.

## Protocol notes

Readings chosen where design 216 §6 leaves room, matching `docs/design/216-examples/`:

- `ui` without `source`: `format` defaults to `openui` and the SDK writes the card above (without
  `controls` when there are no buttons). With another `format` and no `source`, `source` is the `text`.
- `render`: the SDK sends whatever `on_render` (else `on_start`) paints, then replies `{}`; a plugin with
  neither answers `-32601`, which §6.2 allows for eager renderers.
- Paint happens after `hello`, not before (§13.1 paints first): the default placement id comes from the grant.
- `notifications/cancelled` does not stop the handler; its reply is still sent and the host discards it.
- A request that arrives before `hello` is answered `-32601`; `hello.protocol != 1` logs an error and exits 1.
- Pending `call`/`request`s fail with `-32006` when stdin closes.
- `log` omits `fields` when empty; `notify` always sends `body` (default `""`) and `level` (default `info`).
- Tool results: a returned value that is not a string or a `CallToolResult` is sent as JSON text, plus
  `structuredContent` when it is an object (MCP requires `structuredContent` to be an object).
