# 136 F5 — as-built: fold `origin/main` 68b9fc26 into `feat/136-dreaming`

Tree `~/Develop/theuth-136`, branch `feat/136-dreaming`. Base 373e1d4f (fully gated). Folded
`origin/main` = 68b9fc26 (v2.12.3 + 134 W5/W6 + 130 PARITY + 138 UI-FIXES + 140 docs + 137 ENDLESS).

## Commits

| sha | what |
|---|---|
| 68cedbff | `Merge origin/main (134 W5/W6, 138, 140, 137 ENDLESS) into feat/136-dreaming` — 7 files conflicted, 15 blocks, all resolved both-sides |
| e167d641 | `136 F5: Cargo.lock — dreaming at workspace version 2.12.3` (cargo settled the lock after v2.12.3's bump; no dependency change) |
| d3d00aba | `136 F5: fmt the merged loop_ import list in supervisor.rs` (my resolution of the import list wrapped differently from rustfmt) |

No test needed fixing: the brief's anticipated breakages (137 test assuming no budget field, 136 test assuming the old frame, duplicated match arm, missing `..Default::default()`) did not occur — 137's lander had already folded 136's budget/frame shape where it overlapped, and git auto-merged the rest.

## Conflict resolutions

Every block kept both sides; none dropped 136 or 137 behaviour.

| file | region | resolution |
|---|---|---|
| `crates/agent/src/lib.rs` | re-exports after `pub use cost::{…}` | `pub use dreaming_ledger::{Ledger, …}` (136) **and** `pub use endless::{Backfill, Endless, …}` (137), both lists intact |
| `crates/agent/src/loop_.rs` | after `PermitsOf` test message (~L1327) | 136's `#[cfg(test)] BudgetOf` handler kept, then 137's `impl Message<OpenerContext> for Session` |
| `crates/agent/src/loop_.rs` | `prompt_parts_in_order_with_blank_lines` test (~L8429) | 136's comment (header prose may say "rules") + 137's `assert!(…, "{full}")` message |
| `crates/agent/src/supervisor.rs` | `use crate::loop_::{…}` | both `SetBudgetNow` (136) and `SetMemoryNow` (137) in the import list |
| `crates/agent/src/supervisor.rs` | `spawn()` before the `match row` (~L770) | `let budget = store.budget(id).await?;` (136 R-22) then `let desk = …` (137) |
| `crates/agent/src/supervisor.rs` | `Session::new(…)` builder chain (~L902) | `.with_budget(budget).with_endless(self.endless.clone()).with_parent_view(parent_view)` |
| `crates/rpc/src/api.rs` | `Rpc` trait after `double_settings_set` | 136's nine `dreaming.*` methods then 137's `endless_settings_get/set` |
| `crates/rpc/src/api.rs` | `impl RpcServer` after `double_settings_set` | 136's `dreaming_*` bodies (closing brace restored) then 137's `endless_settings_*` bodies |
| `crates/rpc/src/bridge.rs` | `forward!(…)` event list | `DreamNightStarted, DreamNightFinished, DreamUndone, EndlessProgress` |
| `crates/runtime/src/side_call.rs` | `CallKind` enum, `as_str`, `FromStr`, round-trip test (4 blocks) | both `Dream` ("dream") and `Endless` ("endless") variants/arms |
| `crates/desktop/ui/src/rpc/events.ts` | imports + `AgentEvent` union (2 blocks) | `DreamNightStarted/Finished/Undone` (136) and `EndlessProgress` (137), both imported and in the union |

Auto-merged and checked by hand: `crates/desktop/ui/src/components/settings/SettingsDialog.tsx` lists both `['dreaming','Dreaming',Moon]` and `['memory','Memory',InfinityIcon]` tabs and renders both panes; migrations `0037_endless.sql` (137) and `0039_budget.sql` (136) both present (0038 reserved for 139); `Cargo.lock` taken as merged then settled by cargo.

Bindings: `just ts-desktop` regenerated `bindings/` → `crates/desktop/ui/src/rpc/bindings/` with no diff (137's lander had already shipped the union of both sides' types).

## Gate (all through `build-lock`, label `136-F5`, `CARGO_TARGET_DIR=…/theuth-136-target/C`, jobs 4, incremental off)

| step | result |
|---|---|
| `BUILD_SLOTS=1 just ts-desktop` | 8 export tests ok; bindings unchanged |
| `bun run check` (tsc -b) | 0 errors |
| `bunx vitest run --maxWorkers=2` | 199 files · 1377 tests passed |
| `bun run build` | built (chunk-size warnings only) |
| `cargo test -p knowledge -p dreaming -p schedule -p tools -p rpc -p roster -p meeting --lib` | knowledge 55 · dreaming 59 · schedule 75 · rpc 52 · roster 67 · meeting 69 · tools 176 (first run: 3 `shell::tests::*` login-PATH probe tests timed out at loadavg 38 on 16 cores; re-run once per brief → 176 passed, 1 ignored) |
| `cargo test -p agent --lib` | 505 passed · 2 ignored (473 at 373e1d4f + 137's) · no SIGSEGV |
| `cargo clippy -p knowledge -p agent -p schedule -p tools -p dreaming -p rpc -p roster -p meeting -p theuth --tests -- -D warnings` | clean |
| `cargo fmt -p … -- --check` (not crates/theuth) | clean after d3d00aba |
| `BUILD_SLOTS=1 cargo check --workspace --tests` | clean (pre-existing `block v0.1.6` future-incompat note only) |
| `BUILD_SLOTS=1 cargo build -p theuth` | `…/C/debug/t` built |

## Left

Nothing. Tree is at the fold + two small follow-ups, gate green; ready for the lander.
