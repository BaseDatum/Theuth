# 46 — Apps: progressive interactive UI on OpenUI Lang

Status: implemented on `feature/apps` · design revised 2026-09-18, as-built
record §12 2026-09-18. §§0–11 are the design as decided, kept as the
reference for the constraints; where the code settled differently, an
**As built** note follows the affected section and §12 records what was
exercised and what was not. This revision replaces this document's Rust
parser, Monty program-registration/replay, second-approval, and no-egress
proposals. User decisions: a button press approves its action; links,
external images and external services are normal app functionality;
progressive rendering is required. Extends `25-desktop.md`, `30-surfaces.md`,
`35-artifacts.md`, `31-link.md`, and the tool/permission contract in
`00-architecture.md`. Uses `04-prompt-skills-mcp.md` for backend integrations
and `37-secrets.md` for credential handling. `28-untrusted-content.md` remains
a separate, unimplemented taint design, not a safety property this app claims.

## 0. Decisions

1. **One app plugin, existing artifact store.** `crates/app` owns publishing,
   app RPC, invocation lifecycle and its prompt. `Kind::App` is a versioned
   `.openui` file. Static `json-render`/Mermaid fences remain pictures in a
   reply; apps are reopenable interactive screens. No second artifact DB.
2. **Use OpenUI's current TypeScript implementation on both sides.** No Rust
   grammar, no Rust evaluator, no resurrection of the old WASM parser.
   The frontend runs the upstream streaming parser/renderer locally. A small,
   persistent headless helper runs the same pinned parser for final validation
   and atomic patch preparation; it never executes app tools or model code.
3. **Monty is the model's working memory, not the app server.** It may help the
   model inspect data and develop an integration; it does not own app state,
   callables, background queries or app restart. Use existing native/MCP tools,
   a generic HTTP tool, or an ordinary model-authored MCP integration (§5).
   Delete `register_tool(fn)`, `.tools.py` cell replay and app-specific program
   approvals. A browser button does not need a model turn to do useful work.
4. **The press is approval.** One click/tap/keyboard activation executes the
   chosen action; no subsequent “approve the thing you just requested” card,
   including destructive actions and secret-bearing calls. Explicit policy
   `Deny` still means disabled, not a confirmation dialog. Local and relay
   clients behave identically. Scope consent to that action, never a session.
5. **Network functionality is intentional.** Ordinary links open directly;
   external HTTP(S) images render normally. Queries/mutations may call external
   APIs through harness tools, including credentialed APIs on the harness host.
   A prototype may use real services; “prototype” does not mean offline.
6. **Progressive rendering is the first-class path.** Open a provisional panel
   on argument progress, before `ArtifactShown`. Render incomplete source with
   OpenUI's incremental parser. Promote the same panel when validation commits
   the artifact. A spinner until the last byte is not acceptance.
7. **One definition of library semantics.** The TS library generates its spec,
   compact prompt and compatibility hash. Both TS consumers use that definition;
   Rust transports the metadata and owns persistence, not component semantics.
8. **Bound the work, not just the source.** Expanded render nodes, evaluation,
   result bytes, pending queries, cancellation and stale responses have limits
   and lifecycle rules (§7). No unbounded “excess queued”.

## 1. Facts and alternatives

### 1.1 Why not their Rust/WASM parser?

OpenUI's [March 13 engineering article](https://www.openui.com/blog/rust-wasm-parser)
is titled **“Rewriting our Rust WASM Parser in TypeScript.”** They replaced the
WASM parser: input/output copies and object conversion cost more than parsing.
Their small-fixture measurements report 2.2–4.6× faster one-shot parsing and
2.6–3.3× lower full-stream cost with incremental TS. These are upstream numbers,
not theuth benchmarks. The relevant lesson is avoiding boundaries on every chunk
and caching complete statements, not “Rust is slow”.

Use `@openuidev/lang-core` **0.3.0**, `@openuidev/react-lang` **0.3.0**, and
`@openuidev/react-ui` **0.16.1** as the initial exact package pins. Language v0.5
is not an npm package version. Verify the lockfile and package contents together;
upstream source reviewed at
[`ac1176a7fcfa52166ba2976b29b7af5e18435a63`](https://github.com/thesysdev/openui/tree/ac1176a7fcfa52166ba2976b29b7af5e18435a63).
Keep the smallest required provenance/work-budget patches in-tree with their
reason and upstream base, per 00 §1.3. Do not fork the whole language.

Set `OPENUI_TELEMETRY_DISABLED=1` and `DO_NOT_TRACK=1` for package installation,
builds and helper execution; do not opt into parser/runtime telemetry.
Rejected:

- A hand-written Rust parser: two grammars, duplicate correction behavior and
  ongoing compatibility work for no user benefit.
- The retired WASM parser: not the maintained v0.5 contract; boundary overhead
  returns precisely on the hot frontend path.
- Shipping only a client parser: headless generation and patch validation must
  work without an open desktop. Client rendering remains local nevertheless.
- Feeding app operations into Monty: adds heap ownership/re-entry, callable
  transport and replay problems unrelated to the runtime's context-management
  purpose (08). The apps do not need that coupling.

### 1.2 Existing theuth contracts this reuses

- Artifact bytes are content-addressed, but **the record key is `call_id`**.
  `Artifact.id` is a blob hash, not a session or execution identity
  (`artifact/src/lib.rs`, `migrations/0001_artifacts.sql`).
- Artifact version allocation is atomic inside insertion, but read–merge–write
  is not already an atomic operation (`artifact/src/store.rs`).
- `ToolRegistry` resolves native/MCP tools globally with session visibility
  filtering. It is not a session-local callable registry. `RegisterTool` replaces
  by name; this design introduces no app registration into that map.
- The existing gate sees risk, secret mentions and approval switches, **not**
  user activation. §6 is an explicit tools-owned extension, not “unchanged Invoke”.
- `ToolCallProgress` carries cumulative partial JSON at ≥250 ms and ≥512 new
  bytes. The current desktop keeps only progress metadata. 35's as-built note
  records buffered Anthropic argument streaming without its fine-grained option.
- The desktop and relay page CSPs previously excluded external images; allowing
  `https: http:` in their `img-src`, and in the HTML artifact image policy, is
  intentional. Scripts and unrelated permissions are not opened by that change.

### 1.3 Actual OpenUI interfaces

The [core exports](https://github.com/thesysdev/openui/blob/ac1176a7fcfa52166ba2976b29b7af5e18435a63/packages/lang-core/src/index.ts)
include:

```ts
const parser = createParser(librarySpec.schema);
const result = parser.parse(source);
const stream = createStreamingParser(librarySpec.schema);
stream.push(delta); // newly received suffix
stream.set(cumulative); // cumulative input; owns incremental caching
const merged = mergeStatements(base, patch, "root");
```

`mergeStatements` replaces by identifier, treats `name = null` as deletion,
and removes unreachable non-state statements. State declarations are retained.
Validate the complete merged source; a patch alone need not contain `root`.

The [React export](https://github.com/thesysdev/openui/blob/ac1176a7fcfa52166ba2976b29b7af5e18435a63/packages/react-lang/src/exports.ts)
is `Renderer`, not `OpenUIRenderer`:

```tsx
<Renderer
  response={source}
  library={library}
  isStreaming={phase !== "committed"}
  toolProvider={provider}
  onAction={onAction}
/>
```

It owns its store; no `source` or `store` prop is assumed. Its streaming mode
already suppresses query submission/mutation registration and disables buttons.
The host also refuses invocation from uncommitted previews.

`McpClientLike.callTool({ name, arguments })` expects MCP output:
`{ content: [{ type: "text", text }], structuredContent?, isError? }`.
Convert `AppToolResult` explicitly, or use the supported function-map provider;
never pass the differently named RPC envelope through unchanged. The unpatched
provider receives **no statement ID or trigger**. §6 defines the small required
provenance extension rather than inferring a press from a tool name.

Other details that affect the design:

- Component calls can be nested expressions; general expression assignments,
  forward references, state, queries and mutations are upstream syntax.
- Arguments follow schema key order. `Card` takes children first, not a title.
  `TextContent(text)` is the normal markdown-capable text component; `Text`
  has a different signature. Use generated signatures, not guessed positions.
- Mutation results have `status`, `data`, `error`; not `.value`.
- `@Run(mutation)` awaits and stops the action on failure. `@Run(query)` starts
  a refetch and continues; it does not await success. Put dependent server work
  in one backend tool instead of pretending query refresh is a transaction.
- `@ToAssistant` accepts expressions and optional context. Default button
  actions can continue the conversation with their label/form values. Support
  these intentionally; they are not literal-only or tool-output-free.
- Unknown components may return `null`, including the root. The **host** provides
  an incompatible-version/source view; upstream does not promise a labelled node.

## 2. Ownership and deployment

```text
model --app(source/patch)--> app plugin --> artifact store
                              |
                              +--> headless TS helper: parse/validate/merge only

ToolCallProgress -----------------------> provisional desktop panel
ArtifactShown --------------------------> committed OpenUI Renderer
                                                  |
                                  Query / pressed action
                                                  |
                                       app RPC --> ToolRegistry
                                                      |
                                               native / MCP / HTTP
```

`crates/app` depends on `artifact`, `tools`, `runtime`, and agent-owned event/input
contracts where needed. Neither `agent` nor `artifact` imports `app`. The app
plugin subscribes to agent events; RPC imports app-owned command/event types.
No handle threading through `main`, no new runtime API, no central event enum.

### 2.1 Headless parser helper

One lazy, long-lived `theuth-app-parser` executable per harness, built with
`bun build --compile` from the same TS adapter/core version used by the desktop.
It bundles the generated library JSON schema, not React or the component UI.
JSON-lines over stdio: request ID, operation (`check` or `merge_check`), source,
optional base, and a snapshot of visible tool names/schemas/risks. Response:
canonical merged source when applicable, diagnostics, statement/tool metadata,
and compatibility hash. **No full render tree crosses this boundary.**

The helper is trusted, harness-owned parser code. It has no callback that runs
app programs or tools. It does not import the user's Python cells or execute
source as JavaScript. One bounded command queue; compile schema once at startup;
final document validation only, not a subprocess call for every streaming token.
The Rust actor services unrelated RPC while a parser request is pending.

This is a deliberate exception to 00's in-process preference: maintaining one
upstream parser is cheaper than a second grammar, and the boundary is cold,
one final source in / compact diagnostics out. The hot streaming path stays
entirely inside the browser's JS heap. A generic JS plugin runtime is not built.

Packaging is part of this feature: ship the signed helper beside the harness
on macOS and Linux, include it in release/update manifests, and bundle it with
the desktop sidecar. The iOS client needs no helper. No runtime npm install,
Node/Bun prerequisite, or network download on first app use. Missing helper →
a clear app-unavailable error; ordinary harness features keep working. Stop it
on shutdown; crash/2 s validation deadline kills and replaces the helper on
next request, with the current request failed and nothing committed. Start
lazily, so sessions without apps pay neither process nor parser startup cost.

**As built.** The executable is `theuth-app-parser-<library-hash>` beside the
`theuth` binary (`THEUTH_APP_PARSER` overrides the path for tests), built by
`packages/apps/scripts/build-helper.ts` with `bun build --compile --minify`
from `src/helper.ts`; the build refuses a bundle that pulls in React,
`react-ui`, `react-lang` or zustand. `app::helper::Helper` spawns it on the
first request, runs `--version` first and refuses a helper whose printed hash
is not this binary's `app::HASH`, then speaks JSON lines (`protocol.ts`:
`{id, op: check|merge_check, source, base?, tools[]}` →
`{id, source, library_hash, statements, bindings, diagnostics, state,
orphaned}`) with `OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1`. Constants:
`PENDING = 8` queued requests (beyond → busy error), `DEADLINE = 2 s` per
request, `MAX_LINE = 4 MiB`; a timeout, malformed reply or crash kills the
child and the next request spawns a replacement. `t --app-parser-hash`
prints the expected hash for packaging and the updater (39).

## 3. Publishing, identity and edits

### 3.1 Tool contract

```rust
// crates/app; wire types derive serde/schemars/ts-rs as usual.
pub struct AppArgs {
    pub name: String,
    pub source: String,
    pub title: Option<String>,
    pub edit: bool,                 // serde(default), false = complete source
    pub base_call_id: Option<String>, // required for edit; receipt supplies it
}
```

`app`: `Risk::ReadOnly`, `DIRECT = true`. It publishes UI under harness-owned
storage; it does not run the UI's tools. Direct exposure is necessary for
progressive tool-argument rendering with the default runtime-only tool list.

For a new app: enforce input limits → helper checks complete source + current
visible tools → artifact actor commits bytes/metadata → `ArtifactShown` → receipt.
Errors are `ToolOutput::err` with located upstream diagnostics, the statement
and relevant signature. A normal next tool call corrects them; the existing
`max_tool_rounds` bounds this, not an invented one-retry guarantee. If initial
publication failed, there is no base record: retry full source.

Receipt: name, version, **record `call_id`**, statement/query/mutation counts,
referenced tools and risks. Do not echo source or live query output into context.

### 3.2 Record versus blob

Every executing app is keyed by `(harness, record_call_id)`. The artifact row
supplies session, name and version; client-supplied session never overrides it.
`Artifact.id` remains the hash for deduplicating `.openui` bytes. Identical source
in different sessions must have different execution contexts even with one blob.

Store a content-addressed `.app.json` metadata file in `Artifact.source`, using
that file's **own hash**, not the OpenUI hash. It contains the language/package/
library compatibility hash, declared query/mutation bindings, and tool identity
fingerprints (native identity or MCP server identity, tool name and schema).
For a model-authored MCP integration, include its entry point and exact source
snapshot in this metadata for inspection, subject to the source cap; it is not
loaded as executable app state. Registry resolution uses the installed MCP
integration, not an arbitrary path from an artifact. No `.tools.py` sidecar.

The artifact actor owns writing the source metadata and row atomically enough
that `ArtifactShown` can never reference missing bytes. Orphans after interruption
are collected by the existing source-aware sweep. No app SQL connection writes
around the artifact owner. Add a typed artifact commit/get command supporting
`Kind::App` and opaque source bytes; do not make artifact depend on app types.

An artifact versions its UI/declared backend contract, **not the remote world**.
On open/invoke, a missing/changed tool identity or schema is visibly unavailable,
not silently rebound to a different same-named server. Updating an integration
requires revalidating/publishing the app contract; old cards do not silently
adopt it. Built-in MCP source pinning from 28 is still unimplemented; these app
binding checks do not claim to sandbox or freeze remote service code.

### 3.3 Atomic edits and live adoption

`edit:true` requires the receipt's `base_call_id`. The app actor reads that record
in the same session/name, asks the helper to merge with upstream semantics, then
requests artifact commit **only if that base is still latest**. The compare and
version insert happen in one artifact transaction. A loser returns a conflict
with the latest record ID; it does not publish an overwrite or silently rebase.
The base must be a committed compatible app, not a pending preview.

This is more than the existing atomic version counter. Two concurrent patches
against v1 cannot both succeed as v2/v3 while dropping one another's changes.
Full replacement is intentional and uses the ordinary version insertion path.

The open panel has an explicit generation. In “latest” mode a successful patch
adopts a new generation; a user viewing an older card/version stays pinned.
Pause query scheduling during preview/adoption. Retain user-edited state only
for unchanged state declarations with compatible bindings; changed/deleted
state resets/removes. Query/mutation result stores are reset, old query results
cannot update the new generation, and actions are resolved against the generation
visible when activated. Defer adoption while a pressed mutation is in flight;
show that it is finishing rather than replacing the controls beneath the user.
No persisted user-state store is introduced.

## 4. Library, validation and examples

Start with the upstream `openuiLibrary` definition, then ship a curated dependency-
closed subset: `Stack`, `Card`, `CardHeader`, `TextContent`, `Button`, `Buttons`,
`Input`, `Select`, `SelectItem`, `Switch`, `TextArea`, `Image`, `Separator`,
`Table` and its child schemas, and selected charts. The actual exported schema
names, not this prose, decide spellings and required child types. Only include
theuth-specific display components when an existing upstream component cannot
express a real screen. There is no requirement to align names with surface's TUI
catalog; that requirement was driving duplicate evaluator work.

One TS library export produces:

- Upstream `toSpec()` (`root`, `components` signatures/descriptions,
  `componentGroups`, JSON `schema`) and compact named signatures.
- Canonical compatibility hash covering package pins, patches and the selected
  component schemas/order, including bindability semantics.
- Any additional machine-readable binding metadata from those same definitions;
  JSON Schema alone does not preserve OpenUI's WeakSet reactive markers.

Rust embeds/transports the generated prompt/hash as opaque data. The helper and
renderer consume the spec in TS, where positional property order is preserved.
No second hand-maintained Rust catalog or type checker. Upstream parsing is
permissive for streaming: final `check` additionally requires `root`, no unresolved
references/cycles/errors, supported action targets, valid component args and
visible tool bindings; it uses upstream AST/schema utilities. Dynamic argument
values are schema-checked again immediately before invocation. Same-typed swapped
props remain a model-quality issue, measured with actual renders, not “fixed” by
more parser rules.

**As built.** `packages/apps` (`@theuth/apps`): `library.ts` exports 21
dependency-closed upstream components — `Stack` (root), `Card`, `CardHeader`,
`Separator`, `TextContent`, `Image`, `Table`, `Col`, `BarChart`, `LineChart`,
`AreaChart`, `PieChart`, `Series`, `Input`, `TextArea`, `Select`, `SelectItem`,
`SwitchGroup`, `SwitchItem`, `Button`, `Buttons` — with pins `lang-core`/
`react-lang` 0.3.0 (both patched in-tree under `patches/`) and `react-ui`/
`react-headless` 0.16.1. `bun run generate` writes `generated/library.json`,
`catalog.txt` (the prompt/skill catalog) and `library-hash.txt`; the hash is
SHA-256 over the pins, each patch file's digest, the root, the ordered
component schemas, the reactive markers and the §7 limits, so any of them
changing invalidates every committed app's `library_hash`. `check.ts` is the
final validator (`root`, strict reparse, duplicates, unresolved/cyclic
references, `$state` declarations, action step shapes, string-literal tool
names present in the snapshot, Query ≤4 / Mutation ≤2 args, required keys and
`additionalProperties: false` of the tool schema); full argument-value schema
validation is the registry's job at `AppInvoke`, not the helper's. Diagnostic
codes: `protocol.ts` `DiagnosticCode`.

### 4.1 Prototype: real state, ordinary link, external image

```openui
root = Stack([title, status, picture, controls])
$stage = "idle"
title = TextContent("## DR cutover drill")
status = TextContent("Stage: " + $stage)
picture = TextContent("![Architecture](https://www.rust-lang.org/logos/rust-logo-128x128.png)")
controls = Stack([begin, reset, docs], "row")
begin = Button("Begin drill", Action([@Set($stage, "cutover")]))
reset = Button("Reset", Action([@Reset($stage)]))
docs = Button("Open runbook", Action([@OpenUrl("https://argo-cd.readthedocs.io/")]))
```

This particular prototype has no server calls; that is not a restriction on
prototypes. The image loads from the web and the runbook opens in one activation.
A real-service prototype simply adds a Query/Mutation, as below.

### 4.2 Live ArgoCD panel

`argo_app_health` and `argo_sync` are installed tools with explicit input schemas;
§5 explains providing them when no existing integration fits. No Monty registration.

```openui
root = Stack([title, pick, healthText, controls, resultText])
$app = "portal-web"
title = TextContent("## ArgoCD health")
pick = Select("app", [SelectItem("portal-web", "Portal web"), SelectItem("portal-api", "Portal API")], "Choose application", null, $app)
health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 30)
healthText = TextContent($app + ": " + health.status + (health.synced ? " — synced" : " — not synced"))
syncResult = Mutation("argo_sync", {name: $app})
controls = Stack([syncBtn, explain], "row")
syncBtn = Button("Sync " + $app, Action([@Run(syncResult), @Run(health)]))
explain = Button("Explain health", Action([@ToAssistant("Explain this application's health", health.status)]))
resultText = TextContent(syncResult.status == "success" ? syncResult.data.message : (syncResult.status == "error" ? syncResult.error : ""))
```

Load/30-second refresh uses the read tool without a model turn. Selection change
refetches and rejects stale results. **Sync** approves and runs the selected
mutation once, then starts a health refresh: no second modal. **Explain health**
sends the displayed context to the agent intentionally; it is app-originated
input, not proof that the tool text was written by the human.

## 5. External services and backend code

Use the first adequate rung, independent of whether the UI is called a prototype:

1. State/literals when no service is needed.
2. An existing native, site-route or MCP tool. OpenUI builtins can reshape data
   and combine displays; multiple read queries do not require a backend wrapper.
3. `http_request` for a direct REST endpoint. It belongs to **`crates/web`**, not
   app or Monty, reusing the existing fetch/URL/redirect policy and secrets hook.
   Contract: `{url, method, headers?, body?}` → structured `{status, headers, body}`;
   bounded JSON/text responses, 1 MiB max and 30 s timeout, no implicit retries of
   writes. Fixed `Risk::Mutating` is conservative for the generic verb-capable
   tool; use it behind an action. For unattended polling, use an appropriately
   classified read tool/integration rather than declaring every HTTP method safe.
   Secret headers use placeholders; deny unsupported schemes and credential-
   forwarding redirects with existing web policy. No browser CORS workaround
   or client-side copy of harness credentials. This tool ships with apps if
   not already present; it is useful independently of them.
   **As built:** `crates/web/src/http.rs` (`HttpRequestTool`, `Risk::Mutating`),
   registered by `WebPlugin`; new on this branch.
4. An **ordinary MCP integration** for joins, multi-step service transactions,
   custom logic or reusable APIs. The model writes source under `mcp/repos/`,
   exercises it using the existing shell/tools, and installs via `add_mcp_server`
   with `command/args/cwd` (04 §3). Use the existing SDK appropriate to its language,
   not a new theuth program format. Give custom tools collision-resistant names;
   verify the installed tool belongs to the intended server before binding it.
5. `@ToAssistant` for work that genuinely needs the model rather than deterministic
   application code. It uses the existing input/steering path, not a second agent.

An ArgoCD integration can expose `argo_app_health({name})` returning
`{status, synced, degraded, images}`, and `argo_sync({name})` returning `{message}`.
It owns its service clients, error handling, concurrency and startup just like
any MCP server. Running its installed executable on restart is the existing MCP
installation contract, **not replaying development cells**. Self-tests/smoke calls
are separate from server startup. Sessions can close, compact, or run a model
turn without affecting an open app's query execution.

Important boundary: theuth gates the **MCP tool invocation**. The server's internal
HTTP/library calls do not individually re-enter theuth's registry; claiming they do
would be false. Its tool contract must describe the action being approved.
Installing model-authored server code already goes through the existing
`add_mcp_server` risk/approval path, not a new “dynamic tools” dialog every session.
There is no per-session callable export, registry overwrite/reload mechanism,
source extraction from a heap, or second program runtime in this feature.

### 5.1 Secrets through composition

Client state/source/metadata contains placeholders, never expanded credentials.
The parser helper does not expand them. App-side orchestration and any
harness-authored composite adapter preserve placeholders until the final
native/MCP call. That call performs gate → expansion → capability → output scrub.
No expanded value enters Monty or is used to build a later `ToolCallStarted`.

If a backend needs a secret to talk directly to its service, the MCP invocation
is the terminal credential boundary: only the server receives it. Prefer one
service-specific credentialed operation over passing raw tokens between tools.
A backend that calls back into theuth must forward placeholders, not expanded
values, and must not receive a reusable blanket app consent token.

Use 37's scrubbing for content/structured results and event/log displays. It is
accidental-disclosure protection, not DLP or a proof that transformed/short secret
values can never appear. The acceptance case checks a nested adapter's events,
args and result, not merely the final RPC response.

## 6. One activation, one approved action

### 6.1 Tools-owned consent extension

Extend the internal invocation context with an optional user-activation record,
constructed by the app bridge after validating the committed artifact binding.
It identifies the app record, action execution and concrete invocation. Default
for every existing caller is absent. No mutable approval bypass on the session,
no change to model-call defaults, no `press_mutating` option.

The gate resolves policy first. `Deny` is final. When a valid user activation
covers this call, `Ask` is already answered **Allow** by that activation, including
secret use, mutating and destructive risk. No `ApprovalRequested` is emitted.
This is the same approval contract with evidence that the user already acted,
not a separate app permission system. Risk remains visible in the button/action
presentation; controls describe their consequence rather than opening a generic
confirmation modal after activation. Future taint integrates with this explicit
approval evidence; it must not turn one intentional press into two prompts.

Automatic Query loads/refreshes have no activation. Effective `Allow` executes;
`Deny` returns unavailable; `Ask` becomes an inline “Load”/“Refresh” affordance
whose activation is the approval, not a repeating modal from a polling timer.
Do not silently inherit the last press on future refresh ticks or model turns.
This accommodates secret-bearing reads without approval storms. A timer never
runs a Mutation. A query is not classified read-only merely by its syntax.

### 6.2 Provenance from renderer to bridge

Patch OpenUI's execution boundary narrowly to forward `{statement_id, trigger,
action_id?}` with each provider call. Triggers: `load`, `refresh`, `state_change`,
`press`. The action executor creates the action ID on real pointer/keyboard
activation; it passes that context explicitly through sequential `@Run` calls.
The query manager retains it for an explicit pressed refetch only, not subsequent
ticks. No global “currently clicking” flag; concurrent queries remain automatic.
The same patch exposes start/end of an action so the adapter can scope/clear it.
Use the existing parser's statement metadata; do not guess from tool name/args.

The ordinary unpatched `McpClientLike` interface is insufficient. This patch is
an early adoption check, alongside bounded evaluation, and its declarations are
shared by the frontend adapter. It changes neither the language nor component
syntax. Authenticated owner clients are the same trust boundary as today's
`send_input`/approval RPC; the renderer's expressions cannot call arbitrary RPC.

### 6.3 RPC and commands

All methods are lookup + app-actor ask; types belong to app:

```text
get_app(record_call_id)
  -> {artifact, source, library_hash, bindings, diagnostics}
app_call_tool({record_call_id, instance_id, generation, statement_id,
               request_id, args, trigger, action_id?})
  -> {request_id, content, structured?, is_error, status}
app_call_status(request_id)
  -> {status: running|succeeded|failed|unknown, result?}
app_close(instance_id)
  -> ()
```

`instance_id` distinguishes two open views; `generation` distinguishes versions
within one view. Both are scoped to the authenticated connection and app record,
not substitutes for artifact ownership. The server derives the tool name/session
from the pinned statement binding, checks current visibility/identity/schema,
checks argument limits/schema, admits under §7, constructs consent context if
pressed, then invokes the real registry. No client-chosen arbitrary tool name.
Inputs driving an action are captured at activation, not read again after a
network wait. Inspect integration source through the metadata in `get_app`; the
old `get_app_program`/Monty lookup is unnecessary.

**As built.** Exactly these four methods exist (`rpc::Api`): `get_app`,
`app_call_tool`, `app_call_status`, `app_close`. The three `app_*` methods are
`with_extensions` and take the authenticated `link::RpcPeer` the listener put
in the request (local socket or link conn); no peer → `-32002`, never "local".
`get_app` needs only the ordinary connection auth. `Info.app_enabled` tells a
client whether the plugin is running; a disabled plugin answers `-32001` and
the client shows the artifact's source. `AppDoc` adds `state` (declared
`$state` defaults) and `recent_calls` (≤64 durable pressed invocations of the
record, newest first, from `artifact::ListInvocations`) so a reopened or
reconnected view shows what its actions did; `diagnostics` gains live
`tool-unknown`/`tool-identity`/`tool-denied` entries per binding. Views are
bound `(instance_id → peer, record, generation)`, ≤64 per connection; a
foreign peer is refused. The registry's `AppInvoke` re-checks visibility,
the pinned identity and the arguments against the current schema before the
gate. `app_call_status` is scoped to a live authenticated connection of the
harness, not to the view that made the request.

`@ToAssistant`/implicit conversation actions use existing `send_input`, with
app record/action provenance and the chosen message/context/form values shown
in the transcript. Treat attached service text as app data, not system guidance.
This context is allowed; no literal-only restriction or warning dialog. Following
model tool calls are a **new** intent and do not inherit the button's approval.

### 6.4 Completion, duplicate requests and reconnect

A request ID is stable across transport retries for one invocation; a new
intentional action gets a new ID. Persist pressed invocation intent/result metadata
through artifact-owned commands into an `app_invocations` table in the **existing**
`artifacts.sqlite`, keyed by request ID and bound to record/statement/args hash.
Same ID + same input returns running/stored result; same ID + different input is
an error. Insert the running record before dispatch. A crash with a running
record becomes `unknown`, not an automatic retry. This cannot guarantee exactly
once at an arbitrary remote API; pass the ID as an idempotency key when supported.
Otherwise report unknown outcome and let the user inspect before repeating.

Only pressed invocations are durable; polling updates use bounded current status,
not an append-only audit flood. Keep durable result/argument displays redacted and
bounded; purge with the owning session. App-owned `AppCallChanged` carries session,
record/request IDs, tool/risk, trigger, status and elapsed time. It is not a fake
model turn. Reconnect reads status from RPC; missing a bus event does not make a
completed action unknowable. Closing a panel stops reads, not an already accepted
external mutation; its status remains discoverable.

**As built.** `artifacts.sqlite` migrations `0002_app_invocations` (table
keyed by `request_id`, bound to session/record/statement/tool/`args_hash`,
status, result, timestamps) and `0003_app_invocation_owner` (`owner TEXT`).
Each store opener holds one exclusively locked file in
`<data_dir>/artifacts.owners/` and stamps its name on the rows it claims; on
open, `running` rows whose owner file is gone or lockable become `unknown`
(the kernel drops the lock with the process), rows of a live owner are left
alone, nothing is re-dispatched. Stored results are capped at 64 KiB
(`{"omitted": …}` beyond); `ListInvocations` clamps to 64. Duplicate identity
compares session, record, statement, tool and canonical args hash — not
`action_id`. `SessionDeleted` deletes the session's artifacts and invocation
rows in one transaction. `AppCallChanged` is published once, when an admitted
call finishes (detached reads report `cancelled`); there is no `running` event.

## 7. Work bounds and lifecycle

Initial defaults, subject to the acceptance measurements, not performance claims:

| Boundary             | Limit / rule                                                                                                                                                                                             |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Source / metadata    | 128 KiB source; 512 statements; depth 32; 256 KiB metadata                                                                                                                                               |
| Parser admission     | 8 pending final validations; helper timeout 2 s; backpressure error beyond it                                                                                                                            |
| Materialization      | 2,048 expanded component nodes per parse and 65,536 AST visits (`materializedNodes`); reference occurrences counted, not declarations                                                                    |
| Dynamic evaluation   | 50,000 operations and 2,048 produced nodes per budget; nested `@Each` shares it. As built a budget is per render pass, per action plan and per evaluated query argument, not one allowance per UI update |
| Tool results         | 1 MiB serialized response before client forwarding; no silent JSON truncation                                                                                                                            |
| Query scheduling     | 5 s minimum interval (as built: enforced by the patched renderer, not the harness); ≤8 active calls and ≤32 pending queries per app record across its views                                              |
| Harness total        | ≤32 active app calls; queued reads bounded globally at 128; presses return busy rather than wait indefinitely                                                                                            |
| Input-driven refresh | coalesce changed args for 150 ms; one running + one latest desired value per query/view                                                                                                                  |
| Hidden/closed views  | stop polling; discard queued reads; ignore late results from obsolete generation                                                                                                                         |
| Transport loss       | stop scheduling; reconnect reads once; never automatically replay a mutation                                                                                                                             |

Source/depth caps do not imply an expanded-work cap. Thread budgets through the
upstream materializer/evaluator **before** allocating arrays or recursively
expanding shared references; checking the finished tree is too late. Apply the
same patch in the helper and renderer, including partial streaming. Return a
located “app exceeds render budget” diagnostic and leave the last good frame.
No CSS-hidden infinite DOM tree or billion-node rejection check after allocation.

Drop superseded refreshes, not FIFO-queue them. An active query is tagged with
its args/generation; cancellation discards its result even if the underlying tool
cannot cancel. Close/disconnect cleans instance state and queued work. Repeated
close/open and multi-client viewing must not accumulate timers, listeners or
providers. Active native/MCP tool lifetime follows its existing cancellation/
timeout contract; app timeouts do not claim to roll back side effects. No
unlimited replacement dispatch while a supposedly cancelled tool still runs.

Panel work does not re-render transcript rows. Coalesce progressive source once
per animation frame; maintain the upstream streaming parser for that preview.
Lazy-load app UI/code/CSS on first preview/open. Hidden sessions retain progress
text only, not live app trees. Pin component imports/child-schema closure and
measure the bundle rather than pulling every chart into initial chat load.

Performance acceptance: on the desktop, a 100-statement progressive app under
25's 50-session background stream remains interactive (p95 input/frame work
≤16 ms; no >50 ms long task attributable to apps during a 60 s trace). On the
phone, record the same trace and retained heap after 50 open/close cycles; no
monotonic per-cycle retained growth. First meaningful frame arrives before the
final tool arguments and within 250 ms of receiving a renderable partial source.
Record helper cold start/RSS, warm validation latency and release size on both
supported harness platforms. A failed budget changes the implementation before
release, not these numbers after the fact.

## 8. Progressive UI and versions

### 8.1 Preview → commit, without a blank minute

The app plugin's progress consumer recognizes pending direct `app` calls by
`ToolCallPending`. On `ToolCallProgress`, the visible frontend repairs only the
partial **argument JSON envelope**, extracts source/name/edit/base, and feeds
OpenUI the source accumulated so far. Never repair final source into different
semantics. Open the provisional panel keyed by `(session, tool_call_id)` on its
first renderable chunk, subject to 35's visible/focused/one-auto-open-per-turn
rule. Manual close suppresses reopening for that turn.

For `edit:true`, keep the committed base frame. Use upstream token/split
utilities to separate complete patch statements from the pending tail; only
the complete prefix participates in a provisional merge. Never pass a partial
parent replacement into merge's deletion/reachability collection. Render the
pending tail separately as source/progress until it forms a complete statement.
This still updates layout progressively by statement, without transiently
deleting the old screen. No draft effects activate before commit. Do not create
a new renderer on every chunk; app RPC independently rejects pending record IDs.

During preview, layout/text/images render progressively. Actions and service
queries wait for validated commit; the purpose is seeing the screen take shape,
not running incomplete instructions. `ArtifactShown` promotes the same view to
committed, with its validated metadata and hash, and enables interaction. Failure
or cancellation retains the last committed app when editing, or an error/source
preview for a new app; no failed preview becomes a historical artifact.

Providers that support fine-grained tool arguments must have their supported
streaming option enabled through the existing backend configuration. Specifically
verify Anthropic's option and OpenAI-compatible streaming; do not assume today's
progress event fixes provider buffering. A provider incapable of argument
streaming may still produce an app with honest progress text, but is explicitly
not evidence that progressive rendering passed. Do not silently make all apps
wait for commit as a fallback implementation.

**As built.** Only the `app` tool definition opts into Anthropic's eager
input streaming (`agent::loop_::direct_tool`, `with_eager_input_streaming`);
other tools are unchanged. The desktop turns `ToolCallProgress` of an `app`
call into a draft (`store/reduce.ts`), repairs only the argument envelope,
auto-opens the panel once per focused turn on the first renderable chunk, and
`ArtifactShown` replaces the draft with the record. `AppHost` merges only
complete patch statements over the committed base while editing and shows the
unfinished tail as text; a failed new draft stays a readable source card.
Live progressive rendering through a real streaming provider was observed in
the desktop store tests and the fixture-model smoke, not on a captured
provider trace (§12).

### 8.2 Desktop, TUI, channels and remote clients

- Desktop/phone: artifact card + shared panel/explorer app viewer. Never mount
  live queries inside a virtualized transcript row. Risk/consequence is visible
  on actions; native controls provide keyboard focus, labels and touch targets.
- Links: `@OpenUrl` and Markdown links use existing HTTP(S) opener/navigation
  behavior immediately. No app consent modal, no model round-trip. Keep platform
  scheme validation; an ordinary URL is not executable script.
- Images: HTTP(S), data/blob and existing local image sources are allowed by
  desktop/relay CSP as appropriate. External hosts can observe image requests;
  that is intentional functionality, not a promised network-less sandbox.
  HTML artifacts keep their inert script/form sandbox while allowing web images.
- External API work: use harness RPC/tools so phone clients reach the same service
  environment and credentials. This is not an image/link proxy or hostname
  allowlist. Existing tool/network policies continue to apply to that host.
- TUI: app receipt, `/apps` listing via artifact records, Ctrl+O readable source,
  `o` OS open, and a clear “interactive on desktop/web” indication. No custom
  Rust initial-frame evaluator. Static surface fences remain available when a
  terminal picture is the right output. A richer text preview must use the
  same upstream evaluator if later justified, not a second language implementation.
- Slack/Signal: receipt plus source file, “open on desktop/web”; ACP: existing
  artifact ResourceLink. No pretend clickable controls in text-only channels.
- Remote identity: scope record/instance/query state by harness ID. Tool execution
  remains on the paired harness. Relay transport does not reduce a press's approval.

**As built.** TUI: the artifact card shows `interactive on desktop/web`,
Ctrl+O expands the `.openui` source (200-line cap), `o` opens the source with
the OS opener; `/apps` lists the session's `Kind::App` records (⏎ open, `s`
source, `e` export) and is offered only while `Info.app_enabled`. Slack and
Signal attach the source file with `Interactive app — open on desktop/web`.
ACP emits the ordinary `ArtifactShown` `ResourceLink` to the `.openui` file.
Desktop: `AppView` lazy-loads `AppHost` (and the OpenUI runtime/CSS) on first
preview/open; there is no separately named Vite chunk. A library-hash mismatch,
a disabled plugin or a failed `get_app` renders the source, never a
best-effort app. Phone/iOS client work was not part of this branch.

### 8.3 Compatibility and failures

`get_app` returns the compatibility hash. Before executing, the client checks
its parser/library/patch contract. A mismatch produces a host-owned readable
source/receipt panel with update guidance, not a best-effort active app with
possibly wrong positional argument semantics. Missing root/parent components
must not produce a blank sheet. Old clients that do not understand App still
have artifact metadata/download; method-not-found yields source fallback.

Runtime query errors remain visible with last successful data explicitly marked
stale and a retry action. Defaults are labelled loading/example data, never a
successful live read. A mutation failure shows its error and stops subsequent
mutation action steps. Render failures preserve last-good content with a visible
diagnostic; final validation cannot guarantee that arbitrary future data renders.

## 9. Prompt, config and on-disk state

Static `RegisterPrompt { plugin: "app", text }`, cache-safe and library-generated:

> Use `app {name, source}` for a clickable prototype or a live interactive screen.
> It renders progressively on desktop/web. Use surface fences for a static picture
> and `show` for other files. Write layout first, then referenced components/data;
> use the supplied named positional signatures. A button press approves its action:
> make the consequence clear, do not ask for a second confirmation. Links/images
> and external services are welcome. Query reads; Mutation runs on activation.
> Prefer installed tools, then HTTP or a normal MCP integration; Python is working
> memory, not an app backend. Iterate with `edit:true` and the receipt's
> `base_call_id`. Tool errors name what to fix; no need to re-emit unchanged source.

A generated `apps` skill holds complete signatures, actual examples and backend
instructions. Upstream prompt flags: `toolCalls`/`bindings` on, `inlineMode` off;
patch guidance in the skill/tool docs. Do not mutate the system prompt mid-turn
for an edit flag. Tool results stay outside the model context unless an app
conversation action deliberately sends them. Such input includes provenance and
ordinary app data; never promote it to system instructions or claim injection
is impossible merely because the human clicked.

```yaml
app:
  enabled: true
```

Disabled: no publishing tool, prompt, parser helper, app tool dispatch or active
renderer; existing artifacts remain downloadable/readable as source. In-flight
accepted mutations finish under their existing contract; no new work starts.
No `dynamic_tools`, `max_programs` or `press_mutating` configuration. Operational
limits above are named constants first, not a settings page.

```text
<data_dir>/
  artifacts/<source-hash>.openui
  artifacts/<metadata-hash>.app.json   # Artifact.source, separate content hash
  artifacts.sqlite                     # existing rows + 0002_app_invocations + 0003 owner column
  artifacts.owners/<opener>            # one exclusively locked file per store opener (§6.4)
  mcp/<integration>.json               # existing MCP installation ownership
  mcp/repos/<integration>/             # ordinary source/build, not replay cells
  skills/apps.md                       # rewritten on start from crates/app/apps.md + generated catalog
```

Beside the binary: `theuth-app-parser-<library-hash>` (§2.1). Sidecar contents
(`app::meta::AppMeta`): library hash, statements, pinned bindings with tool
identities, publish diagnostics, state defaults, orphaned names, the bound
`ToolInspection`s and inspection-only entry-point snapshots of bound stdio
MCP servers under `mcp/repos` (32 KiB each, 96 KiB per app; `node_modules`,
`dist`, `build`, `target`, `.git`, `.venv` skipped). Metadata is capped at
256 KiB: snapshot text is dropped with a note first, then the publish fails.

The helper ships beside the binary, not in writable artifact state. Artifact
session deletion removes invocation rows; existing blob/source sweep handles
unreferenced app bytes. Query data and `$state` are view-local and not persisted.
No background app keeps an agent session checkout alive.

## 10. Implementation order and falsifiers

1. **Upstream feasibility, before app framework work.** Pin actual package
   versions; exercise §4's exact examples, nested components, state/actions,
   mutation `.data`, parser errors and merge deletion/reachability. Build the
   core-only helper executable and run its check/merge protocol on macOS/Linux.
   Verify no React/browser module or npm install is needed on the harness.
2. **Renderer adapter and bounds spike.** Mount real `Renderer` in our React
   desktop with the provenance/work-budget patches. Feed partial arguments
   through the actual progress path. Show content before completion, preserve
   state while streaming, and prove zero tool calls before commit. Repeated-ref
   and nested-Each fixtures must fail within work budgets before large allocation.
3. **Artifact publication and edit transaction.** Add `Kind::App`, opaque metadata
   source and app-invocation persistence via artifact-owned commands. Headless:
   publish, reload, same bytes in two sessions, conflicting simultaneous patches,
   failed initial publish/retry, source-metadata sweep, historical version reopen.
4. **Real tool bridge.** One press through an authenticated client/relay produces
   one mutation and **zero approval dialogs**; cover destructive risk and a
   placeholder-bearing call. Explicit Deny executes nothing. Timer/state refetch
   never inherits consent. Concurrent model calls retain their policy. Duplicate
   request IDs do not duplicate effects; lost response/crash shows known/unknown
   status correctly. Nested adapter args/events retain placeholders.
5. **External service integration.** Exercise native/MCP tools and `http_request`
   against a controlled real HTTP endpoint; exercise a model-authored MCP service
   through `add_mcp_server`, restart and call again without an agent turn. No Monty
   re-entry, heap serialization or cell replay. Wrong server/schema binding is
   refused visibly. Request errors and response caps remain real errors.
6. **Actual surfaces and performance.** Desktop + phone: one-click links, visible
   external images, loading/stale/error states, incompatible root fallback;
   progressive provider capture and §7 traces. Close/reopen, hidden view, relay
   disconnect and generation changes produce no query pileup or stale overwrite.
   TUI pty source/list/open; Slack/Signal file delivery; ACP ResourceLink.
7. **Model steering.** 15 tasks (5 prototypes, 5 read panels, 5 action panels)
   with a fixed **realistic non-empty tool catalog**: ≥12/15 valid first-pass
   frontier outputs, ≥9/15 small-model outputs, then record correction success
   and positional mistakes from renders. Separately 20 prompts choosing app vs
   fence vs show, target ≥16 correct. Fix prompt/catalog on failure, not an
   unrequested per-model feature flag or a banner claiming invalid code is safe.
8. **Release integration and documentation.** Helper packaging/signing/update
   artifacts, disabled mode, clean-machine launch; generated bindings/skill;
   README/config/crate list and 00 amendments below. Record observed outcomes
   and exclusions; no claimed performance from parser-only microbenchmarks.

After the helper/provenance contracts stabilize, artifact/backend and desktop
work can run concurrently. Parent owns shared tool-consent and artifact commands;
workers do not independently invent those interfaces.

## 11. Architecture amendments and explicit exclusions

When implemented, update 00 in the same series, marking the design's state honestly:

- §2: app validation uses a packaged, lazy TS helper; rationale is maintained
  upstream grammar, cold boundary, no per-token process/serialization cost.
- §4: explicit user activation satisfies Ask for that action, including
  destructive/secret-bearing calls; Deny wins; model calls unchanged. App input
  is not a new loop-local Python operation. Opaque composition keeps placeholders
  until the terminal native/MCP call; output scrub retains 37's limitations.
- §5/§7: `app` direct tool; app-owned get/call/status/close RPC and events;
  `http_request` belongs to web. The agent does not host app programs.
- §8/§10: app plugin, upstream TS library/helper, progressive desktop/web panel,
  TUI source view. No Rust parser/evaluator, session callable registry or app DB.
- §12/§13: real-service app + progressive rendering + one-activation actions are
  the new acceptance row; jarvis's bespoke programs sandbox is not returning.

Not included: custom backend language/sandbox, generic callable registration,
user source-edit synchronization, app-to-app router, browser credential export,
new DLP/scanning layer, or a bespoke TUI renderer. Multi-step UI can use state,
normal tools and assistant actions. These exclusions remove machinery, not the
ability to build a live, service-backed app.

## 12. Verification record

### 12.1 As built (2026-09-18, `feature/apps`)

Implemented: `crates/app` (`AppPlugin`, `Apps` actor, `AppTool`, `helper`,
`sched`, `meta`), `packages/apps` (library, `check`, helper, patched renderer
adapter), `tools::AppInvoke` + `Tool::CONSUMES_SECRETS`, `artifact` `Kind::App`
/ `Commit { expected_base }` / `app_invocations` + owners, four `rpc::Api`
methods + `AppCallChanged` + `Info.app_enabled`, desktop `components/apps`,
TUI/Slack/Signal/ACP source surfaces, `web::http_request`, helper build,
packaging, updater probe, `scripts/apps` acceptance drivers.

Exercised locally on macOS arm64, all from the tree at library hash
`8dc9c11b…cc46c4`:

- `packages/apps`: 78 tests (check, bounds, library, helper protocol, jsdom
  renderer), `tsc` check and the helper build pass.
- Desktop UI: 150 tests, TS check and a production build pass.
- Real RPC smoke (`scripts/apps/smoke.ts` against a headless harness, a
  scripted OpenAI-compatible fixture model and an ordinary stdio MCP fixture
  exposing `argo_*`): all 14 scenarios pass — `publish`, `get_app`,
  `auto_read`, `needs_approval`, `auto_mutation`, `press`, `duplicate`,
  `changed_args`, `destroy`, `echo_auto`, `echo_press`, `stale_generation`,
  `peer_disconnect`, `cas_edit`. That covers a single press running a
  mutating, a destructive and a `{{secret:…}}`-bearing call with **zero**
  `ApprovalRequested`, an automatic read that needs approval becoming
  `needs_approval` without a prompt, a timer never running a Mutation, the
  placeholder staying literal in the committed source and reaching the MCP
  server expanded only at the terminal call, duplicate/mismatched request IDs,
  a stale generation, peer close detaching reads, and two patches racing for
  one base where the loser gets `conflict` with the latest record.
- Registry unit tests: press answers Ask for destructive and secret calls but
  not Deny; automatic Ask never prompts or runs; stale binding, bad args or a
  foreign activation never dispatch; a concurrent model call still asks while
  a press runs; identity ignores object key order but not schema/array order.
- `crates/app/tests/publish.rs`: publish → open → call → edit end to end
  against the real helper and store.
- Browser DOM check of the committed ArgoCD panel in the desktop UI: clicking
  **Sync** produced `Healthy`/`synced` and a fixture call count of 1. This is a
  DOM observation. **Visual acceptance is the user's**: the user reviewed the
  production-build URL and accepted the presentation ("visuals acceptable").
  The assistant makes no visual claim.
- Model steering (§10.7) with a frontier model (`amazon-bedrock/us.openai.gpt-6-astra`)
  over `app`, `show`, `load_skill`, `argo_app_health`, `argo_sync`: 15/15
  first-pass valid (5/5 prototype, 5/5 read, 5/5 action), 0 corrections;
  vehicle choice 19/20. An earlier 0/15 first-pass run was a test-harness
  defect — it hid `load_skill`, so the model could not read the `apps` skill —
  and every one of those 15 was corrected on the next call; it is not a
  product result. The passing run used the harness's earlier prompt text, so
  the later compact prompt is not what produced 15/15. Small-model steering
  was not run.
- Helper, earlier source revision (different library hash, not rerun after the
  review edits): 100-statement `check` warm median 0.51 ms, p95 1.04 ms, first
  request 34.91 ms; executable 59,881,826 bytes (57.1 MiB) at the current hash.
- Helper signing configuration, ad hoc, once on the built `dist` helper:
  `codesign --force --options runtime --timestamp=none --sign - --entitlements
scripts/entitlements-app-parser.plist`, `codesign --verify --strict` and the
  signed helper's `--version` printing the current hash. The entitlement is
  `com.apple.security.cs.allow-jit` only. Release signing runs on another
  machine; no further signing command or check is run here, and no Developer
  ID or notarization is claimed.
- Desktop §7 trace, **dev UI build**, browser DOM, 62 s uninterrupted: a
  100-statement progressive app while 50 background sessions streamed
  (all 50 finished, 30,050 deltas, 0 errors). 3,695 rAF callbacks; frame
  interval p95 18 ms; one 126 ms long task, the cold lazy load of the app
  module; first rendered statement 9.23 s before the artifact committed.
  Frame interval is not main-thread work, so §7's "p95 ≤16 ms input/frame
  work" is **not** shown by this trace. Progressive rendering before commit
  is shown for the fixture provider, not a real streaming provider.
- Desktop §7 trace, **production build**: cold load of the app module is one
  146 ms long task, over the 50 ms budget; the harness collected 0 rAF
  samples, so it could not certify the frame budget either way — no pass is
  claimed. **User decision: merge with this limitation.** The cold-load long
  task is an accepted, known limit; performance is no longer a merge gate for
  this feature. §7's numbers stay as the design targets.
- Relay probe (`/tmp/…/relay-result.json`), real local relay + headless
  harness, a Bun client using the desktop's own `LinkTransport` crypto: all
  7 steps pass — `status`, `pair` (IKpsk2 pairing key, `LinkPaired`),
  `session`, `get_app`, `press` (health `load` succeeded, press succeeded,
  fixture count 0→1, secret value absent from the reply, `app_call_status`
  over the relay agrees), `reconnect` (IK re-handshake, harness saw
  offline→online, a load after reconnect succeeded; 512 ms step), `revoke`
  (`LinkRevoked`, relay client lost its session, device gone from
  `link_status`). No phone hardware, no visual claim.

Not run, recorded as unrun rather than passed: Linux runtime (the Docker build
and clean-`ubuntu:22.04` probe are release-script gates, not exercised here);
physical phone/iOS trace, heap or open/close cycles; Developer ID signing,
notarization and a clean-machine launch (external, another machine); a frame-
budget measurement on the production build (see the accepted limitation above);
first-meaningful-frame timing against a real streaming provider;
a model-authored MCP server installed through `add_mcp_server`, restarted and
called without an agent turn (the smoke used a fixture server); app RPC from
a physical phone (the relay path itself is the probe above); Slack/Signal live
delivery of an app source file (code path
shared with 35's artifact delivery); `enabled: false` in a pty.

Discrepancies between the code and §§0–11 found while writing this record.
They are reported, not accepted as scope changes; each stays open until the
user decides or the code changes:

- The 5 s minimum refresh interval is enforced by the patched renderer only;
  the harness bounds by slots and coalescing, not by time (§7 table).
- Evaluation budgets are per render pass / action plan / query argument, not
  one allowance per update (§7); `materializedNodes = 65,536` was added.
- `AppCallChanged` fires once on completion; there is no `running` event, and
  a refusal before admission publishes nothing (§6.4).
- `app_call_status` is scoped to an authenticated connection of the harness,
  not to the requesting view (§6.3).
- A native tool's identity origin is `native:<crate version>`; a rebuild at the
  same version does not change it (§3.2).
- Durable duplicate detection compares args, not `action_id` (§6.4).
- The helper's 4 MiB request guard originally checked only an unterminated
  tail; fixed 2026-09-18: `handle(line)` now caps the UTF-8 byte length of
  every line, completed or final-at-EOF, before `parseRequest` (§2.1).

### 12.2 Design-revision checks (2026-09-18, before implementation)

Isolated checks used published `lang-core`/`react-lang` 0.3.0 and `react-ui`
0.16.1:

- Both §4 examples parsed with zero reported errors/unresolved references:
  prototype 9 statements; live panel 11 statements, one Query and one Mutation.
- Incremental parsing produced a populated root before the final source bytes.
  Merge updated a statement, removed an unreachable statement and retained state.
- A core-only Bun-compiled executable handled two JSON-lines requests in one
  process without a browser or Node/Bun command at execution time. The local
  macOS arm64 probe was **59,914,850 bytes (57.1 MiB)**: bundling a JS runtime has
  a real release-size cost.
- A browser DOM check of the actual upstream renderer showed partial title/text
  before completion, then programmatic button activation changed `$stage` to
  `cutover`. `@OpenUrl` emitted the expected `open_url` event. This checked state
  and callback behavior, not physical pointer interaction or the OS opener.
- The external-image policy check loaded an image from a second local origin
  under both desktop and relay CSP strings. The server also recorded the request
  from an HTML `srcdoc` iframe with `sandbox=""` and the actual artifact image
  CSP. These are network/DOM observations, **not visual appearance validation**.
- Desktop `bun run check` and `cargo check -p relay` passed. The image permission
  changes are implemented in desktop, relay and HTML-artifact CSP; unrelated
  script/form restrictions remain unchanged.

Visual appearance is never inferred by the assistant from DOM/network checks;
it is the user's review, given in §12.1.

### Implementation verification scope agreed with the user

For the implementation/merge of this feature, the user approved **merge after
local checks and their visual review**. Linux runtime/performance, physical-phone
measurements and Developer ID/notarized clean-machine checks are not merge gates
in this environment; document them as unrun, not passed. Packaging support for
those platforms remains in implementation scope. Signing happens on another
machine: the scripts carry the signing configuration, and no signing command or
verification runs here beyond the one ad-hoc configuration check recorded in
§12.1. Local parser/helper execution, backend/RPC/relay behavior, desktop
automated interaction, installation checks and disabled mode remain required.
Outcome (2026-09-18): the user accepted the visual presentation at the
production URL and decided **merge with limitation** on performance — the
production cold-load long task (§12.1) is accepted and the frame budget is
uncertified; performance no longer gates this merge. The assistant does not
inspect images; visual acceptance is performed by the user.
