# 126 — as-built I (integration and gates)

Status: integration of design 126 "double" on `feat/126-double` (worktree `~/Develop/theuth-126`, target `~/Develop/theuth-126-target`), 2026-10-07 14:00 → 15:10. The closer omp (`omp-126-I`) did the merges, the hand-off wiring and the fold (14:00 → 14:43); the 14:45 engine restart killed it while its backgrounded gate job ran, so the orchestrator ran the gates in a detached script and made the four gate fixes below. Sibling as-builts: `126-as-built-{C,W0,W1,W2,W3,W4,W5}.md`.

## Merge order and shas

| step | sha | what |
|---|---|---|
| base | `5cdcbfde` | C's contract (crate `double`, store + migration, 13 RPC stubs, 4 bridge events, decision/tools/agent/mesh seams) |
| merge W1 | `7764c8aa` | `feat/126-w1` acabda0e — actor, labels, predictor, ladder, take-over (no conflicts) |
| merge W2 | `b8b0607a` | `feat/126-w2` 07cb677f — Tier A head, scheduler, venue, mesh bundle, versioning; conflicts: `Cargo.lock`, `crates/double/Cargo.toml` (dep union), `crates/double/src/lib.rs` (modules + start: all kept), `crates/rpc/src/api.rs` (W1's actor asks + W2's scheduler asks, one `ask` style) |
| merge W4 | `2ce7dfa6` | `feat/126-w4` 81a880e7 — desktop UI (no conflicts) |
| merge W5 | `116b8313` | `feat/126-w5` bb0d216b — TUI, channel lines, chat tools, docs, skill, prompt; conflicts: `Cargo.lock`, `crates/double/src/lib.rs` |
| merge W3 | `f7766092` | `feat/126-w3` 2d35678d (+ the four W0 spike commits) — Tier B LoRA trainer, node entry; conflicts: `Cargo.lock`, `crates/double/Cargo.toml` (`decision` dep from W1 and W3 deduped), `crates/double/src/lib.rs`, `crates/theuth/src/main.rs` (config template lines) |
| hand-offs 1 | `6e3c17a6` | roster attention `Source` seam — the derived inbox pulls the double's rows (`crates/roster/src/attention.rs`, +33); `DoublePredicted`/`DoubleAnswered` added to the `republish!` list in `crates/rpc/src/client.rs` so an attached TUI gets the chip |
| hand-offs 2 | `01beb12d` | `crates/double/src/tier_b.rs` (new, 623 lines): `LocalTierB` / `NodeTierB` / `NodeRunner` adapt W3's `train_b` to W2's `sched::Trainer` + `mesh::Runner`, gate via `train_a::gate` + `train_b::rescore_golden`, `PlatformIdle` (W3's `IdleSeconds` over `platform::idle`), per-night cap; predictor uses W2's Tier A head + `versions::load_active`; ONE card prompt (`predict::question`) shared by training and inference; `SettingsChanged` / `SetConfig` / `DoubleVersionActivated` handlers in the actor; `double_status` merges the scheduler's state; `double_forget` by `question_id` (W4's wire shape); local Tier B only behind `training.tiers.b.local`; the actor `Handle` implements W5's `tool::Seams` and the roster `attention::Source` (replaces `tool::NotYet`) |
| fold | `0a440a67` | `origin/main` 1098ae72 (125 advisor, 127 favourites, 128 HUD scene, 129 event-focus, 130 docs, no_emoji scope, v2.10.0, 126 Revision 2) — `api.rs`: 129's stream + 126's republish both kept; TUI tests: both blocks; migration renumbered `0034_double` → `0036_double` (0034 is 125's advisor); release notes renamed to 2.11.0 |
| gate fixes | `1fe77db7` | see "Gate fixes" |

Diff of the whole wave against main (1098ae72): 121 files, +20,424 / −124. Against the contract commit: 204 files, +36,637 / −680.

## Gates (merged tree, `~/Develop/theuth-126-target`, every step through `build-lock` with `BUILD_LABEL=126-I`, `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`; workspace-class steps under `BUILD_SLOTS=1`; log `gate-I.log` / `gate-I2.log` in the target dir)

| gate | result |
|---|---|
| `cargo test -p double -p platform` | 94 passed / 0 failed (9 s) |
| `cargo test -p decision --lib -- --skip parity_ --skip metal` | 88 passed / 0 failed — the checkpoint-loading parity/Metal tests are filtered by the machine rule (no model load on Sky's Mac) |
| `cargo test -p decision --features train --lib -- --skip parity_ --skip metal` | 88 passed / 0 failed |
| `cargo test -p tools -p agent -p mesh -p replica -p roster` | 729 passed / 0 failed (97 s) |
| `cargo test -p tui -p slack -p signal -p telegram -p rpc` | 522 passed / 0 failed (170 s); `tui/tests/no_emoji.rs` passes after the fold (main narrowed its scope) |
| `cargo clippy -p double -p platform --all-targets -- -D warnings` | clean |
| `cargo clippy -p decision --features train --all-targets -- -D warnings` | clean (covers `examples/lora_step`) |
| `BUILD_SLOTS=1 just ts-desktop` | bindings regenerated and copied; no tracked binding changed |
| `bun install` (ui) + `bun install` (`packages/apps`) | ok — the `packages/apps` install is REQUIRED in a fresh worktree, else `@openuidev/*` is unresolved and tsc/vite/vitest go red for the wrong reason |
| `bunx tsc --noEmit` | 0 errors (after the fixes; 165 before, of which 154 were the unresolved `@openuidev/*` cascade) |
| `bunx vite build` | ✓ built in 7.1 s |
| `bunx vitest run src/components/settings src/components/chat src/components/attention src/lib` | 121 files / 938 tests passed |
| `cargo check -p browser --all-targets` | clean (after the fix) |
| `BUILD_SLOTS=1 cargo check --workspace --all-targets` | clean (61 s) |

Nothing in this run loaded a model checkpoint or trained anything.

## Gate fixes (`1fe77db7`)

- `crates/desktop/ui/src/store/reduce.ts:1269` — `e.session === null` → `e.session == null`: the double events carry `session?: SessionId` (serde skips `None`, so the binding is honestly optional) and the strict null check left `string | undefined` as an index type.
- `crates/desktop/ui/src/components/attention/double.test.ts` — the item fixture spreads a `Partial<AttentionItem>`; typed the result `as AttentionItem` and added the regenerated `ask: null`, `turn: null` fields.
- `crates/desktop/ui/src/components/settings/Double.tsx:385,391,392` — three `<Input className="… text-xs">` → `md:text-xs` (the `mobile-zoom.test.ts` rule: never shrink an input below 16 px on mobile).
- `crates/browser/tests/live.rs:546` — `decision::Scores { … embedding: None }` (C added `Scores.embedding`; the browser crate's live test is only compiled by `--all-targets`).

## Deviations from the design across the wave (collected)

- C: ALPN constant lives in `replica`; `Accelerator` is `Metal | Cuda | Cpu` in an `Option`; key 9's checkpoint hash is the real `b9474647c6a4`; the real-checkpoint embedding test is env-gated (`DECISION_EMBED_TEST=1`).
- W1: only approvals and asks are subscribed — the tree has no attention/packet/draft raise events (§4.1's five kinds collapse to the two that exist); true 30-day half-life; k-NN ranks by `cos × decay`; multi-question asks are observe-only.
- W2: `pmset` instead of IOKit for AC power; settings persisted by rewriting the whole `double:` block (`write_config_value` breaks nested blocks); venue `auto` requires `cuda|metal`; `TrainResult` stored as JSON text.
- W3: `lora.rs` is not feature-gated (inference needs it); adapter merged at load (model.rs's `Linear`s are private); `TrainOutcome` instead of a bare `CandidateVersion`; pause polled per micro-batch; hold-out excluded from training. Tiny-model proofs: accumulation 8×2 ≡ 1×16 (max relative gradient diff 1.17e-6), pause within one micro-batch with resume Δ = 0.
- W4: undo = `double_take_over`; forget by `question_id`; the overall curve is merged client-side from `double_class`; the run log is `last_run` + live events.
- W5: TUI chip uses `·` glyphs (no emoji in scanned crates); `/double cap` parses `<rung> [opt-in]` from the tail because shell class keys contain spaces.
- I: Tier B's local path exists but is reachable only with `training.tiers.b.local: true` (Revision 2); `approvals_off_sessions` is still a documented no-op pending the session registry.

## Shadow-run recipe (14 days at L0)

Shipped defaults: `double.enabled: false`, `double.training.tiers.b.local: false`, every class at L0 (the ladder starts at L0 by construction; nothing acts). To start the shadow run Sky sets `double.enabled: true` (restart needed — `enabled` and `model.*` are read at start) and keeps `tools.approvals: true`. From then on every approval/ask card gets a prediction row before he sees it; his answers are the labels. Nothing is auto-answered until a class earns L1 by the §7 thresholds and he opts in per class (Settings › Double, `/double cap`). The improvement curve (hold-out log-loss, accuracy, ECE per class and overall) is in Settings › Double; `t double status` prints the same. Tier A (plain-Rust head) trains nightly when the preconditions hold; Tier B stays off until a mesh GPU node joins or he flips the local flag.

## Open items for Sky

1. Engine + desktop rebuild owed (migration `0036_double`, 13 new RPC methods, 4 events).
2. The two-engine mesh smoke for Tier B (`126-as-built-W2.md` §Smoke) and the first real Tier A night have not run — no training ran on this machine, by rule.
3. `approvals_off_sessions` needs the session registry seam (W1 open item).
4. `tui/tests/no_emoji.rs` is green only because main narrowed its scope; the 21 surface-crate emoji strings from design 123 remain.
5. Release notes stub is at `docs/release-notes-2.11.0.md` (next version after 2.10.0) — verify the number at release time.
