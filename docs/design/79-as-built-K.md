# 79 — As built: K (the contract commit)

Branch `feature/79-K`, base `14801163`, worktree `../theuth-79-K`. Seven commits; every 79 name of `79-contract.md` §2 compiles, stubs answer `not built`, bindings regenerate. No behaviour.

## Built, per commit

1. `07326e4b build(sandbox,rpc)` — `crates/sandbox/Cargo.toml`: `schemars`, `ts-rs` (optional), `tools`, `agent`; `[features] ts = ["dep:ts-rs", "runtime/ts", "tools/ts", "mesh/ts"]`, `mock = []`. `crates/rpc/Cargo.toml`: `sandbox` dep + `"sandbox/ts"`. Cycle check: only `theuth` depended on `sandbox` before; `agent`/`tools` do not (grep of `crates/*/Cargo.toml`); `cargo metadata --offline` resolves.
2. `63ec37f2 feat(sandbox)` — `src/vm/mod.rs` (§2.1 verbatim: `VmRuntime`, every type, `runtime()` → `Err("vm runtime not built")`, `Resources::derive` + 2 tests), `vm/{install,preflight,microsandbox}.rs` placeholders (module doc + `// S fills this (79-contract §0)`), `vm/mock.rs` (`MockVmRuntime`: per-VM state machine under `parking_lot::Mutex`, canned exec by prefix, `put` store, resize within ceilings / `next_start` + `restart_required`, mounts/ports pending, constant metrics, snapshots/restore/list bookkeeping, configurable preflight, `calls()` log; 5 tests), `src/types.rs` (§2.2 verbatim + `SandboxConfig`/`VmConfig`/`VmDefaults` defaults, `AutoOr<T>`/`AutoWord`, `GuestConfig`; 3 tests). *This commit alone does not build: `pub mod vm; pub mod types;` landed in the next one.*
3. `40db0e69 feat(sandbox)` — `src/lib.rs`: the 20 §2.3 message structs, `not_built!` stub impls, `List → Vec::new()`, `PreflightNow → Preflight` with one blocker `not_built`, wire-type re-exports at the crate root, `SandboxPlugin::start` reads `rt.section("sandbox")` (default on missing) and logs it. `MakeEngine`/`Engines` untouched.
4. `b1dc17a9 feat(rpc)` — `api.rs`: the 18 `sandbox_*`/`sandboxes_list` methods (lookup + one `ask` via `fn sandbox()`), `handoffs_list`/`handoff_resolve` stubs returning `RpcError` "not built" (`ASK_FAILED`); `bridge.rs`: `forward!` + `rt.subscribe` for `sandbox::SandboxEvent` **and** `mesh::MeshEvent` (verified by grep: `MeshEvent` was published on the bus but not bridged); `export.rs`: 27 sandbox types + `roster::attention::Handoff`, `secrets::{SecretRequest, SecretRequestField}`. `Handoff` struct in `crates/roster/src/attention.rs`, `SecretRequest{,Field}` in `crates/secrets/src/lib.rs` (§2.7 columns; `fields` = `Vec<SecretRequestField{name,label,masked,current}>`).
5. `f8c7b1da feat(mesh)` — `EngineClass::Vm` (serde `vm`), `MeshConfig.class: Option<EngineClass>` (default `None`), `lease::class_rank` `Laptop | Vm => 0`, `store::class_str`/`class_from` `vm` arms, `DEFAULT_CONFIG` mesh block line.
6. `3e110789 feat(roster)` — `types::RunsIn { Any, OwnVm, Vm(String) }` (`Default = Any`, `Display`/`FromStr`, serde as the same string via `into`/`try_from`), `AgentFile.runs_in` (`#[serde(default)]`), front matter `runs_in: Option<String>` parsed (`any | vm | vm:<name>`, else `Err("runs_in: …")`), rendered only when not `Any`; `AttentionKind::{Handoff, Secret}` + id formats in the `AttentionItem.id` doc; `RunsIn` re-exported from `roster`. Tests: `file::tests::runs_in_forms`, round-trip test carries `Vm("lab")`.
7. `d614d55f fix(desktop)` — UI compile fix (below).

Bindings: `just ts-desktop` (runs `just ts`) succeeded once; `bindings/methods.json` lists all 20 new methods; `crates/desktop/ui/src/rpc/bindings/` has `SandboxInfo.ts`, `Preflight.ts`, `VmMetrics.ts`, `SandboxEvent.ts`, `SandboxCreate.ts`, `DesktopAccess.ts`, `Handoff.ts`, `SecretRequest.ts`, `SecretRequestField.ts`; `EngineClass.ts` has `"vm"`, `AttentionKind.ts` has `"handoff" | "secret"`, `AgentFile.ts` has `runs_in: "any" | "vm" | \`vm:${string}\``. Bindings are gitignored; not added.

## Deviations (each a decision, nobody to ask)

- **`Preflight` derives `kameo::Reply`** (not in the contract's derive list): `PreflightNow → Preflight` is an infallible kameo reply and needs it (`mesh::Peer`, `roster::AttentionItem` do the same). `Vec<SandboxInfo>` needs nothing.
- **`types::DesktopAccess` is not re-exported at the crate root**: the §2.3 message struct `DesktopAccess { name }` lives in `lib.rs` with the same name. Wire type is `sandbox::types::DesktopAccess`; RPC and `export.rs` use that path. TS name is unchanged (`DesktopAccess.ts`).
- **Reply types the contract left implicit**: `Retry`/`Start`/`Stop`/`Pause`/`Resume`/`Restore` → `Result<SandboxInfo, String>` (their RPC methods return `SandboxInfo`); `Destroy` → `Result<(), String>`.
- **`#![allow(missing_docs)]`** on `vm/mod.rs` and `types.rs`: the workspace warns on undocumented pub fields and the contract types are verbatim; the module docs point at the contract (precedent: `crates/firecrawl-native/src/lib.rs`). `#[allow(clippy::large_enum_variant)]` on `SandboxEvent` (`Changed { sandbox: SandboxInfo }` is the contract shape; precedent `crates/codeindex/src/event.rs`).
- **`sandbox` also depends on `tools`**: the brief's feature list names `tools/ts`, which needs the dep; A's `tools.rs` needs it anyway.
- **`DEFAULT_CONFIG` line is `#   class: ~`, not `#   class: auto`**: `MeshConfig.class` is `Option<EngineClass>` per the contract, so `auto` would fail to parse if uncommented; `~` follows the `bind: ~` line in the same block and the comment says "~ → detect".
- **`RunsIn` wire form is the front-matter string** (`"any" | "vm" | "vm:<name>"`), always serialised (no `skip_serializing_if`) so the TS field is required and the UI never sees `undefined`. TS type is a template-literal union via `ts(type = …)`; `RunsIn` itself has no TS export (nothing else references it).
- **RPC optional-arg defaults**: `sandbox_stop.grace_secs` 20, `force` false; `sandbox_snapshot.full` false; `sandbox_destroy.keep_snapshots` true; `sandbox_logs.tail` 200 — from the design (§5.4, §4.6). A may move them into the actor.
- **`MockVmRuntime` ceilings**: a `cpus`/`memory_mib` above the *current* ceiling goes to `next_start` (restart) rather than erroring, so `{cpus: 8, max_cpus: 8}` in one call works; root-disk shrink is `Err`. `Pending.restart_required` is `false` when the VM is stopped (applied by the next start anyway).
- **`Blocker` doc lists a `not_built` code**: what the K stub `PreflightNow` returns; S/A drop it with the stub.
- Commit 2 does not build standalone (see above); every later commit does.

## Not built (by design)

Every body: bring-up, tools, prompt blocks, record `vm:`, `runtime()`, preflight, install, microsandbox impl, `stats::probe` reading `mesh.class`, `place::Inputs.pin`, handoff/secret rows and derivations, UI. `crates/tasks` needed no change (no exhaustive `EngineClass` match). No `Target::Vm`, no `VmBackend` (A/S).

## Tests written

`sandbox`: `vm::tests::{derive_clamps_from_the_host, derive_honours_overrides}`, `vm::mock::tests::{lifecycle_and_call_log, canned_exec_and_put, resize_within_ceilings_is_live_and_ceilings_wait, mounts_pending_snapshots_restore, preflight_is_configurable}`, `types::tests::{auto_or_reads_auto_or_a_number, config_defaults_are_the_documented_ones, sandbox_state_and_event_wire_shape}`. `roster`: `file::tests::runs_in_forms` (+ `full_file`, `render_parse_round_trip` extended).

## Gate

| command | result |
|---|---|
| `cargo check -p sandbox -p rpc -p roster -p mesh -p tasks -p theuth --tests` | ok (10m05s; two doc warnings fixed afterwards, re-verified by clippy) |
| `cargo clippy -p sandbox -p mesh -p roster -- -D warnings` | ok |
| `cargo test -p sandbox --lib` | 21 passed |
| `cargo test -p roster --lib file` | 11 passed |
| `cargo test -p mesh --lib lease` | 8 passed |
| `just ts-desktop` (→ `just ts`) | ok, 8 export tests passed |
| `cd crates/desktop/ui && bun run check` (`tsc -b --noEmit`; there is no `tsc` script) | ok after the UI fix below; needed `bun install --frozen-lockfile` in `crates/desktop/ui` and `packages/apps` (fresh worktree) |
| `bunx vitest run src/components/{agents,hud,tasks}` (touched fixtures only) | 7 files, 38 passed |

`cargo` is at `/opt/homebrew/opt/rustup/bin` (not on PATH by default).

## Edits outside K's files (flag)

- `crates/desktop/ui` (E's) — compile fix only, commit `d614d55f`: `runs_in: 'any'` added to `AgentFile` literals in `components/agents/{wizard.ts,queries.ts,queries.test.ts,tree.test.ts}`, `components/hud/model.test.ts`, `components/tasks/{helpers.test.ts,mock.ts}`; `components/hud/Hud.tsx` `KIND_LABEL` gained `handoff: 'hand-off'`, `secret: 'secret'` (`Record<AttentionKind, string>` is exhaustive). E owns the wording.
- `crates/mesh/src/store.rs` (C's) — `class_str`/`class_from` `vm` arms: exhaustive match, compile-required.
- `crates/mesh/src/lease.rs` (C's) — the `Vm` arm only, as briefed.
- `crates/theuth/src/main.rs` (A's) — one commented `DEFAULT_CONFIG` line, as briefed.
- `crates/roster/src/{lib.rs,tools.rs}` (C/H) — `runs_in: Default::default()` in two `AgentFile` literals + `RunsIn` re-export.
- `crates/secrets/src/lib.rs` (H's) — `SecretRequest{,Field}` structs, as briefed.

## Follow-ups for the integrator

- E: `ui/src/rpc/api.ts` + its method-list test need the 20 new methods (`methods.json` is the source); rename the HUD labels if wanted.
- A: `SandboxActor` still holds only `rt`; the `SandboxConfig` read in `start` is logged and dropped — thread it in. Replace the `PreflightNow`/`List` stubs and the `not_built!` list; `sandbox::types::DesktopAccess` is the wire type. `origin` is `None` from RPC.
- S: `vm::runtime()` body, the three placeholder modules; drop `not_built` from the `Blocker` doc when the stub goes.
- C: `stats::probe` must return `MeshConfig.class` when `Some`; `mesh_peers` prints `vm` via serde already.
- H: `Handoff`/`SecretRequest` are plain structs; `handoffs_list`/`handoff_resolve` return `ASK_FAILED "not built"` from `api.rs` directly (no actor lookup) — replace with the roster ask.
- Cargo.lock changed (new `sandbox` → `tools`/`agent`/`schemars`/`ts-rs` edges); merge K first as planned.
