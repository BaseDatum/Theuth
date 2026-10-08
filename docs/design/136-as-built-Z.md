# 136 — As-built Z (closer)

Status: as-built · 2026-10-08 · integration tree `~/Develop/theuth-136`, branch `feat/136-dreaming` · on top of the orchestrator's merges W1 → W2 → W4 → W3 (`dc752a07`, no conflicts). Three Z commits: `8e645c84`, `30a18e61`, `4bc662e5` (+ this file). Read with `136-contract.md` §3.5/§4 and the five as-builts; every hand-off below names the as-built it came from.

## 0. One screen

- **Merged tree compiled as merged** (`cargo check -p knowledge -p agent -p schedule -p dreaming -p rpc --tests` clean before any Z edit); the only merge residue was an uncommitted `Cargo.lock` (W3's six dependency edges) — committed.
- **Budget guard now covers side calls**: the cap moved from the turn-owned `SessionState.budget` to `Shared.budget` (one `Arc<RwLock<Option<Budget>>>` the running turn and the session actor share); `Side::call` passes it to `Reserved::open`, so `replay_case`'s two `CallKind::Dream` calls, title, escalate and advisor side calls are refused like round calls. The refusal text starts `budget exhausted:`; the skill names it as the second stop signal.
- **`knowledge_put` with a consolidation key is a dreaming write**: `status`/`hint`/`verified`/`valid_until`/`superseded_by`/`merged_into` on a put now require the dreaming-only predicate and commit as `source: dreaming`, so the tidy-mark puts the skill prescribes land in the night's undo receipt (`put` group) and a live session cannot archive or un-hint a concept through a plain put. Before this the receipt never saw them (`Night` listens only to `KnowledgeChanged{source: dreaming}`).
- **Harvest amendment folded into `knowledge::harvest`** (W2 hand-off): the template's `type:`/`title:` lines carry the 136 text, `split_title` moved into `knowledge::harvest` and writes the `## Summary` heading `retitle` writes; `crates/agent/src/harvest.rs` deleted.
- **`Dream*` events reach the RPC stream and the UI union** (W4 hand-off, preferred option); `useDreamStatus` subscribes to them instead of `Fired`/`TurnFinished`/`KnowledgeChanged`.
- **`t dreaming dry-run [--data-dir] [--json]`** wired before `first_run` so the dry run creates nothing, not even `config.yaml`. On a pre-136 store the `uses` table is empty, so the dry run (and the live night's candidates) derive impressions/opens from `sessions.sqlite` until the live counters exist — without it archive-by-use was 0 on the live copy; with it 95 (the 361/0, 331/0 ids among them).
- All gates green; dry run on the APFS copy wrote nothing; the scratch engine installed the three bundled files, answered `dreaming.status`/`list_schedules`, and an undo created the `Dreaming` document once and reused it.

## 1. Hand-offs closed (by source)

| From | Hand-off | Done |
|---|---|---|
| C | `crates/tui` test `Meta {` literals | `..Default::default()` on both (`model.rs`, `ui.rs`) |
| C / Z brief | `t dreaming dry-run --data-dir <dir>` (+ `--json`) | `Cmd::Dreaming { DreamingCmd::DryRun { data_dir, json } }` in `main.rs`, dispatched before `first_run`; `--data-dir` on the subcommand falls back to the global one |
| C | `agent::loop_::tests::prompt_parts_in_order_with_blank_lines` | needle tightened to `- rules:` (an `always` skill is inlined, not indexed) |
| C | `rpc/tests/prompts.rs` second `answer_ask` → `agent actor is not running` | `agent::answer_ask`: a late answer with no agent actor is warned and dropped, still `false` (production always has the actor; the test harness does not) |
| C | dotted `dreaming.*` names vs `api.ts` symmetry | kept dotted: W4 typed them as `api['dreaming.status']()` and `methods.json` carries them; no rename |
| W1 | `restore(id, None)` re-applies after one restore | verified: `undo.rs:61` passes `Some(&t.commit_before)`; an empty `commit_before` is an error line |
| W1 | `DEFAULT_CONFIG` `knowledge.lifetimes:` block | added (commented, defaults 14/30/90/60) |
| W1 | `links` retarget is index-only; citing bodies | left as is; the skill's Merge step writes one living entry, old citations resolve through `merged_into` (W1 §5) |
| W1 (optional) | `source` on the five ops for hand-run attribution | not done; every consolidation write (five ops and the gated put) commits as `dreaming` — consistent, recorded |
| W2 | side calls counted but not refused | done (§0); test `budget_tests` asserts a `CallKind::Dream` side call under the cap answers `budget exhausted: …` with nothing sent, then the round path as before |
| W2 | fold the amendment into `HARVEST_INSTRUCTION`, drop the agent append | done; `knowledge::harvest::{TITLE_MAX, split_title}`; tests moved |
| W2 | retitle `## Summary` vs harvest `Summary:` | aligned on `## Summary` (the store's shape); `harvest_compact_tests` asserts it |
| W2 | `SkillMeta` literals outside agent | untouched (no field added) |
| W3 | `Cargo.lock` | committed in `8e645c84` |
| W3 | `decide_many` refuses `double.correction` | the skill never asks for that family: the correction statement runs as a `{"type":"bool"}` row under the default `tool` family (zero-shot gliner); recorded in the skill text. The double's calibrated family stays unreachable from the tool in v1 |
| W3 | `propose` tier unused | Settings hint: "v1: the proposer runs as the routine's own model (the Tidy tier); this pick is saved for when the proposer gets its own call"; design §9.2 comment amended |
| W3 | journal-document creation untested live | asserted on the scratch engine (§4): one `Dreaming` note created, id in `state.json.journal`, second append reuses it, `doc_markdown` shows both lines |
| W3 | attention rows / permanent watcher `max_runs` | not this wave; unchanged |
| W4 | `Dream*` not in `WireEvent` | `rpc::bridge` forwards + subscribes `DreamNightStarted/Finished/Undone`; `rpc::client` republishes them over mesh; `events.ts` union + `queries.ts` subscriptions |
| W4 | `just ts-desktop` drift | none beyond doc comments; `tsc`, vitest (90), `vite build` green on the regenerated bindings |
| W4 | `api.test.ts` resolves root `bindings/methods.json` | left (pre-existing; passes here because `just ts` writes the root copy) |
| W4 | `ModelCombobox` pickers, client-side resolved default | kept |

## 2. Z's own fixes (not in a hand-off)

1. **Skill prose against the real tool contracts** (`crates/dreaming/src/skill.md`, read end to end as the model): `decide_many` row tags are lowercase (`"type":"choice"` / `"bool"`, serde `rename_all = "lowercase"` — the skill said `Bool`/`Choice`, which the tool rejects); the tidy mark is spelled out as a python recipe that takes the structured meta from `knowledge_list` and the body from `knowledge_get`'s verbatim text (no `yaml` in Monty; `knowledge_get` returns a string, not a dict); the retitle list is named as capped at 300 per pass; a `replay_case` answering `budget exhausted:` is the second stop signal (Budget order and Rehearse); warning-shaped facts (`sky-atuin-history-contains-private-key`, typed `fact` before 136) are retyped, not archived; the family note in Mine says what the classifier actually is. Proposer and judge prompts unchanged (complete; no task rules, no evaluator text, no run instructions). Size 21.4 KB (< 24 KB test cap).
2. **`knowledge_put` consolidation gate** (§0) — `crates/knowledge/src/tools.rs`, test extended (`consolidation_tools_need_owner_root_or_unattended`: a plain put stays `source: model` for anyone; a `status: tidy` put is refused elsewhere and lands as `dreaming` for the allowed session).
3. **Uses fallback** — `dry_run::uses_from_sessions` (regex over `<knowledge>` note bodies for `shown`, `knowledge_get{id}` rows of `inner_calls`/`tool_calls` for `opened`); `dry_run::read_uses` and `ledger::candidates` use it when the `uses` table has no counters. Costs one `LIKE '%<knowledge>%'` scan (≈ 10 s on the 889 MB live store), only until impressions exist.
4. **Pre-existing clippy** `nonminimal_bool` in `crates/firecrawl-native/src/noise.rs:142` (new toolchain lint; a workspace member is linted when it is a dependency) — simplified, behaviour identical.

## 3. Gates (all through `build-lock`, label `136-Z`, target `~/Develop/theuth-136-target/C`)

| Gate | Result |
|---|---|
| `cargo test -p knowledge -p schedule -p dreaming -p rpc --no-fail-fast` | knowledge 44 · schedule 74 · dreaming 36 · rpc 52 + `tests/prompts.rs` 1 — 0 failed |
| `cargo test -p agent --lib --no-fail-fast` | **468 passed, 0 failed, 2 ignored** (W2's 470 − the 3 moved harvest tests + the new side-call assertions inside `budget_tests`; the pre-existing `prompt_parts…` failure is gone). No SIGSEGV in this run |
| `cargo clippy -p knowledge -p agent -p schedule -p dreaming -p rpc -p theuth -p tui --tests -- -D warnings` | clean after the `firecrawl-native` one-liner |
| `cargo fmt -p knowledge -p agent -p schedule -p dreaming -p rpc --check` | clean (`theuth`/`tui` have pre-existing drift outside 136 — not reformatted) |
| `BUILD_SLOTS=1 just ts-desktop` | 8 export tests, 675 bindings, 14 `Dream*` files |
| `crates/desktop/ui`: `bun run check` · `bunx vitest run` (W4's six files) · `bun run build` | tsc clean · 6 files / 90 tests · built (needed `bun install` in `packages/apps` and `packages/documents` first) |
| `BUILD_SLOTS=1 cargo check --workspace --tests` | clean |
| `cargo build -p theuth` | `…/C/debug/t` |

## 4. Phase II

**Dry run** (`t dreaming dry-run --data-dir /tmp/136/dd`; APFS clone of `~/.theuth/knowledge`, `knowledge.sqlite(+wal)`, `sessions.sqlite(+wal)` only):

```
dreaming dry run
  sessions in window      109
  friction events         287
  fresh entries           0
  retitle candidates      1515
  duplicate stems         14
  archive-by-use          95
  verification sample     15
```

Against contract §4: retitle 1,515 (≈ 1,250 expected; the bundle grew from 1,584 to 1,665 files since the measurement) · duplicate stems 14 (≈ 19; `tidy::stem` strips `-YYYY-MM-DD`, `-YYYYMMDD`, `-vN` — the five missing groups are stems that differ by more than a date suffix; the skill tells the model to add groups it notices while triaging) · archive-by-use 95 (expected "several hundred records + the never-opened ids": there are **no `type: record` entries yet** — the type is new — so the 95 are the ≥ 50 shown / 0 opened / ≥ 14 d ids, `facts/qartez-mcp-recon` (368 impressions in the copy) and `facts/laya-browser-system1-loop` included; 0 before the uses fallback) · verification sample 15 · fresh 0 (no `status: fresh` exists before the 136 harvest runs). Wall time 14 s (23 s cold).

**Zero writes**: knowledge tree fingerprint (path, mtime, size over 30,594 entries) identical before/after; `git status --porcelain | wc -l` 1,682 before and after (the live bundle is gix-written with no git index, so the baseline is not 0 — the count is the invariant); `knowledge.sqlite`/`-wal` and `sessions.sqlite`/`-wal` mtimes unchanged; only `-shm` files appeared (W3 deviation 14). No `config.yaml`/`prompt.md` created.

**Scratch engine** (`t --headless --data-dir /tmp/136/dd2`, `THEUTH_RPC__LISTEN=127.0.0.1:17936`, mesh/update off): after start `skills/dreaming.md`, `sensors/dreaming.py`, `schedules/dreaming.md` present; the routine file has `every: 30m`, `when: dreaming.py window=01:00-06:00 idle=20 heavy_concepts=300 heavy_sessions=40`, `quiet: true`, `approvals: false`, `budget: 1 USD`, `run_timeout: 45m`, no `enabled:` line (= enabled; `list_schedules` shows `enabled: true`). `dreaming/signals.json` written (`on_ac: true`, `battery_pct: 100`, engine id, tz offset); `state.json` with the engine id. `dreaming.status` → `{enabled: true, next_window: 2026-10-09T00:00:00Z (= 01:00 local), budget 1, backlog zeros}`; `list_schedules` row `dreaming` with `sensor: dreaming.py`, `last_check.outcome: "quiet"` (03:45 local, outside the window — correct). Journal document: a planted empty night record + `dreaming.undo("2026-10-07")` over RPC → `doc_list` shows one `Dreaming` note, `state.json.journal` = its id, a second undo appends without creating another, `doc_markdown` returns both lines. First attempt failed with `documents helper not found` (no `theuth-documents-59.5` beside a debug `t`); the document row was created but the id not saved — `just documents-helper` + copy fixed it. Note for packaging: a `t` without its documents helper half-creates the note on every append (document crate behaviour, not 136's).

## 5. Deviations from the contract (and why)

1. **`Budget` lives on `Shared`, not `SessionState`** (contract §2.2 "`SessionState.budget`"): the side-call hand-off needs the cap outside the turn; `SessionState.budget`/`Session.pending_budget` removed, `SetBudgetNow` writes the shared cell. Message and wire types unchanged.
2. **`Refused::Budget` Display** is `budget exhausted: spent … + estimate > cap … (CUR nanos)` (was `budget: …`); the `<budget>` note and `Abort::Budget` texts are unchanged.
3. **`knowledge_put` gains a gate on six fields** (not in the contract's tool table): see §0. A put without those keys is exactly as before.
4. **`Night` and the dry run read impressions from `sessions.sqlite` when `uses` is empty** — a bootstrap rule, not a new seam.
5. **`answer_ask` tolerates a missing agent actor** for late answers (pre-existing test; outside 136).

## 6. Not verified

No live night was run (no provider key in the scratch dir; the user's engine was not touched). Not exercised: the budget guard against a real provider (unit-tested at the fake provider), `replay_case` end to end, `RunNow` → `FireWith` on a live schedule actor, the sensor firing inside the window, the proposer/judge prompts on real cases, the UI against the real `dreaming.*` RPCs (W4's fixture only; `tsc`/vitest/build on the regenerated bindings), the `uses` fallback's cost on a multi-week window (one 889 MB scan ≈ 10 s), `Dream*` events observed by a desktop client. The `DoubleTrainingState`-style TUI bridge does not forward `Dream*` (the TUI has no dreaming surface in v1).

## 7. Open for the orchestrator

- Ship `theuth-documents-59.5` beside `t` as before; the journal needs it.
- First night: expect ≈ 300 retitles per pass (the cap), ~95 archive candidates, 15 verifications, 0 fresh until the 136 harvest has run a day; `Dream now → Backlog` with a larger budget chews the 1,515 retitles in ≈ 5 passes.
- Pre-existing fmt drift in `crates/theuth` and `crates/tui` (not 136); `block v0.1.6` future-incompat note from the workspace check (not 136).
