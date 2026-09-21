# 12 — Course correction: closing the design gaps

Status: shipped 2026-09-11 (steps 1–7, 9, 10; step 8 opportunistic) · executes [`../design-gaps-expanded.md`](../design-gaps-expanded.md) (G1–G11) against `00-architecture.md`. Every claim below was re-verified against the tree before the work; where the gaps doc is stale it says so. Verified headless over RPC on a fresh data dir: `list_tools` has `grep`/`edit`/`knowledge_*`; `knowledge_put/get/history/delete/restore` round-trip with gix commits and `KnowledgeChanged` on the event stream; `knowledge.enabled: false` removes the tools and RPC answers `knowledge is disabled`; `server-everything` tools land as `read_only (allow)` / `mutating (ask)` from their annotations, `set_mcp_gate` overrides to `destructive`, `off` unregisters, server `off` disconnects and clearing reconnects; `allow` is rejected as invalid. Not exercised: TUI `/mcp` and `/knowledge` on a pty, `--link-signal` pairing, a Cursor turn.

The diagnosis is not "the features are wrong." Signal, knowledge, cursor, hot reload, oauth were not in v0's phases; that made them additions past v0, not violations. The failure is that 00 stopped describing the tree and nothing noticed — jarvis died the same way: the postmortem stayed true while the tree moved. So the first deliverable is making 00 true again, and the rule going forward is that a feature doc amends 00 in the same commit. Enforcement is by review of the docs, not by scripts: the user's call — the record is the docs.

## 0. Rules for this plan

1. **No new feature doc (13+) until steps 1–6 land.** Steps 7–8 may interleave with features. Overridable by the user, not by momentum.
2. Every step ends with a runnable check (00 §12 style), not a test suite.
3. Every 00 amendment names the alternative it beat (00 line 9–10). A "we did it because it was needed" sentence is not an amendment.
4. No crate splits, no deletions of Signal / knowledge / cursor. The features stay; the constitution catches up (gaps doc, "Suggested order").
5. Each step is one commit, `cargo check` green.
6. No tooling enforces the docs (no lint recipe, no test that greps). When 00 and the tree disagree, the fix is a doc edit or a code edit, chosen by the reason — never a script.

## 1. Honesty patch to `00-architecture.md` (no behaviour change)

### 1.1 The edits

One commit, text only. Each edit points at the doc that holds the decision.

| Clause | Edit |
|---|---|
| §1.6, §10 "Six crates for v0" | Replace listing with the actual members (twelve after step 9), one line each: plugin or library, and which doc earned it (`models` 02, `markdown` 03, `oauth` 05, `cursor` 05 §2.5 → §5 of this doc, `signal` 09, `knowledge` 10 + 12 §9). Strike "Six crates". Add the budget sentence from G10: *runtime ≤ 400. agent is the loop and what the loop must own (store, frame, compact, repl); frontends do not grow it. New subsystems earn a plugin.* |
| §1.7 / §4 "That is the whole permission system for v0" | Replace with the §4 contract from step 5 below, marked *(target; today `ErasedTool::gate` still exists, see 12 §5)*. Struck for real when step 5 lands. |
| §1.8 "Not a coding agent" | *The tool set is what an assistant needs to act on the user's machine: shell, files (read, write, list, `grep`, `edit`), MCP, python, and whatever plugins register. We do not ship coding-specific natives (LSP, notebooks, git, diff review). Providers may be whoever mints the token; a provider-required identity line (05 §2.3) is not product branding and is not shown for key auth.* The gaps doc's "no grep/edit" reading of §1.8 was wrong: any assistant that touches files needs to find text and change part of a file; today the only edit is `write_file` replacing the whole file (step 10). |
| §4 "never at start" | *A plugin may wait for a named actor at start (`Runtime::lookup_when`, step 2); it must not assume vector order.* |
| §4 registry paragraph | Add G8 exception: *`todo` and `python` are loop-local (07 §5, 08). `todo` is turn state, not a capability. Every host call `python` makes is `Invoke` and takes the gate. A third loop-local name amends this sentence first. Test: if it can affect the world without `Invoke`, it is a bug.* |
| §5 | Add the G5 reason: *Cursor's AgentService is Connect-RPC/protobuf over HTTP/2; genai has no adapter and the protocol is not OpenAI-shaped. Crate `cursor` is that adapter (05 §2.5), reached only through `agent::backend` (12 §6). Rejected: upstreaming to genai (protocol unstable); tonic (protoc, heavy).* |
| §6 MCP | `<server>__<tool>` → *no prefix (01 §1.4). Clash: native wins, then first server by sorted name; the loser is logged and skipped (`mcp.rs` `taken`), never silently replaced.* (The gaps doc's G11 "last register wins" is stale: the code already logs and skips.) `Risk` from config → *Risk from the server's tool annotations, overridable in `mcp/config.yaml` (12 §5).* |
| §9 | Add G7: *`mcp/` is watched (`notify`, 300 ms debounce). Skills and knowledge resync at turn start by mtime. The direct tool list is read once per turn (12 §4).* |
| §13 | Rewrite: Signal returned as a plugin (09) — alternative in-agent module rejected, only plugins may return. OKF returned as the `knowledge` plugin (10, 12 §9): the store, index, history and tools are the plugin; the harvest *model call* stays in the agent because it is a continuation of the live frame (07) — alternative keep-it-in-agent rejected by the user (G2). Strike "not planned". |
| `crates/tools/src/lib.rs:67` | `ToolSpec.name` doc comment: drop `<server>__<tool>`. |

Check: `grep -c "12 §" docs/design/00-architecture.md` ≥ 5; 00 contains no "Six crates", "not planned", `<server>__<tool>`, "ordinary bus subscriber", "never at start".

## 2. G4 — `Runtime::lookup_when`

Target: `crates/runtime/src/lib.rs`, `crates/signal/src/lib.rs:319-335`, `crates/agent/src/supervisor.rs:1496-1538`.

```rust
impl Runtime {
    /// `ActorRef::<A>::lookup(name)` until it resolves or `shutdown` fires.
    /// Polls every 50 ms: kameo's registry has no watch API.
    pub async fn lookup_when<A: Actor>(&self, name: &'static str) -> Option<ActorRef<A>>;
}
```

- Signal: delete the `main.rs order` comment and the `match lookup`; `tokio::spawn` a task that awaits `lookup_when::<ToolRegistry>(REGISTRY_ACTOR)` then the two `RegisterTool`s.
- Agent: the `for _ in 0..50 { … sleep(100ms) }` body becomes the same `lookup_when` await; the eight `RegisterTool`s and `knowledge::tools::register` are unchanged.
- Approval lookup on first `Ask` (`registry.rs:200`) stays as is: it registers nothing.

Alternative rejected: `RegistryReady` event — subscribers would have to subscribe before tools publishes it, which is the same order coupling. Alternative rejected: registry handle on `Runtime` — god-wiring in the other direction.

Check: swap `signal` before `tools` in `main.rs`'s plugin vector, run, `/mcp`-style `list_tools` over RPC shows `signal_link` and `signal_status`; swap back. `grep -rn "main.rs order" crates/` is empty.

## 3. G3 — pairing moves into the Signal plugin

Target: `crates/theuth/src/main.rs:127-178, 219-223`, `crates/signal/src/lib.rs`.

- `SignalPlugin` gains `link: Option<LinkOpts { owner: Option<String>, device_name: Option<String> }>`.
- `SignalPlugin::start` with `link: Some(_)`: run today's `link_signal` body (QR, `e164`, `write_config_value` on `signal.owner`/`signal.device_name`), then `rt.shutdown.cancel()`, return `Ok(())`.
- `main.rs`: delete `fn link_signal`; when `cli.link_signal`, the plugin vector is `vec![Box::new(SignalPlugin { channel: false, link: Some(..) })]` and nothing else (no TUI, no agent, no tools — pairing needs none). Building the vector from CLI flags is the binary's job (gaps doc G3, "That part is ordinary").
- `main.rs` no longer imports anything from `signal::` except `SignalPlugin`/`LinkOpts`.

Leave in `main.rs`, documented in 00 §2 as deliberate: the `subprocess` argv branch before clap and `#[global_allocator]` (the Python worker *is* this binary, 08 §9), `first_run` (data-dir skeleton), the `sleep(100ms); process::exit(0)` (known gap, 00 §12 — a stuck `spawn_blocking` drain; fix the drain later, do not call it architecture).

Check: `t --link-signal --owner +1555…` still shows a QR and writes `signal.owner`; `theuth` normal start unchanged. `main.rs` names nothing from `signal::` but `SignalPlugin`/`LinkOpts`.

## 4. G7 — direct tool list frozen per turn

Target: `crates/agent/src/loop_.rs:645-697` (`Turn::tools`), `Turn::rebase`, `tools_seen`, `reset_this_round`.

Today `tools()` runs `ListTools` every round, hashes the result and `warn!`s when it moved. 07 §0's invariant says the tool list is part of the frozen prefix; the code polices the violation instead of preventing it.

- `Turn::rebase` (turn start, where the baseline is rendered) calls `ListTools` once and stores the built `Vec<genai::chat::Tool>` on the turn. Every round uses it.
- Delete `tools_seen`, the hash diff, the `warn!`, and `reset_this_round`'s tools cause (~35 lines). `frame::tools_hash` goes if nothing else uses it.
- 01 §1.4 and 07 §5: "a server added mid-turn is visible on the next round" → *next turn*. With the runtime on (the shipped default, README), MCP tools are `direct: false` and were never in this list anyway; the observable change is only for `--no-runtime`-style configs and for native tools registered late at startup (which step 2 makes race-free in practice).

Alternative rejected: dirty flag from the watcher applied at rebase (gaps doc G7.2) — more state for the same result; fetching once per turn is the freeze.

Check: start `theuth`, type before MCP servers connect, run a multi-round turn; log has no `tools changed mid-turn` (the string is gone from the code); `TurnFinished` cache ratio on round 2+ is non-zero.

## 5. G1 — one permission contract: `Risk → PolicyAction`

Target: `crates/tools/src/lib.rs` (`ErasedTool::gate`), `crates/tools/src/registry.rs:189`, `crates/tools/src/mcp.rs` (`Gates`, `ServerGate`, `McpTool`, `SetGate`, `McpToolInfo`), `crates/tui/src/model.rs` (`/mcp` Space cycle), `crates/rpc/src/api.rs` (`set_mcp_gate`), 01 §1.4, 02 §4–5, README.

Today an MCP tool is `Risk::Mutating` in its `ToolSpec` and `gate() = Some(Allow)` — the approval modal would say *mutating* for a tool the registry never asks about. Two constitutions for one disk write.

**Contract (00 §4 replacement text):**

> One contract: `Risk → PolicyAction` from `tools.policy`. Two UI switches rewrite `Ask` to `Allow` and nothing else: the persisted `tools.approvals: false` and the in-memory per-session set (inherited by subagents through `SetParent`). `Deny` is never rewritten. MCP config assigns a tool's `Risk` or hides it; it is not a second policy.

**Code:**

1. `McpTool.spec.risk` comes from the remote tool's annotations (`rmcp::model::ToolAnnotations`, verified in the vendored source): `read_only_hint == Some(true) → ReadOnly`, `destructive_hint == Some(true) → Destructive`, else `Mutating`. Note the spec default for an *unset* `destructive_hint` is `true` (rmcp `is_destructive()`); we deliberately read unset as `Mutating`, not `Destructive`: most servers set nothing, and `tools.policy.mutating = ask` already prompts. A server that means it says so.
2. `mcp/config.yaml` values become `Risk` or `off`:
   ```yaml
   servers:
     everything:
       risk: read_only            # every tool of this server; default: annotations
       tools:
         longRunningOperation: off  # unregistered, model never sees it
     filesystem:
       risk: off                  # not connected
   ```
3. Delete `ErasedTool::gate` and `McpTool.gate`. `registry.rs:189` becomes `let mut action = self.policy.action(spec.risk);`.
4. `SetGate.action: Option<PolicyAction>` → `Option<Gate>`; `McpToolInfo { gate, effective }` → `{ risk_override: Option<Gate>, risk: Risk, action: PolicyAction }` (the last computed from `tools.policy`, so the menu can still show `ask`/`allow`). `/mcp` Space cycles `read_only → mutating → destructive → off`; the row shows the resulting action in parentheses.
5. No migration for existing `mcp/config.yaml` files: `allow`/`ask`/`deny` fail to parse → the existing "gates invalid, all default" warning fires, naming the file. The gates file has existed for two days.

**Behaviour change, deliberately:** an MCP tool without annotations prompts by default (`tools.policy.mutating = ask`), where today it runs silently. A user who trusts a server sets `risk: read_only` on it once. README and 01 §1.4 say so in one sentence each.

Alternative rejected: keep `allow|ask|deny` words and translate to Risk internally — the second policy in costume; the file would still say `allow` where the contract says `Risk`. Alternative rejected: `gate()` kept and documented — that is what the gaps doc names as the failure path.

Check (headless RPC, `server-everything`): fresh dir → `echo` (no annotations) yields `ApprovalRequested` with `risk: mutating`; `set_mcp_gate(everything, risk: read_only)` → next `echo` runs unprompted; `set_mcp_gate(everything, echo, off)` → gone from `list_tools`; `/approvals off` → mutating tools run without a prompt, a `tools.policy.destructive = deny` tool still denied. `ErasedTool::gate` no longer exists.

## 6. G5 — one backend seam, `if ns == "cursor"` gone from the loop

Target: `crates/agent/src/loop_.rs:615-639` (`transport`), `:701-810` (`call`), `crates/agent/src/providers.rs:45`, new `crates/agent/src/backend.rs`.

```rust
// agent::backend
pub enum Backend { Genai(Arc<genai::Client>), Cursor { oauth: Arc<oauth::Store> } }
impl Backend {
    /// Chosen when the model is set, never per call.
    pub fn for_model(resolved: &Resolved, shared: &Shared) -> Backend;
    pub fn transport(&self, resolved: &Resolved) -> Transport;   // Cursor → Plain
    pub async fn stream(&self, resolved: &Resolved, req: ChatRequest, opts: &ChatOptions)
        -> Result<EventStream, String>;
}
```

- `Session` holds `backend: Backend`, rebuilt in the `SetModel` handler alongside `Resolved`.
- `loop_.rs` loses both `ns(&model.genai) == "cursor"` sites; `call()` is `self.backend.stream(..)`.
- `crates/cursor` stays a crate (generated proto is loud inside `agent/src`); `agent/Cargo.toml` still depends on it, but the only `use cursor::` in `agent` is in `backend.rs`. That satisfies §1.6's intent: a crate the agent owns as an adapter, reached through one seam.
- `providers.rs:45` (`name == "cursor"` in the configured predicate) stays: that is *which namespaces have a backend*, and it is the one place that knowledge belongs. Comment says so.

Alternative rejected: `trait ChatBackend` + `Arc<dyn>` (gaps doc G5.1) — two impls, closed set; an enum is one `match`, no vtable, no `async fn` in trait. Alternative rejected: fold `crates/cursor` into `agent/src/cursor/` — moves 1.5k generated lines for no seam change.

Check: a turn on `anthropic/…` and a turn on `cursor/<slug>` both stream; `/model` mid-session between them works. `loop_.rs` has no `"cursor"` literal; `backend.rs` is the only agent file naming `cursor::`.

## 7. G6 — §8 tells the truth; Tauri is gated

Target: `00-architecture.md` §8, §7. No code.

§8 replacement:

> The TUI is a control plane over the same actor commands RPC exposes. It subscribes to bus events for the transcript, approvals, MCP progress and logins, and `ask`s named actors for everything else (`tui::Command`, 36 variants today). It is not a thin subscriber. Tauri uses RPC (`rpc::Api`, 51 methods); the TUI uses in-process asks of the same command types. The two lists are maintained by hand and drift; **phase 4 (Tauri) does not start until they are one list.** Alternative rejected: TUI as a jsonrpsee client of localhost (00 §2's serialization cost on the chat path; acceptable for slash commands, but not worth doing before a third frontend exists).

Why not unify now: `Command` exists because iocraft hooks are render-only and the command runner is a separate task fed by an mpsc; `Api` exists because jsonrpsee needs a trait. Collapsing them is a real refactor with one beneficiary (Tauri) that has no date. Writing the gate is the brake; the refactor is phase-4 step 1. `forward!(…)` in `rpc/src/bridge.rs` and the five `rt.subscribe` lines in `tui/src/lib.rs` are the same story: six types, listed twice, unified when a third consumer appears.

Check: 00 §8 no longer says "ordinary bus subscriber" and names the phase-4 gate.

**Resolved (2026-09-11):** the gate was lifted by doing the refactor. `rpc::Api` is the one list of harness commands; `rpc::api::Server` implements it as lookup + one `ask` per method. The TUI holds a `Server::new(rt)` in-process (fresh broadcast sender, no bridge, no socket) and its `execute` arms call the trait methods directly — plain async fn calls, no JSON; it keeps its own bus subscription for events so `TextDelta` never serializes. Tauri will call the same trait over the WebSocket. One method was added to `Api`, `mcp_status` (`ask::<Mcp, _>(MCP_ACTOR, tools::Progress)`), to cover the one ask only the TUI made; the TUI's actor lookups and message imports are gone (`tui` depends on `rpc`, not on the actor crates' command structs). Errors surface as `e.message()` → `UiEvent::Error` (`-32001` not running, `-32000` ask failed). Rejected: a `TypeId` registry both facades consume — reflection machinery for what a trait already does — and the TUI as a jsonrpsee client of localhost (00 §2 cost, RPC must always listen). 00 §8 carries the shipped text.

## 8. G10 — `agent/src/lib.rs` as re-exports (opportunistic)

Target: `crates/agent/src/lib.rs` (722 lines, 33 command structs, `AgentEvent` 16 variants).

Rule, added to 00 §10 by step 1: `lib.rs` re-exports; command structs live next to the handler that answers them (`knowledge/types.rs` already does this; skills, logins, providers, compaction follow when next touched). `AgentEvent` is *the session transcript stream*: a variant earns a place if a transcript renders it. No sweep; each file moves when it is edited for another reason.

No check beyond review. Not a step with a date.

## 9. Knowledge becomes a plugin (`crates/knowledge`)

User decision, overriding the gaps doc's "keep in agent". The coupling argument (harvest is a cache-hot continuation of the frame) is real and is answered by the cut: the **model call** stays in the agent loop, which owns the frame; everything else — bundle, FTS index, gix history, `knowledge_*` tools, the instruction/parse pure functions, the `KnowledgeChanged` event, config — moves. Nothing crosses the bus that did not already cross an `Arc<Knowledge>` method boundary.

### 9.1 Crate

`crates/knowledge`, plugin. Dependency order becomes `runtime ← tools ← knowledge ← agent ← {tui, rpc, signal} ← theuth`. Moved verbatim from `crates/agent/src/knowledge/`: `store.rs`, `repo.rs`, `types.rs`, `harvest.rs`, `tools.rs` (+ their tests). `okf`, `gix` deps move with them.

```rust
// knowledge (lib.rs)
pub const KNOWLEDGE_ACTOR: &str = "knowledge";
pub struct KnowledgePlugin;                       // impl runtime::Plugin; name() = "knowledge"
#[derive(Deserialize)] #[serde(default)]
pub struct KnowledgeConfig { pub enabled: bool }  // section `knowledge:`; default true

/// Actor registered as KNOWLEDGE_ACTOR; wraps the store.
pub struct KnowledgeActor { k: Arc<Knowledge> }
pub struct Handle;                                // ask → Arc<Knowledge>: the loop's hot path (hint/sync/put per round, no mailbox hop)
pub struct Search  { pub query: String, pub limit: Option<u32> }         // ask → Result<Vec<Hit>, String>
pub struct Get     { pub id: String }                                   // ask → Result<Concept, String>
pub struct List    { pub prefix: Option<String>, pub type_: Option<String>, pub tag: Option<String>, pub limit: Option<u32> } // ask → Result<Vec<Meta>, String>
pub struct PutConcept { pub put: Put, pub source: Source }              // ask → Result<Meta, String>
pub struct Delete  { pub id: String }                                   // ask → Result<(), String>
pub struct HistoryOf { pub id: Option<String>, pub limit: Option<u32> } // ask → Result<History, String>
pub struct Restore { pub id: String, pub commit: Option<String> }       // ask → Result<Change, String>

/// Event (own struct, not an AgentEvent variant): a concept was written or removed.
#[derive(Clone, Serialize, Deserialize, JsonSchema)] #[cfg_attr(feature = "ts", derive(TS))]
pub struct KnowledgeChanged { pub id: String, pub op: KnowledgeOp, pub source: Source }

pub use types::*;                                  // Meta, Concept, Hit, Put, Source, KnowledgeOp, Sync, HarvestResult, Change, History
pub use store::{Knowledge, terms};
pub mod harvest;                                   // instruction, render_hints, render_related, parse_harvest
pub mod tools;                                     // GET: &str = "knowledge_get" and the seven tools
```

`KnowledgePlugin::start`: `rt.section::<KnowledgeConfig>("knowledge")`; disabled → return `Ok(())` (no actor, no tools — the agent's lookup finds nothing and hints/harvest are off). Else `Knowledge::open(&rt.data_dir, rt.clone())`, spawn `KnowledgeActor`, `register(KNOWLEDGE_ACTOR)`, then a task: `rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR)` → `tools::register` (the seven `RegisterTool`s, unchanged). The store publishes `KnowledgeChanged` through the `Runtime` it already holds. Store handlers await on the mailbox (local I/O), as the supervisor's did.

### 9.2 Agent

- Delete `agent/src/knowledge/`, `pub mod knowledge`, the seven `Knowledge*` command structs in `lib.rs` and their `Agents` handlers, `Agents::knowledge()`, `AgentEvent::KnowledgeChanged`. `Harvest { session }`, `HarvestNow`, `Harvesting`, `Harvested` stay: they are a session's model call.
- `Shared.knowledge: Option<Arc<Knowledge>>` is filled per turn in `Turn::rebase`: `ActorRef::<KnowledgeActor>::lookup(KNOWLEDGE_ACTOR)` then `ask(Handle)`; `None` → knowledge off. First use, not start. Everything downstream (`sync`, `hint`, `put`, `harvest::instruction`, `parse_harvest`, `render_hints`) is unchanged code against `knowledge::` paths.
- `KNOWLEDGE_GET` const → `knowledge::tools::GET`.
- Config: `agent.knowledge` keeps `{ hint_limit, hint_rounds, harvest_every, harvest_max }` (loop policy) and loses `enabled`; `AgentConfig.knowledge: KnowledgeUse` is defined in `agent/src/lib.rs`. `knowledge.enabled` lives in the plugin's section. Alternative rejected: one `knowledge:` section read by two plugins — 00 §9 says each plugin reads its own.

### 9.3 Frontends, binary, docs

- `tui`: `Command::Knowledge{List,Search,History,Restore}` ask the knowledge actor (`lookup(KNOWLEDGE_ACTOR)`, `Err("knowledge is disabled")` when absent); `Command::Harvest` still asks the agent. `use knowledge::{Change, HarvestResult, History, Meta}`; `HarvestResult` stays where it is defined (`knowledge::types`), re-exported by `agent` only if the agent's own `Harvest` reply needs it — it does, so `agent` depends on `knowledge` and returns `knowledge::HarvestResult`.
- `rpc`: `knowledge_*` methods ask the knowledge actor; `knowledge_harvest` still asks the agent; `forward!` gains `KnowledgeChanged`; `export.rs` paths become `knowledge::…`; feature `ts` forwards to `knowledge/ts`.
- `theuth/main.rs`: `Box::new(knowledge::KnowledgePlugin)` after `tools`, before `agent` (order is convention only after step 2); first-run `config.yaml` comment block gains `knowledge:\n  enabled: true` and drops `enabled` from `agent.knowledge`.
- Docs: 10 §0.2 ("Module `agent::knowledge`" → crate `knowledge`, plugin; config split), 10 §6 heading, 10 §8 (`KnowledgeChanged` is the plugin's own event), README crate list + config block, 00 §10 listing (step 1 text already names it).

Check: `knowledge_search` over RPC on a fresh dir returns `[]`; a turn that reads a file three times triggers a harvest (`Harvested` event, a `put <id>` commit in `~/.theuth/knowledge/.git`); `knowledge.enabled: false` → `list_tools` has no `knowledge_*`, a turn runs with no `<knowledge>` note and no harvest; `/knowledge` in the TUI lists concepts. `crates/agent/src/knowledge/` is gone; `crates/knowledge/src/lib.rs` has `impl runtime::Plugin for KnowledgePlugin`.

## 10. `grep` and `edit` natives (`crates/tools/src/fs.rs`)

User decision: §1.8 rules out *coding-specific* tools, not searching and editing files. Today search is `shell` + whatever `grep` the box has, and a one-line change is `read_file` → `write_file` of the whole file (64 KiB read cap, whole-file approval diff). Two tools, same module as `read_file`, same `Tool` trait, registered as builtins in `ToolRegistry::start` next to the others.

```rust
// grep — Risk::ReadOnly
struct GrepArgs { pattern: String /* regex */, path: Option<String> /* file or dir; default cwd */, glob: Option<String> /* e.g. "*.rs" */, limit: Option<u32> /* matches; default 200 */ }
// -> "path:line: text" per match, sorted by path; respects .gitignore and skips hidden dirs; "no matches" when none.
// edit — Risk::Mutating
struct EditArgs { path: String, old: String, new: String, all: bool /* default false */ }
// -> `old` must occur exactly once (or ≥1 with `all`); otherwise Err naming the count and nothing is written.
//    "edited <path>: 1 replacement" ; file rewritten atomically (tmp + rename), bytes outside the match untouched.
```

Library, not reinvention (00 §1.2): `ignore` for the walk (gitignore, hidden, glob override) and `grep-regex` + `grep-searcher` (ripgrep's own crates) for matching — no hand-rolled walker or line splitter. `edit` is `str::match_indices` + `replacen`; nothing to import. Alternative rejected: shelling out to `rg`/`grep` — not guaranteed present, output shape varies, quoting hazards. Alternative rejected: line-number-anchored edits (this harness's own `edit`) — the model has no stable line view of a file it just changed; exact-string is the boring, self-checking form.

`describe` in the TUI already renders `key: value` per argument, so an `edit` approval shows `old`/`new` verbatim. 00 §6 gains one sentence listing the five fs tools; README's crate line likewise.

Check: `grep pattern="lookup_when" glob="*.rs"` returns the three call sites; `edit` with an `old` that occurs twice returns `Err` and leaves the file byte-identical; with `all: true` replaces both; a successful `edit` prompts (Mutating) and shows `old`/`new` in the modal.

As-built 2026-09-21: a `read_file` cut at 64 KiB is marked at both ends — a `[read_file: …]` header with exact byte/line counts of the selection and the `[truncated …]` tail (`fs::TRUNCATED_HEAD_PREFIX`/`TRUNCATED_TAIL`) — after a 74 KB doc was read, sliced in Python and written back minus its tail.
`write_file` and `edit` (`new`) refuse content carrying either marker; nothing is written. No `structured` flag: the Python runtime returns `structured` *instead of* the text (`agent/src/repl.rs::tool_result`), so a flag there would hide the content.

## 11. Not doing

- Splitting `agent` further. It is the loop plus what the loop must own; 00 §10 now says so (step 1).
- Deleting `cursor`. The login does work; step 6 gives it a seam and 00 §5 a reason.
- Per-session bypass and parent inheritance in the registry. They are the documented `Ask → Allow` rewrite, one function (`bypassed`), and the contract in step 5 names them.
- Fixing the `process::exit` after sleep, unbounded mailboxes, session-actor eviction, `setsid` escaping `killpg`, RPC token `==`. All still listed in 00 §12 as known gaps; none is a constitution failure.

## 12. Order and size

| Step | Behaviour change | Crates touched | Size |
|---|---|---|---|
| 1 honesty patch | none | 00, `tools/lib.rs` comment | text |
| 2 `lookup_when` | none | runtime, signal, agent | ~30 net, −20 |
| 3 link-signal | none | theuth, signal | move ~50 |
| 4 tools per turn | MCP visible next turn not next round (runtime-off only) | agent | −35 |
| 5 Risk contract | **MCP defaults prompt** | tools, tui, rpc, docs | ~150 |
| 6 backend enum | none | agent | ~80, −2 branches |
| 9 knowledge plugin | config key moves (`knowledge.enabled`) | knowledge (new), agent, tui, rpc, theuth, docs | move 3.3k, ~200 new |
| 10 grep + edit | two new native tools | tools, 00 §6, README | ~120 |
| 7 §8 + Tauri gate | none | 00 | text |
| 8 lib.rs re-exports | none | agent | as touched |

Wave 1, parallel: {1} · {2+3, one owner: both edit `signal/src/lib.rs` `start`} · {4+6, one owner: both edit `loop_.rs`} · {5}. Wave 2, after 1 is green (needs `lookup_when` and a stable `loop_.rs`): step 9 as three parallel owners against the §9.1 contract — the crate, the agent cutover, the frontends+binary+docs — plus step 10 (`tools/fs.rs`, free once step 5 has released `crates/tools`). Wave 3: 7, then the final gates.
