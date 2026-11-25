# 216 — Contract: external plugins build wave

Status: contract v1 · 2026-11-25 · binds the builders of design 216 (`docs/design/216-external-plugins.md`, revised 2026-11-24, with the owner amendments of 2026-11-25 applied in its §3, §12, §14, §17 and §20). Where this contract and the design disagree on a name, a signature, a file or an owner, **this contract wins**; where the contract is silent, the design wins; where both are silent, the builder who owns the file decides and writes it in `docs/design/216-as-built-<TAG>.md`. The SDK (`sdk/**`) is specified by `/tmp/theuth-216/research-sdk.md` and is being built by its own builder (TAG `SDK`); this contract references it and does not redesign it.

## 0. Working rules (paste verbatim into every builder brief)

- Worktree: `/Users/sky/Develop/skymoore/theuth-216-plugins`, branch `feat/216-plugins`. Every builder works in this one worktree at the same time. Never touch `~/Develop/skymoore/theuth` (the owner's checkout), never push, merge, rebase, add worktrees, run `t` against `~/.theuth`, or restart the engine.
- Read first: this contract, then the design sections your TAG names in §13, then `docs/design/216-as-built-B0.md` once it exists.
- You own only the paths in your row of §2. Everything else is read-only to you. A change you need outside your paths is a hand-off line in your as-built, not an edit. Never `git add -A`, `git add .`, `git stash`, `git reset`, `git checkout <file>`, `git clean`, `cargo fmt --all`. Commit with explicit paths only: `git add <your paths> && git commit --no-gpg-sign -m "216 <TAG>: …"`. A checkpoint commit at least every 30 minutes; never amend a commit you did not make in this run.
- **Never change a §4–§9 name, field, serde shape, message, RPC method, param order or tool name.** If one does not fit, implement against it and write the mismatch in your as-built. B0 is the only builder that writes `types.rs`, `messages.rs`, `events.rs`, `error.rs`, `config.rs`, `lib.rs`; after B0 they are frozen for everyone.
- **Build lock** (one shared MacBook, several sessions): every `cargo`, `just`, `tsc`/`bun run check`, `vitest`/`bun run test`, `vite build` runs under `/tmp/theuth-build.lock`:
  ```bash
  lockrun() {  # one scoped command per hold
    until mkdir /tmp/theuth-build.lock 2>/dev/null; do sleep 10; done
    "$@"; local rc=$?
    rmdir /tmp/theuth-build.lock; sleep 45; return $rc
  }
  export CARGO_BUILD_JOBS=6 CARGO_INCREMENTAL=0
  lockrun mise exec -- cargo check -p plugins
  ```
  Waiting can take an hour when the machine is busy; that is normal, not an error. Never remove a lock you did not create. Never wrap `bun install` in a long hold together with cargo. Crate-scoped cargo only (`-p <crate>`); never `--workspace`, `just check`, `just test`, `--release`, benchmarks. Clippy always with `--no-deps` (`cargo clippy -p <crate> --no-deps -- -D warnings`; without it clippy fails in `crates/voice`). Only one builder runs `just ts-desktop` / `just wire-schema-accept` at a time: B0 (first accept + bindings) and B1 (second accept + one regen); B2, B3 and B4 never do.
- `crates/plugins` is shared by B1 and B2 after B0. Keep it compiling: run `cargo check -p plugins` before every commit. If the crate fails to compile in a file you do not own, do not touch it: wait one lock cycle and retry; if it still fails, filter the errors to your files, continue, and note it in your as-built.
- `write_file` and `edit` expand secret placeholders written in their arguments. Do not put a literal vault placeholder (double brace, `secret:`, a name, double brace) into a file through those tools; build such strings by concatenation in code (`["{{", "secret:", name, "}}"].concat()`).
- Finish: your gates of §11 green, `docs/design/216-as-built-<TAG>.md` written (what you built file by file; every deviation from this contract; hand-offs; what you did not verify) and committed; last line of your final message exactly `DONE <TAG> <sha>`.

## 1. Scope and the v1 cut

v1 is design §1 D16 plus the owner's 2026-11-25 rulings:

| In v1 | Not in v1 (design §20, keys reserved) |
|---|---|
| Child process, JSON-RPC 2.0 NDJSON, protocol 1 (§6) | `header`, `composer`, `card`, `tray`, `settings`, `view` placements |
| `subscribe`, `emit`, `status`, `ui`, `invalidate`, `notify`, `log`, `call`, `input`, `catalog`, `ping`; static manifest tools | `format: html`, `mcp:`, `isolation:`, `sessions: focused`, capability `open`, `emits.*.exposure` |
| Placements `sidebar` (popover / tile), `status`, `tile`, `command` | Dev tile + Inject, git-URL install, `when: event` routines |
| Exposure in `wire` + snapshot + name lint; meeting-session text withheld for the **whole life** of the session (Q1: No) | Origin on relayed frames (Q2: not easy → later) |
| Router drops frames of sessions seen `SessionOwnerChanged{here:false}` unless `plugins.remote_sessions: true` (Q2 mitigation) | Grant narrowing by unticking lines in Settings (§10.3 last bullet) — v1 Settings shows grants read-only; narrowing = edit the manifest |
| Authenticated grants (HMAC), code hash, install / widen / code-changed cards, human press | `notify` to channels (`message_user`) — v1 `notify` is inbox + OS notification only |
| **Settings › Plugins** (13th settings category) — owner amendment 4 | `t plugin check --events <file>` replay |
| **Agent tools** `plugin_*` (§7) — owner amendment 5; agents can never approve | Windows (host refuses to start on Windows, design §9.5) |
| **SDKs in v1**: py, js, sh, go, rb, rs, vendored by `t plugin new`; `--raw` for the no-SDK templates — owner amendment 6 | Typed SDK stubs (`t plugin types`) |
| CLI `t plugin list|new|install|approve|enable|disable|restart|logs|uninstall|check|catalog`; `t plugin approve` is a human press (Q3) — owner amendments 3, 7 | |
| TUI `/plugins`, status segments, plugin `/commands` (text fallbacks only; no approve) | |

## 2. Builders, file ownership, order

| TAG | Runs | Owns after B0 (may create and edit) | Done when |
|---|---|---|---|
| **B0** Skeleton | first, alone, ~45 min | everything in §2.1 "B0 writes"; after B0 only its frozen files stay B0's (none of them may change) | §11 B0 gates green; bindings regenerated; `216-as-built-B0.md` with a **"TS surface (verbatim)"** section listing the generated `method.plugins.*` / `event.plugins_2e*` type names; `DONE B0 <sha>` |
| **B1** Host runtime | after B0, parallel with B2/B3/B4 | `crates/plugins/src/{actor,host,manifest,grants,proc,route,catalog,ui,broker,ptools,watch,attention,logs}.rs`, `crates/plugins/tests/**` except `tests/scaffold.rs`; `crates/wire/**`; `crates/secrets/**`; `crates/tools/src/{mcp,registry,lib}.rs` (the 216 additions of §5.6 only); `crates/agent/src/{lib,communication}.rs` (the §5.7 additions only); the WireEvent consts in the owner crates of §5.5; `crates/sessionsync/src/events.rs` (test twins' consts); `crates/theuth/tests/wire_exposure.rs` + `crates/theuth/tests/wire_exposure.txt`; `crates/theuth/tests/fixtures/wire/openrpc-v1.json` (its second accept); `crates/rpc-compose/tests/{wire_golden,wire_parity_inc5}.rs` + `crates/rpc-compose/tests/fixtures/wire/plugins.*.json` | §11 B1 |
| **B2** Management | after B0, parallel | `crates/plugins/src/{rpc,atools,scaffold,press}.rs`, `crates/plugins/build.rs`, `crates/plugins/tests/scaffold.rs`; `crates/theuth/src/plugin_cli.rs`; `crates/theuth/harness.md` (one new `## Plugins` section); `crates/tui/**` (the `/plugins` menu, status segments, plugin commands: new `src/model/plugins.rs`, edits to `src/model/{slash,menus,status,mod}.rs`, `Cargo.toml`, tests) | §11 B2 |
| **B3** UI-A | after B0, parallel | §2.3 "B3" list | §11 B3 |
| **B4** UI-B | after B0, parallel | §2.3 "B4" list | §11 B4 |
| **SDK** | already running | `sdk/**` only | its own brief; B2 consumes `sdk/` |
| **Z** closer | after B1–B4 + SDK | cross-builder hand-offs from the as-builts; the §16 amendments to `docs/design/00-architecture.md` and `docs/plugins.md`; the design §15 end-to-end checks on a scratch data dir; `216-as-built-Z.md` | |

Order: B0 → {B1, B2, B3, B4} in parallel → Z. B3 and B4 never wait for B1/B2: the RPC methods exist (stubbed) after B0 and their bindings are generated; behaviour arrives when B1/B2 land.

### 2.1 What B0 writes (exactly)

Rust, new crate `crates/plugins/`:

| File | Content | Owner after B0 |
|---|---|---|
| `Cargo.toml` | §3.1, every dependency B1/B2 will need (nobody else edits it; a missing dep is a hand-off to Z) | frozen (Z) |
| `src/lib.rs` | module list (§3.2), re-exports, `PluginsPlugin` (`runtime::Plugin`), `declare_wire`, `rpc_module` re-export; `start` exactly as §3.3 | frozen |
| `src/config.rs` | `PluginsConfig` (§4) | frozen |
| `src/types.rs` | every type of §5.1 | frozen |
| `src/messages.rs` | every actor message of §5.2 | frozen |
| `src/events.rs` | `PluginEvent`, `PluginUi` + `WireEvent` impls (§5.3) | frozen |
| `src/error.rs` | `PluginsError` (§9) | frozen |
| `src/press.rs` | `HumanPress` complete (§8) | B2 |
| `src/actor.rs` | `PluginsActor` + `impl Message<_>` for every message, each body returning `Err(PluginsError::NotImplemented("<Message>"))` (or an empty `Vec` for the infallible lists) | B1 |
| `src/rpc.rs` | the full `PluginsApi` trait of §6 with `#[rpc_descriptors]`, stub impl: `owner(ext)?` then `Err(not_implemented("<method>"))`; `rpc_module` | B2 |
| `src/{host,manifest,grants,proc,route,catalog,ui,broker,ptools,watch,attention,logs}.rs` | `//! Owner: B1 — <one line from §3.2>` + the cross-builder fns of §3.4 as stubs | B1 |
| `src/{atools,scaffold}.rs` | `//! Owner: B2 — …` + the §3.4 stubs (`atools::register` does nothing) | B2 |

Outside the crate:

| File | Edit | Owner after B0 |
|---|---|---|
| `Cargo.toml` (root) | `plugins = { path = "crates/plugins" }  # 216: external plugins (child processes, any language)` in the internal-crates block (near `webprofile`, L56) | Z |
| `crates/theuth/Cargo.toml` | `plugins = { workspace = true }  # 216` | Z |
| `crates/theuth/src/main.rs` | (1) `mod plugin_cli;` between `mod mcp;` and `mod secret_cli;`; (2) `Cmd::Plugin { #[command(subcommand)] cmd: plugin_cli::PluginCmd }` with doc `/// External plugins (design doc 216): list, scaffold, install, approve, logs.` after `Dreaming`; (3) in `run()`, `Some(Cmd::Plugin { cmd }) => return plugin_cli::run(&data_dir, &config, cmd).await,` directly before `Some(cmd) => return cmd_67(…)`; (4) in `cmd_67`, `Cmd::Plugin { .. } => unreachable!("t plugin is served by plugin_cli::run"),` beside the other `unreachable!` arms; (5) `plugins::rpc_module(server.clone()),` in `rpc_composition`'s `merge_described([...])` after `webprofile::rpc_module`; (6) `Box::new(plugins::PluginsPlugin),` in `engine_plugins` directly after `Box::new(share::SharePlugin::engine()),` with `// 216: external plugins; after roster (attention Source), before update.`; (7) the commented `# plugins:` block of §4 in `DEFAULT_CONFIG`, after `# sandbox:` | frozen (Z) |
| `crates/theuth/src/plugin_cli.rs` | `PluginCmd`, `LangArg` exactly as §7.2; `pub(crate) async fn run(data_dir: &Path, config: &figment::Figment, cmd: PluginCmd) -> anyhow::Result<()>` that bails `"t plugin: not implemented yet (216 B0)"` | B2 |
| `crates/rpc-compose/Cargo.toml` | `plugins = { workspace = true }` | Z |
| `crates/rpc-compose/src/api/tests.rs` | `plugins::rpc_module(server.clone()),` in `test_rpc_module` before `sandbox::rpc_module(server)` | Z |
| `crates/rpc-compose/src/client_mirror.rs` | `plugins::PluginUi, plugins::PluginEvent,` appended to `republish!(…)` | Z |
| `crates/roster/src/attention.rs` | `AttentionKind::Plugin` (§5.4) last in the enum; `plugin:<name>:<m8>:<c8>:<seq>` added to the `AttentionItem.id` doc list | B1 |
| `crates/agent/src/communication.rs` | `ChannelVia::Plugin` (§5.4) last in the enum; `ChannelOrigin.via` doc `slack | signal | telegram | plugin` | B1 |
| `crates/tools/src/lib.rs` | `ToolSource::Plugin` (§5.4) last in the enum | B1 |
| `crates/tools/src/registry.rs` | the exhaustive match at ~L1150 gains `ToolSource::Plugin { .. } => None,` (and any other exhaustive match `cargo check` finds) | B1 |
| `crates/theuth/tests/fixtures/wire/openrpc-v1.json` | first accept (`just wire-schema-accept`) after reviewing that only additions appear | B1 |
| `crates/desktop/ui/src/components/icons/channels.tsx` | a `plugin` entry (lucide `Puzzle`) so `satisfies Record<ChannelVia, …>` compiles | B3 |
| `crates/desktop/ui/src/components/attention/{labels.ts,Screen.tsx,model.ts}` | placeholder `plugin` entries: `KIND_LABEL.plugin = 'plugin'`, `TITLE.plugin = 'Plugin'`, `SCREEN.plugin = HomeScreen`, `'plugin'` appended to `KIND_ORDER` — only what `tsc` demands | B3 |
| `crates/desktop/ui/src/components/plugins/{icons.tsx,PluginDot.tsx,state.ts}` | the shared UI primitives of §12.3, complete | frozen (both UI builders import, neither edits) |
| other UI files `tsc` flags after the regen (exhaustive `Record<AttentionKind|ChannelVia|ToolSource …>`) | the minimal placeholder entry | the B3/B4 owner per §2.3 |

B0 runs `bun install` in `crates/desktop/ui`, `packages/apps`, `packages/documents` (each in its own short lock hold), then `just ts-desktop`, so the gitignored bindings exist in the worktree for B3/B4.

### 2.2 Inside `crates/plugins` after B0

B1 and B2 share the crate; files are split so no file has two owners. Cross-builder calls go only through the §3.4 functions, whose signatures are frozen. `actor.rs` (B1) calls `scaffold::new_plugin` (B2); `rpc.rs` and `atools.rs` (B2) only `ask` the actor with §5.2 messages; `plugin_cli.rs` (B2) calls `manifest::check_offline` (B1) and `scaffold::new_plugin` (B2) when no engine runs. Nobody but B0 edits `lib.rs`; a new module is a hand-off to Z (declare your helpers as private submodules of a file you own, e.g. `proc/lanes.rs` with `#[path]` or `mod lanes;` inside `proc.rs` + a `src/proc/` dir).

### 2.3 Desktop UI split (`UI` = `crates/desktop/ui/src`)

**B3 (UI-A: management)** creates `UI/components/settings/categories/plugins.tsx`, `UI/components/settings/Plugins.tsx` (+ `Plugins.test.ts`), `UI/components/plugins/GrantLines.tsx` (+ test), `UI/components/plugins/NewPluginPopover.tsx`, `UI/components/plugins/LogsSheet.tsx`, `UI/components/attention/PluginInstallScreen.tsx` (+ test); edits `UI/components/settings/registry.ts`, `UI/components/settings/registry.test.ts`, `UI/components/attention/{Screen.tsx,labels.ts,model.ts,actions.tsx,Card.tsx}`, `UI/components/icons/channels.tsx`, `UI/lib/rpc.ts` (queryKeys + `useInvalidateOn` lines; B3 is its only editor), and any attention fixture that enumerates kinds.

**B4 (UI-B: placements)** creates `UI/lib/plugins.ts` (+ test), `UI/components/plugins/{PluginSurface.tsx,provider.ts,SidebarSlot.tsx,StatusStrip.tsx,PlacementHost.tsx,Attribution.tsx}` (+ tests), `UI/components/tiles/kinds/plugin.tsx`; edits `UI/components/sidebar/AppSidebar.tsx`, `UI/components/sidebar/Rail.tsx` (exports only), `UI/components/status/StatusBar.tsx` (the `right` slot only), `UI/components/tiles/registry.tsx` (`TILE_KINDS.plugin`, `TILE_ICONS.plugin`), `UI/components/tiles/open.ts` (`PluginTileProps`, `openPluginTile`), `UI/lib/popout.test.ts` (one case), `UI/components/chat/slash.ts`, `UI/components/chat/Composer.tsx`, `UI/components/chat/slash.test.ts`, `UI/components/chat/Composer.test.ts` (new tests only; the `COMMANDS` pins stay).

Neither touches `UI/rpc/bindings/**` (generated), `UI/lib/panes.ts`, `UI/routes/**`, `UI/lib/nav.ts`, `UI/components/status/model.ts` (`dotOf` stays pinned), `packages/apps/**`.

## 3. Crate `plugins`

### 3.1 `crates/plugins/Cargo.toml` (B0, verbatim)

```toml
[package]
name = "plugins"
version = "2.22.2"  # frozen: only the binaries carry the product version (platform::version)
edition.workspace = true
publish.workspace = true
description = "External plugins (design doc 216): child processes in any language, JSON-RPC over stdin/stdout, grants, placements"

[dependencies]
platform = { workspace = true }
runtime = { workspace = true }
wire = { workspace = true }
rpc-core = { workspace = true }
rpc-core-macros = { workspace = true }
jsonrpsee = { workspace = true }
secrets = { workspace = true }
tools = { workspace = true }
agent = { workspace = true }
app = { workspace = true }
surface = { workspace = true }
roster = { workspace = true }
meeting = { workspace = true }
kameo = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
futures = { workspace = true }
async-trait = { workspace = true }
anyhow = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
serde_yaml = { workspace = true }
schemars = { workspace = true }
tracing = { workspace = true }
sha2 = { workspace = true }
hmac = { workspace = true }
hex = { workspace = true }
base64 = { workspace = true }
rand = { workspace = true }
zeroize = { workspace = true }
notify = { workspace = true }
globset = { workspace = true }
shlex = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
libc = { workspace = true }
ts-rs = { workspace = true, optional = true }

[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true, features = ["test-util"] }

[features]
ts = ["dep:ts-rs", "runtime/ts", "tools/ts", "agent/ts", "roster/ts"]

[lints]
workspace = true
```

If a crate above has no `ts` feature, B0 drops it from the list. If `tempfile` is not a `[workspace.dependencies]` entry, B0 writes `tempfile = "3"`. The desktop bindings come from `bindings/openrpc.json` (JSON Schema via `schemars`), so every wire-visible type needs `JsonSchema`; the `ts` derive is for parity with the other crates.

### 3.2 Modules (`src/lib.rs`, B0)

```rust
//! External plugins (design doc 216): a folder with `plugin.yaml` and a `run:` command in any
//! language, spawned as a child process speaking JSON-RPC 2.0 over stdin/stdout.
mod actor;      // B1: PluginsActor and the Message impls (§5.2)
mod attention;  // B1: roster::attention::Source — install / widen / code-changed cards, notify items
mod broker;     // B1: plugin → host requests: call (headless session), input, notify, catalog, ping
mod catalog;    // B1: dynamic catalog — exposed wire topics split per `type` + plugin events
mod grants;     // B1: grants.yaml read/write, HMAC-SHA256 under the vault row plugins/grants-key
mod host;       // B1: boot (orphan reap, scan, start), per-plugin supervisors, lifecycle state machine
mod logs;       // B1: per-plugin redacted log files, tail
mod proc;       // B1: spawn under ProcessTree, two-lane writer, stdout reader, backoff, .pgids reaper
mod ptools;     // B1: manifest tools → tools::DynTool (ToolSource::Plugin)
mod route;      // B1: wire feed → external_view → grants → session filters → redaction → event lane
mod ui;         // B1: placement surfaces, ui validation (app::helper, surface::parse), rate limits
mod watch;      // B1: notify watcher on plugins/, 300 ms debounce, widening detection
mod atools;     // B2: agent-facing plugin_* tools (§7.1)
mod rpc;        // B2: PluginsApi (§6)
pub mod config; // B0, frozen
pub mod error;  // B0, frozen
pub mod events; // B0, frozen
pub mod manifest; // B1 — pub only for check_offline / check_name (§3.4)
pub mod messages; // B0, frozen
pub mod press;  // B2 (B0 writes it complete)
pub mod scaffold; // B2 — pub for new_plugin (§3.4)
pub mod types;  // B0, frozen

pub use actor::PluginsActor;
pub use config::PluginsConfig;
pub use error::PluginsError;
pub use events::{PluginEvent, PluginUi};
pub use messages::*;
pub use press::HumanPress;
pub use types::*;
pub use rpc::rpc_module;

/// Registered actor name.
pub const PLUGINS_ACTOR: &str = "plugins";
/// The wire protocol major this host speaks (design §6).
pub const PROTOCOL: u32 = 1;
/// Topic of [`PluginEvent`].
pub const EVENTS_TOPIC: &str = "plugins.events";
/// Topic of [`PluginUi`].
pub const UI_TOPIC: &str = "plugins.ui";
```

No module is named after a dependency crate (`tools`, `surface`, `app`, `agent` would shadow them).

### 3.3 `PluginsPlugin` (B0, verbatim behaviour)

```rust
/// Declare both wire topics (always, even when disabled — the ABI must not depend on config).
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<PluginEvent>().await?;
    rt.register_wire_event::<PluginUi>().await
}

/// The native host of external plugins (216).
pub struct PluginsPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for PluginsPlugin {
    fn name(&self) -> &'static str { "plugins" }
    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> { declare_wire(rt).await }
    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: PluginsConfig = rt.section("plugins").unwrap_or_else(|e| {
            tracing::debug!(error = %e, "plugins: bad section, using defaults");
            PluginsConfig::default()
        });
        if !cfg.enabled { tracing::info!("plugins: disabled (plugins.enabled: false)"); return Ok(()); }
        if cfg!(windows) { tracing::warn!("plugins: not supported on Windows in v1 (216 §9.5)"); return Ok(()); }
        let actor = actor::spawn(rt, cfg.clone())?;          // registers PLUGINS_ACTOR
        tokio::spawn(atools::register(rt.clone()));            // B2: agent tools (§7.1)
        tokio::spawn(host::boot(rt.clone(), actor, cfg));     // B1: reap, scan, start, watcher, attention Source
        Ok(())
    }
}
```

`rt.section` is whatever helper the sibling crates use to read their section (`docs/plugins.md` §3.2); B0 uses the same call `webprofile`/`sandbox` use. When disabled or on Windows there is no actor: every RPC method answers `NOT_RUNNING` (`-32001`, "plugins not running") and every agent tool answers "plugins is not running"; the topics stay declared; folders and `grants.yaml` are untouched.

### 3.4 Cross-builder functions (signatures frozen; B0 writes them as stubs)

```rust
// actor.rs (B1)
/// Spawn the actor on its own mailbox and register it as PLUGINS_ACTOR.
pub(crate) fn spawn(rt: &runtime::Runtime, cfg: PluginsConfig) -> anyhow::Result<kameo::actor::ActorRef<PluginsActor>>;

// host.rs (B1)
/// Background start: reap orphans from `.pgids`, scan `plugins/`, validate, start what the
/// authenticated grants cover, file cards for the rest, register the attention Source and the
/// tool registry entries, start the watcher. Never panics; logs and returns on shutdown.
pub(crate) async fn boot(rt: runtime::Runtime, actor: kameo::actor::ActorRef<PluginsActor>, cfg: PluginsConfig);

// manifest.rs (B1)
/// `^[a-z][a-z0-9-]{0,31}$`, not a reserved name (a built-in slash command of the desktop or
/// the TUI, or `plugins`). Used by the loader and by `scaffold::new_plugin`.
pub fn check_name(name: &str) -> Result<(), PluginsError>;
/// Built-in slash command names a plugin may not take (without the `/`). B2's TUI test asserts
/// every `crates/tui` COMMANDS name is in this list.
pub const RESERVED_NAMES: &[&str] = &[/* B1 fills from crates/tui/src/model/slash.rs and UI/components/chat/slash.ts */];
/// Validate a folder without an engine: parse + every §4.3 rule that needs no vault and no
/// catalog; those two are reported as warnings ("engine not running: secrets and topics not
/// checked"). Also computes both hashes and the card lines.
pub fn check_offline(dir: &std::path::Path) -> CheckReport;

// scaffold.rs (B2)
/// Write `<plugins_dir>/<name>/` from the bundled SDK template for `lang` (raw: the no-SDK
/// template; only `py` and `sh` have one). Fails with `Exists` if the folder exists.
pub fn new_plugin(plugins_dir: &std::path::Path, name: &str, lang: Lang, raw: bool) -> Result<ScaffoldReport, PluginsError>;

// atools.rs (B2)
/// Register the agent-facing tools of §7.1 once the tool registry answers.
pub(crate) async fn register(rt: runtime::Runtime);
```

Data dir: the actor and `host` take the data dir from the runtime the same way sibling plugins do; `plugins_dir = data_dir.join(cfg.dir)`, `plugin-data/<name>`, `logs/plugins/<name>.log.<YYYY-MM-DD>` (design §4.1, §11).

## 4. Config section `plugins:` (B0 `src/config.rs`, frozen)

```rust
/// `plugins:` in config.yaml (216 §3). Every key optional; a missing section is the defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PluginsConfig {
    /// false: no actor, no processes, no plugin tools, no placements; folders untouched.
    pub enabled: bool,                 // true
    /// Plugin folders, relative to the data dir.
    pub dir: String,                   // "plugins"
    /// Hot-load plugin.yaml changes (300 ms debounce).
    pub watch: bool,                   // true
    /// Author mode: no code hash in grants, restart on any file change in the folder.
    pub dev: bool,                     // false
    /// Deliver frames of sessions this engine has seen `SessionOwnerChanged { here: false }` for
    /// (sessions owned by another engine of the mesh). Off by default (216 §17 Q2).
    pub remote_sessions: bool,         // false
    pub restart_first_ms: u64,         // 1000
    pub restart_cap_ms: u64,           // 300000
    pub crash_loop: u32,               // 5
    pub crash_window_s: u64,           // 60
    pub shutdown_grace_ms: u64,        // 3000
    pub max_line_bytes: usize,         // 1048576
    pub queue_frames: usize,           // 2048
    pub emit_per_s: u32,               // 50
    pub ui_per_s: u32,                 // 4
    pub query_per_s: u32,              // 10
    pub input_per_min: u32,            // 6
    pub request_timeout_ms: u64,       // 10000
    pub string_cap: usize,             // 65536
    pub log_max_bytes: u64,            // 10485760
    pub log_days: u32,                 // 5
}
```

`Default` gives exactly the values in the comments; each field carries a `///` doc (lints). Env overlay as usual (`THEUTH_PLUGINS__ENABLED=false`). `DEFAULT_CONFIG` block (B0, verbatim):

```yaml
# plugins:                  # external plugins (design 216): folders under plugins/, any language
#   enabled: true           # false: nothing runs; folders and grants.yaml untouched
#   watch: true             # hot-load plugin.yaml changes
#   dev: false              # author mode: no code hash in grants, restart on any file change
#   remote_sessions: false  # also deliver events of sessions owned by another engine of the mesh
#   query_per_s: 10
#   input_per_min: 6
#   request_timeout_ms: 10000
```

## 5. Public Rust types (B0, verbatim; `src/types.rs`, `src/messages.rs`, `src/events.rs`)

Every type below derives what its line says; every wire/RPC-visible type also has `#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]` and a `///` doc on the type and each field (lints: `missing_docs`). `SessionId` is `runtime::SessionId`; `Risk` is `tools::Risk` (snake_case `read_only | mutating | destructive`). Optional fields serialize as `null` (no `skip_serializing_if`) unless marked.

### 5.1 `src/types.rs`

```rust
use std::collections::BTreeMap;
use runtime::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::Risk;

/// Lifecycle of one plugin (216 §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginRunState { Invalid, AwaitingApproval, Starting, Running, Backoff, Failed, Exited, Stopping, Disabled }

/// A placement's status dot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dot { Ok, Busy, Warn, Error, #[default] Off, None }

/// A badge: a count (0 hides it) or up to 4 characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Badge { Count(u64), Text(String) }

/// v1 placement kinds (216 §9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlacementKind { Sidebar, Status, Tile, Command }

/// What a sidebar icon opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenMode { Popover, Tile }

/// Content format of a placement or a command card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceFormat { Openui, JsonRender, Markdown }   // "openui" | "json-render" | "markdown"

/// What a surface without a renderer shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Fallback { #[default] Text, Hide }

/// Default size of a popover or tile, CSS px.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Size { pub width: u32, pub height: u32 }

/// A `command` placement as clients list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommandInfo {
    /// As declared (`name:`).
    pub name: String,
    /// What the composer inserts: `/<plugin>` when `name == plugin`, else `/<plugin>:<name>`.
    pub slash: String,
    pub description: String,
    /// Argument hint (`args:`), shown after the command.
    pub args: Option<String>,
}

/// One declared placement, as approved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlacementInfo {
    pub id: String,
    pub kind: PlacementKind,
    /// `label:` (tooltip / menu text / tile title).
    pub label: String,
    /// A lucide icon name (kebab-case, e.g. `gauge`), or `svg:<data-url>` for a sanitised SVG from the folder.
    pub icon: String,
    /// Draws a status dot fed by `status`.
    pub status: bool,
    /// Sidebar only.
    pub open: Option<OpenMode>,
    pub size: Option<Size>,
    /// Absent for `status` and `command`.
    pub format: Option<SurfaceFormat>,
    pub fallback: Fallback,
    /// The owner hid it (Settings › Plugins / context menu); hidden placements are not drawn, commands still work.
    pub hidden: bool,
    /// `command` placements only.
    pub command: Option<CommandInfo>,
}

/// One row of Settings › Plugins and `plugin_list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct PluginInfo {
    /// Folder name = plugin name.
    pub name: String,
    /// `version:` (free text); empty when the manifest does not parse.
    pub version: String,
    pub description: String,
    pub state: PluginRunState,
    /// Last error: manifest error (`invalid`), crash reason, stderr's last line.
    pub error: Option<String>,
    /// The grant's `enabled` (false when there is no grant yet).
    pub enabled: bool,
    /// A card waits for a press (install, widen or code-changed), whatever the state.
    pub needs_approval: bool,
    /// Attention item id of that card (`plugin:<name>:<m8>:<c8>:<seq>`).
    pub card: Option<String>,
    pub placements: Vec<PlacementInfo>,
    /// Names of the tools it offers the model.
    pub tools: Vec<String>,
    /// Absolute folder path.
    pub dir: String,
    /// Absolute path of today's log file.
    pub log: String,
    /// The grant's `approved_at` (RFC 3339), none before the first approval.
    pub approved_at: Option<String>,
    /// Position in the sidebar/status order (0 = first).
    pub order: u32,
}

/// One plain-words line of an install card (216 §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrantLine {
    pub key: GrantKey,
    /// The card wording, e.g. "Reads agent.events: TurnFinished — a turn finished; …".
    pub text: String,
    pub risk: Risk,
    /// On a widen card: this line is new since the last approval.
    pub new: bool,
}

/// Which manifest entry a [`GrantLine`] stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantKey { Run, Secret, Subscribe, Emit, Placement, Tool, Call, Input, Notify }

/// Which card it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardKind { Install, Widen, CodeChanged }

/// A pending card, as the install screen and `t plugin approve` show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct InstallCard {
    /// Attention item id `plugin:<name>:<m8>:<c8>:<seq>` (m8/c8: first 8 hex of the hashes, `00000000` for no code hash; seq: 0, bumped by each `RequestApproval`).
    pub id: String,
    pub plugin: String,
    pub version: String,
    pub kind: CardKind,
    /// Pass back verbatim to `plugin_approve`.
    pub manifest_sha256: String,
    /// Pass back verbatim to `plugin_approve`; `None` when the grant will carry no code hash.
    pub code_sha256: Option<String>,
    /// The highest line's risk (always at least `destructive`, because `run` is).
    pub risk: Risk,
    /// Install: every line. Widen: the new lines, plus "Runs …" when run/cwd/env changed.
    pub lines: Vec<GrantLine>,
    /// Code-changed: the relative paths whose bytes changed.
    pub changed_paths: Vec<String>,
    /// The saved approval did not verify ("the saved approval for <name> could not be verified").
    pub unverified: bool,
    /// Absolute folder path.
    pub dir: String,
    /// When the card was filed (ms since epoch).
    pub at_ms: u64,
}

/// What a plugin may do (grants.yaml `grants:`, `hello.params.grants` minus run/cwd/env/secrets).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Grants {
    /// Catalog names: `topic`, `topic#type`, `<plugin>.<event>`, `<plugin>.*`.
    pub subscribe: Vec<String>,
    pub emits: Vec<String>,
    /// Placement ids.
    pub placements: Vec<String>,
    pub tools: Vec<ToolGrant>,
    pub capabilities: Capabilities,
    /// Vault secret names referenced by `env`.
    pub secrets: Vec<String>,
    /// As approved, unexpanded (a change is widening).
    pub run: Vec<String>,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
}

/// A manifest tool and the risk it was approved at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolGrant { pub name: String, pub risk: Risk }

/// Host-brokered powers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Capabilities { pub notify: bool, pub input: bool, pub call: Option<CallGrant> }

/// `call: {tools, max_risk}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CallGrant { pub tools: Vec<String>, pub max_risk: Risk }

/// Everything about one plugin (`plugin_get`, `plugin_status` tool).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct PluginDetail {
    pub info: PluginInfo,
    /// The parsed manifest as JSON (env values unexpanded); `null` when it does not parse.
    pub manifest: Value,
    /// What the manifest asks for now.
    pub requested: Grants,
    /// What the verified grant holds; `None` before the first approval or when it does not verify.
    pub granted: Option<Grants>,
    /// The card waiting for a press, if any.
    pub card: Option<InstallCard>,
    /// Catalog entries of the events it emits.
    pub catalog: Vec<CatalogEntry>,
    /// Last 40 stderr lines, redacted.
    pub stderr_tail: Vec<String>,
    /// Validation warnings (reserved keys, unknown top-level keys).
    pub warnings: Vec<String>,
}

/// How an event is scoped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scoped { Session, Global }

/// One catalog entry (216 §7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CatalogEntry {
    /// `topic#type`, `topic` or `<plugin>.<event>`.
    pub name: String,
    pub topic: String,
    pub version: u32,
    /// The serialized `type` tag (or `<plugin>.<event>`); `None` for single-struct topics.
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    /// Owner crate (`agent`) or `plugin:<name>`.
    pub owner: String,
    /// Always `"public"` in the catalog (private and internal topics are absent).
    pub exposure: String,
    /// First sentence of the schema description, or the manifest's `description`.
    pub describe: String,
    pub scoped: Scoped,
    /// Delta-class: needs `deltas: true` on the subscription.
    pub delta: bool,
    /// JSON Schema: inlined (resolved) for wire topics, the manifest schema for plugin events.
    pub schema: Value,
}

/// The latest status of one placement (snapshot-first for clients, `plugin_surfaces`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SurfaceSnapshot {
    pub plugin: String,
    pub placement: String,
    /// Already overridden by state (backoff/failed/exited/awaiting_approval → off/error/off/off).
    pub dot: Dot,
    pub badge: Option<Badge>,
    pub label: Option<String>,
    /// The plugin's tooltip, or the state when overridden ("starting", "exited", …).
    pub tooltip: Option<String>,
    /// Content revision, 0 = no content held.
    pub revision: u64,
    pub format: Option<SurfaceFormat>,
}

/// Whether a surface has content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceLoad { Ready, Loading, Error }

/// OpenUI statement kind of a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind { Query, Mutation }

/// One `Query(...)` / `Mutation(...)` statement of a validated OpenUI source (from
/// `app::helper::CheckResult.bindings`). The plugin provider routes by it (§12.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SurfaceBinding {
    /// The statement id the renderer reports as `ProviderContext.statement_id`.
    pub statement_id: String,
    /// The query or mutation name the plugin receives (`query` / `action`).
    pub name: String,
    pub kind: BindingKind,
}

/// A placement's content (`plugin_surface`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct SurfaceContent {
    pub plugin: String,
    pub placement: String,
    pub revision: u64,
    /// `loading`: the host sent `render` and waits for the plugin's `ui` (a `PluginSurface` event follows).
    pub state: SurfaceLoad,
    pub format: Option<SurfaceFormat>,
    /// Validated source; `None` while loading.
    pub source: Option<String>,
    /// Text fallback.
    pub text: Option<String>,
    /// OpenUI only; empty for json-render and markdown.
    pub bindings: Vec<SurfaceBinding>,
    /// "plugin did not render", "plugin <name> is not running", …
    pub error: Option<String>,
}

/// A `/command` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct CommandReply { pub text: Option<String>, pub ui: Option<CommandUi> }

/// An ephemeral card in a command reply (json-render or markdown; OpenUI is refused in v1 replies).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommandUi { pub format: SurfaceFormat, pub source: String, pub text: String }

/// One located validation problem.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckIssue {
    /// JSON Pointer into the manifest (`/placements/0/kind`), or `""` for the whole folder.
    pub path: String,
    pub message: String,
}

/// `plugin_check` / `t plugin check`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct CheckReport {
    /// No errors.
    pub ok: bool,
    pub name: Option<String>,
    pub errors: Vec<CheckIssue>,
    pub warnings: Vec<CheckIssue>,
    pub manifest_sha256: Option<String>,
    pub code_sha256: Option<String>,
    /// The install card's lines for this manifest.
    pub lines: Vec<GrantLine>,
}

/// SDK language of `plugin_new` / `t plugin new --lang`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Lang { #[default] Py, Js, Sh, Go, Rb, Rs }

/// What `plugin_new` wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct ScaffoldReport {
    pub name: String,
    pub dir: String,
    pub lang: Lang,
    pub raw: bool,
    /// Relative paths written.
    pub files: Vec<String>,
    /// Always "awaiting approval: press Approve in theuth or run `t plugin approve <name>`".
    pub next: String,
}

/// What `plugin_install` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct InstallReport { pub name: String, pub dir: String, pub next: String }

/// A log tail (`plugin_logs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
pub struct LogTail { pub name: String, pub path: String, pub lines: Vec<String> }
```

`#[derive(kameo::Reply)]` is on every type an actor message returns directly; `Vec<_>` and `Result<_, PluginsError>` replies need no derive. If `kameo::Reply` cannot derive on a type with `Value` (`PartialEq` only), B0 drops `Eq` from that type only.

### 5.2 Actor messages (`src/messages.rs`)

`ask` → the reply in the comment. Every struct is `pub`, `#[derive(Clone, Debug)]` plus `Serialize, Deserialize` — except `Approve`, which holds a [`HumanPress`] and derives `Clone, Debug` only (it can never arrive deserialized). `name` is always the plugin name; unknown → `PluginsError::NotFound`.

```rust
pub struct ListPlugins;                                                     // → Vec<PluginInfo> (order: grants.yaml `order`, then name)
pub struct GetPlugin { pub name: String }                                   // → Result<PluginDetail, PluginsError>
pub struct CheckPath { pub path: std::path::PathBuf }                       // → Result<CheckReport, PluginsError> (full check: vault + catalog)
pub struct Install { pub path: std::path::PathBuf }                         // → Result<InstallReport, PluginsError> (copy folder → plugins/<name>; `Exists` if present; card filed by the watcher)
pub struct New { pub name: String, pub lang: Lang, pub raw: bool }          // → Result<ScaffoldReport, PluginsError> (calls scaffold::new_plugin)
pub struct Approve {                                                        // → Result<PluginInfo, PluginsError>
    pub name: String,
    pub manifest_sha256: String,
    pub code_sha256: Option<String>,
    /// Built only by `HumanPress::from_rpc` (§8).
    pub press: HumanPress,
}
pub struct RequestApproval { pub name: String }                             // → Result<InstallCard, PluginsError> (re-files the pending card with seq+1; `Invalid` if the manifest does not parse; error "nothing to approve" (`Invalid`) when the grant already covers it)
pub struct SetEnabled { pub name: String, pub on: bool }                    // → Result<PluginInfo, PluginsError> (no grant: Ok with state awaiting_approval, nothing starts)
pub struct SetHidden { pub name: String, pub placement: String, pub hidden: bool } // → Result<PluginInfo, PluginsError>
pub struct SetOrder { pub order: Vec<String> }                              // → Result<Vec<PluginInfo>, PluginsError> (names not listed keep their relative order after the listed ones)
pub struct Restart { pub name: String }                                     // → Result<PluginInfo, PluginsError> (also clears `failed`)
pub struct Uninstall { pub name: String, pub keep_data: bool }              // → Result<(), PluginsError>
pub struct Action {                                                         // → Result<Value, PluginsError>
    pub name: String, pub placement: String, pub action: String, pub args: Value,
    pub session: Option<SessionId>, pub client: String,
    /// The renderer's action id of the press; required for an OpenUI Mutation.
    pub activation: Option<String>,
    pub form: Option<Value>,
}
pub struct Query {                                                          // → Result<Value, PluginsError>
    pub name: String, pub placement: String, pub query: String, pub args: Value,
    pub session: Option<SessionId>, pub client: String,
}
pub struct Command {                                                        // → Result<CommandReply, PluginsError>
    pub name: String, pub command: String, pub args: String,
    pub session: Option<SessionId>, pub client: String,
}
pub struct Catalog { pub topic: Option<String> }                            // → Vec<CatalogEntry>
pub struct Surfaces;                                                        // → Vec<SurfaceSnapshot>
pub struct Surface {                                                        // → Result<SurfaceContent, PluginsError> (no content held + plugin running → sends `render`, returns state loading)
    pub name: String, pub placement: String, pub session: Option<SessionId>, pub client: String,
}
pub struct Logs { pub name: String, pub tail: u32 }                         // → Result<LogTail, PluginsError> (tail capped at 2000)
```

### 5.3 Wire events (`src/events.rs`)

```rust
/// A plugin emitted one of its declared events (216 §7.6). Topic `plugins.events` v1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct PluginEvent {
    pub plugin: String,
    /// Declared name without the plugin prefix (`limit_reached`).
    pub event: String,
    /// The plugin event's own version (`emits.<name>.version`).
    pub version: u32,
    pub session: Option<SessionId>,
    /// Redacted, a JSON object.
    pub data: Value,
}
impl wire::WireEvent for PluginEvent { const TOPIC: &'static str = "plugins.events"; }

/// Placement and lifecycle changes for clients (216 §2, §9). Topic `plugins.ui` v1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum PluginUi {
    /// A placement's dot/badge/label/tooltip changed (already state-overridden).
    PluginStatus { plugin: String, placement: String, dot: Dot, badge: Option<Badge>, label: Option<String>, tooltip: Option<String> },
    /// A placement has new content; fetch it with `plugin_surface` (big sources stay out of the feed).
    PluginSurface { plugin: String, placement: String, format: SurfaceFormat, revision: u64 },
    /// Re-run the placement's OpenUI queries without replacing the source (`invalidate`).
    PluginInvalidate { plugin: String, placement: String },
    /// A plugin changed lifecycle state.
    PluginState { plugin: String, state: PluginRunState, error: Option<String> },
    /// The set of plugins, their placements, order, hidden flags or cards changed: re-read `plugin_list`.
    PluginsChanged,
}
impl wire::WireEvent for PluginUi { const TOPIC: &'static str = "plugins.ui"; }
```

Wire JSON examples (what clients and other plugins see):
`{"type":"PluginStatus","plugin":"turns","placement":"turns","dot":"warn","badge":51,"label":"51/50","tooltip":"Turns today: 51"}`,
`{"type":"PluginSurface","plugin":"turns","placement":"turns","format":"openui","revision":7}`,
`{"type":"PluginsChanged"}`,
`{"type":"PluginEvent","plugin":"turns","event":"limit_reached","version":1,"session":null,"data":{"day":"2026-11-24","turns":50}}`.
Both topics are `Public` (design §7.3 default). The `event` notification a subscribed plugin receives keeps design §6.2's shape (`type: "turns.limit_reached"`, `data` = the `PluginEvent` JSON).

### 5.4 Enum additions outside the crate (B0)

```rust
// crates/roster/src/attention.rs — AttentionKind, last variant
/// 216 §10.2: an external plugin waits for a human press — install, widened manifest, or changed
/// code. Id `plugin:<name>:<m8>:<c8>:<seq>`. No timeout; Dismiss leaves it awaiting approval.
Plugin,

// crates/agent/src/communication.rs — ChannelVia, last variant (serde lowercase → "plugin")
/// 216 §10.3: a plugin's `input` request; `ChannelOrigin.from` is the plugin name, `chat` is `dm`.
Plugin,

// crates/tools/src/lib.rs — ToolSource, last variant (serde tag kind → "plugin")
/// 216 §8: declared in an external plugin's manifest.
Plugin {
    /// Plugin name (its folder).
    name: String,
    /// The manifest's `version:`.
    version: String,
},
```

### 5.5 Wire exposure (B1; design §7.3 verbatim)

B1 implements design §7.3 as written: `wire::Exposure`, the consts `EXPOSURE`, `DROP`, `STRIP`, `REVIEWED_PUBLIC`, `CONDITIONAL`, `fn plugin_view(json: &Value) -> Option<Value>` on `WireEvent`; the matching `EventDescriptor` fields; `x-theuth.events[].exposure`; and `impl WireFrame { pub fn external_view(&self) -> Option<std::borrow::Cow<'_, Value>> }` — **a method computed on call, no new field on `WireFrame`** (recon §2: a field breaks nine struct literals; the host has one feed receiver and caches per frame itself). This return type replaces the design's `Option<&Value>`. Owners marked (consts only): `secrets`, `webprofile`, `link`, `signal`, `telegram`, `mesh`, `a2a`, `sandbox`, `meeting`, `voice`, `agent`, `artifact`, `browser`, `rpc-compose` (`rpc.stream`), `sessionsync` (`follow-state` + the four `#[cfg(test)]` twins in `events.rs`, given the same consts as their production impls so the same-`TOPIC` agreement test holds). New tests: `crates/theuth/tests/wire_exposure.rs` (snapshot `wire_exposure.txt`, name lint, agreement test).

### 5.6 `tools` additions (B1)

`pub async fn expand(raw: &str) -> anyhow::Result<String>` and `pub async fn expand_map(…)` made public in `crates/tools/src/mcp.rs` (unchanged bodies); `pub fn risk_from_hints(read_only: Option<bool>, destructive: Option<bool>) -> Risk` that `annotated` now calls; `pub fn gate_for(mcp_dir: &Path, server: &str, tool: &str) -> Option<Gate>`; the loader refuses server names starting with `plugin.` (warning, entry skipped); the registry's gate-key match maps `ToolSource::Plugin { name, .. }` to the key `plugin.<name>`; a human-only Invoke for brokered calls (B1 picks the shape inside `crates/tools`, e.g. `pub struct InvokeHumanOnly(pub ToolCall)` whose `Ask` raises `RequestHumanOnly` titled "plugin <name> wants to run <tool>"; recon §6 "open design gap").

### 5.7 `agent` and `secrets` additions (B1)

`AgentEvent::MessageDelivered` gains `#[serde(default)] contact: bool` (from `PeerOrigin.contact_mode.is_some()`); `PeerDelivered`/`ContactFileChanged` views per design §7.3. `secrets`: `pub const PLUGINS_PREFIX: &str = "plugins/"`, `writable_name` accepts it (share's rules), `pub const PLUGINS_GRANTS_KEY: &str = "plugins/grants-key"`, `Vault::plugins_grants_key(&self) -> anyhow::Result<Zeroizing<[u8; 32]>>` (create-if-absent, as `mesh_device_key`), test `plugins_rows_are_versioned_hidden_and_unexpandable`. **Replication:** B1 checks whether `mesh/`/`share/` rows replicate with the vault; `plugins/grants-key` must not (a replicated key would make one engine's approvals verify on another, D9). If excluding it needs a change outside `crates/secrets`, it is a hand-off.

## 6. RPC (`src/rpc.rs`; B0 declares all, B2 implements)

```rust
#[rpc_core_macros::rpc_descriptors(owner = "plugins", descriptors = rpc_descriptors, audience = "owner")]
#[jsonrpsee::proc_macros::rpc(server, client)]
pub trait PluginsApi {
    /// Every plugin with its state, placements and pending card.
    #[method(name = "plugin_list", with_extensions)]
    async fn plugin_list(&self) -> RpcResult<Vec<PluginInfo>>;
    /// One plugin: manifest, requested and granted powers, pending card, stderr tail.
    #[method(name = "plugin_get", with_extensions)]
    async fn plugin_get(&self, name: String) -> RpcResult<PluginDetail>;
    /// Validate a plugin folder (absolute path) without starting it.
    #[method(name = "plugin_check", with_extensions)]
    async fn plugin_check(&self, path: String) -> RpcResult<CheckReport>;
    /// Copy a local plugin folder into plugins/ and file its install card. Runs nothing.
    #[method(name = "plugin_install", with_extensions)]
    async fn plugin_install(&self, path: String) -> RpcResult<InstallReport>;
    /// Scaffold a new plugin from the bundled SDK template (default `py`; `raw`: no SDK). Runs nothing.
    #[method(name = "plugin_new", with_extensions)]
    async fn plugin_new(&self, name: String, lang: Option<Lang>, raw: Option<bool>) -> RpcResult<ScaffoldReport>;
    /// Approve the pending card with exactly these hashes. Human press only (216 D14).
    #[method(name = "plugin_approve", with_extensions)]
    async fn plugin_approve(&self, name: String, manifest_sha256: String, code_sha256: Option<String>) -> RpcResult<PluginInfo>;
    /// Enable (start, if a verified grant covers it) or disable (stop) a plugin.
    #[method(name = "plugin_set_enabled", with_extensions)]
    async fn plugin_set_enabled(&self, name: String, on: bool) -> RpcResult<PluginInfo>;
    /// Hide or show one placement on every client.
    #[method(name = "plugin_set_hidden", with_extensions)]
    async fn plugin_set_hidden(&self, name: String, placement: String, hidden: bool) -> RpcResult<PluginInfo>;
    /// Reorder sidebar and status-strip items (names, first = top).
    #[method(name = "plugin_set_order", with_extensions)]
    async fn plugin_set_order(&self, order: Vec<String>) -> RpcResult<Vec<PluginInfo>>;
    /// Restart a plugin (clears a crash loop).
    #[method(name = "plugin_restart", with_extensions)]
    async fn plugin_restart(&self, name: String) -> RpcResult<PluginInfo>;
    /// Stop a plugin, drop its grant, move its folder (and, unless keep_data, its data) to plugins/.trash.
    #[method(name = "plugin_uninstall", with_extensions)]
    async fn plugin_uninstall(&self, name: String, keep_data: Option<bool>) -> RpcResult<()>;
    /// A press on a placement (OpenUI Mutation): delivered to the plugin as `action`.
    #[method(name = "plugin_action", with_extensions)]
    async fn plugin_action(&self, name: String, placement: String, action: String, args: serde_json::Value, session: Option<SessionId>, activation: Option<String>, form: Option<serde_json::Value>) -> RpcResult<serde_json::Value>;
    /// An OpenUI Query on a placement: delivered as `query`; rate-limited per plugin.
    #[method(name = "plugin_query", with_extensions)]
    async fn plugin_query(&self, name: String, placement: String, query: String, args: serde_json::Value, session: Option<SessionId>) -> RpcResult<serde_json::Value>;
    /// A `/plugin[:cmd]` command typed in a composer: delivered as `command`.
    #[method(name = "plugin_command", with_extensions)]
    async fn plugin_command(&self, name: String, command: String, args: String, session: Option<SessionId>) -> RpcResult<CommandReply>;
    /// Latest status of every placement (snapshot before following `plugins.ui`).
    #[method(name = "plugin_surfaces", with_extensions)]
    async fn plugin_surfaces(&self) -> RpcResult<Vec<SurfaceSnapshot>>;
    /// One placement's content; asks the plugin to render when none is held.
    #[method(name = "plugin_surface", with_extensions)]
    async fn plugin_surface(&self, name: String, placement: String, session: Option<SessionId>) -> RpcResult<SurfaceContent>;
    /// The event catalog plugins can subscribe to (optionally one topic).
    #[method(name = "plugin_catalog", with_extensions)]
    async fn plugin_catalog(&self, topic: Option<String>) -> RpcResult<Vec<CatalogEntry>>;
    /// The last `tail` lines (default 200, max 2000) of a plugin's log today.
    #[method(name = "plugin_logs", with_extensions)]
    async fn plugin_logs(&self, name: String, tail: Option<u32>) -> RpcResult<LogTail>;
}
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule;
```

| Method | Admission (B2) | Actor message | Notes |
|---|---|---|---|
| `plugin_list`, `plugin_get`, `plugin_check`, `plugin_surfaces`, `plugin_surface`, `plugin_catalog`, `plugin_logs` | `rpc_core::owner(ext)?` (guests refused; internal peer allowed) | `ListPlugins`, `GetPlugin`, `CheckPath`, `Surfaces`, `Surface`, `Catalog`, `Logs` | `client` = `ext.get::<rpc_core::Client>()` text, else `"desktop"` |
| `plugin_install`, `plugin_new`, `plugin_set_enabled`, `plugin_set_hidden`, `plugin_set_order`, `plugin_restart`, `plugin_uninstall` | `owner(ext)?` | `Install`, `New`, `SetEnabled`, `SetHidden`, `SetOrder`, `Restart`, `Uninstall` | `keep_data` default `false`, `tail` default 200, `lang` default `py`, `raw` default `false` |
| `plugin_action`, `plugin_query`, `plugin_command` | `owner(ext)?`; `plugin_action` with `activation: Some(_)` from the internal peer is refused (`NotAPress`): only a real client press carries an activation (as `app_call_tool`) | `Action`, `Query`, `Command` | |
| `plugin_approve` | `HumanPress::from_rpc(ext)?` (§8) — nothing else | `Approve` | `Stale` when the hashes are not the pending card's |

Errors: every actor `Err(PluginsError)` maps through `fn rpc_err(e: PluginsError) -> ErrorObjectOwned` (B2) using `e.code()` and `e.to_string()` (§9); a missing actor is `rpc_core::lookup`'s `NOT_RUNNING`. Events reach clients only through `subscribe_topics` on `plugins.events` and `plugins.ui`.

## 7. Agent tools, CLI, TUI (B2)

### 7.1 Agent tools (`src/atools.rs`)

The sandbox family is the template (`crates/sandbox/src/tools.rs`): one struct per verb, `impl tools::Tool`, `RegisterTool` once `rt.lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)` answers, each call one `ask` to `PLUGINS_ACTOR` (missing actor → `ToolOutput::err("plugins is not running")`; `PluginsError` → `ToolOutput::err(e.to_string())`). Output: a short text line plus `structured` = the reply as JSON. `CONSUMES_SECRETS = false`, `DIRECT = false`, `HUMAN_ONLY = false` for all ten — **no tool approves**; no agent tool constructs or mentions `HumanPress`.

| Tool | Risk | Args (JSON Schema from the `Args` struct) | RETURNS | Message | Result text |
|---|---|---|---|---|---|
| `plugin_list` | `ReadOnly` | `{}` | `list[dict]` | `ListPlugins` | one line per plugin: `name version state[ · needs approval][ · error]` |
| `plugin_status` | `ReadOnly` | `{name: string}` | `dict` | `GetPlugin` | state, error, the pending card's lines, the last 10 stderr lines |
| `plugin_logs` | `ReadOnly` | `{name: string, tail?: integer = 100}` | `dict` | `Logs` | the lines (already redacted) |
| `plugin_enable` | `Mutating` | `{name: string}` | `dict` | `SetEnabled{on:true}` | `started` / `awaiting approval — the owner must approve first` |
| `plugin_disable` | `Mutating` | `{name: string}` | `dict` | `SetEnabled{on:false}` | `stopped` |
| `plugin_restart` | `Mutating` | `{name: string}` | `dict` | `Restart` | new state |
| `plugin_new` | `Mutating` | `{name: string, lang?: "py"\|"js"\|"sh"\|"go"\|"rb"\|"rs" = "py", raw?: boolean = false}` | `dict` | `New` | files written + `next` |
| `plugin_install` | `Mutating` | `{path: string}` (absolute folder with `plugin.yaml`) | `dict` | `Install` | name + `next` |
| `plugin_uninstall` | `Destructive` | `{name: string, keep_data?: boolean = false}` | `dict` | `Uninstall` | `uninstalled; folder in plugins/.trash` |
| `plugin_approve_request` | `ReadOnly` | `{name: string}` | `dict` | `RequestApproval` | the card's lines + "the owner must press Approve in theuth or run `t plugin approve <name>`; agents cannot approve" |

Risk reasons: enable/restart start only code a human already approved for exactly these hashes; new/install write files and file a card, nothing runs; uninstall throws away an approval that only a human can give back; `plugin_approve_request` only puts an inbox item in front of the owner (ReadOnly so it is never itself an approval prompt). One `RegisterPrompt { plugin: "plugins", text: PROMPT }` with a constant `PROMPT` (no config in it):

> External plugins (design 216) are folders under the data dir's `plugins/` with a `plugin.yaml`; `plugin_new` scaffolds one (SDK in py, js, sh, go, rb or rs), `plugin_install` copies one in, `plugin_list`/`plugin_status`/`plugin_logs` inspect them, `plugin_enable`/`plugin_disable`/`plugin_restart`/`plugin_uninstall` manage them. Nothing new runs until the owner approves it in theuth or with `t plugin approve`; you cannot approve — use `plugin_approve_request` to put the card in front of the owner and say so.

`crates/theuth/harness.md` gains a `## Plugins` section (after `## MCP`): the folder layout (design §4.1), the ten tools with their risks, "approval is a human press: the app's Approve button or `t plugin approve <name>` on a terminal", and the `plugins:` keys of §4.

### 7.2 CLI `t plugin …` (`crates/theuth/src/plugin_cli.rs`; B0 writes the enums, B2 the behaviour)

```rust
/// `t plugin …` (216 §12).
#[derive(clap::Subcommand, Debug)]
pub(crate) enum PluginCmd {
    /// List plugins with their state.
    List { #[arg(long)] json: bool },
    /// Scaffold plugins/<name>/ from the SDK template (no engine needed).
    New { name: String, #[arg(long, value_enum, default_value_t = LangArg::Py)] lang: LangArg, /// No SDK: the raw protocol template (py or sh).
          #[arg(long)] raw: bool },
    /// Copy a local plugin folder into plugins/ and file its install card.
    Install { path: std::path::PathBuf },
    /// Review a plugin's pending card on this terminal and approve it (a human press).
    Approve { name: String },
    /// Enable a plugin (starts it if approved).
    Enable { name: String },
    /// Disable (stop) a plugin.
    Disable { name: String },
    /// Restart a plugin.
    Restart { name: String },
    /// Print today's log (no engine needed).
    Logs { name: String, #[arg(short = 'n', long, default_value_t = 100)] tail: u32, #[arg(short, long)] follow: bool },
    /// Uninstall: stop, drop the grant, move the folder to plugins/.trash.
    Uninstall { name: String, #[arg(long)] keep_data: bool },
    /// Validate a plugin folder without starting it.
    Check { path: std::path::PathBuf, /// Also run the plugin with --theuth-manifest and warn where code and plugin.yaml drift.
            #[arg(long)] sync: bool, #[arg(long)] json: bool },
    /// The event catalog plugins can subscribe to.
    Catalog { topic: Option<String>, #[arg(long)] json: bool },
}
/// `--lang` values (Lang).
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LangArg { Py, Js, Sh, Go, Rb, Rs }
```

| Subcommand | Engine | RPC | Output (exit 0 unless noted) |
|---|---|---|---|
| `list [--json]` | required | `plugin_list` | one line per plugin `NAME  VERSION  STATE[  needs approval][  ERROR]`; `--json`: the array |
| `new <name> [--lang L] [--raw]` | optional: RPC `plugin_new` when an engine answers, else `plugins::scaffold::new_plugin(data_dir.join("plugins"), …)` directly | `plugin_new` | the files, then `next` |
| `install <path>` | required | `plugin_install(abs path)` | name + `next` |
| `approve <name>` | required | `plugin_get`, then `plugin_approve` | refuses (exit 2, "t plugin approve needs a terminal") unless stdin **and** stdout are TTYs (`std::io::IsTerminal`); prints the card (title, kind, each line with its risk, changed paths, folder), asks `Approve <name> <version>? [y/N] ` on the tty; only `y`/`yes` approves; prints `approved <name>; starting`. No `--yes` flag, ever |
| `enable|disable|restart <name>` | required | `plugin_set_enabled` / `plugin_restart` | new state |
| `logs <name> [-n N] [-f]` | none: reads `<data_dir>/logs/plugins/<name>.log.<today>` | — | last N lines; `-f` polls every 500 ms and follows midnight rollover |
| `uninstall <name> [--keep-data]` | required | `plugin_uninstall` | `uninstalled <name>` |
| `check <path> [--sync] [--json]` | optional: `plugin_check` when an engine answers, else `plugins::manifest::check_offline` | `plugin_check` | errors as `path: message`, warnings, the card lines; exit 1 when `ok: false`. `--sync`: run `run` with `--theuth-manifest` appended (5 s timeout, stdin closed), parse the YAML, print drift warnings (tools, subscriptions, placements absent from plugin.yaml); never an error |
| `catalog [topic] [--json]` | required | `plugin_catalog` | `NAME  SCOPE  DESCRIBE` |

Without a running engine an engine-required subcommand prints `no harness on <listen>; start \`t --headless\` first` and exits 1 (the `cmd_67` wording). The CLI connects with `link_cli::connect(data_dir, &listen)` (bearer token → a non-internal, non-guest peer).

### 7.3 TUI (`crates/tui`, B2)

- `/plugins` built-in in `COMMANDS` (`src/model/slash.rs`), opening `Menu::Plugins { cursor }` (`src/model/menus.rs`): one row per plugin `● name state` (dot colour from `SurfaceSnapshot.dot` of its first status-bearing placement, else from state); `⏎` shows the plugin's `text` fallbacks and markdown/json-render content inline, OpenUI as "interactive on desktop/web"; `r` restart, `d` disable/enable, `l` log tail (`plugin_logs`, 40 lines). No approve: a row needing approval says `approve in the theuth app or with \`t plugin approve <name>\``.
- Status line (`src/model/status.rs` `status()`): one segment per visible `status` (and `sidebar` with `status: true`) placement, `● <label>` (or the plugin name), coloured by dot, at most `status_icons` (3).
- Plugin `command` placements join the slash specs as `/<plugin>` / `/<plugin>:<name>` with their description, through the existing components/slash-spec path (`src/components/registry.rs`), built-ins winning; running one calls `plugin_command` and prints `text` (or the `ui.text` fallback) as a note.
- Data: `plugin_list` + `plugin_surfaces` on connect, refreshed on `plugins.ui` events.
- Check `crates/tui/tests/no_emoji.rs` and `model/tests.rs` (`COMMANDS.len()` count) after adding `/plugins`. New test: every TUI `COMMANDS` name (without `/`) is in `plugins::manifest::RESERVED_NAMES`.

## 8. The human press (D14, Q3)

`src/press.rs` (B0 writes it complete; B2 owns it):

```rust
/// Proof that a request is a human press (216 D14; 171 §12.1 invariant 1): an authenticated RPC
/// peer that is not a share guest, not the engine-internal peer, not closed, and not forwarded
/// from another engine. Only `from_rpc` builds one; it is not `Deserialize`.
#[derive(Clone, Debug)]
pub struct HumanPress { peer: u64, client: String, remote: bool }

impl HumanPress {
    /// Admit the RPC call behind `ext` as a press, or `PluginsError::NotAPress`.
    pub fn from_rpc(ext: &jsonrpsee::Extensions) -> Result<Self, PluginsError> {
        let Some(p) = ext.get::<rpc_core::RpcPeer>() else { return Err(PluginsError::NotAPress) };
        if p.guest().is_some() || p.is_internal() || p.is_closed() { return Err(PluginsError::NotAPress); }
        if ext.get::<agent::rpc_forward::Forwarded>().is_some() { return Err(PluginsError::NotAPress); }
        let client = ext.get::<rpc_core::Client>().map(|c| c.0).unwrap_or("desktop").to_owned();
        Ok(Self { peer: p.id(), client, remote: p.remote() })
    }
    /// `x-theuth-client` of the pressing connection (`desktop`, `ios`, …; the CLI reports `desktop`).
    pub fn client(&self) -> &str { &self.client }
    /// The connection id.
    pub fn peer(&self) -> u64 { self.peer }
    /// A relay (phone/web) connection rather than this machine.
    pub fn remote(&self) -> bool { self.remote }
    #[cfg(test)]
    pub(crate) fn for_test() -> Self { Self { peer: 0, client: "desktop".into(), remote: false } }
}
```

| Who | Path | Result |
|---|---|---|
| Desktop / phone / web app Approve button (install screen, Settings › Plugins) | RPC `plugin_approve` over the owner's socket or relay | **press** |
| `t plugin approve` on a terminal | `link_cli::connect` with the data dir's bearer token → non-internal, non-guest peer; the CLI itself demands a TTY and a typed `y` | **press** (owner decision Q3) |
| The model (any tool, including `plugin_approve_request`) | in-process tools; no tool sends `Approve` and none can build a `HumanPress` | refused (no path) |
| The double | answers approval rows only; the install card is an attention item, not an approval row | refused (no path) |
| A channel user (Slack/Telegram/Signal) | channels render no install card; they reach only the agent | refused (no path) |
| TUI, ACP half, MCP console, family-forwarded calls | the internal peer / `Forwarded` | `NotAPress` (`-32002`) |
| A plugin | its pipe has no approve method and no tool approves | refused (no path) |
| A share-link guest | `guest()` | `NotAPress` |

What the press writes is design §10.2 "What the press writes"; the grant also records `approved_via: "<client>"` (not MAC-relevant beyond being in the entry). Residual, stated in design §10.4: any local process holding the token (an approved `shell`, a plugin, a PTY the agent drives) can act as a client; the TTY rule only stops non-interactive scripts.

## 9. Errors

### 9.1 `PluginsError` (`src/error.rs`, B0, frozen)

```rust
/// Every failure of the plugins host, as actor replies, RPC errors and tool errors carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginsError {
    NotFound { name: String },                       // "no plugin named {name}"
    Invalid { message: String },                     // "{message}" (manifest error with its JSON Pointer, bad name, nothing to approve)
    Exists { name: String },                         // "plugins/{name} already exists"
    AwaitingApproval { name: String },               // "{name} is awaiting approval: press Approve in theuth or run `t plugin approve {name}`"
    NotAPress,                                       // "plugin_approve needs a human press: the Approve button in theuth or `t plugin approve` on a terminal"
    Stale { name: String },                          // "the card for {name} changed; review it again"
    NotRunning { name: String },                     // "plugin {name} is not running"
    NotGranted { what: String },                     // "not granted: {what}"
    RateLimited { retry_after_ms: u64 },             // "rate limited; retry in {retry_after_ms} ms"
    Exited { name: String },                         // "plugin {name} exited"
    Timeout { name: String, method: String },        // "plugin {name} did not answer {method} in time"
    Plugin { code: i64, message: String },           // "{message}" — the plugin's own JSON-RPC error, passed through
    Io { message: String },                          // "{message}"
    NotImplemented { what: String },                 // "{what}: not implemented yet (216)"
}
impl std::fmt::Display for PluginsError { /* exactly the strings in the comments */ }
impl std::error::Error for PluginsError {}
impl PluginsError {
    /// JSON-RPC code for RPC callers (216 §6.5 plus rpc-core's).
    pub fn code(&self) -> i32 {
        match self {
            Self::NotAPress => -32002,                // rpc_core::UNAUTHORIZED
            Self::NotGranted { .. } => -32003,
            Self::RateLimited { .. } => -32005,
            Self::Exited { .. } | Self::NotRunning { .. } => -32006,
            Self::Invalid { .. } => -32602,
            _ => -32000,                              // rpc_core::ASK_FAILED
        }
    }
}
```

`rpc_err` (B2) attaches `data` for `RateLimited` (`{"retry_after_ms": n}`) and `Plugin` (`{"code": n}`). "plugins is disabled" is not a variant: a disabled host has no actor, so `rpc_core::lookup` answers `NOT_RUNNING` (`-32001`).

### 9.2 Pipe errors (host ↔ plugin, B1)

Exactly design §6.5: `-32601`, `-32602` (`data.path` JSON Pointer), `-32003` (`data: {capability|topic|placement|tool}`), `-32004`, `-32005` (`data.retry_after_ms`), `-32006`. Mapping when a plugin's answer goes back to an RPC caller: a plugin `error` → `PluginsError::Plugin { code, message }`; no answer in `request_timeout_ms` → `Timeout`; the plugin exited with the request pending → `Exited`.

## 10. Attention item, cards, scaffold

### 10.1 Install card as an attention item (B1 `attention.rs`)

The host registers one `roster::attention::Source` (`RegisterSource`, a `Weak` to its state as `a2a/src/contacts.rs` does). `items()` returns one `AttentionItem` per pending card:

| Field | Value |
|---|---|
| `id` | `plugin:<name>:<m8>:<c8>:<seq>` (as `InstallCard.id`) |
| `kind` | `AttentionKind::Plugin` |
| `title` | Install: `Install plugin <name> <version>?` · Widen: `Plugin <name> asks for more` · Code changed: `Plugin <name> changed its code` — prefixed by nothing; the UI adds the "plugin · `<name>`" attribution |
| `detail` | the first two card lines joined with ` · `, then `· open theuth to review` (channels and phone see this) |
| `at_ms` | `InstallCard.at_ms` |
| `session`, `task`, `agent`, `approval`, `decision`, `packet`, `ask`, `turn` | `None` |
| `expires_ms` / `waiting_since_ms` | unset (never times out) |

The screen fetches the card with `plugin_get(name)` → `PluginDetail.card` (the full hashes travel there, never in the item). Dismiss is the attention actor's normal dismiss of that id: the plugin stays `awaiting_approval`, no card until the manifest or code changes or `RequestApproval` bumps `seq`. `notify` items (capability `notify`) are items of the same Source, also `kind: AttentionKind::Plugin`, id `plugin-notify:<name>:<n>`, `title` = the plugin's title, `detail` = body; the UI tells them apart by the id prefix `plugin-notify:` (no Approve button; Open → the placement in `open`). OS notification at `warn`/`error` through the desktop's existing path.

### 10.2 Scaffold (B2 `scaffold.rs` + `build.rs`)

- `build.rs` walks `../../sdk/templates/**` and `../../sdk/{python,ts,go,rust,ruby,sh}/**` (skipping `target/`, `node_modules/`, dot-files) and writes `$OUT_DIR/sdk_files.rs`: `pub static SDK_FILES: &[(&str, &[u8])] = &[("templates/py/main.py", include_bytes!("…")), …];` with `cargo:rerun-if-changed=../../sdk`. A missing `sdk/` yields an empty table (the crate still builds; `new_plugin` then answers `Io { "the SDK templates are not bundled in this build" }`).
- Language → SDK directory and vendored files (copied into the plugin folder root unless the template says otherwise):

| `--lang` | SDK dir | Vendored file(s) | Template dir |
|---|---|---|---|
| `py` | `sdk/python/` | `theuth_plugin.py` | `sdk/templates/py/` |
| `js` | `sdk/ts/` | `theuth-plugin.js`, `theuth-plugin.d.ts` | `sdk/templates/js/` |
| `sh` | `sdk/sh/` | `theuth-plugin.sh` | `sdk/templates/sh/` |
| `go` | `sdk/go/` | `theuth.go` | `sdk/templates/go/` |
| `rb` | `sdk/ruby/` | `theuth_plugin.rb` | `sdk/templates/rb/` |
| `rs` | `sdk/rust/` | `theuth-plugin/Cargo.toml`, `theuth-plugin/src/lib.rs` | `sdk/templates/rs/` |
| `py --raw`, `sh --raw` | — | none | `sdk/templates/raw-py/`, `sdk/templates/raw-sh/` |

  The SDK builder owns those paths. If its committed layout differs (e.g. `sdk/templates/presses/<lang>/` with a shared `plugin.yaml`, research §10), B2 follows the committed tree, keeps the `--lang` values above, and records the mapping in its as-built. The template's own files (its `plugin.yaml`, `go.mod`, `Cargo.toml`) decide where the vendored SDK must sit; B2 reads them, it does not edit `sdk/`.
- Renaming: the templates use the placeholder plugin name `presses`. In every text file copied, replace `presses_` with `<name with '-' → '_'>_` first, then `presses` with `<name>`. File names containing `presses` are renamed the same way.
- `check_name(name)` first; `Exists` if `<plugins_dir>/<name>` exists; write to a temp dir beside it and rename into place (no half-written plugin for the watcher to see); mark `run.sh`/`main` scripts executable (`0o755`).
- Raw is allowed only for `py` and `sh` (`Invalid { "--raw has templates for py and sh only" }`).
- Default (no `--lang`) is `py` with the SDK. The new folder has no grant, so the watcher files its install card; `next` says how to approve.

## 11. Build gates and acceptance (each command under `lockrun`, §0)

`M` = `mise exec --`. Gates are crate-scoped. Test names below are the names the builder must give its tests, so the acceptance is runnable as written.

### 11.1 B0

1. `lockrun $M cargo check -p plugins -p theuth` — green.
2. `lockrun $M cargo test -p plugins` and `lockrun $M cargo clippy -p plugins --no-deps -- -D warnings` — green (stubs + a test that both topics declare and that `PluginsConfig::default()` equals §4).
3. `lockrun $M cargo check -p rpc-compose --tests` — the test composition compiles with `plugins::rpc_module`.
4. `lockrun $M just wire-schema`, then `lockrun $M cargo test -p theuth --test wire_schema_snapshot`: the classifier reports **additions only** (two topics, 19 `plugin_*` methods, `AttentionKind::plugin`, `ChannelVia::plugin`, `ToolSource::plugin`). Then `lockrun $M just wire-schema-accept` and the snapshot test again — green.
5. `bun install` in `crates/desktop/ui`, `packages/apps`, `packages/documents` (three short holds), `lockrun $M just ts-desktop`.
6. `cd crates/desktop/ui && lockrun bun run check` — green (placeholders of §2.1 in place).
7. Acceptance: `grep -o '"plugin_[a-z_]*"' bindings/openrpc.json | sort -u | wc -l` prints 19; `ls crates/desktop/ui/src/rpc/bindings | grep -E '^method\.plugins\.|^event\.plugins_2e(ui|events)\.'` lists the files; `git diff --name-only HEAD~1 -- crates/plugins | wc -l` ≥ 20.

### 11.2 B1

Gates: `cargo test -p plugins`, `cargo clippy -p plugins --no-deps -- -D warnings`, `cargo test -p wire`, `cargo test -p secrets plugins_rows`, `cargo test -p tools --lib mcp`, `cargo test -p agent --lib contact_`, `cargo check -p <crate>` for every owner crate of §5.5, `cargo test -p theuth --test wire_exposure`, `just wire-schema` + `cargo test -p theuth --test wire_schema_snapshot` (additions only: `exposure` keys, `MessageDelivered.contact`) + `just wire-schema-accept`, `cargo test -p rpc-compose --test wire_golden`, then one `just ts-desktop` and `bun run check` in `crates/desktop/ui` (B1 may regenerate; it must not edit UI files — a UI type error goes to B3/B4 as a hand-off).

Acceptance (unit tests in `crates/plugins`, run with `cargo test -p plugins <name>`):
- `manifest::tests::design_example_parses` (design §4.2), `examples_parse` (`docs/design/216-examples/*/plugin.yaml`), `errors_have_pointer_paths`, `isolation_and_emit_exposure_are_errors`, `reserved_kind_is_skipped_with_warning`, `tools_inline_is_an_error`, `code_hash_covers_run_and_watch_only`, `name_rules` (regex, `RESERVED_NAMES`).
- `grants::tests::roundtrip_verifies`, `any_byte_change_fails_verification`, `enabled_is_mac_covered_order_hidden_are_not`, `handwritten_grant_does_not_verify`.
- `proc::tests::backoff_doubles_then_fails` (1 s, 2 s, 4 s…, `failed` after 5 in 60 s; use tokio paused time), `exit0_on_failure_stays_exited`, `long_line_discarded_next_read`, `banner_lines_in_first_5s_tolerated`, `control_lane_before_queued_events` (an `action` queued behind 5 000 events is written first), `orphan_group_reaped_at_start`.
- `route::tests::vault_value_reaches_plugin_redacted`, `meeting_session_text_stripped_for_life`, `remote_session_dropped_unless_enabled`, `overflow_queues_one_lagged`, `descriptorless_frame_dropped`, `ungranted_topic_not_delivered`.
- `ui::tests::masked_input_refused` (an OpenUI `Input` with a `"password"` argument → `log` error with a path, old surface kept), `identical_source_keeps_revision`, `bindings_come_from_check` (a `Mutation("reset", {})` statement yields `SurfaceBinding { kind: Mutation, name: "reset" }`), `oversize_surface_refused`.
- `actor::tests::approve_stale_hashes_refused`, `approve_writes_grant_and_starts`, `card_survives_approval_timeout` (still pending after `tools.approval_timeout_secs`), `code_change_with_secret_files_card_keeps_old_process`.
- `wire` unit tests: `external_view_none_without_descriptor`, `drop_and_strip_apply`; `crates/theuth/tests/wire_exposure.rs`: snapshot, name lint, same-`TOPIC` agreement.
- `host::tests::turns_example_end_to_end` (a unit test, so it may use `HumanPress::for_test()`): an in-process `Runtime` with the plugins host, `docs/design/216-examples/turns` installed into a temp data dir and approved, one published `AgentEvent::TurnFinished` → `plugin-data/turns/state.json` has `"turns": 1` within 5 s; `plugin_action(turns, turns, reset)` → the next surface shows `**0**`.

### 11.3 B2

Gates: `cargo test -p plugins press:: rpc:: atools:: scaffold::`, `cargo test -p plugins --test scaffold`, `cargo clippy -p plugins --no-deps -- -D warnings`, `cargo check -p theuth`, `cargo test -p tui`, `cargo clippy -p tui --no-deps -- -D warnings`.

Acceptance:
- `press::tests::internal_peer_refused` (`RpcPeer::internal()`), `guest_refused` (`RpcPeer::open_guest`), `forwarded_refused` (`agent::rpc_forward::Forwarded` in the extensions), `local_peer_admitted` (`RpcPeer::open()`), `no_peer_refused`.
- `rpc::tests::approve_over_internal_peer_is_32002`, `action_with_activation_from_internal_peer_refused`.
- `scaffold::tests::every_lang_writes_manifest_and_sdk` (skipped with a message when `SDK_FILES` is empty), `raw_only_for_py_and_sh`, `placeholder_renamed` (`presses_count` → `my_tool_count` for `my-tool`), `existing_folder_refused`, `name_rules_enforced`.
- `crates/plugins/tests/scaffold.rs`: for each language whose toolchain is on `PATH` (python3, node, bash, ruby, go, cargo), `new_plugin` into a temp dir then `<run> --theuth-manifest` exits 0 within 10 s (go/rs: build first; skip when absent).
- `! grep -n HumanPress crates/plugins/src/atools.rs` (exit 0 = no match).
- CLI: `lockrun $M cargo run -q -p theuth --bin t -- --data-dir /tmp/216-cli plugin new demo --lang sh && test -x /tmp/216-cli/plugins/demo/run.sh`; `echo y | ./target/debug/t --data-dir /tmp/216-cli plugin approve demo; test $? -eq 2` (TTY check runs before connecting); `./target/debug/t --data-dir /tmp/216-cli plugin check /tmp/216-cli/plugins/demo` exits 0 offline.
- `cargo test -p tui tui_commands_are_reserved_plugin_names`.

### 11.4 B3 and B4

Gates (in `crates/desktop/ui`): `lockrun bun run check`; `lockrun bun run test -- <your test files>`; the ratchets `src/lib/no-emoji.test.ts`, `src/components/settings/registry.test.ts`, `src/components/settings/route-target.test.ts` (B3), `src/components/chat/Composer.test.ts`, `src/components/status/StatusBar.test.ts`, `src/components/sidebar/AppSidebar.test.ts`, `src/lib/popout.test.ts` (B4). Never hand-edit `src/rpc/bindings/**`.

B3 acceptance: `registry.test.ts` pins thirteen categories ending in `plugins` and `plugins → [['plugins','Plugins']]`; `Plugins.test.ts` — rows from a fake engine show name, version, state chip; Approve shows only when `needs_approval` and calls `plugin_approve(name, card.manifest_sha256, card.code_sha256)` verbatim; Uninstall goes through an `AlertDialog`; a placement Hide switch calls `plugin_set_hidden`; New plugin calls `plugin_new(name, lang, raw)`; Open folder is absent when `container()` is `web` or `ios`. `PluginInstallScreen.test.ts` — renders the card lines and changed paths, Approve calls `plugin_approve`, a `plugin-notify:` item renders without Approve. `GrantLines.test.ts` — one row per line, `new` lines marked, risk shown.

B4 acceptance: `lib/plugins.test.ts` — the store applies `PluginStatus`, `PluginSurface`, `PluginInvalidate`, `PluginState`, `PluginsChanged`; `provider.test.ts` — mutation binding + `action_id` → `plugin_action`, query binding (load or press) → `plugin_query`, mutation without `action_id` and unknown statement → rejected; `SidebarSlot.test.ts` — 6 plugins → 5 icons + an overflow button, hidden placements absent, a `warn` dot in the overflow marks the overflow button; `StatusStrip.test.ts` — cap 3 + overflow, renders nothing on mobile; `PluginSurface.test.ts` — the attribution text `plugin · turns` is outside the content node, markdown raw HTML (`<input>`) is not rendered, json-render renders; `slash.test.ts` — `/turns` and `/turns:reset` come after built-ins with source `plugin · turns`, `isCommand('/turns:reset …')` is true, `COMMANDS` unchanged; `popout.test.ts` — a `{tile:'plugin', props:{plugin:'turns', placement:'turns'}}` target round-trips.

## 12. UI contract (B0 primitives, B3/B4 consume)

### 12.1 Types

Generated from `bindings/openrpc.json` by `just ts-desktop`; import types from `@/rpc/bindings/method.plugins.output` (methods) and `@/rpc/bindings/event.plugins_2eui.output` / `event.plugins_2eevents.output` (topics). The TS names are the Rust names of §5 (`PluginInfo`, `PluginDetail`, `InstallCard`, `GrantLine`, `GrantKey`, `CardKind`, `Grants`, `PlacementInfo`, `PlacementKind`, `CommandInfo`, `SurfaceSnapshot`, `SurfaceContent`, `SurfaceBinding`, `BindingKind`, `SurfaceFormat`, `Dot`, `Badge`, `PluginRunState`, `CommandReply`, `CommandUi`, `CheckReport`, `ScaffoldReport`, `Lang`, `LogTail`, `CatalogEntry`, `PluginUi`); B0's as-built lists the exact generated names and files. Call methods through `useApi()` (`api.plugin_list()`, `api.plugin_approve(name, manifest_sha256, code_sha256)`, …; positional, in §6 order). Gate UI with `useQuerySupport(['plugin_list'])`; topic keys are `"plugins.ui@1"` / `"plugins.events@1"`.

### 12.2 Who listens to what

- B3 adds to `lib/rpc.ts`: `queryKeys.plugins = ['plugins']`, `queryKeys.plugin = (name: string) => ['plugins', name]`, `queryKeys.pluginLogs = (name: string) => ['plugins', 'logs', name]`; in `useInvalidateOn`: `on("plugins.ui@1", 'PluginState', …)` and `on("plugins.ui@1", 'PluginsChanged', …)` invalidating `['plugins']`.
- B4's `lib/plugins.ts` (zustand) seeds from `plugin_list` + `plugin_surfaces` and registers its own `on("plugins.ui@1", …)` handlers for all five variants (it does not edit `lib/rpc.ts`). Exports: `usePluginPlacements(kind: PlacementKind): Array<{ plugin: PluginInfo; placement: PlacementInfo; status: SurfaceSnapshot | undefined }>` (ordered by `PluginInfo.order`, hidden removed), `usePluginCommands(): Array<{ plugin: string; command: CommandInfo }>` (hidden included: commands still work), `usePluginSurface(plugin: string, placement: string, session: string | null): { content: SurfaceContent | undefined; refresh(): void }`.

### 12.3 Shared primitives (B0 writes, frozen)

```ts
// UI/components/plugins/icons.tsx
export const PLUGIN_ICONS: Record<string, LucideIcon>   // ~40 curated lucide icons by kebab-case name; never `import * as`
export function PluginIcon(props: { icon: string; className?: string }): JSX.Element   // 'svg:<data-url>' → <img src>, unknown → Puzzle
// UI/components/plugins/PluginDot.tsx
export function PluginDot(props: { dot: Dot; className?: string }): JSX.Element | null  // ok bg-success · busy bg-success animate-pulse · warn bg-warning · error bg-danger · off bg-fg-tertiary/40 · none → null
// UI/components/plugins/state.ts
export const STATE_LABEL: Record<PluginRunState, string>   // running, starting, crashed (backoff), failed, exited, needs approval, disabled, invalid, stopping
export const STATE_DOT: Record<PluginRunState, Dot>         // running ok · starting/stopping busy · backoff warn · failed/invalid error · exited/disabled/awaiting_approval off
```

No emoji or `⋯`/`●` code points in UI source (use lucide `Ellipsis`, spans with classes).

### 12.4 OpenUI bridge — the query/mutation question, resolved

`ProviderContext` (`packages/apps/src/runtime-types.ts`) carries no query/mutation discriminator: a pressed `@Run(Query)` and a Mutation both arrive with `trigger: 'press'` and an `action_id`. **Decision:** the host classifies statements when it validates a `ui` (`app::helper::Helper::check` returns `CheckResult.bindings: Vec<AppBinding { statement_id, tool, kind: "query"|"mutation", … }>`; B1 passes one `ToolSnapshot` per `Query("…")`/`Mutation("…")` name found in the source — schema `{"type":"object"}`, risk `read_only`/`mutating`, identity `plugin:<plugin>:<name>` — so the validator binds them) and ships the table as `SurfaceContent.bindings`. B4's `components/plugins/provider.ts`:

```ts
export function pluginProvider(api: Api, p: { plugin: string; placement: string; session: string | null; bindings: readonly SurfaceBinding[] }): AppToolProvider {
  const byId = new Map(p.bindings.map((b) => [b.statement_id, b]))
  return (_tool, args, ctx) => {
    const b = byId.get(ctx.statement_id)
    if (!b) return Promise.reject(new Error(`unknown statement ${ctx.statement_id}`))
    if (b.kind === 'mutation') {
      if (!ctx.action_id) return Promise.reject(new Error('a mutation needs a press'))
      return api.plugin_action(p.plugin, p.placement, b.name, args ?? {}, p.session, ctx.action_id, null)
    }
    return api.plugin_query(p.plugin, p.placement, b.name, args ?? {}, p.session)
  }
}
```

`onAction`: `open_url` → `window.open` for `^https?://` only; `CopyToClipboard` → `copyToClipboard`; `continue_conversation` → `openDraftSession` + `appendDraft` (drafted, never sent). A new `revision` with an identical source must not remount (design §6.3); `PluginInvalidate` re-runs queries (renderer `refresh`). The masked-input refusal is host-side (B1 `ui.rs`, over the validated source: any `Input(…)` call with a `"password"` argument is refused) — **no change to `packages/apps`** in v1 (avoids the apps-bundle gate).

### 12.5 Placements (B4)

- Sidebar: rail icons after `<RailSections/>` in `AppSidebar.tsx`; expanded row between `<Desks/>` and `<SessionList/>`; cap 5 then a lucide `Ellipsis` `DropdownMenu`; dot + badge on the icon (badge pattern of the Inbox); click → `PlacementHost`: popover (`components/ui/popover`, `PlacementInfo.size` or 360 × 280, `max-h-(--radix-popover-content-available-height)`, no viewport units) or, for `open: tile`, `openPluginTile(focusedChat, { plugin, placement: <its tile placement> })`, falling back to the popover when `openTile` returns `null`; on mobile a bottom `Sheet`. Context menu: Hide (`plugin_set_hidden`).
- Status: `StatusStrip` in `StatusBar`'s `right` slot only (cap 3 + overflow; tooltip = label + tooltip); hidden under `useIsMobile()`; on the phone the items render in the sidebar sheet footer.
- Tile: `TILE_KINDS.plugin` / `TILE_ICONS.plugin` (`Puzzle`); `PluginTileProps = { plugin: string; placement: string }`; `openPluginTile(host, p) = openTile({ kind: 'plugin', host, key: \`${p.plugin}/${p.placement}\`, props: p })`; the leaf renders `TileFrame` + `PluginSurface`.
- Commands: `slashMenu(word, harness, plugins?: readonly { plugin: string; command: CommandInfo }[])` appends after built-ins and harness commands with `source: \`plugin · ${plugin}\``; `isCommand(line, skills, plugins?)`; `run()` gains a branch before `default:` calling `api.plugin_command(plugin, command.name, rest, session)` and showing `text` as an ephemeral note or `ui` in an attributed ephemeral `PluginSurface` card. `COMMANDS` and `dictationCommands` stay untouched.
- `PluginSurface` (props `{ plugin, placement, session, content }`): `Attribution` strip (plugin icon + `plugin · <name>`) above and outside the content node; `openui` → `AppRenderer` with `pluginProvider`; `json-render` → `JsonRenderFence`'s renderer; `markdown` → the chat markdown components with raw HTML off; `state: loading` → a host-drawn loading frame, `error` → the message.

### 12.6 Settings › Plugins and the install screen (B3)

- `categories/plugins.tsx`: `{ id: 'plugins', label: 'Plugins', icon: Puzzle, tabs: [{ id: 'plugins', label: 'Plugins', icon: Puzzle, Component: Plugins, required: { methods: ['plugin_list','plugin_get','plugin_approve','plugin_set_enabled','plugin_set_hidden','plugin_set_order','plugin_restart','plugin_uninstall','plugin_logs','plugin_new'], topics: ['plugins.ui@1'] } }] }`, appended last to `CATEGORIES` and `'plugins'` last in `TAB_IDS`.
- `Plugins.tsx` (pattern: `components/library/Tools.tsx` `McpServersTab`): a `Section` "Plugins" with actions Refresh + New plugin (`NewPluginPopover`: name, a `ToggleGroup` py/js/sh/go/rb/rs, a Raw switch enabled for py/sh → `plugin_new`; afterwards a note pointing at the card). One collapsible row per plugin: icon, name, version, state chip (`STATE_LABEL`/`STATE_DOT`), enable `Switch`, Restart, Logs (`LogsSheet`: `plugin_logs(name, 200)`, refresh button), Uninstall (`AlertDialog` with a keep-data `Switch`), up/down reorder (`plugin_set_order`). Expanded: the pending card (when `needs_approval`) as `GrantLines` + Approve; "Granted" `GrantLines` from `PluginDetail.granted` (read-only in v1); per-placement Hide switches; tools with risk; Open folder (`openPath(info.dir)` from `@tauri-apps/plugin-opener`, only when `container()` is `macos|linux|windows`); stderr tail; error text. `?focus=<name>` expands and scrolls to that row. Notices go through `@/lib/notify` (no toasts).
- `PluginInstallScreen.tsx`: for ids `plugin:<name>:…` — parse `<name>`, `plugin_get(name)`, show the attribution, title, `GrantLines(card.lines)`, changed paths, folder, the unverified notice; Approve → `plugin_approve(name, card.manifest_sha256, card.code_sha256)`; Review → `/settings/plugins?focus=<name>`; Snooze/Dismiss as usual. For ids `plugin-notify:…` — title/body and Open (opens the plugin's placement). `labels.ts`: `KIND_LABEL.plugin = 'plugin'`, not in `LEFT_KINDS`, its own dismiss hint "Dismiss: the plugin stays unapproved until it changes"; `Card.tsx` `needsPane()` true for `plugin`.

## 13. Per-builder reading list and work (for the briefs)

- **B0** — read §2.1, §3–§6, §7.2's enums, §8's `press.rs`, §9.1, §12.3; design §2, §3, §12. Write exactly §2.1; stubs compile; no behaviour. Typical traps: `missing_docs` on every pub item; `#[serde(tag = "type")]` + `schemars(transform = wire::serde_type_tag::<Self>)` on the struct event; the local variable `plugins` in `main.rs`'s `engine_plugins` does not shadow the crate path `plugins::…` (paths resolve in the type namespace), but do not rename it.
- **B1** — read §3.4, §4, §5, §9, §10.1, §12.4 (the host half), §11.2; design §0, §1 (D3–D15), §4–§8, §9.2–§9.4, §10, §11, §13, §15 steps 1–11; `/tmp/theuth-216/recon-backend.md` §2–§7, §10, §11. Implement the host per the design with this contract's names; the router order is: feed frame → `external_view()` → drop when the frame's `policy.session` is in the remote set (unless `remote_sessions`) → per-plugin grant match → meeting-session strip (for the session's whole life, Q1) → contact-sender strip (in `plugin_view`) → `redact_value` → `string_cap` → event lane. Seed the meeting set from `meeting::ListMeetings { limit: Some(u32::MAX), project: None }` **after** subscribing to the feed. Track remote sessions from `agent.events#SessionOwnerChanged` (`here: false` inserts, `here: true` removes).
- **B2** — read §6, §7, §8, §9.1, §10.2, §11.3; design §12, §14; `/tmp/theuth-216/research-sdk.md` §5, §6, §10; `sdk/README.md` once the SDK builder commits it; recon-backend §8, §9. RPC impls are lookup + one `ask` (handlers that wait on a child are B1's concern). `t plugin approve` checks the TTY before it connects.
- **B3** — read §2.3, §12 (all but 12.5); design §9.6 (with recon-ui's corrections), §10.1–§10.2; `/tmp/theuth-216/recon-ui.md` §0–§2, §7, §10.
- **B4** — read §2.3, §12 (all but 12.6); design §9.1–§9.5; recon-ui §3–§6, §8, §10.

## 14. Decisions this contract made (beyond the design and the owner's rulings)

1. **Query vs Mutation routing** (recon-ui open question): the host classifies OpenUI statements at validation time and ships `SurfaceContent.bindings`; the client routes by `ProviderContext.statement_id`. No change to `packages/apps`.
2. **Masked-input refusal is a Rust scan of the validated source**, not a new mode in `packages/apps` (keeps the apps-bundle gate out of v1).
3. **`external_view()` is a method computed on call** returning `Option<Cow<'_, Value>>`, no cached field on `WireFrame`; the host caches per frame.
4. **Forwarded calls are not a press** for `plugin_approve` (stricter than `cells`): a plugin runs on the engine whose disk holds it, so its approval must come from a client connected to that engine (desktop, phone over relay, or `t plugin approve` there).
5. **`t plugin approve` requires a TTY on stdin and stdout and a typed `y`; no `--yes`.** It is a press like the app's button; the TTY rule only keeps scripts from approving by accident.
6. **Agent-tool risks:** list/status/logs/approve_request `read_only`; enable/disable/restart/new/install `mutating`; uninstall `destructive`. `plugin_approve_request` is `read_only` because its only effect is an inbox item.
7. **Grant narrowing from Settings is not v1** (Settings shows granted lines read-only); the owner narrows by editing the manifest. `notify` to chat channels is not v1.
8. **Sidebar/status caps are UI constants** (5 and 3); the design's `sidebar_icons` config key is dropped (the UI cannot read config, and an unused key would lie).
9. **Install-card ids gain a `:<seq>` suffix** so `plugin_approve_request` can re-file a card the owner dismissed.
10. **`plugin_new` runs offline too** (the CLI calls `scaffold::new_plugin` when no engine answers); the SDK files are embedded in `t` by `crates/plugins/build.rs` from `sdk/`, so no extraction to `~/.theuth/sdk/` is needed (research §6's extraction step is dropped).
11. **New RPC `plugin_set_order`** (Settings reorder) and **RPC `plugin_new`**; the design's "Install" message, `plugin_check`, `plugin_catalog`, `plugin_logs` keep their names.
12. **Notify items reuse `AttentionKind::Plugin`** with id prefix `plugin-notify:` instead of a second new kind.
