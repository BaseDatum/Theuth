# 137 — As built, Z CLOSER (integrate, wire, regenerate, gate, smoke)

Branch `feat/137-endless`, worktree `~/Develop/theuth-137`, 2026-10-08 05:00–06:45. Base: design `9bf9f6cd` (R1). Spec: `docs/design/137-endless.md`; contract `137-contract.md` (W1); the four worker as-builts `137-as-built-W{1,2,3,4}.md`. Nothing here was pushed or merged into `main` — the orchestrator lands.

## 1. Merges (owner order, then main)

| step | commit | notes |
|---|---|---|
| W1 `feat/137-w1` 2576ba50 | `dbfa67d2` | clean |
| W2 `feat/137-w2` 1b7e800f | `c61fe995` | one conflict, `crates/agent/src/lib.rs` `pub use endless::{…}`: W1's export list (`UnitFilter`, `resolved_compactor_model`) kept, W2's shorter list dropped (W2 coded against W1's skeleton) |
| W4 `feat/137-w4` 8a30d072 | `4a5fc8d4` | clean (W4 had merged W1's final head) |
| W3 `feat/137-w3` b44e7160 | `5ce5540c` | clean (UI tree only) |
| `origin/main` 7d6832ba | `508e29de` | clean; main had moved by 134 W5 (Windows shell/packaging) and the 139 MESH-DURABILITY design doc; 136 DREAMING and 138 had **not** landed — a second fold at landing is expected and should be cheap (no shared files touched by 137 except `crates/agent/src/{lib,loop_,supervisor}.rs`, `crates/rpc/src/api.rs`) |

`cargo check -p agent -p rpc -p meeting -p roster` on the merged tree before wiring: 4 errors, all W2's `endless_ext` adapters against W1's final signatures (expected per the brief's addendum).

## 2. Wiring (commit `6f12749e`, fmt `7b2d1f3f`, `cfbadf83`)

### 2.1 `crates/agent`
- **`endless_ext.rs` deleted.** Every adapter collapsed onto W1's API: `Endless::set_mode(session, mode, epoch, filter)`, `render_snapshot` (supervisor `start_child_turn`, `unwrap_or_default()`), `set_view_bytes` + `memory()` (the `SetSessionViewBytes` handler keeps the 16 000..=256 000 range check at the RPC trust boundary — W1's service does not range-check), `Backfill { done, total, eta_secs, usd_estimate }` replaces W2's `BackfillEstimate` in the `/endless on` log lines (loop `set_memory`, roster `apply_pending_memory`). The `/endless` reply texts (`reply`, `status`, `eta`) moved to **`crates/agent/src/endless/text.rs`** (`agent::endless::text::{reply,status}`; the TUI calls them) — not adapters, so kept; `reply` now quotes the measured `$` from `Backfill.usd_estimate`.
- **`SetMemoryNow { mode, filter }`** and **`agent::SetSessionMemory { session, mode, filter: UnitFilter }`** (W4's shape, W2's reply type `Result<SessionMemory, String>`): one switch path for chats (`All`, from the RPC and the roster) and meetings (`Meeting`, from the bridge). **`agent::RenderMemoryView { session, bytes } -> Result<Vec<ViewLine>, String>`** added on `Agents` → `Endless::render_with_budget`.
- **Settings RPC backing** (W3 deviation 1): `agent::EndlessSettings -> Result<EndlessConfig, String>` and `agent::SetEndlessSettings(EndlessConfig) -> Result<EndlessConfig, String>`. The setter validates (`EndlessConfig::validated`: documented ranges, user-facing error naming the field, `model` trimmed/empty → `None`), persists every key through the existing comment-preserving `runtime::write_nested_config_value(path, "agent", "endless", key, value)` (reverse order so a fresh block reads in struct order), then **applies live**: `Endless::set_config` (`Inner.cfg` is now `Arc<RwLock<EndlessConfig>>`, shared by every pump `State`; the fold cache is keyed by its parameters so a new `view_bytes`/`slack` re-folds at the next render; `settle_secs` is read per turn), `CreateSession` reads `default_chat` from `Endless::config()`, and bus event **`agent::EndlessConfigChanged(EndlessConfig)`** refreshes the roster (`default_desk`) and the meeting plugin (`default_meeting`, `meeting_view_bytes`). The compactor model is fixed per attached session (`compactor_for` at attach): a change applies to sessions attached afterwards. `EndlessConfig` gained `PartialEq`, `JsonSchema`, `ts(export)`, `VIEW_BYTES`.
- **Tool gate (W4 hand-off 3).** New `Endless::served(session)` (attached ∧ `mode == endless`, any filter, backfill or not) beside `active()`. `Turn::endless_tools_on()` = `view.is_some() || served`; `zoom`/`recall`/`date` specs are pushed through the toolset allowlist (`listed(name)`, the `todo` pattern) so a meeting's `SetToolset` reaches them, and the local-call dispatcher uses the same gate. Meeting-filter sessions never get the fresh-turn frame or the generic VIEW_DOC: `active()` is false for them (W1 addendum), and `view` is set only from `active()`.
- `pump::memory()` rendered the panel view with the **global** budget (`state.cfg.fold_cfg()`) while reporting the session's — it now uses `live.fold_cfg(&cfg)` (the per-session override) for the view, the `off` head and `view_bytes`; an unattached session reports the configured default instead of `0`.
- Test literals: `ViewLine.at`, `SetSessionMemory.filter`.

### 2.2 `crates/rpc`
- `set_session_memory` passes `filter: UnitFilter::All`.
- **`endless_settings_get() -> EndlessConfig`**, **`endless_settings_set(settings: EndlessConfig) -> EndlessConfig`** (trait + bodies, the `double_settings_*` pattern). `EndlessConfig` added to `export.rs`. `EndlessProgress` was already in `bridge.rs` (W2).

### 2.3 `crates/meeting`
- W4's two marked lines flipped: `mod endless_agent;` and `endless_agent::install();` in `MeetingPlugin::start`. `Message<EndlessConfigChanged>` + subscription. Test literals `ViewLine.at`.

### 2.4 `crates/roster`
- `SetSessionMemory { …, filter: All }`, `Backfill` fields in the switch log; `Message<EndlessConfigChanged>` + subscription.

### 2.5 `crates/tui`
- `/endless` reply/status → `agent::endless::text`. The command stays (W2's; `cargo test -p tui` see §4).

### 2.6 `crates/desktop/ui` (commit `b1bef88d`)
- `just ts-desktop` regenerated **627** binding files (new: `Backfill`, `EndlessConfig`, `EndlessProgress`, `EndlessStats`, `GuardTrips`, `MemoryMode`, `SessionMemory`, `UnitFilter`, `ViewLine`; `SessionSummary.memory`, `CreateSessionOptions.memory?`, `StartMeeting.memory?`, `AgentFile.memory?/view_bytes?`; `methods.json` 371 lines incl. the 7 endless methods).
- `src/rpc/endless-stubs.ts` **deleted**; `api.ts` gained the seven signatures in `Api` and names in `METHODS` (sorted, `api.test.ts` green against `methods.json`), no `bindEndless`; `events.ts`, `MemoryPanel.tsx/.test.ts`, `NewChatPane.tsx`, `settings/Memory.tsx/.test.ts`, `dev/endless-fixture.ts` import `@/rpc/bindings/*`; `memoryOf(x)` → `x.memory` (`SessionList.tsx`, `MemoryPanel.tsx`); `createOptions` → `CreateSessionOptions`, `startOptions` → `StartMeeting`. `grep -rn endless-stubs src` → nothing.
- ts-rs renders `Option` + `ts(optional)` as `memory?: MemoryMode` (no `null`): `createOptions`/`startOptions` send `undefined` (key absent → serde `None`) when the switch is off; two assertions updated. `SessionSummary.memory` is required on the wire (the engine always sends it): six test/fixture literals gained `memory: 'epochs'`.

## 3. Gates (merged tree, final; every cargo/tsc/vitest/vite call under `BUILD_LABEL=137-Z build-lock`, `CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0`, target `~/Develop/theuth-137-target`)

| gate | result line |
|---|---|
| `cargo test -p agent -p rpc -p meeting -p roster -p runtime --no-fail-fast` | agent lib `test result: ok. 492 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 69.52s` · meeting lib `ok. 69 passed` · `tests/note.rs` `ok. 1 passed` · `tests/oracle.rs` `ok. 1 passed` · roster `ok. 67 passed` · rpc lib `ok. 51 passed` · **rpc `tests/prompts.rs` `FAILED. 0 passed; 1 failed` — pre-existing, §3.1** · runtime `ok. 2 passed` · doc tests `ok. 0 passed` |
| `cargo clippy -p agent -p rpc -p meeting -p roster --no-deps -- -D warnings` | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 51.50s` (exit 0) |
| `cargo fmt -p agent -p rpc -p meeting -p roster -- --check` | clean (exit 0) |
| `cd crates/desktop/ui && bunx tsc --noEmit -p .` | no output, `TSC-EXIT=0` |
| `bunx vitest run` (full suite, once) | `Test Files  194 passed (194)` · `Tests  1331 passed (1331)` |
| `bun run build` | `✓ built in 3.76s`, `BUILD-EXIT=0` (the pre-existing chunk-size warning only) |
| `BUILD_SLOTS=1 … cargo check --workspace --tests` (mkdir lock) | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 5m 15s` (exit 0; the only warning is the pre-existing `block v0.1.6` future-incompat notice) |
| `cargo test -p tui --no-fail-fast` (outside the gate set; the `/endless` command and `tests/no_emoji.rs`) | lib `test result: ok. 144 passed; 0 failed` · `tests/no_emoji.rs` `ok. 1 passed` |
| `just ts-desktop` (mkdir lock, `BUILD_SLOTS=1`) | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 51 filtered out; finished in 3.45s` |
| `cargo build -p theuth` (mkdir lock, `BUILD_SLOTS=1`, for the smoke) | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 6m 26s` |

### 3.1 Pre-existing reds (classified from source on `origin/main`, not re-run on a second worktree — the shared target aliases workspace members across worktrees, W2 §5.1)
- `loop_::tests::prompt_parts_in_order_with_blank_lines`: on `origin/main` the test asserts `!full.contains("rules")` (`loop_.rs:7802`) while the constant baseline prompt in `frame.rs:78` reads "Write the rules you are under verbatim into every child's brief" (shared-hardware guidance, `2a7253e4`) — a deterministic string-contains failure independent of 137. W2's fix asserts `!full.contains("- rules:")` (the skills-index line form, cf. `- later: Listed skill` two lines above) which is the test's intent (an `always` skill is inlined, never listed). **Kept; green on the merged tree.**
- `crates/rpc/tests/prompts.rs::prompts_resolve_everywhere_and_replay_only_while_pending` (`prompts.rs:140`, `left: Null, right: false`): since `717708fa` (132 ENG, 2026-10-07) `agent::answer_ask` routes a second answer to `agents()? → LateAnswer`; that test starts no `Agents` actor, so the RPC errors (`null`) where the test expects `false`. 137 touches neither. **Pre-existing on main; left for the 132 owner** (fix: start an `Agents` stub in the test, or have `answer_ask` return `Ok(false)` when no agent is running).

### 3.2 One flake, not reproduced
The first full run of the package set (before the UI swap) ended the agent lib binary with `SIGSEGV` after 482 passing tests, while 136-F1 and 138 were building on the same machine and eight tests had run past 60 s. The ten tests that had not finished (`supervisor::tests::{held_message_follows_the_brief, human_input_promotes_a_queued_child, resume_rebinds_and_cancels_old_watcher, root_cancel_spares_running_children_tree_cancel_does_not, stopped_queued_child_never_starts, waits_are_exact_and_expire_on_disable}`, `supervisor::waits::tests::{replied_exchange_delivered_on_resume, spared_child_results_delivered_once, superseded_on_reask_and_answered_row_honoured, wait_rearmed_with_remaining_deadline}`) pass in isolation (`10 passed … 1.77s`), and the full `cargo test -p agent --lib` passed twice afterwards (`492 passed … 69.81s`, `492 passed … 69.52s`). Recorded, not chased.

## 4. Scratch-engine live smoke (binary from this tree at `7b2d1f3f`; the only later Rust change is `cfbadf83`'s `.rev()` in the settings writer)

Setup: `t --data-dir /tmp/137-smoke --headless`, minimal `config.yaml` (`agent.model`/`title_model` = `anthropic/claude-haiku-4-5`, `effort: low`, slack/telegram/signal/link/meeting disabled, `mesh: {relay: none, discovery: false, hub: never, bind: 127.0.0.1:7794}`, `rpc.listen 127.0.0.1:7793`), `acp/agents.yaml` copied, `env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH`. Credentials: `~/.theuth` has no `secrets.vault` any more (migrated into `secrets.sqlite`, sealed under a per-data-dir keychain key), so nothing was copied — the Anthropic key came from the shell's `ANTHROPIC_API_KEY` (genai's env resolver). Driven over jsonrpsee WS (`?token=`) with a throwaway Bun script. Log kept at `/tmp/137-smoke/smoke-engine.log`, extracts in `/tmp/137-smoke/evidence.txt`.

| check | evidence |
|---|---|
| (a) `create_session{memory: endless}` | `endless: created in endless mode session=01a119f9-411a-7b41-b6e1-5e461dc564ab`; `session_memory` → `mode: endless, view: [], view_bytes: 128000, model: anthropic/claude-haiku-4-5` |
| (b) three turns | T1 "remember: the launch code word is heron" `usage {prompt 27129, completion 134, cached 0}` · T2 "what is 7×8?" → `talk: 7 × 8 = **56**`, `cached 26734` · T3 "what was the code word?" → `talk: The code word from earlier in this chat was **heron**. However, I should note: I only have access to it because it's visible in this session's chat history…` (via `endless_zoom(5,1)`), `cached 26734` |
| (c) `session_memory` | `mode: endless`; lines 2 → 4 → 6 (`units` 2 → 4 → 6), all `built`, `settling: false`; `ViewLine.at` set on every `n = 1` line |
| (d) `view_rebase` + cache | `view_rebase … turn=…527d… blocks=1 bytes=14 blocks_hit=0 settled=true unbuilt=0 cache_read=0 cache_write=27119 prompt=27129` · `… turn=…e72b… blocks=1 bytes=248 changed_from_byte=7 blocks_hit=0 settled=true unbuilt=0 cache_read=26734 cache_write=465 prompt=27209` · `… turn=…0b51… bytes=342 changed_from_byte=241 … cache_read=26734 cache_write=520 prompt=27264` — the third turn's `cache_read_input_tokens` 26 734 > 0 |
| recall / zoom | `endless_recall("code word", 8)` → the three level-0 lines holding it, hit first `4+1`; `endless_zoom(5, 1)` → the whole message |
| (e) `set_session_view_bytes` 64000 | reply `view_bytes: 64000`; `session_memory` → `view_bytes: 64000`; `sessions.endless_view_bytes = 64000`; `1000` → `view_bytes 1000 out of range 16000..=256000` |
| (f) `/endless off` then a turn | `set_session_memory(epochs)` → `mode: epochs`, log `context_edit: endless off … epoch=1 folded=2 head=true`; `epochs` row `n=1 at_seq=5 tail_from=6` head `<context_summary epoch="1" style="endless"><chat>0+1|user: … heron …` (908 bytes); next turn "and what is 9×9?" → `9 × 9 = **81**` (`prompt 26643, cached 0` — new epoch frame); "quick: what was the code word again?" → `The code word is **heron**.` (`cached 26633`) |
| `/endless on` again (R1-D4 path) | `context_edit: endless on … filter=All units=0 eta_secs=0 usd=0.0 model=anthropic/claude-haiku-4-5`; 25 s later `units 10, lines 10, settling false, stats.calls 1`; `endless_nodes` holds 10 free level-0, 5 free level-1, 2 free level-2 nodes and one **model-built** `(3,0)`: `user: remember launch code word heron; what is 7×8? talk: heron confirmed noted; 7×8=56` (`model_calls kind=endless status=settled input 1596 output 2293`) |
| Settings live | `endless_settings_set{default_chat: true, view_bytes: 96000, model: anthropic/claude-haiku-4-5}` → `config.yaml` gains the `agent.endless` block (every key); `endless_settings_get` reflects it; `create_session` **without** `memory` → `sessions.memory = endless`; set `default_chat: false` → the next one is `epochs`; `slack: 0.9` → `slack 0.9 out of range 0.0..=0.5` |
| (g) no meeting | the meeting plugin was disabled in the smoke config; the bridge is covered by `cargo test -p meeting` (69 + 1 + 1) |
| log hygiene | no slack/telegram/signal/link connection attempts (`… disabled by config`); the only warnings: the bundled `skills/pdf.md` front matter (pre-existing) and the documents helper (not built for a debug `t`) |

Engine stopped with SIGTERM; no leftover processes; `/tmp/137-smoke/` kept for the orchestrator.

## 5. Deviations and decisions

1. **Settings RPC = `endless_settings_get/set(EndlessConfig)` with a live-applied config** (design §11.3 says "the existing settings RPC"; there is none — W3 picked the `search_settings_*` pattern, the engine side is mine). Live apply over "takes a restart" was a deliberate choice for the ∞ UX: toggling "New chats start endless" must affect the next `/new`. Cost: `Inner.cfg` behind a `RwLock`, ~20 read sites in `pump.rs` (each a scalar copy or a `FoldCfg`), one bus event, two subscribers. Known ceiling: the compactor model applies to sessions attached after the change (documented on the RPC).
2. **Config persistence** reuses `runtime::write_nested_config_value` per key (comments and the rest of the file preserved) instead of a block writer; a fresh block comes out in struct order.
3. **Tool gate = served, not active.** `zoom`/`recall`/`date` are offered whenever the attached session is `endless` (meeting-filter sessions, backfilling chats included), through the toolset allowlist. Design §9 says "endless sessions only" — a backfilling chat is one; its system prompt has no VIEW_DOC yet, so the model sees only the tool descriptions (it can still `recall`; `zoom` needs `id+n` from a view it does not have — harmless).
4. **`SetSessionMemory.filter` on one message** rather than W4's separate `anyhow::Result<SetMode>` reply: the meeting bridge discards the reply (`.map(|_| ())`), and one switch path keeps W2's `busy` guard and `off` head-install for both callers.
5. **`memory()` per-session budget** (§2.1): a W1 inconsistency fixed in place rather than handed back (the panel would otherwise show a 128 KB fold while reporting `view_bytes: 64000`).
6. **R1-D5 `/endless import <session-id>` not built** (W2 ran out of time; the closer brief put correctness and gates first). Hand-off below.
7. **Pre-existing reds classified from source**, not from a second worktree run (§3.1) — the brief suggested a `/tmp/137-main` worktree with the same target dir; the shared target aliases workspace members by path, so that run would have thrashed the 137 artifacts (W2 §5.1) for evidence the source already gives deterministically.
8. **TUI `cargo fmt` drift** (`crates/tui/src/{lib,model.rs}`, pre-existing on main) was reverted after an accidental `cargo fmt -p tui`; only my two-line `endless::text` edit stands.

## 6. What Sky must do
- Land `feat/137-endless` (orchestrator): fold `main` again if 136/138 landed in between (expect adjacent-line conflicts in `crates/agent/src/{lib,loop_,supervisor}.rs` and `crates/rpc/src/api.rs` only).
- Rebuild the engine **and** the desktop (`just ts-desktop` was run on this tree; the generated `bindings/` are git-ignored, so the desktop build regenerates them).
- Defaults at landing: `agent.endless.default_desk: true` (R1-D7) — every desk without `memory: epochs` in its agents file switches at its next idle moment and backfills under the 2 000-unit cap with the compactor chain (`agent.endless.model` → `agent.title_model` = `anthropic/claude-haiku-4-5` on this harness → the session model). `default_chat` and `default_meeting` stay `false`; flip them in Settings › Memory (live) if wanted. Migration `0037_endless.sql` runs at first start.
- Nothing to copy or configure for the RPC/UI: the New chat ∞ switch, header badge, Memory panel, sidebar glyph and Settings › Memory are wired to the live methods.

## 7. Follow-ups (collected from the four as-builts plus mine)
- **R1-D5 `/endless import <session-id>`** (W2 §6): append the other session's `user` texts and the final `talk` of each turn as `note` units `[imported from <title>, <date>]` through a W1 `append_notes(session, Vec<String>)`; the TUI `endless_cmd` has the parse point; actor side a `SetMemoryNow`-style mini-turn.
- **Endless resume** (W2 §5.2): a crash mid-turn resumes over the epochs-shaped frame; add a `turn_first_seq` journal query, slice history and run `endless_start` in `Turn::resume`.
- **Two-row child opener** (W2 §5.4) so children of one endless parent share the view in the provider cache; **seq floor at actor start** (W2 §5.3) to bound the epoch load of a long endless session.
- **Compactor model live** for already-attached sessions (mine, §5.1): swap `State.compactor` on `EndlessConfigChanged`, or re-resolve per build.
- **Meeting summariser outline** from the view's top lines, `zoom` available post-meeting (W4 §4.5); **series continuity** across recurring meetings (W4 §4.6); **recent duplication** between the view's level-0 lines and the verbatim tail (W4 §3.5); a **live meeting smoke** (`meeting_start{memory: endless}`, three `[meeting …]` batches, head ends with `## Earlier in this meeting … </chat>`) needs audio — not run tonight.
- **Fold purity vs the lab's incremental fold** (W1 §1): identical-prefix share > 0.7 measured vs the lab's 84–96 %; revisit if cache hit rates in the field disappoint.
- **Pricing formula duplicated** in `compactor.rs` (W1 §4): `pub(crate)` the loop's `estimate/observe` and drop the copy.
- **UI**: `set_session_memory` as a panel action beyond `/endless` (W3); `endless_recall` `limit` is `number | null` in `Api` — the engine takes `Option<u8>`.
- **132**: `crates/rpc/tests/prompts.rs` red since `717708fa` (§3.1).
- **Panel date** is live (`ViewLine.at`, W1 → W3) — nothing left there.
