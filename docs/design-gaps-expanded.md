# Design gaps, expanded

Status: audit · 2026-09-10 · resolved by [`design/12-course-correction.md`](design/12-course-correction.md) on 2026-09-11

What was checked: the constitution in `docs/design/00-architecture.md` as
first committed (`fa50c31`, phase 0, 2026-09-09), against the tree as of
this writing. Later docs (01, 05, 08, 09, 10) describe what was built;
several clauses in **current** 00 still describe what was promised not to.
The interesting gaps are the ones 00 still asserted.

How to read each item: the original rule, the code that diverged from it
(path and snippet), why that matters (the jarvis shape, as opposed to a
feature), then one recommended fix. Features that were not in v0 are
additions past v0, not violations; the gap is 00 not describing them.
“Amend 00” is a valid fix when the feature earned its keep and the
constitution is what is stale — but then the amendment has to name the
alternative it beat, same as every other decision in 00. Each `### Fix`
ends with a **Resolved** line saying what actually shipped, including
where it differs from the recommendation.

---

## Resolution

What 12 did with each item. Where the outcome differs from this doc's
recommendation, the row says so.

| Id | Outcome | Where |
|---|---|---|
| G1 | `ErasedTool::gate` deleted. MCP `Risk` comes from the server's `ToolAnnotations` (`read_only_hint` / `destructive_hint`; unset = `Mutating`); `mcp/config.yaml` values are `read_only \| mutating \| destructive \| off`; the registry decides with `tools.policy.action(spec.risk)` alone. The Ask→Allow switches (global, per-session with parent inheritance) stay as the documented rewrite. | 12 §5, 00 §4 |
| G2 | 00 §10 lists all twelve crates; §13 says “not in v0” with a returned-as table. Knowledge was extracted to plugin crate `knowledge` — user decision, overriding this doc's keep-in-agent; the harvest *model call* stays in the loop. Cursor stays a crate reached only through `agent::backend`. | 12 §1, §9 |
| G3 | Pairing moved into `SignalPlugin::start` (`LinkOpts`); `main.rs` imports only `SignalPlugin` / `LinkOpts`. | 12 §3 |
| G4 | `Runtime::lookup_when`; signal and agent register tools through it; the order comment and the `0..50` retry are gone. | 12 §2 |
| G5 | `agent::backend::Backend { Genai, Cursor }` enum — a closed set of two, not the trait proposed here; `loop_.rs` has no `"cursor"` literal. | 12 §6, 00 §5 |
| G6 | 00 §8 rewritten as the control-plane truth; Tauri gated on one command list; no refactor yet, deliberately. | 12 §7 |
| G7 | Tool list fetched once per turn in `Turn::rebase`; `tools_seen` and the warn deleted; 00 §9 names the watcher. | 12 §4 |
| G8 | 00 §4 carries the `todo` / `python` loop-local exception verbatim. | 12 §1 |
| G9 | 00 §1.8 rewritten. This doc's “no grep/edit natives” reading was wrong: any assistant needs to search and edit files. `grep` and `edit` are natives now; coding-specific natives (LSP, notebooks, git, diff review) stay out. | 12 §10 |
| G10 | 00 §10 budget sentence; `lib.rs` re-export rule; opportunistic. | 12 §8 |
| G11 | `ToolSpec` comment and 00 §6 fixed. This doc's collision claim was stale: `mcp.rs` already logged and skipped clashes (`taken` map), never replaced. | 12 §1 |
| Suggested order | Executed as 12 §12 waves. Differences: the guard/lint idea was dropped (docs are the record, no scripts enforce them), knowledge became a plugin, grep/edit were added. | 12 §0, §12 |

---

## What still holds

Do not read the rest as “the architecture broke.” These rules held:

| Rule | Evidence it held |
|---|---|
| `runtime` ≤ 400 lines | `crates/runtime/src/lib.rs` is 225 lines |
| No shared `Event` enum | Events are per-plugin types; `AgentEvent` is one stream owned by agent (00 §4 allows this) |
| Typed kameo bus | `Runtime::publish` / `subscribe` in `runtime/src/lib.rs` |
| One `Approval` actor | `tools::APPROVAL_ACTOR`; `Invoke` still ends at `RequestApproval` |
| `Plugin` trait is the extension point | `wth/src/main.rs` still `for plugin in plugins { plugin.start(&rt) }` |
| One process | TUI, agent, tools, optional RPC/Signal in one binary |
| Tool types live in `tools`, not `runtime` | Better than original §4 table, which put `Tool`/`Risk` in `runtime` |

The cache invariant (07) is also being kept. That is not an original-00
rule, but it is the actual product and it is not on this list.

---

## G1. Permission accretion

### The rule

00 §1.7: *“Permissions are one contract type from day one, not a subsystem
that accretes. (jarvis: tiers, approvals, kill switch, safe mode grew ad
hoc.)”*

00 §4, still on the page:

> `Invoke` → registry checks the tool's `Risk` against config policy
> `{ read_only = "allow", mutating = "ask", destructive = "ask" }` → on
> `ask`, publishes `ApprovalRequest` … Timeout → deny. **That is the whole
> permission system for v0.**

Original (`fa50c31`) stopped there. Current 00 then immediately describes
two switches that turn `ask` into `allow`. The sentence and the paragraph
under it disagree. MCP gates are not mentioned in 00 at all; they live in
01 §1.4.

### What the code does

One `Invoke` now consults four extra axes before `Risk × policy`.

The registry owns the kill switches (`crates/tools/src/registry.rs`):

```rust
pub struct ToolRegistry {
    tools: HashMap<String, DynTool>,
    policy: Policy,
    /// `tools.approvals`: false turns every `Ask` into `Allow`.
    approvals: bool,
    /// Sessions that turned prompts off for themselves (and, through
    /// `parents`, for their subagents).
    bypass: HashSet<SessionId>,
    /// child → parent, from [`SetParent`]
    parents: HashMap<SessionId, SessionId>,
    ...
}
```

The decision, same file, `Message<Invoke>`:

```rust
let mut action = tool.gate().unwrap_or_else(|| self.policy.action(spec.risk));
if action == PolicyAction::Ask && (!self.approvals || self.bypassed(call.session)) {
    action = PolicyAction::Allow;
}
```

`tool.gate()` is the second policy. `ErasedTool` in `crates/tools/src/lib.rs`:

```rust
/// Explicit approval action; `None` → registry uses `policy.action(spec.risk)`.
/// Only MCP tools return `Some`.
fn gate(&self) -> Option<PolicyAction> { None }
```

01 §1.4 is explicit that this is *not* `Risk × policy`:

> MCP tools always carry an explicit action — `allow` unless this file says
> otherwise — so `tools.policy` (a *risk* → action map for tools whose risk
> we know) **never applies to them**.

Worked example. Native `write_file` is `Risk::Mutating` → policy `ask` →
prompt. An MCP tool that writes a file, unless `mcp/config.yaml` says
otherwise, is `gate() = Some(Allow)` → runs with no prompt, and
`tools.policy.mutating = ask` is irrelevant. Two tools that mutate the
disk, two different constitutions.

The lineage walk (`bypassed`) is a third decision procedure:

```rust
fn bypassed(&self, mut id: SessionId) -> bool {
    loop {
        if self.bypass.contains(&id) { return true; }
        match self.parents.get(&id) {
            Some(p) => id = *p,
            None => return false,
        }
    }
}
```

So for one `Invoke` the registry answers: MCP gate? else risk policy? then
global kill switch? then this session or any ancestor opted out?

### Why it matters

00 named the jarvis failure mode in the same bullet as the rule. The four
axes are that failure mode with better names: a kill switch
(`tools.approvals`), a per-session switch (`bypass`), inheritance
(`SetParent`), and a parallel policy file (`mcp/config.yaml`). The
`Approval` actor is still one object. The *contract* is no longer one type.

### Fix

Keep the UI modes. Collapse the decision back to one function of `Risk`.

1. Give every MCP tool a `Risk` (default `Mutating`, as original 00 §6
   already said). `mcp/config.yaml` becomes a **Risk assignment** (or a
   hide): `deny` → `UnregisterTool` (model never sees it); `allow` /
   `ask` map onto Risk + `tools.policy`, not a parallel `PolicyAction`.
   Stop returning `Some` from `gate()`.
2. Delete `ErasedTool::gate`. The Invoke line becomes
   `self.policy.action(spec.risk)` plus the documented UI rewrite
   `Ask → Allow` when `!approvals || bypassed(session)`.
3. Keep `deny` un-liftable (already true).
4. Rewrite 00 §4 to: *One contract: `Risk → PolicyAction`. Two UI modes
   rewrite `Ask` to `Allow` (persisted global, in-memory per-session with
   parent inheritance). `Deny` is never rewritten. MCP config assigns Risk
   or hides a tool; it is not a second policy.* Strike “that is the whole
   permission system for v0” or make it true.

Do not keep `gate()` and “document it.” That is how the paragraph under
“that is the whole system” appeared.

This is a behaviour change for MCP defaults (today: allow; after:
`tools.policy` on `Mutating` = ask). Call it out. That is the point.

**Resolved (12 §5):** `ErasedTool::gate` deleted; MCP `Risk` comes from
`ToolAnnotations` (unset = `Mutating`); `mcp/config.yaml` assigns
`read_only | mutating | destructive | off`; the registry decides with
`tools.policy.action(spec.risk)` alone. The Ask→Allow switches stay as the
documented rewrite (00 §4). Behaviour change landed: unannotated MCP tools
prompt.

---

## G2. Crates that did not earn a phase, and §13’s not-planned list

### The rule

00 §1.6: *“A subsystem earns a crate only when the current phase needs it.”*

00 §10, **still**: *“Six crates for v0.”* The listing under that sentence
already has seven names (`oauth` was inserted; `models`, `markdown`,
`cursor`, `signal` were not).

00 §13, **still**:

> Subsystems that were never used daily (meetings, **signal**, slack, voice,
> goals, loops, notes, **okf**, character, continuity): **not planned**. They
> return as plugins only when needed, **and only as plugins**.

Original phases (00 §12): 0 workspace, 1 agent+tui+tools+mcp, 2 rpc+skills,
3 subagents, 4 Tauri (oos). No Signal, no OKF, no Cursor.

### What the code does

Counts as of 2026-09-10. `knowledge` has since moved out of `agent` into
its own plugin crate (12 §9), so `agent` is smaller today.

| crate | lines | `Plugin`? | In an original phase? |
|---|---|---|---|
| runtime | 225 | trait only | yes |
| wth | 388 | binary | yes |
| models | 393 | no | no (could have lived in agent) |
| markdown | 685 | no | no (could have lived in tui) |
| rpc | 815 | yes | phase 2 |
| oauth | 942 | no | earned by login (05) |
| cursor | 1481 | **no** | **no** |
| tools | 3318 | yes | phase 1 |
| tui | 3484 | yes | phase 1 |
| signal | 5453 | yes | **§13 not planned** |
| agent | 9875 | yes | yes, then everything else |

Signal at least came back as a plugin (`crates/signal/src/lib.rs`
`impl runtime::Plugin for SignalPlugin`). That is the half of §13 they
kept.

Knowledge did not. It is a module inside the agent:

```
crates/agent/src/knowledge/
  mod.rs      // "builds while the user works (design doc 10)"
  harvest.rs  // cache-hot continuation, called from the loop
  repo.rs     // gix
  store.rs    // OKF bundle + FTS5
  tools.rs    // knowledge_* registered onto the tools registry
  types.rs
```

`supervisor::start` opens it and registers seven tools onto `ToolRegistry`
(`crates/agent/src/knowledge/tools.rs`):

```rust
/// Register every `knowledge_*` tool on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, knowledge: Arc<Knowledge>) {
    let _ = reg.ask(RegisterTool(Arc::new(SearchTool(k.clone())))).await;
    // get, list, put, delete, history, restore
}
```

Harvest is a method on the session loop (`loop_.rs` `harvest()`), not a
plugin actor. That is exactly “a subsystem growing inside the agent.”

Cursor is a library the agent calls by namespace. It is not a plugin, not
in the 00 crate list, not in the README crate list. See G5.

### Why it matters

§1.6 and §13 were the brake on jarvis’s crate/module sprawl. Signal is a
5.4k-line plugin that 00 still says is not planned. Knowledge is OKF
returned *not* as a plugin. Cursor earned a crate without earning a phase.
00 still says “Six crates for v0” while the workspace has eleven members.

The constitution was not updated when the tree moved. That is the jarvis
pattern 00 §13 was a postmortem of.

### Fix

Two layers, both required.

**Honesty (do first, today):**

- Replace the §10 listing with the actual members. Strike “Six crates for
  v0.”
- Rewrite §13: Signal returned as a plugin because a phone frontend was
  needed and only a plugin is allowed (name the alternative: in-agent
  module — rejected). OKF returned *inside agent* because harvest is a
  cache-hot continuation of the live frame (07) and extracting it would
  force the frame across the bus — **or** extract it (below). Cursor: see
  G5.

**Structure:**

- `models` and `markdown` can stay as libraries. They are not subsystems;
  they are data and a renderer. One sentence in §10.
- `oauth` stays a library (no actors). Fine.
- Knowledge: **keep in agent** and amend §13 with the harvest-coupling
  reason. Prefer that over extracting a plugin: harvest on the cached
  frame is the whole point of 10, and a plugin boundary there is costume.
  The gap is the stale §13, not the module.
- Cursor: do not let it stay a free-floating crate. Either fold it under
  `agent` (it is a backend, see G5) or it becomes a plugin that registers a
  `ChatBackend` actor. A crate that only the agent calls, with no `Plugin`,
  is the thing §1.6 forbids.
- Signal stays a plugin. Strike it from “not planned.” It planned itself.

Do not split `agent` into ten crates to make the line counts look even.
That is §1.6 in reverse.

**Resolved (12 §1, §9):** 00 §10 lists all twelve crates; §13 says “not in
v0” with a returned-as table. Knowledge did **not** stay in agent: the user
overrode this doc's recommendation and it became plugin crate `knowledge`
(store, index, history, tools, `KnowledgeChanged`); the harvest *model
call* stays in the loop, so the cache-hot argument above still holds for
the part that needed it. Cursor stays a crate reached only through
`agent::backend` (G5).

---

## G3. `main.rs` is not “build Runtime, start plugins, wait”

### The rule

00 §1.4: *“No god-wiring. The binary holds a `Vec<Box<dyn Plugin>>` and
nothing else. Plugins register themselves on the bus. (jarvis: `jarvisd`
imported every subsystem and threaded handles between them.)”*

00 §4, still:

> `wth/main.rs` is, in full: build `Runtime`, `for p in plugins {
> p.start(&rt).await? }`, wait for shutdown.

### What the code does

`crates/wth/src/main.rs` is 267 lines plus `worker.rs` (121). The plugin
loop is still there (`run()` around the `Vec<Box<dyn Plugin>>`). Around it:

1. A process-global allocator and a `subprocess` argv branch *before clap*,
   because monty workers are this binary (`08 §9`):

   ```rust
   #[global_allocator]
   static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;
   ...
   if std::env::args().nth(1).as_deref() == Some("subprocess") {
       std::process::exit(i32::from(worker::run()));
   }
   ```

2. `first_run` writes `config.yaml`, `prompt.md`, `credentials.yaml`, `mcp/`.

3. `--link-signal` never builds a `Runtime` and never starts plugins. It
   calls `signal::link_oneshot`, `signal::qr`, `signal::e164`, then
   `runtime::write_config_value` on `signal.owner` / `signal.device_name`,
   then `process::exit(0)`. The binary knows a plugin’s pairing protocol.

   ```rust
   if cli.link_signal {
       link_signal(&config, &data_dir, &config_path, cli.owner, cli.device_name).await?;
       drop(_guard);
       std::process::exit(0);
   }
   ```

4. After shutdown, `sleep(100ms)` + `process::exit(0)` because a stuck
   `spawn_blocking` pipe drain would pin the runtime (comment in `run()`).

The plugin vector itself is also conditional: Signal always, RPC if
`--serve`/`--headless`, TUI unless headless, TUI gets `resume` and
`prompt` as struct fields. That part is ordinary.

### Why it matters

The spirit of §1.4 is “don’t thread handles.” They mostly don’t: plugins
still look each other up by name. The literal rule is “the binary holds a
Vec and nothing else.” `--link-signal` is `jarvisd` knowing a subsystem:
`main` imports `signal::` pairing and writes that plugin’s config keys.

The monty worker path is process-level, not handle-threading. It is not
the same thing. The `process::exit` after sleep is a bug working around
another bug, not an architecture choice.

### Fix

1. Move pairing into the plugin. `Cli` keeps `--link-signal` / `--owner` /
   `--device-name`. `main` only does:

   ```rust
   Box::new(signal::SignalPlugin {
       channel: cli.headless,
       link: cli.link_signal.then(|| LinkOpts { owner: cli.owner, device_name: cli.device_name }),
   })
   ```

   `SignalPlugin::start` does the QR, writes config via its own path, then
   `rt.shutdown.cancel()`. `main` never mentions `link_oneshot` / `qr` /
   `e164`. Delete `link_signal()` from `main.rs`.

2. Document the worker argv and the global allocator in 00 §2 (process
   model) as a deliberate exception: the Python worker *is* this binary, so
   `main` must branch before the plugin list. That is not god-wiring.

3. Leave `first_run` in `wth` (data-dir skeleton is the binary’s job) or
   move it onto `Runtime::new`. Either is fine; it is not a plugin handle.

4. The `process::exit` after sleep is a known gap (00 §12 already lists
   stuck drains). Fix the drain or keep the exit; don’t call it
   architecture.

**Resolved (12 §3):** pairing moved into `SignalPlugin::start` via
`LinkOpts`; `link_signal()` deleted from `main.rs`, which now imports only
`SignalPlugin` / `LinkOpts` from `signal::`. Worker argv, allocator,
`first_run` and the `process::exit` stay, documented in 00 §2 / §12.

---

## G4. Start-order coupling

### The rule

00 §4, still:

> Plugin order is the vector order; a plugin needing another looks it up by
> name at **first use, never at start** (no start-order coupling).

The approval actor already follows this: `ToolRegistry.approval` is
`Option<ActorRef<Approval>>`, filled on the first `Ask`
(`registry.rs`, comment: *“Looked up at first `Ask`, never at start”*).

### What the code does

Signal **at start**, and the comment names the vector order
(`crates/signal/src/lib.rs`):

```rust
// tools starts before us (main.rs order), so one lookup is enough.
match ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR) {
    Ok(Some(reg)) => {
        let _ = reg.ask(tools::RegisterTool(Arc::new(SignalLinkTool { ... }))).await;
        let _ = reg.ask(tools::RegisterTool(Arc::new(SignalStatusTool))).await;
    }
    _ => tracing::warn!("tool registry not running; signal tools not registered"),
}
```

If anyone ever reorders `main.rs` to start Signal before Tools, `signal_link`
and `signal_status` silently vanish. That is start-order coupling.

Agent **at start**, with a retry because it refuses to assume the same
order (`crates/agent/src/supervisor.rs`):

```rust
// The tools plugin may start after us; retry the lookup briefly.
tokio::spawn(async move {
    for _ in 0..50 {
        if let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
            // RegisterTool: skills, knowledge, spawn_subagent, send_message
            ...
            return;
        }
        tokio::time::sleep(...).await;
    }
});
```

Two plugins, two policies, for the same dependency. Approval does the thing
the rule asked for. Signal and agent do not.

### Why it matters

The rule exists so `main.rs` is a list, not a DAG. Signal’s comment makes
the list a DAG. Agent’s 50-retry is a workaround that still runs at start
(it is not “first use”; it is “first 50 ticks of start”). Reordering
plugins should be boring. Today it is not.

### Fix

Do not put the registry in `Runtime` (that is god-wiring the other
direction).

Add one helper, used by every plugin that registers tools:

```rust
// runtime
impl Runtime {
    /// Resolve `name` as `A`, waiting until it exists or shutdown.
    pub async fn lookup_when<A: Actor>(&self, name: &'static str) -> Option<ActorRef<A>>;
}
```

Signal and agent `start` spawn the same task: `lookup_when::<ToolRegistry>
("tools")` then `RegisterTool`. No `main.rs` order comment. No `for _ in
0..50`. Approval can keep looking up on first Ask (it does not need to
register anything).

Amend §4: *start may wait on a named actor; it must not assume vector
order.* “Never at start” was the wrong phrasing for “don’t assume order.”
Registration of tools *has* to happen before the first `ListTools` or the
model never sees them, so “first use” is start-of-session, not
start-of-invoke.

Delete the Signal comment as part of the same patch.

**Resolved (12 §2):** `Runtime::lookup_when` added; signal and agent
register their tools through it. The `main.rs order` comment and the
`for _ in 0..50` retry are gone. 00 §4 now says a plugin may wait for a
named actor at start but must not assume vector order.

---

## G5. A second LLM client, handwritten

### The rule

00 §1.2: *“No reinvented wheels. Off-the-shelf library first; we write
glue. A custom implementation needs a written reason in this doc.”*

00 §5: *“Library: genai … Rejected: `async-openai` (one provider),
`llm-chain`/`rig` (framework opinions we would fight).”* One client.

00 §7 rejected `tonic` for the RPC plugin (“no webview story without
grpc-web”). Cursor then needed an HTTP/2 protobuf client and did not use
tonic either.

### What the code does

The session loop’s stream open is a namespace branch
(`crates/agent/src/loop_.rs`):

```rust
if ns(&model.genai) == "cursor" {
    let t = self.shared.auth.oauth.access(&oauth::CURSOR).await?;
    let slug = model.id.split_once('/').map_or(model.id.as_str(), |(_, m)| m);
    cursor::chat_stream(&t.access_token, slug, req, &self.shared.id.to_string()).await
} else {
    self.shared.client.exec_chat_stream(&model.genai, req, Some(&opts)).await
    ...
}
```

`transport()` has the same branch: `cursor` → `Transport::Plain` because
“the `cursor` namespace never reaches genai” (`frame.rs`).

`crates/cursor/src/lib.rs` is a Connect-RPC protobuf client at
`https://api2.cursor.sh` (`agent.v1.AgentService`), “mirrors omp’s
`cursor.ts`.” ~1481 lines, `prost` generated proto, custom HTTP/2 framing
in `connect.rs`. Not a `Plugin`. Agent depends on it directly
(`crates/agent/Cargo.toml`).

`configured()` in `supervisor.rs` special-cases it again: *“`cursor` is not
a genai adapter: only a login configures it.”*

05 documents the *how*. 00 still does not contain the *why we wrote a
client*.

### Why it matters

The loop is supposed to be provider-agnostic glue over genai. A string
compare on `ns` in the hottest function is the opposite. It is also §1.2:
a custom HTTP/2 protobuf stack with no written reason in the constitution,
after 00 rejected tonic for a weaker use case.

This is the same shape as “just one more backend” in jarvis
(`genai_backend.rs` vs a second adapter). Once there are two, the third is
free.

### Fix

Recommended, in order:

1. **Hide the branch.** Introduce a small trait in `agent` (not in
   `runtime`):

   ```rust
   trait ChatBackend: Send + Sync {
       async fn stream(&self, model: &Resolved, req: ChatRequest, opts: &ChatOptions)
           -> Result<EventStream, String>;
   }
   ```

   `GenaiBackend` and `CursorBackend` implement it. `Session` holds
   `Arc<dyn ChatBackend>` chosen when the model is set, not on every
   `call()`. `loop_.rs` loses `if ns == "cursor"`. `transport()` asks the
   backend.

2. **Write the reason in 00 §5.** *Cursor’s AgentService is Connect-RPC
   protobuf over HTTP/2; genai has no adapter and the protocol is not
   OpenAI-shaped. Crate `cursor` is that adapter. Alternative: contribute
   upstream to genai — rejected until the protocol stabilises. Alternative:
   tonic — rejected (heavy, protoc). We generate with protox/prost.*

3. **Crate shape (G2).** Once it is an adapter the agent owns, fold
   `crates/cursor` into `agent/src/cursor/` **or** keep the crate but
   depend on it only from the backend impl, never from `loop_.rs`. Prefer
   keep-the-crate: generated proto is loud in the agent crate. The
   gap is the `if ns` and the missing reason, not the directory.

Do not add a third `if ns == "foo"` while this is open.

**Resolved (12 §6):** an enum, not the trait proposed above:
`agent::backend::Backend { Genai, Cursor }`, chosen in `SetModel`, one
`match`, no vtable — a closed set of two did not need dynamic dispatch.
`loop_.rs` has no `"cursor"` literal; `backend.rs` is the only agent file
naming `cursor::`. 00 §5 carries the reason and the rejected alternatives.

---

## G6. The TUI is a control plane; RPC is a second protocol

### The rule

00 §8, still:

> The TUI is an ordinary bus subscriber: it registers recipients for the
> agent’s events and approval requests, and sends `UserInput`/`Steer`/
> `Resolve` by `ask`/`tell`. It does not know the agent exists beyond those
> types. **The same code path a Tauri app will use over RPC.**

00 §1.5: *“No central event enum, no protocol dumping ground.”*

00 §7: *“The RPC layer is generic glue, not a second protocol. … Method
params/results and event payloads are the plugins’ own types; the RPC crate
defines none.”* The examples were `agent.send_input`, `agent.steer`,
`agent.list_sessions`, `tools.resolve_approval`, `tools.list`.

### What the code does

`crates/tui/src/model.rs` `Command` was 31 variants at audit time (36 by
2026-09-11), `Send` through `Logout` (and growing — `Providers` /
`AddProvider` sit next to it). `UiEvent` was 27 variants (30 by
2026-09-11). Original surface was three asks.

The TUI plugin subscribes to five event types, not two
(`crates/tui/src/lib.rs`):

```rust
rt.subscribe::<Bridge, AgentEvent>(&bridge).await;
rt.subscribe::<Bridge, ApprovalRequested>(&bridge).await;
rt.subscribe::<Bridge, ApprovalResolved>(&bridge).await;
rt.subscribe::<Bridge, McpProgress>(&bridge).await;
rt.subscribe::<Bridge, LoginFinished>(&bridge).await;
```

It also reads `AgentConfig`, `tools::ToolsConfig`, and
`models::Catalog::load` at start — it knows those plugins exist beyond
their event types.

Slash commands (`/model`, `/mcp`, `/skills`, `/knowledge`, `/login`,
`/approvals`, `/sessions`, `/prompt`, `/compact`, `/harvest`, …) are not
bus events. They are a control plane. `run_commands` in `tui/src/lib.rs`
is the in-process version of `rpc/src/api.rs`.

RPC `Api` (`crates/rpc/src/api.rs`) is 51 methods (this doc first counted
`~93 async fn`; 12 §7 counts 51). It imports agent
(including `knowledge::{Change, Concept, HarvestResult, History, Hit,
Meta, Put, Source}`), tools (including `git::SyncResult`), and signal
(`Allow`, `Link`, `RemoveAllowed`, `SignalStatus`, …). Payloads are still
the plugins’ types (00 §7 held). The **method list** is a second protocol,
duplicated by `tui::Command`.

`rpc/src/bridge.rs` hard-codes the event types it forwards:

```rust
forward!(
    AgentEvent, LoginFinished, ApprovalRequested, ApprovalResolved,
    McpProgress, SignalEvent
);
```

A new event type that a UI should see is a three-crate edit (owner, tui
Bridge, rpc Bridge). That is a dumping ground split across files.

### Why it matters

00 §8 described a chat widget. The TUI is an operator console. Tauri-over-
RPC cannot reuse the TUI path: the TUI talks kameo types, RPC talks
jsonrpsee. Every new slash command is added twice (`Command` + `Api`).
That is the protocol dumping ground, just not named `Event`.

00 §2 rejected “daemon + thin clients from day one” so the TUI could be
in-process. That is still right. It does not require the TUI to *be* a
second RPC implementation.

### Fix

Pick one. Do not do both.

**A — Amend §8 (recommended).** Tell the truth:

> The TUI is a control plane over the same actor commands RPC exposes. It
> subscribes to bus events for the transcript and approvals, and `ask`s
> named actors for everything else. It is not a thin subscriber. Tauri
> uses RPC; the TUI uses in-process asks of the same command types.

Then make `tui::Command` mechanically the same list as `rpc::Api` (one
source: the actor command structs). A new command is added on the owning
plugin and picked up by both facades, not designed in `model.rs`. The
bridges subscribe via a small registry of `TypeId`s each plugin declares
as wire-visible, so `forward!(...)` dies.

**B — Restore §8.** Always start the RPC plugin. TUI is a jsonrpsee
*client* of localhost, same as Tauri. Delete `tui::Command`’s actor
knowledge. Cost: serialization on every slash command, which §2 rejected
for the chat path. Acceptable for `/mcp` etc.; do not put `TextDelta`
through it (keep the bus subscription).

Do not add Tauri until A or B is done. Otherwise Tauri becomes a third
list.

**Resolved (12 §7):** option A's amendment landed — 00 §8 is rewritten as
the control-plane truth, with the counts above, and phase 4 (Tauri) is
gated on the two command lists becoming one. The unification itself was
not done, deliberately: a real refactor with one beneficiary that has no
date. `forward!` and the five `rt.subscribe` lines stay until a third
consumer appears.

---

## G7. Hot reload in v0

### The rule

Original 00 §9: *“No hot reload in v0.”* Current 00 still describes
figment-at-start; it no longer says the sentence, but it also never
replaced it with a watcher decision. 01 and 04 shipped the watcher.

### What the code does

MCP is a filesystem watcher (`crates/tools/src/mcp.rs`):

```rust
let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
    if res.is_ok() { let _ = tx.send(()); }
})?;
watcher.watch(&dir, notify::RecursiveMode::NonRecursive)?;
```

Reconcile on any `mcp/*.json` change, 300ms debounce. Skills and knowledge
resync at turn start (mtime), which is not a watcher but is also not
“load once at start.”

Mid-turn, a tools-hash change already warns (07 §5, `loop_.rs`
`tools_seen`). So a drop-in MCP file mid-turn can change the tool list
the 07 invariant said was frozen for the turn.

### Why it matters

A v0 constraint was ignored because the MCP drop-in dir is unusable
without it. Fair. It was never struck from the constitution, and it
interacts with 07 (tool list is part of the cached prefix).

### Fix

1. Write in 00 §9 / 01: *MCP dir is watched; reconcile is debounced.
   Skills and knowledge resync at turn start, not via inotify.*
2. Reconcile must **not** mutate the in-flight turn’s tool list. Apply
   on the next turn start (the same moment 07 already re-renders the
   baseline). The watcher can set a dirty flag; `Turn::rebase` is the
   apply point. That keeps 07’s prefix intact.
3. Until (2), a file drop mid-turn is a known cache miss. Say so.

**Resolved (12 §4):** the direct tool list is fetched once per turn in
`Turn::rebase`; `tools_seen`, the hash diff and the warn are deleted. No
dirty flag — fetching once per turn *is* the freeze. 00 §9 names the
watcher and the turn-start resync; 01 §1.4 / 07 §5 say “next turn.”

---

## G8. Loop-local tools skip the registry

### The rule

00 §4: the registry `answers ask(ListTools) / ask(Invoke)`. MCP servers
add tools to the **same** registry. Original §5: tool calls in one
assistant message run concurrently via `join_all` on `ask(Invoke)`.

Current 07/08 deliberately made `todo` and `python` loop-local. That is
an amendment that 00 §4 was not updated for.

### What the code does

`crates/agent/src/loop_.rs`:

```rust
// Registry batch first (parallel), then the loop-local calls in order.
let local = |name: &str| name == todo::NAME || name == repl::NAME;
```

`python` and `todo` never hit `Invoke`, so they never hit `Risk × policy`,
`gate()`, or the kill switches. Specs are spliced in *after* `ListTools`:

```rust
/// plus the loop-local `todo` and `python`.
specs.push(todo::spec());
if runtime { specs.push(repl::spec()); }
```

Inner host calls from python (`await shell(...)`) *do* go through
`Invoke` (`repl.rs` routes via the registry). So `python` itself is
`Risk::ReadOnly` and silent, and `shell` inside it still prompts. That
is why this mostly works.

`todo` is pure session state. No approval needed. Fine as an exception,
undocumented in 00.

### Why it matters

“Every invocation through the registry” is how you promise there is one
gate (G1). Two names short-circuit it. Three ways to call a tool now
exist: direct `Invoke`, loop-local dispatch, runtime coroutine →
`Invoke`. The model has to know which is which (`direct: true` vs
runtime-only MCP). 00 still describes one path.

### Fix

Do not force `todo` through `Invoke`. It is the turn’s own state. Write
the exception in 00 §4 / 07 §9: *`todo` is loop-local. It is not a
capability. It does not pass the approval gate.*

`python`: keep it loop-local (the `Repl` is session-owned, dump/restore
is in the agent). Write the exception: *`python` is loop-local; every
host call it makes is `Invoke` and takes the gate.* That is the 08
design; 00 should say it.

Do not add a third loop-local name without amending that sentence. The
test is: if it can affect the world without `Invoke`, it is a bug.

**Resolved (12 §1):** 00 §4 carries the `todo` / `python` loop-local
exception in the words above, including the “if it can affect the world
without `Invoke`, it is a bug” test.

---

## G9. “Not a coding agent” vs Cursor AgentService

### The rule

00 §1.8: *“Not a coding agent. oh-my-pi is inspiration for loop shape,
tool ergonomics, sessions, steering, subagents, skills — **not for its
tool set**.”*

Original §6 already specified the tool set: shell (brush), filesystem
(`read_file` / `write_file` / `list_dir`), MCP. So §1.8 never meant “no
shell.” It meant “don’t copy oh-my-pi’s grep / edit / notebook / LSP.”

### What the code does

Native tools are still shell + fs. No grep, no search_replace, no
notebook. That half of §1.8 held.

Beyond §6 they added: persistent Python, knowledge harvest, skill/MCP git
forges (`tools/src/git.rs` is clone/pull for those, not a model-facing
`git` tool), and **Cursor’s coding-agent backend**.

`crates/cursor/src/lib.rs`:

> Cursor agent backend client … `agent.v1.AgentService` at
> `api2.cursor.sh` … one `Run` per assistant step, history replayed as
> content-addressed blobs, **tools served as MCP definitions**.

Turns with `cursor/...` as the model are Cursor’s agent, with this
harness’s tools handed over as MCP. That is a coding agent as a
provider.

Also `frame.rs`:

```rust
const CLAUDE_CODE_IDENTITY: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
```

That is a provider constraint for the Claude Code OAuth login (05 §2.3),
and it is also product branding in the system prompt (00 §1.1: *the only
product name is the binary `wth`*).

### Why it matters

§1.8 is a vibe rule, and shell+fs already made it thin. The sharp
case is Cursor: 00 said we would not take another product’s coding
agent as our shape, then we rented Cursor’s. Claude Code identity is
§1.1, not §1.8, but it is the same gravity — we look like whoever minted
the token.

### Fix

1. Replace §1.8 with something true: *The tool set is shell, fs, MCP,
   python, and whatever plugins register. Coding is a use of the
   harness, not a product line. We do not ship grep/edit/LSP as natives.
   Providers are allowed to be whoever mints the token.*
2. Keep Cursor as a **provider** (G5), not as a tool set. Do not add
   Cursor-specific tools.
3. Claude Code identity: keep the sentence when that login is active
   (the API 403s without it). Document under 00 §1.1: *Provider-required
   identity lines are not product branding; they are not shown for key
   auth.* Do not put `wth` in that sentence.

If you want §1.8 to stay as written, delete the cursor crate. That is the
only consistent reading. Not recommended: the login is doing work.

**Resolved (12 §10):** 00 §1.8 rewritten — but not with the text above.
This doc's reading of §1.8 as “no grep/edit natives” was wrong: the user
ruled that any assistant acting on a machine needs to find text and change
part of a file, so `grep` and `edit` are now natives in `tools/src/fs.rs`.
What stays out is *coding-specific* natives (LSP, notebooks, git, diff
review). Cursor stays a provider (G5); the provider-identity sentence is
documented as not product branding.

---

## G10. `agent` is the new `types.rs`

### The rule

00 §1.5: cap the shared crate, each plugin owns its types. 00 §13: *keep
the shape, cap the core.* Original runtime table listed Tool/Risk in
`runtime`; they moved those to `tools` (good). They did not cap `agent`.

### What the code does

`crates/agent/src` was 9875 lines, 17 files at audit time (10701 / 18 on
2026-09-11 before knowledge moved out; smaller since, 12 §9). `lib.rs` is ~662 lines of
command structs (`SendInput`, `Steer`, `KnowledgeSearch`, `AddSkillRepo`,
`Login`, …) plus `AgentEvent` (16 variants: the original stream plus
`SubagentSpawned`, `MessageDelivered`, `ModelChanged`, `Compacting`,
`Compacted`, `KnowledgeChanged`, `Harvesting`, `Harvested`,
`ToolCallPending`, …).

Modules living in agent because they had nowhere else to earn a crate
(G2): `knowledge/`, `repl.rs` (~1465), `loop_.rs` (~1543),
`supervisor.rs` (~1386), `skills.rs`, `peers.rs`, `compact.rs`,
`spill.rs`, `todo.rs`, `frame.rs`.

This is not a 129 KB `types.rs`. It is one crate that owns the loop, the
store, the REPL, the knowledge bundle, the peer router, compaction, and
the public command surface every UI imports.

### Why it matters

§1.5 forbade a dumping ground in `runtime`. The dump moved one hop. UIs
(`tui`, `rpc`, `signal`) all `use agent::{...}` for a supermarket of
types. Adding a slash command means `agent::lib.rs` + TUI `Command` +
RPC `Api` (G6). The core was capped; the core’s neighbour was not.

### Fix

Do **not** explode agent into `agent-types`, `agent-loop`,
`agent-knowledge`, … That is jarvis with more Cargo.toml.

Do:

1. Keep command structs next to the actor that handles them
   (`knowledge` types in `knowledge/types.rs` — already partly true;
   skills commands in `skills.rs`). `lib.rs` re-exports, it does not
   define.
2. Cap `AgentEvent` as “the session stream.” Knowledge/harvest events
   can be their own type the TUI also subscribes to (G6’s wire-visible
   registry). The tagged enum is allowed; stuffing every plugin’s
   lifecycle into it is not.
3. Knowledge stays in agent (G2). REPL stays in agent (it *is* the
   loop). That is allowed size if 00 says so: *agent is allowed to be
   large; runtime is not. New subsystems still earn a plugin.*
4. A one-line budget in 00 §10: *runtime ≤ 400. agent is the loop and
   what the loop must own (store, frame, compact, repl, knowledge).
   Frontends do not grow it.*

**Resolved (12 §8):** 00 §10 carries the budget sentence and the `lib.rs`
re-exports rule; command structs move next to their handler when the file
is next touched. Opportunistic, no date. Point 3 above is superseded:
knowledge is a plugin now (G2), which also took `KnowledgeChanged` out of
`AgentEvent`.

---

## G11. MCP names: 00 vs 01 vs the code

### The rule

00 §6, **still**: MCP tools register under `<server>__<tool>` with `Risk`
from config (default `Mutating`).

01 §1.4: *Rejected: prefixing (`server__tool`) — the model gains nothing
and prompts get longer.*

### What the code does

`crates/tools/src/mcp.rs` crate docs: *“each remote tool registers under
its own name (no prefix).”* README: *“tools registered without prefix.”*
`ToolSpec` in `tools/src/lib.rs` still says *“MCP tools are
`<server>__<tool>`.”*

Three texts, two answers. The code matches 01 and the README, not 00,
not `ToolSpec`’s doc comment.

### Why it matters

This one is doc rot, not a secret feature. It is included because 00 §6
is still false, and because no-prefix plus `gate()` (G1) is how two MCP
servers that both export `search` collide. Prefixing was the collision
plan. 01 rejected it without replacing the plan.

### Fix

1. Fix `ToolSpec`’s doc comment and 00 §6 to “no prefix,” pointing at 01.
2. Collision: last register wins (current `RegisterTool` replaces by
   name). Make that loud: on reconcile, if name N is owned by another
   server, log and skip, or namespace only the colliding ones. Write the
   choice in 01. Silent replace is the bug prefixing existed to avoid.
   *(Stale — see Resolved.)*

This is an amendment that already happened. Record it.

**Resolved (12 §1):** `ToolSpec` comment and 00 §6 fixed to “no prefix,”
pointing at 01. The collision claim above was stale at audit time:
`mcp.rs` already tracked names in a `taken` map, logged the clash and
skipped the loser (native wins, then first server by sorted name); nothing
was ever silently replaced. 00 §6 now says so.

---

## Deliberate rewrites (not on the gap list)

These changed the requirement in a later doc. Do not treat them as
silent drift; do make 00 point at the doc that won.

| Original 00 | Later | Where |
|---|---|---|
| config.toml | config.yaml | 01 §1 |
| blocking subagent = parent tool result | detached, result as a new turn | 01 §2 |
| MCP `<server>__<tool>` | no prefix | 01 §1.4 (see G11) |
| Tool/Risk in `runtime` | moved to `tools` | current 00 §4 (improvement) |
| Resume = load messages, rebuild request | epochs, compaction, append-only frames | 07 |
| All tools through registry | `todo` / `python` loop-local | 07/08 (see G8) |

---

## Suggested order of work

Executed as 12 §12 waves; differences: the guard/lint idea was dropped
(docs are the record, no scripts enforce them), knowledge became a plugin,
grep/edit were added.

Not by moral severity — by “restores a brake before the next plugin.”

1. **Honesty patch to 00** (G2 listing, G7 hot reload, G8 loop-local
   exceptions, G9 §1.8 rewrite, G11 prefix, G1’s “whole system”
   sentence). One commit, no behaviour change. After this, 00 is allowed
   to be cited again.
2. **G4 `lookup_when`** + delete the Signal order comment. Small, stops
   the DAG.
3. **G3 `--link-signal` into the plugin.** `main.rs` stops importing
   `signal::`.
4. **G1 `gate()` out, MCP gets Risk.** Behaviour change for MCP defaults
   (today: allow; after: `tools.policy` on `Mutating` = ask). Call it
   out in the changelog.
5. **G5 backend trait, kill `if ns == "cursor"`.**
6. **G6 §8 amendment + one command list.** Do this before any Tauri
   work (phase 4).
7. **G7 apply MCP reconcile at turn start only**, so 07’s tool list
   stays frozen for the turn.
8. **G10 `lib.rs` as re-exports, `AgentEvent` not a junk drawer.**
   Opportunistic, not a project.

Do not extract ten crates. Do not delete Signal. Do not delete
knowledge. The gaps are brakes that went unmaintained, not features that
should die.

---

## Cross-reference

Paths as of the audit; the Resolution table says where things moved
(`crates/agent/src/knowledge/` is now `crates/knowledge/`, `ErasedTool::gate`
is gone).

| Id | 00 clause | Primary files |
|---|---|---|
| G1 | §1.7, §4 permission | `crates/tools/src/registry.rs`, `crates/tools/src/lib.rs` (`ErasedTool::gate`), `docs/design/01-config-and-subagents.md` §1.4 |
| G2 | §1.6, §10, §13 | `Cargo.toml` members, `crates/agent/src/knowledge/`, `crates/signal/`, `crates/cursor/` |
| G3 | §1.4, §4 `main.rs` | `crates/wth/src/main.rs` (`link_signal`, `subprocess`, `process::exit`) |
| G4 | §4 start-order | `crates/signal/src/lib.rs` (`Plugin::start`), `crates/agent/src/supervisor.rs` (`for _ in 0..50`) |
| G5 | §1.2, §5 | `crates/agent/src/loop_.rs` (`ns == "cursor"`), `crates/cursor/src/lib.rs` |
| G6 | §8, §1.5, §7 | `crates/tui/src/model.rs` (`Command`, `UiEvent`), `crates/rpc/src/api.rs`, `crates/rpc/src/bridge.rs` |
| G7 | §9 | `crates/tools/src/mcp.rs` (`notify::recommended_watcher`) |
| G8 | §4 registry | `crates/agent/src/loop_.rs` (`local`, `todo::spec`, `repl::spec`) |
| G9 | §1.8, §1.1 | `crates/cursor/src/lib.rs`, `crates/agent/src/frame.rs` (`CLAUDE_CODE_IDENTITY`) |
| G10 | §1.5 | `crates/agent/src/lib.rs`, `crates/agent/src/{loop_,repl,supervisor,knowledge}` |
| G11 | §6 vs 01 §1.4 | `crates/tools/src/mcp.rs` header, `ToolSpec` doc comment, `README.md` |
