# 81 — As built: MEETING-DIET (lean knowledge hints and a pinned title while recording)

Branch `feat/meeting-session-diet`, base `95f6496a`, worktree `../theuth-wt-meeting-diet`. Brief: `/tmp/mc-2026-10-02/omp-meeting-diet.md`. Sibling workers: COMPACT-FLOOR (compaction sizing), MEETING-HEAD (deterministic compaction head); nothing here touches their seams.

## What changed

### 1. Per-session knowledge-hint limit (06 §1.5)

- `crates/agent/src/lib.rs:372-375` — `Toolset.hint_limit: Option<u32>`: `None` = the configured `knowledge.hint_limit`; `Some(n)` = at most `n` lines per `<knowledge>` note; `Some(0)` = no hints and no knowledge search. `PartialEq`/`Eq` kept. The three `Toolset` literals in `loop_.rs` tests gained `hint_limit: None`.
- `crates/agent/src/loop_.rs:3653-3664, 3685` — the 10 §4.2 hint site reads `Shared.toolset` (the `SetToolset` store, `SetToolsetOverride` handler at `loop_.rs:1706`) and uses its `hint_limit` in place of `cfg.hint_limit`; `hint_limit == 0` skips the whole block (`if hint_limit > 0 && let Some(knowledge) = …`), so a configured `knowledge.hint_limit = 0` also skips the search now instead of searching for nothing.
- `crates/meeting/src/lib.rs:557-558, 601` — `MeetingConfig.live_hint_limit: u32`, default `2` (the struct is `#[serde(default)]`, so the serde default is the `Default` impl). `lib.rs:561-571` — `MeetingConfig::live_toolset()` builds the recording `Toolset` with `hint_limit: Some(self.live_hint_limit)`; `start_task`'s `Setup` uses it (`lib.rs:1711`). Stop already clears the toolset.

### 2. Title pinned while recording (06 §1.7)

- `crates/agent/src/lib.rs:389-399` — `PinTitle { session, pinned }`, `ask` → `Result<(), String>`, beside `SetToolset`.
- `crates/agent/src/loop_.rs:357-359` — session message `SetTitlePin(bool)`; `loop_.rs:616-617, 729` — `Shared.title_pinned: Arc<AtomicBool>` (in memory, like `toolset`; nothing persisted, a restarted session is unpinned); `loop_.rs:1718-1726` — the handler; `loop_.rs:3410-3418` — `maybe_title()` returns before computing anything while pinned. `title.rs` untouched.
- `crates/agent/src/supervisor.rs:2755-2769` — `impl Message<PinTitle> for Agents`, the `SetToolset` route (`actor → native → tell(SetTitlePin)`).
- `crates/meeting/src/lib.rs:1782-1791` — `PinTitle{true}` right after the `RenameSession` ask in `start_task`; `lib.rs:2294-2304` — `PinTitle{false}` after `SetToolset(None)` at stop; `lib.rs:1974-1985` — `PinTitle{false}` on `Starting → Failed` beside the toolset clear (see Deviations). The `SetCompaction` blocks were not moved or reflowed.

### 3. Docs

- `docs/design/meetings/06-live-toolset.md` §1.5 (the `hint_limit` field and `live_hint_limit`, with the measurement), §1.6 (the pin's place in the start/stop order), new §1.7 "Title pinned while recording".
- `docs/design/meetings/README.md` config table: `live_hint_limit: 2`.
- `crates/theuth/harness.md` does not list the `meeting:` keys — unchanged.

## Behaviours and their tests

| behaviour | test |
|---|---|
| no override or `hint_limit: None` → the configured limit (all three seeded concepts hinted) | `agent` `loop_::meeting_diet_tests::hint_limit_and_title_pin` (real `Turn` at the fake provider, knowledge plugin seeded with three shared concepts the input matches on two terms) |
| `Some(2)` → exactly two hint lines | same |
| `Some(0)` → no `<knowledge>` note at all | same |
| a fresh session's first turn titles (`title_model` set, `title_turns` reset, a title task spawned); pinned → none of that | same (the `title_pinned` flag set on `Shared` directly: the smallest seam below `PinTitle → SetTitlePin → store`, which is a two-hop forward) |
| the toolset sent at start carries `hint_limit: Some(live_hint_limit)` — default `2`, `0` → `Some(0)`, a parsed `4` → `Some(4)` | `meeting` `tests::live_toolset_carries_the_hint_limit` |

The harness bypasses `Session::start_turn`, which sets `input_text` (the round-0 hint query) and `title_chars`; the test sets both as `start_turn` does (`loop_.rs:1063-1064`).

## Gate (crate-level only, as briefed)

| command | result |
|---|---|
| `cargo test -p agent -p meeting` | ok — agent lib 351 passed, 0 failed; meeting lib 57 passed, 0 failed; meeting `tests/note.rs` 1, `tests/oracle.rs` 1 passed; doc-tests 0 |
| `cargo clippy -p agent -p meeting --all-targets -- -D warnings` | ok — `Finished dev profile`, no warnings |
| `cargo fmt --all -- --check` | **fails on the base commit**, not on this change: rustfmt 1.9.0-stable (2026-06-26 and 2026-09-01 builds, both installed here) reports 274 diffs in 46 files at `95f6496a` itself (`git stash` → same 274), among them pre-existing hunks in `agent/src/loop_.rs:3853` and `agent/src/supervisor.rs:94, 850, 1767, 2543, 6330`. Every hunk the check reports in the five files this branch touches is in code this branch did not write; `crates/meeting/src/lib.rs` and `loop_/meeting_diet_tests.rs` are clean. The first commits had absorbed rustfmt's reflow of that untouched code (from an unguarded `cargo fmt --all`); `style:` commit `8461c7fc` reverts those hunks to the base text so the merge against the siblings stays clean. |

## Deviations

- **`PinTitle{false}` also on `Starting → Failed`** (`crates/meeting/src/lib.rs:1974-1985`). The brief names start and stop only; a start that fails after `RenameSession` would otherwise leave the surviving session pinned forever (06 §1.6 already clears the toolset there for the same reason). Twelve lines beside the toolset clear.
- **`MeetingConfig::live_toolset()`** (`lib.rs:561-571`) instead of the inline `Toolset` literal in `start_task`'s `Setup`: the brief's meeting test ("the toolset sent at start carries `hint_limit`") has no reachable seam otherwise — the meeting test bench has no `Agents` actor and `start_task` returns at `agent is not running` before any ask. The start/stop *order* of the `PinTitle` asks is therefore not covered by a test (the brief allowed "the smallest seam you can reach and say so"); it is covered by reading `start_task`/`StopMeeting`.
- **`crate::with_test_registry` evicts a dead `knowledge` actor** (`crates/agent/src/lib.rs:69-82`, test-only). `KnowledgePlugin::start` registers `KNOWLEDGE_ACTOR` without the `evict_dead` the tools registry does for its own names, so the second test body in a process to start the plugin failed with `name already registered` (`harvest_tests` was the only one before; now there are two, in either order). Fixing it in `knowledge` is outside this seam; the helper is the one place both tests pass through.
- **`knowledge.hint_limit = 0` in config now also skips the search** (`loop_.rs:3664`): a side effect of the `Some(0)` rule being applied to the resolved limit. Previously it searched and appended nothing.
- `crates/theuth/src/main.rs` `DEFAULT_CONFIG` (the commented `meeting:` keys, 06 §3) was not touched: outside the seam and adjacent to lines siblings may edit. See follow-ups.

## Open follow-ups

- `DEFAULT_CONFIG` in `crates/theuth/src/main.rs:1033-1034`: add `#   live_hint_limit: 2  # 06 §1.5 / 81: lines per <knowledge> note while recording; 0 = none` after the merge.
- `KnowledgePlugin::start`: an `evict_dead`-style guard before `register(KNOWLEDGE_ACTOR)` would let the test-only eviction in `with_test_registry` go.
- Measure on the next recorded meeting: with `live_hint_limit: 2` the `<knowledge>` block per batch should drop from ~1.8 KB to ~0.7 KB; the titler should run zero times between `started.` and stop.
