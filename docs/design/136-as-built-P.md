# 136 — As-built P (live proof)

Status: as-built · 2026-10-08 · tree `~/Develop/theuth-136`, branch `feat/136-dreaming`, on top of Z's `aa9696fe`. Three full dreaming nights plus four budget-refusal runs on scratch engines built from this tree against APFS clones of the user's knowledge + sessions (seven fresh clones, `/tmp/136/live1..7`); `~/.theuth` was only ever copied. P's commits: `e4eff6a4` sensor · `14c1d064` uses bootstrap · `62ac8793` skill/routine · `53bbdc55` undo receipt + journal · `6c888466` close gate · `08feccab` turn-end close · `eb2e3058` no decisions by use · `2ff37cb3` settling grace · `7f4160e2` fire-dated night (+ this file). Every number below is measured; no session text, private title or secret left the copies.

## 0. One screen

- **The nightly gate had never fired.** `sensor.py` parsed `read_file("dreaming/signals.json", numbers=False)` as JSON, but `read_file` always answers a `[read_file …: N lines …]` header line first (`crates/tools/src/fs.rs:124`); every check answered `events=no-signals Expecting value: line 1 column 2`. Z's scratch check read `last_check.outcome: quiet` as "outside the window — correct". Fixed (`e4eff6a4`, sensor v2 so the installed copy is rewritten, python regression test); from night 2 on the run envelope carries a real rule: `gate: a root session turned recently`.
- **Undo was not complete.** `commit_before` came from `history(id, 2).changes[1]` *when the event was handled*; the `Night` actor fell ≈ 6 s/event behind (a gix history walk over the 1.2 GB bundle per id), so when a verify of the same id landed before its merge event was handled, `commit_before` became the merge commit: 1 of 50 ids (`facts/theuth-dev-remotes-…-2026-10-06`) kept the merge's tags/source/status after "50 concepts restored". Fixed (`53bbdc55`): `KnowledgeChanged` carries its commit, `commit_before` = its first parent (one tree diff via `Knowledge::revision`), "" when that commit created the id. Nights 2 and 4 (315 and 390 ids, 31 of them written twice) restored byte-identical.
- **Archive-by-use went 95 → 0** the moment the live `uses` table had any row (16 of 1,664 ids): the fallback keyed on "table empty". Now per id — index rows win, the rest take the session-derived counts (`14c1d064`); nights 2/4 offered 60 (the cap).
- **The model closed early.** Night 1 (skill as shipped): 20 of 300 retitles, 10 of 14 merges, 10 verifies in 1 s each with no read tool, Mine "skipped due to budget constraints" at $0.37 of $3, final reply `paused`. Night 2 (skill tightened, `62ac8793`): 300/300 retitles but 0 merges, 0 archives, 0 cases at $0.36. Night 4 (close gate, `6c888466`): the first `section="journal"` was refused with `12 of 14 merge groups, 50 of 60 archive-by-use ids, 0 cases from 490 friction events (Mine)`; the model then merged 13 groups and archived 60, wrote `Skipped`, closed at $0.66.
- **A budget-ended night stayed open.** At `budget_usd: 0.05` the guard refused the second call and the turn ended `end_turn` — but the `<budget>` note is queued for a *next* turn a routine never has (`agent/src/loop_.rs:5174-5181`), so nothing journaled: `finished_at: null`, `run_request` left for the next night, `dreaming.nights` showed an open night. Fixed (`08feccab`, `2ff37cb3`): the actor closes the night on the session's `TurnFinished` after a 5 s settling grace — `Ended` line, `Receipt`, `run_request` cleared, `DreamNightFinished`.
- **The receipt's spend missed the first model call.** `spend_nanos` asks the agent ledger for `[started_at, now)` over `model_calls.created_at`, and `started_at` was the `dreaming_ledger` call — after the first (uncached, $0.09–0.13) round. Every night under-read by that call; a refused night read `$0.00 of $0.05`. Fixed (`7f4160e2`): `Night` remembers the routine's `Fired{at, session}` and dates the night from it; live7's receipt reads `$0.09 of $0.05` = the guard's `spent=93165000`.
- Also fixed: journal `## …` headings inside a section parsed as night boundaries (Undone landed above the model's text); a code-owned `Receipt` line on close; `decision`s offered as archive-by-use candidates (night 4 archived 5 of them).
- Held as designed: `RunNow → FireWith` (`<request>` block, run row), every write `dreaming <dreaming@localhost>` / `dreaming <op> <id>`, scope rules (no delete; `visibility`/`scope`/`owner` untouched; stubs `status: merged` + `merged_into` + `hint: false`; canonical `status: tidy`; `links` retargeted; retitles keep the text as `## Summary`; archive = `status: archived, hint: false` + `## Archived` reason, body otherwise intact), `KnowledgeChanged` per write, `Dream*` events on the WS stream, one `Dreaming` document reused, second undo = no-op line, FTS consistent after undo, the guard at $0.05.

## 1. Setup

| | |
|---|---|
| Binary | `~/Develop/theuth-136-target/C/debug/t` rebuilt from HEAD first — Z's binary (03:42) predated Z's last code commit `4bc662e5` (03:50); `theuth-documents-59.5` beside it; rebuilt after every fix (`BUILD_SLOTS=1 BUILD_LABEL=136-P`; waits of up to 14 min behind other waves' builds) |
| Clone | `/bin/cp -Rc` of `~/.theuth/knowledge` (1.2 GB with gix history), `knowledge.sqlite(+wal)`, `sessions.sqlite(+wal)` (872 MB), `acp/agents.yaml`; 2–16 min per clone (other waves' clones contend); one fresh clone per run: `live1` night 1 · `live2` night 2 · `live3` $0.05 (Z-era close) · `live4` night 4 · `live5` $0.05 (turn-end close) · `live6` $0.05 (grace) · `live7` $0.05 (fire-dated) |
| Config | hand-written: `tools:` verbatim, `rpc.listen 127.0.0.1:1794x–1799x`, `dreaming: {enabled: true, budget_usd: 3 \| 0.05}`, `link/update/meeting/talk/signal/slack/telegram.enabled: false`, `mesh: {relay: none, discovery: false, bind: 127.0.0.1:…, hub: never}`, `secrets: {key_store: file}`. Log per engine: 9 × "disabled by config", `mesh: … relay none (discovery false)`, no connection attempt |
| **Vault (deviation)** | the copied `secrets.sqlite` does not open in a scratch dir: `the mesh key does not open with this engine's key` — the vault key is a keychain item bound to the absolute data dir (`crates/secrets/src/key.rs:1-4`, `vault.rs:204-213`). Reported to the orchestrator as the brief's blocker; proceeded on the least-invasive path (approved by the orchestrator at 04:04, after the runs had started): **empty vault** (copied `secrets.sqlite` deleted — zero user secrets in `/tmp`) and **`agent.model: anthropic/claude-sonnet-4-5` on `ANTHROPIC_API_KEY` from the process environment** (genai's adapter fallback, `crates/agent/src/supervisor.rs:7951-7955`; the brief's `nohup env … $T` would have passed that env in). No key was extracted. Consequence: every night ran on Sonnet 4.5, not the user's `openai/gpt-5.6-sol`. Side effect: two `--secret-list` probes under `key_store: auto` minted a keychain item `theuth / secrets:/tmp/136/live1` and the rebuilt binary then blocked on a keychain-access prompt (`SecKeychainFindGenericPassword`); the item is harmless and can be deleted |
| Engine | `t --headless --data-dir /tmp/136/liveN` as a supervised process (a first `nohup … &` start from the tool shell died with its process group after ≈ 3 min, `engine.json` left behind; the restart was clean) |
| Driver | `scripts/apps/rpc.ts` behind two throwaway scripts in `/tmp/136`: `call.ts <dir> <method> [params]`, `watch.ts <dir> <out.jsonl>` (every `subscribe_events(null, descendants=true)` item, one JSON line) |
| Copied-store side effect | 109's resume found 2 open turns + 3 reminders in the copied `sessions.sqlite` and re-ran them on every scratch start; every write was refused by the 68 ownership guard (`… is owned by <live engine id> (epoch 1)`), the turns ended `error`, no model call persisted (`model_calls` unchanged). On a clone only — but it set `signals.last_root_turn_at`, so the sensor held on "a root session turned recently" and `sessions_since` counted them |

## 2. The nights

| | Night 1 · live1 (Z's code) | Night 2 · live2 (sensor, uses, skill, undo fixes) | Night 4 · live4 (+ close gate) |
|---|---|---|---|
| Fired | `run_now {3, pass}` 03:18:56Z → `01a11985-…` | 03:50:18Z → `01a119a2-…` | 04:14:29Z → `01a119b8-…` |
| Sensor envelope | `events=no-signals Expecting value…` | `events=hold / gate: a root session turned recently / sessions_since=2` | same rule |
| Ledger | 129 sessions · 164 plain user messages · 296 tool errors · untidied 1553 · candidates 0 fresh / **300** retitle / **14** stems / **0** archive-by-use / 15 verify | 135 · 168 · 319 · 1669 · 0 / 300 / 14 / **60** / 15 | 133 · — · 321 · — · 0 / 300 / 14 / 60 / 15 |
| Inner calls | knowledge_get 50 · retitle 20 · merge 10 · verify 10 · python 20 · load_skill, dreaming_ledger, knowledge_list, dreaming_journal 1 each | knowledge_get 308 · retitle 300 · verify 15 · python 20 · dreaming_journal 1 · load_skill, ledger 1 | knowledge_get 392 · retitle 300 · merge 25 · archive 60 · verify 15 · python 32 · dreaming_journal 8 (6 sections + refused close + close) · case_list 1 · **read tools 0** |
| Knowledge commits | 40 (20 retitle, 10 merge, 10 verify), all `dreaming <dreaming@localhost>` | 315 (300, 15) | 397 (300 retitle, 22 merge, 60 archive, 15 verify) |
| Counts (`DreamNightFinished`) | retitled 20 · merged 21 · verified 10 | retitled 300 · verified 15 | retitled 300 · merged 46 · archived 60 · verified 15 |
| Receipt ids (first group) | 50: retitle 20 · merge 21 · verify 9 | 315: retitle 300 · verify 15 | 390: retitle 300 · archive 50 · merge 25 · verify 15 (31 ids written twice) |
| Close | model's one `journal` section; actor 2 min 48 s behind | one section + `Receipt` line; 20 s | refused once (`nudge:` in phases) → 13 merge groups + 60 archives → `Skipped` → closed |
| Duration (fire → `TurnFinished end_turn`) | 6 min 18 s | 6 min 41 s | 10 min 44 s |
| Model calls · tokens | 21 · 703 k prompt (649 k cached) / 6.7 k out | 21 · 721 k (668 k cached) / 6.7 k | 33 · 1,249 k (1,172 k cached) / 9.0 k |
| Spend (ledger · `model_calls` estimate) | **$0.367** · $0.498 | **$0.361** · $0.500 | **$0.656** · $0.772 |
| Undo | 50 restored, 0 errors, 21 s; **49/50** byte-identical (§0); 2nd undo no-op | 315 restored, 0 errors, 3 min 13 s; **315/315** identical; 2nd no-op | 390 restored, 0 errors, 7 min 12 s; **390/390** identical; 2nd no-op |

Budget-refusal runs (`run_now {0.05, pass}`, one model call of ≈ 23.5 k prompt tokens each, $0.093–0.094, then `budget spent; turn ends spent≈9.3e7 cap=5e7`, `TurnFinished end_turn`, 0 writes, 0 commits, 0 `KnowledgeChanged`): **live3** (Z-era close, 12 s): record left **open** — `finished_at: null`, `run_request` kept, `dreaming.nights[0].finished_at: null`. **live5** (`08feccab`, 19 s): closed inline — `phases ["ended: end_turn"]`, `### Ended` + `### Receipt — nothing changed · $0.00 of $0.05`, `run_request: null`, `nights: 1`, `DreamNightFinished` observed; spend 0. **live6** (`2ff37cb3`, 5 s grace): closed 5 s after `TurnFinished`; spend still `$0.00` — the window, not the settling, was the cause. **live7** (`7f4160e2`): `started_at 05:46:33Z` = `runs.fired_at`, closed at `05:46:58Z`, `### Receipt — nothing changed · $0.09 of $0.05`, `dreaming.nights[0].spend_usd 0.093165` = the guard's `spent=93165000`.

Total model spend across all seven runs: **≈ $2.14** by `model_calls` estimates (0.498 + 0.500 + 0.093 + 0.772 + 0.094 + 0.094 + 0.093) — cap 10. The three full nights' ledger spend (0.367 / 0.361 / 0.656) under-read by their first call (§0).

### 2.1 Observations that are not in the table

- `<knowledge>` hints are **on** in the dreaming session (message seq 1 of every night is a `<knowledge>` note) and every note records impressions: the night's own chatter (300 titles per pass) inflates `uses.shown` of neighbouring ids with 0 opens — the signature archive-by-use looks for. §6 item 1.
- `knowledge_verify` ×10/15/15 with **no read tool call in the session** on all three nights (`inner_calls`: no `read_file`/`glob`/`code_find`/`session_read`); night 4's notes say "2026-10-08 dreaming pass - quick review". The skill forbids it in words (twice now); it needs a code gate. §6 item 2.
- Mine never produced a case (0 of 60) over 164–168 plain user messages and ~300 tool errors per window; the model's reasons ("budget constraints", "approaching round limit") were false at ≤ 22 % of budget. Rehearse/learned edits/`replay_case` therefore never ran live.
- Night 4's `Skipped`: "1 merge group failed (secret name in content)" — `knowledge_merge` refused a body the secrets redactor flagged; the stub pair stayed as before (correct refusal).
- The receipt dedups by id: an id retitled then archived is one `retitle` row restored to the commit before the retitle (= before both). Counts in `NightCounts` are per op.
- Restore commits are authored `theuth` (`restore <id> from <sha>`) — the ordinary write path, not `dreaming`.

## 3. What the refusal runs proved about the frame

- The guard is the real stop: on a one-turn routine the model never sees `<budget …>` (queued for a next turn) nor `budget exhausted:` (that is the side-call text); the turn simply ends `end_turn`. The skill's "on a `<budget>` note, skip to Journal" can only happen for `replay_case` refusals; the actor's turn-end close is what records a refused night.
- `LedgerSince` windows `model_calls` by `created_at` (`crates/agent/src/store/dreaming.rs:296`); a night's spend is only right when `started_at` precedes the first call — hence the fire-dated night.
- The refused night's one call is the uncached first prompt (≈ 23.5 k tokens: routine, skill via `load_skill`, `<knowledge>` note, tools); at the user's `openai/gpt-5.6-sol` price that first call alone can exceed a very small cap, so a `budget_usd` under ≈ $0.15 buys no work.

## 4. Acceptance checklist

| # | Claim | Result | Evidence |
|---|---|---|---|
| 1 | `dreaming.status` / `list_schedules` show the routine; sensor quiet outside the window | PASS | `{enabled, next_window 2026-10-09T00:00:00Z (= 01:00 BST), budget 3, backlog {0, 1553, 0}}`; row `dreaming`, `when: dreaming.py …`, `last_result: quiet` |
| 2 | The sensor evaluates the real rules | **FAIL → fixed** | night 1 envelope `events=no-signals Expecting value: line 1 column 2`; nights 2/4 `events=hold / gate: a root session turned recently` |
| 3 | `RunNow → FireWith` applies the overrides | PASS | `schedule.sqlite.runs`: `dreaming`, the session id, `fired_at`; first message `<request>\nmode: pass\nbudget_usd: 3\n</request>`; `state.json.run_request` set then cleared on close |
| 4 | Frame: skill present, `<knowledge>` hints off in the dreaming session | **PARTIAL** | `load_skill("dreaming")` is the first inner call; there is no `<dreaming-ledger>` prompt block in the implementation (the ledger is the `dreaming_ledger` tool's structured result, `<dreaming-ledger>` appears nowhere in `crates/`); hints are **on** (§2.1) |
| 5 | Every write authored `dreaming <dreaming@localhost>`, message `dreaming <op> <id>` | PASS | 40 + 315 + 397 commits, 100 % that author; subjects `dreaming retitle|merge|verify|archive <id>` |
| 6 | Op scope | PASS | script over every touched id (git BEFORE vs HEAD): 0 `visibility`/`scope`/`owner`/`type` changes, 0 deletions, 0 `fresh` touched (the store had none); retitles: `## Summary` on 20/20, 300/300, 300/300, titles 124–439 → ≤ 126 chars (one first clause stayed 126 — the tool does not clip); merges: stubs `status: merged` + `merged_into` + `hint: false` + one-line body, canonicals `status: tidy`, `links` `… → dr-failover-cli-design-v2` retargeted; archives (night 4): 60 × `status: archived, hint: false` + `## Archived` reason, body otherwise unchanged — but 5 were `decision`s (**fixed**, `eb2e3058`) |
| 7 | `KnowledgeChanged` per write | PASS | 101 / 630 / 811 `KnowledgeChanged` events on the stream (ops + restores) |
| 8 | Hints exclude archived | PASS (index) | after night 4: `concepts` rows `archived \| hint 0 \| 60`; `hint_for` filters `!hint` and non-live status (`store.rs:1109-1115`, unit-tested); no live turn was run against the archived store |
| 9 | Cases ≤ cap, append-only, scrubbed | NOT EXERCISED | 0 cases on every night (model); `cases.jsonl` never created; scrub path unit-tested |
| 10 | Learned edits / judge under `CallKind::Dream`, budget-checked | NOT EXERCISED | no case → no cluster; unit tests only |
| 11 | Journal: one document, counts, undo line later; `dreaming.nights` matches | PASS (shape fixed) | one `Dreaming` note per engine (`doc_list`), `state.json.journal` = its id, reused by every append; night 4 note: 6 sections (`tidy … suggest`), `journal`, `Skipped`, `Receipt — 300 retitled · 46 merged · 60 archived · 15 verified · $0.66 of $3.00 · undo: dreaming_undo("2026-10-08") or one group: archive, merge, retitle, verify`, then `### Undone` ×2; `dreaming.nights[0]` carries the same counts, `undone: true` after the undo |
| 12 | Undo complete and idempotent | **FAIL → fixed** | night 1: 49/50; nights 2 and 4: 315/315 and 390/390 files byte-identical to BEFORE (only `index.md`/`log.md` differ); second undo `knowledge_restored: 0` and a second `Undone` line |
| 13 | `DreamNightStarted/Finished/Undone` observed by a WS subscriber | PASS | per night: `DreamNightStarted{night}`, `DreamNightFinished{night, counts, spend_nanos, proposals: []}`, `DreamUndone{night, group: null}` ×2 |
| 14 | Budget guard refuses at $0.05 with `budget exhausted`, clean end, night record says so, no write without receipt | **PARTIAL → fixed** | log `budget spent; turn ends spent≈9.3e7 cap=5e7`, `TurnFinished end_turn`, 0 writes, 0 commits on all four runs; the `<budget>` note never reaches a one-turn routine (queued for the next turn) and the record stayed open until `08feccab`; live5–7 close with `ended: end_turn`, live7's receipt carries the real spend (`7f4160e2`) |
| 15 | Privacy | PASS | ids, counts and knowledge titles only; no session text or secret in this file or in messages |

## 5. Bugs found and fixed

| # | Bug | Where | Fix | Commit |
|---|---|---|---|---|
| 1 | sensor parses `read_file` output as JSON → never fires | `crates/dreaming/src/sensor.py:104` (`check`) | `strip_header`, `# theuth-sensor: dreaming 2`; test `python_sensor_reads_signals_under_the_read_file_header` | `e4eff6a4` |
| 2 | `uses` fallback keyed on an empty table | `dry_run.rs:132`, `ledger.rs:144` | `merge_uses(index, sessions)` per id; `ledger::candidates` treats `stats`' zero rows as absent; test | `14c1d064` |
| 3 | `commit_before` from history at handling time (race, 6 s/event) | `night.rs` `KnowledgeChanged` handler; `knowledge/src/lib.rs:117` | `KnowledgeChanged.commit` at every publish site in `store.rs`; `night::commit_before` = parent via `revision`; test reproduces merge-then-verify | `53bbdc55` |
| 4 | headings in appended markdown break the night block | `journal.rs:87` | demote `#`–`###` → `####`; test | `53bbdc55` |
| 5 | no code-owned counts/undo handles in the note | `night.rs` `finish` | `Receipt` line; test | `53bbdc55` |
| 6 | model samples the lists, "verifies" blind, heads sections with `##`, replies "paused" | `skill.md`, `routine.md` | prose: whole lists, read tool in the same cell, bullets, one-line reply | `62ac8793` |
| 7 | the night closes with the lists unworked and the money unspent | `night.rs` `Commit{journal}` | `left_undone` → one refusal (< 75 % spent) naming what is left; `Skipped` section; test | `6c888466` |
| 8 | a budget/error-ended night stays open | `night.rs` (no `TurnFinished` handler) | subscribe `AgentEvent`; `CloseEnded` after a 5 s grace → `Ended` line, `finish`; tests | `08feccab`, `2ff37cb3` |
| 9 | decisions offered as archive-by-use | `tidy.rs:123` | exclude `decision` like `warning`; test | `eb2e3058` |
| 10 | the spend window starts at the ledger call, after the first model call | `night.rs` `BeginNight`/`spend_nanos` | subscribe `ScheduleEvent`; `Fired{name: dreaming}` dates the night; test | `7f4160e2` |

Gates after every fix (build-lock `136-P`): `cargo test -p dreaming` 42 passed (36 + 6; the fire-dated and grace assertions live inside existing tests), `cargo test -p knowledge` 44, `cargo clippy -p knowledge -p dreaming --tests -- -D warnings` clean, `cargo fmt -p knowledge -p dreaming --check` clean, `just ts-desktop` regenerated (`KnowledgeChanged.ts` gains `commit: string | null`; both bindings dirs are gitignored), `cargo build -p theuth`.

## 6. Bugs and gaps left (file:line reason)

1. **Hints in the dreaming session count as impressions** — `crates/agent/src/loop_.rs:5047-5082` builds the note and calls `record_shown` for every chat session; the only off switch is a `Toolset{hint_limit: Some(0)}` *allowlist* (`agent/src/lib.rs:511-522`). Recommendation: skip the note (or at least `record_shown`) when `knowledge.may_consolidate(session)` says "unattended root", or let `dreaming_ledger` mark its session. Until then every pass adds ≈ `rounds/hint_rounds × 5` impressions near the titles it tidies.
2. **`knowledge_verify` has no read-tool gate** — observed on 3/3 nights despite the skill. `crates/knowledge/src/tools.rs` cannot see the session's calls; the cheap gate lives in dreaming: a `verify` is refused unless the session's `inner_calls` since `knowledge_get(id)` include `read_file|glob|code_find|session_read`. Until then `verified:` dates from a night are not evidence.
3. **Mine produces nothing** — the model declines to mine on every night; the close gate now names it but accepts a `Skipped` reason. A code floor (file ≥ 1 case or name the specific events judged unexplainable) is the next step; Rehearse/replay/judge stay unproven live until cases exist.
4. **Undo leaves `links` retargeted** — `merge` rewrites `links` index-only; restoring a stub re-indexes that file only, so the citing concept's row keeps pointing at the canonical until that file is written again (night 1: `links where dst='dr-failover-cli-design'` 1 → 0 after undo).
5. **The night self-harvests** — `loop_.rs:6149-6153` harvests any depth-0 session on compaction; not observed on these ≤ 11-minute nights (0 non-dreaming commits), expected on a long backlog run.
6. **Restore is slow at scale** — 390 restores took 7 min 12 s (≈ 1.1 s each: one commit + `regenerate_indexes` per id). A batched restore (one commit per group) is the fix if undo is used on backlog nights.
7. **One retitle stayed 126 chars** — `knowledge_retitle` accepts the model's first clause as is; clipping or refusing > 120 in the tool would make the §13 metric a contract.
8. `Receipt.commit`/`KnowledgeChanged.commit` are abbreviated ids, `revision().parent` full; both resolve through `rev_parse_single`.
9. Not exercised live: learned edits, `replay_case`, the judge, standing preferences, proposals, backlog mode (one pass per night at ≈ $0.4–0.7 leaves the budget for the user's first real run), the UI.

## 6.1 Model-sensitive parts of the skill and prompts (the proof ran on Sonnet 4.5; the user's night runs on `openai/gpt-5.6-sol`, `effort: max`)

Orchestrator's ask (04:04): what in `skill.md`/`routine.md`/`replay.rs` depends on the model.

- **Phase discipline** — the whole night is one turn following prose; Sonnet sampled, invented budget reasons and skipped Mine on 3/3 nights. The close gate (`6c888466`) and the `Receipt` line are the only model-independent floors; everything else (Mine, Rehearse, Settle, Suggest) is prose-bound and will vary by model.
- **Python-cell batching** — Sonnet batched 300 retitles into ~10 cells (21–33 model calls, $0.36–0.66 per pass). A model that calls one tool per round would spend 10–30× more calls for the same work; `max_tool_rounds` (384) and `run_timeout` (45 m) are the limits it would hit first.
- **`knowledge_verify` without reading** — Sonnet ignored the read-tool rule even after it was spelled out; another model may comply or not. Gap 2's code gate is the fix.
- **The tidy-mark put recipe** (`knowledge_put(..., status="tidy")` built from `knowledge_list` meta + `knowledge_get` body) and the `decide_many` lowercase row tags — exact tool shapes; a model that paraphrases argument names fails the call (the error is returned, the id skipped).
- **Proposer / judge prompts** (`replay.rs:JUDGE_PROMPT`, skill `### Proposer prompt`) — never exercised live (0 cases). The judge must answer one JSON object; `parse_judge` tolerates prose around it but not a missing `score`. The judge is the session model when `models.judge` is unset, so proposer and judge are the same vendor (the design wanted a different one).
- **Final reply** — the routine's only quoted reply text was "paused"; Sonnet echoed it. Now explicit (`dreamt <night>: …`).
- **Prompt cost** — the first call is ≈ 23.5 k tokens (routine, skill, `<knowledge>` note, tool schemas); on `gpt-5.6-sol` with `effort: max` that call alone is likely to exceed a sub-$0.15 cap and reasoning tokens will dominate each cell; expect a pass to cost several × the Sonnet numbers and the 45 m `run_timeout` to matter.

## 7. Recommendations for the first real night on the user's engine

- **Rebuild and restart before the window**: the sensor fix takes effect only when `sensors/dreaming.py` is rewritten at start (v2 ≠ the installed v1). Check `list_schedules → last_check.outcome` and the run envelope's `gate:` line; `events=no-signals` means the old sensor is still installed.
- Expect per pass: 300 retitles, ~14 merge groups, up to 60 archives (none of them decisions now), 15 "verifies" that are not real checks (gap 2 — consider `caps.verifications: 0` until the gate exists), 0 cases. On Sonnet a pass cost $0.36–0.66 and 7–11 min; `openai/gpt-5.6-sol` with `effort: max` will be dearer and slower — keep `budget_usd` at 1 for the first night and read the `Receipt` line before raising it.
- `t dreaming dry-run --data-dir <copy>` first; with the uses merge its archive-by-use count is the one the night acts on.
- The copied-store 109 resumes are a scratch artefact; on the live engine the open turns are its own.
- Undo per night or per group (`dreaming_undo("<night>", "merge")`); after undoing merges, restart (or write the citing concepts) to refresh `links` (gap 4). An undo of a 390-id night takes ≈ 7 min.
- Artefacts for the orchestrator: `/tmp/136/live1/dreaming/` (night 1: `ledger/`, `journal.md`, `journal-export.md`, `events.jsonl`, `undo1.json`) and `/tmp/136/nights/<liveN>-dreaming/` + `<liveN>-events.jsonl` for the other six runs; the data-dir clones themselves are removed.
- Watch `uses.shown` growth on ids the night itself touched (gap 1) before trusting archive-by-use on night 2+.
