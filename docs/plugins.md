# Plugins: design, implement, integrate

Status: guide · 2026-09-11 · derives from `design/00-architecture.md` §3–4, `design/12-course-correction.md`. The worked example throughout is `crates/knowledge` (design doc 10, cut into a plugin in 12 §9); read its `src/lib.rs` (≈280 lines) alongside this.

A plugin is a workspace crate that exports one type implementing `runtime::Plugin`. The binary holds a `Vec<Box<dyn Plugin>>` and nothing else. Plugins talk to each other only two ways: **events** on the typed kameo bus (fire-and-forget, zero or more subscribers) and **commands** (`ask` on a named actor found through the kameo registry). There is no shared event enum, no shared config struct, no handle-threading through `main`.

## 1. Should this be a plugin?

00 §1.6: *a subsystem earns a crate only when the current phase needs it.* Decide with this table before writing code:

| You are adding… | It goes… | Why |
|---|---|---|
| A capability the model calls (`grep`, `signal_link`) | a `Tool` in the plugin that owns the state it touches; native fs/shell tools in `tools` | tools are cheap; crates are not |
| A change to how turns run (hints, compaction, cadence) | `agent` | the loop owns the frame (07); a boundary there is costume |
| A pure library with no actors, no config, no events (`models`, `markdown`, `oauth`, `cursor`) | a library crate, listed in 00 §10 as such | nothing to start, nothing to subscribe |
| A subsystem with its own state on disk, its own actor(s), its own tools and/or events, its own config section (`knowledge`, `signal`, `tools`, `rpc`, `tui`) | **a plugin** | this guide |
| Another frontend (Tauri) | RPC client of `Api`; not a plugin | |

Two tests. *Does it need to exist before `main` starts anything else?* — no plugin does; if yours does, it is part of `runtime` or the binary, and that needs a written reason. *Could a user turn it off and still have a working harness?* — a plugin can be left out of the vector or disabled by its config; if the answer is no, it is not a plugin, it is the loop.

## 2. Design first: the doc

Every plugin has a numbered design doc in `docs/design/` **before** the crate exists, and the doc and the code land in the same commit series. 00 is the constitution; when your plugin changes something 00 says, amend 00 in the same commit (12 §0). What the doc must contain, in the shape the existing ones use (01, 05, 09, 10):

1. `Status:` line with date; which docs it extends; where it wins on disagreement.
2. **Decisions that name the alternative they beat.** "We used X because it was needed" is not a decision (00 line 9–10). Library choices: cite the crate and why not the others (00 §1.2: off-the-shelf first; a custom implementation needs a written reason).
3. The **config section** (`foo:` in `config.yaml`) with every key, default, and what `enabled: false` leaves behind.
4. The **commands** (`ask` messages, reply types) and **events** (structs, when published), exactly as the code will declare them — other plugins code against this contract.
5. The **tools** (name, `Risk`, args, what the model gets back), if any.
6. The **UI surface**: slash commands, RPC method names, what the TUI renders.
7. **Data on disk** under `<data_dir>/`: files, formats, what may be deleted and rebuilt.
8. A **runnable check** per implementation step (00 §12 style), and a **Status** section filled in after verification saying what was exercised and what was not (01 §4, 02 §7, 12 header are the pattern).
9. Amendments to 00: add your crate to §10 with "plugin" or "library" and the doc number; if 00 said the thing was "not in v0", add the row to §13's returned-as table.

Not in the doc: a roadmap of later phases for the plugin. Later can write its own doc.

## 3. Anatomy of a plugin crate

```
crates/foo/
  Cargo.toml
  src/lib.rs        // constants, config, actor + messages, events, Plugin impl
  src/<state>.rs    // the thing the actor wraps
  src/tools.rs      // Tool impls + `register()`, if any
```

### 3.1 `Cargo.toml`

```toml
[package]
name = "foo"
version.workspace = true
edition.workspace = true
publish.workspace = true
description = "One line, named by function (00 §1.1)"

[dependencies]
runtime = { workspace = true }
tools = { workspace = true }          # only if you register tools or invoke them
kameo = { workspace = true }
tokio = { workspace = true }
async-trait = { workspace = true }
anyhow = { workspace = true }
serde = { workspace = true }
schemars = { workspace = true }       # wire-visible types
tracing = { workspace = true }
ts-rs = { workspace = true, optional = true }

[features]
ts = ["dep:ts-rs", "runtime/ts", "tools/ts"]   # forward to every crate whose types you embed

[lints]
workspace = true                       # missing_docs = warn, clippy::all = deny
```

Add `foo = { path = "crates/foo" }` to the root `[workspace.dependencies]`; `members = ["crates/*"]` already globs the directory. New third-party crates go in the root `[workspace.dependencies]` with a one-line comment saying which doc chose them.

**Dependency order is acyclic and downward only**: `runtime ← tools ← knowledge ← agent ← signal ← rpc ← tui ← wth` (`rpc` sits above every plugin because `Api` names all their command types; `tui` calls `Api`, so it sits above `rpc`). Depend on another plugin's crate for its *types* (message structs, events, `Tool` trait) — never to call functions on its state. If you need a crate that depends on you, you have the arrow backwards: the thing you want from it is a message.

### 3.2 `lib.rs` skeleton

```rust
//! Foo plugin: one sentence (design doc NN).
//!
//! Public surface consumed by other plugins:
//! - commands on the [`FOO_ACTOR`] actor ([`FooActor`]): [`Ping`], …
//! - events: [`FooChanged`]

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

/// Registry name of the [`FooActor`].
pub const FOO_ACTOR: &str = "foo";

/// `foo:` config section (NN §x).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FooConfig {
    /// `false`: the plugin starts nothing.
    pub enabled: bool,
}
impl Default for FooConfig { fn default() -> Self { Self { enabled: true } } }

/// Event: something happened (NN §y). Own type; never a variant of another plugin's enum.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]                       // wire shape is { "type": "FooChanged", ... }
pub struct FooChanged { /// … 
    pub id: String }

/// The [`FOO_ACTOR`] actor.
#[derive(kameo::Actor)]
pub struct FooActor { state: std::sync::Arc<State> }

/// `ask` → `Result<String, String>`: … (doc comment states the reply type; every message does)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ping { /// …
    pub text: String }

impl Message<Ping> for FooActor {
    type Reply = Result<String, String>;
    async fn handle(&mut self, m: Ping, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.state.ping(&m.text).await.map_err(|e| format!("{e:#}"))
    }
}

/// The plugin.
pub struct FooPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for FooPlugin {
    fn name(&self) -> &'static str { "foo" }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        // A missing section is Err (figment), not Default — always fall back.
        let cfg: FooConfig = rt.section("foo").unwrap_or_else(|e| {
            tracing::debug!("no [foo] config section, using defaults: {e}");
            FooConfig::default()
        });
        if !cfg.enabled { tracing::info!("foo disabled by config"); return Ok(()); }

        let state = match State::open(&rt.data_dir, rt.clone()).await {   // state publishes via rt.publish
            Ok(s) => s,
            Err(e) => { tracing::warn!("foo unavailable: {e:#}"); return Ok(()); }  // degrade, don't kill startup
        };
        let actor = FooActor::spawn(FooActor { state: state.clone() });
        actor.register(FOO_ACTOR)?;                    // duplicate name → Err; name freed when the actor stops

        // Anything that needs another plugin: wait for it in a task, never block `start`.
        let rt2 = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt2.lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR).await {
                crate::tools::register(&reg, state).await;
            }
        });
        Ok(())
    }
}
```

Rules encoded above, each of which the tree has been burnt by once:

- **`start` returns fast.** Spawn, register, return. Connecting to things, syncing indexes, waiting on other plugins all happen in spawned tasks (`tools::mcp` is the reference: startup does not wait for one MCP server).
- **Never assume vector order.** Another plugin's actor may not exist yet. Registering something *on* it at start → `rt.lookup_when::<A>(NAME)` in a task (polls until it exists or shutdown). Merely *calling* it → `ActorRef::<A>::lookup(NAME)` at first use, as `ToolRegistry` does for `Approval`. No retry loops of your own, no comments about `main.rs` order.
- **Disabled or broken → `Ok(())` with a log line**, not `Err`. The rest of the harness must come up. Other plugins learn you are absent because the registry lookup fails; make their error text say so (`"knowledge is disabled"`).
- **One `Runtime` clone into anything that publishes.** `rt.publish(event)` is the only way events leave a plugin.
- **Messages are documented as `ask → Reply`.** Handlers that await local I/O may do so on the mailbox; handlers that await another actor or a network **must leave the mailbox** (`ctx.reply_sender()` + `tokio::spawn`, see `ToolRegistry`'s `Invoke`) or a slow peer stalls every caller.

### 3.3 Events

- One struct (or one `#[serde(tag = "type")]` enum per *stream* the UI consumes with one `match`, as `AgentEvent`) per event, owned by the publishing plugin. Never add a variant to another plugin's enum.
- Wire-visible = `Clone + Serialize + Deserialize + schemars::JsonSchema` + `#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]` + `#[serde(tag = "type")]` on structs. Without the tag the RPC stream emits an anonymous object (found in 12 §9's smoke test).
- Payloads are cloned once per subscriber: deltas carry `Arc<str>`; big non-`Clone` things go as `Arc<T>`.
- Session-scoped events carry `session: SessionId` so `subscribe_events(session)` can filter.
- Subscribing: your actor must use the unbounded mailbox — `Foo::spawn_with_mailbox(args, runtime::subscriber_mailbox())` — then `rt.subscribe::<Foo, T>(&actor).await` per type. Guaranteed + unbounded is the only combination that is ordered, lossless and non-blocking (00 §3); a subscriber that stops draining grows without bound, which is the loud failure we want.

### 3.4 Commands

- One named actor per plugin is the norm (`"tools"`, `"approval"`, `"mcp"`, `"agent"`, `"knowledge"`). Constants `pub const FOO_ACTOR: &str`.
- Message structs live next to the actor that answers them and are `pub`; frontends import them from your crate. Serde-derive them: RPC hands them JSON.
- Reply types that are your own structs need `kameo::Reply` (derive) — see `knowledge::types`. `Result<T, String>` is the convention for fallible asks; the `String` is what the user sees.
- A hot path that would otherwise `ask` many times per turn may hand out a handle once (`knowledge::Handle` → `Arc<Knowledge>`); document that it exists and why.

### 3.5 Tools

```rust
pub struct PingTool(Arc<State>);

#[async_trait::async_trait]
impl tools::Tool for PingTool {
    const NAME: &'static str = "foo_ping";
    const RISK: tools::Risk = tools::Risk::ReadOnly;   // ReadOnly | Mutating | Destructive — the ONLY permission input
    type Args = PingArgs;                              // #[derive(Deserialize, JsonSchema)]; field docs become the schema
    fn description(&self) -> String { "…".into() }    // pure function of config: no clock, session, roster (07 §2.2.5)
    async fn call(&self, args: PingArgs, cx: tools::ToolCx) -> anyhow::Result<tools::ToolOutput> {
        Ok(tools::ToolOutput::ok(text))                // errors the model should see: ToolOutput::err(..), not Err
    }
}

pub async fn register(reg: &ActorRef<tools::ToolRegistry>, s: Arc<State>) {
    let _ = reg.ask(tools::RegisterTool(Arc::new(PingTool(s)))).await;
}
```

- `Risk` is the whole permission contract (00 §4, 12 §5): `tools.policy` maps it to allow/ask/deny; the two UI switches only rewrite ask→allow. Do not add a gate, a flag, or a per-tool policy. Choose `Risk` by what the tool can do to the world, not by how much you trust it — `knowledge_put` is `ReadOnly` because it only writes inside the harness's own, versioned, restorable bundle (10 §6 argues this; argue yours in your doc).
- Names are global and unprefixed; a clash with a native tool loses (logged). Pick `foo_verb`.
- Fill `ToolOutput::structured` and set `RETURNS` when the Python runtime should get a value, not text (08 §3.2).
- Everything that can affect the world goes through `Invoke`. If your plugin lets the model do something without a tool call, that is a bug (00 §4).

#### 3.5.1 Prompt block (optional)

If the model needs to know *when* to reach for your tools — not their signatures, the schemas carry those — register one paragraph next to them:

```rust
let _ = reg.ask(tools::RegisterPrompt { plugin: "foo", text: FOO_PROMPT.to_owned() }).await;
```

- It lands in the system prompt at the next turn start, sorted with the other plugins' blocks, before the agent's own guidance (07 §4 part 7). The harness owns headings and order; write plain sentences.
- Pure function of config: no clock, session, roster, counts. Anything that varies per turn busts the system cache for every session (07 §2.2.5). Knowledge's block is one `const` (`knowledge::tools::PROMPT`).
- One paragraph. If you need more, the model needs a skill (`~/.wth/skills/`), not a longer prompt.
- `tools::UnregisterPrompt("foo")` when your plugin turns itself off at runtime; a plugin that never starts never registers, so `foo.enabled: false` removes the text with no extra code.
- `/prompt` in the TUI shows the rendered result; that is the check.

### 3.6 Config

- `rt.section::<FooConfig>("foo")` — one section per plugin, `#[serde(default)]`, missing section → `Err` → fall back to `Default`. Never read another plugin's section; if you need a value it owns, that is a message.
- Env overlay is automatic: `WTH_FOO__ENABLED=false`.
- Writing back: `runtime::write_config_value(&rt.data_dir.join("config.yaml"), "foo", "key", "value")` rewrites one line and keeps comments (02 §2 explains why not a serde round-trip).
- Add your section, commented, to `DEFAULT_CONFIG` in `crates/wth/src/main.rs` (first-run template) and a bullet to README's config list.
- State on disk lives under `rt.data_dir` (`~/.wth/`); say in the doc which files are truth and which are derived and deletable.

## 4. Integrate

Order of edits once the crate compiles on its own.

1. **`crates/wth/src/main.rs`** — one line in the plugin vector: `Box::new(foo::FooPlugin)`. Position is convention only (tools first, frontends last); nothing may depend on it. `wth/Cargo.toml` gains the dep.
2. **Commands** — add the method to `trait Api` + `impl ApiServer` in `crates/rpc/src/api.rs` (`#[method(name = "foo_ping")]`; lookup + one `ask`, through a small `fn foo() -> RpcResult<ActorRef<FooActor>>` next to `knowledge()`). That is the whole command list; the TUI calls it in-process, Tauri over the socket. Events: each wire-visible event into `bridge.rs`'s `forward!(…)` **and** its `rt.subscribe` line; each wire type into `export.rs` for `just ts`; `rpc`'s `ts` feature forwards to `foo/ts`.
3. **TUI** (`crates/tui`) — `Command`/`UiEvent` variants in `model.rs` (UI intent), the arm in `execute` calling the `Api` method, the slash entry in `model::COMMANDS`, rendering in `ui.rs`; subscribe the TUI `Bridge` to your event if the transcript shows it. Menus are modal like the approval box; approvals take precedence (02 §4).
4. **Signal** (`crates/signal`) — only if the phone needs it; slash commands there mirror the TUI.
5. **Docs** — your design doc's Status section; 00 §10 line; README crate list and the `TUI:` paragraph; `docs/design/02` if you add a menu.

There is one command list; if you find yourself writing a lookup+ask outside `rpc/src/api.rs`, stop.

## 5. Verify

Every step of the plan ends with a runnable check, not a test suite (00 §12). Minimum before the doc's Status section is written:

```sh
cargo check --workspace --all-targets      # 0 warnings (missing_docs)
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p rpc --features ts           # ts derives compile
```

Then the real binary, headless, on a scratch data dir:

```sh
WTH_RPC__LISTEN=127.0.0.1:7791 ./target/debug/wth --headless --data-dir /tmp/wth-x
# token: /tmp/wth-x/token ; ws://127.0.0.1:7791/?token=<token> ; JSON-RPC 2.0
# list_tools → your tools present with the right risk; your methods round-trip;
# subscribe_events → your events arrive with "type"; set foo.enabled=false, restart → tools gone, methods say disabled.
```

The TUI on a real pty (`pyte`, as 02 §7 did) when you added a menu. Write down what you did **and what you did not exercise** — that line is the most useful thing in every Status section we have.

Tests are for logic a plausible bug would break (parsers, ranking, file-format round trips: `knowledge::store`, `harvest::parse_harvest`), in the crate, `#[cfg(test)]`. Not for wiring.

## 6. Checklist

Design
- [ ] `docs/design/NN-foo.md`: decisions with alternatives, config, commands, events, tools, UI, disk, checks
- [ ] 00 §10 line (plugin/library, doc NN); 00 §13 row if the subsystem was "not in v0"; any 00 sentence your design contradicts is amended in the same commit

Crate
- [ ] `crates/foo`, root `[workspace.dependencies]` entry, `[lints] workspace = true`, `ts` feature forwarding
- [ ] `FOO_ACTOR`, `FooConfig` (`#[serde(default)]`, fallback on missing section), events with `#[serde(tag = "type")]`, messages documented as `ask → Reply`
- [ ] `start`: reads config → disabled/broken returns `Ok(())` with a log → spawns + `register(FOO_ACTOR)` → cross-plugin registration via `lookup_when` in a task → returns
- [ ] Tools: `Risk` argued in the doc; description a pure function of config; `register()` after `lookup_when::<ToolRegistry>`
- [ ] Nothing reads another plugin's config section or calls into its state directly

Integrate
- [ ] `main.rs` vector line; `DEFAULT_CONFIG` section; README config bullet + crate list
- [ ] `Api` method (+ `forward!`/subscribe/`export.rs` for events); TUI `Command`/`UiEvent`/`COMMANDS`/`execute` arm/render

Verify
- [ ] check / clippy `-D warnings` / test / `rpc --features ts` green
- [ ] headless smoke over RPC incl. `enabled: false`; pty for menus
- [ ] Status section in the doc: exercised, not exercised

## 7. Worked example and counter-examples

**`knowledge`** (10, 12 §9). State: `store.rs` (OKF files + FTS5), `repo.rs` (gix). Actor: `KnowledgeActor` as `"knowledge"`, nine messages incl. a `Handle` for the loop's hot path. Event: `KnowledgeChanged`. Tools: seven `knowledge_*`, registered after `lookup_when::<ToolRegistry>`. Config: `knowledge: { enabled }`; the *loop's* use of it (hint cadence, harvest cadence) stayed in `agent.knowledge` because that is agent policy. The harvest **model call** stayed in the agent because only the loop has the frame — the plugin boundary went where the data already crossed an `Arc<Knowledge>` method, not where it would have forced the frame across the bus. That is the general rule for cutting a plugin out of `agent`: move the state and its tools, leave the frame.

**`signal`** (09): a frontend plugin. Subscribes to `AgentEvent`/`ApprovalRequested`, sends `SendInput`/`Resolve` by ask, owns its own `SignalEvent`, registers two tools, and owns the `--link-signal` pairing flow inside `start` (`LinkOpts`) — the binary knows only the plugin type.

**`computer`** (27): the plugin whose tools hold `Arc<Ctx>` directly (screen capture and the enigo input thread need no actor turn) and whose `Computer` actor exists only so RPC has a lookup target for `computer_permissions`.

**Not plugins, deliberately**: `models` (a data file and a resolver), `markdown` (a renderer), `oauth` (PKCE + token store), `cursor` (a protocol client behind `agent::backend`). No actors, no config sections, no events. Listed in 00 §10 as libraries. If one of them grows an actor, it becomes a plugin and gets a doc.
