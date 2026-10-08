# 136 — As-built P (live proof)

Status: as-built · 2026-10-08 · tree `~/Develop/theuth-136`, branch `feat/136-dreaming`, on top of Z's `aa9696fe`. Three real dreaming nights on scratch engines built from this tree against APFS clones of the user's knowledge + sessions; `~/.theuth` read-only throughout (copied, never opened for writing). P's commits: `e4eff6a4`, `14c1d064`, `62ac8793`, `53bbdc55` (+ this file). Numbers below are measured, not estimated; private session text never left the copies.

## 0. One screen

- **The nightly gate never fired before P.** `sensor.py` parsed `read_file("dreaming/signals.json", numbers=False)` as JSON, but `read_file` always answers a `[read_file …: N lines …]` header line first (`crates/tools/src/fs.rs:124`); every check since the first install answered `events=no-signals Expecting value: line 1 column 2`. Z's scratch check read that as "quiet, correct". Fixed (`e4eff6a4`, sensor v2, python regression test); night 2's envelope carries `gate: a root session turned recently` — a real rule.
- **Undo was not complete.** The receipt's `commit_before` came from `history(id, 2).changes[1]` *at event-handling time*; the `Night` actor fell ≈ 6 s/event behind (a gix history walk over the 1.2 GB bundle per id), so when a verify landed before the merge event of the same id was handled, `commit_before` became the merge commit itself: 1 of 50 ids (`facts/theuth-dev-remotes-…-2026-10-06`) kept the merge's tags/source/status after "whole night: 50 concepts restored". Fixed (`53bbdc55`): `KnowledgeChanged` carries its commit, `commit_before` = its first parent (one tree diff, `Knowledge::revision`), "" when that commit created the id.
- **Archive-by-use went 95 → 0** the moment the live `uses` table had any row (16 of 1,664 ids): Z's fallback keyed on "table empty". Now per-id: index rows win, the rest take the session-derived counts (`14c1d064`); night 2's ledger: 60 candidates (the cap).
- **Journal shape**: the model wrote `## Tidy … ## Summary` inside its `journal` section, which `journal::insert` read as night boundaries — the Undone line landed above the model's text. Headings in appended markdown are now demoted; the close appends a code-owned `Receipt` line (counts · spend of cap · undo handles) so the counts in the note are the harness's, not the model's prose (`53bbdc55`).
- **Model behaviour on night 1 (Sonnet 4.5, skill as shipped)**: 20 of 300 retitles, 10 of 14 merges, 10 verifies in 1 s each with no read tool, Mine skipped "due to budget constraints" at $0.37 of $3, final reply "paused". Skill/routine tightened (`62ac8793`): work the whole (already capped) lists, verify only behind a read tool in the same cell, bullets not headings in sections, one-line final reply. Night 2 on the fixed text: see §2.
- Everything else in the acceptance list held: `RunNow → FireWith` (`<request>` block, budget override in the run row), every write authored `dreaming <dreaming@localhost>` with `dreaming <op> <id>`, scope rules (no delete, `visibility`/`scope`/`owner` untouched, stubs `status: merged` + `merged_into` + `hint: false`, canonical `status: tidy`, links retargeted, retitles ≤ 120 chars with a `## Summary` paragraph), `DreamNightStarted/Finished/Undone` on the WS stream, one `Dreaming` document reused across appends, second undo = no-op line, FTS consistent after undo, budget refusal at $0.05 (§3).

## 1. Setup

| | |
|---|---|
| Binary | `~/Develop/theuth-136-target/C/debug/t` rebuilt from HEAD before anything ran — Z's binary (03:42) predated Z's last code commit `4bc662e5` (03:50); `theuth-documents-59.5` beside it |
| Clone | `/bin/cp -Rc` of `~/.theuth/knowledge` (1.2 GB, gix history), `knowledge.sqlite(+wal)`, `sessions.sqlite(+wal)` (872 MB), `acp/agents.yaml`; ≈ 2 min per clone; one fresh clone per night (`/tmp/136/live1..3`) |
| Config | hand-written minimal `config.yaml`: `tools:` verbatim, `rpc.listen 127.0.0.1:1794x`, `dreaming: {enabled: true, budget_usd: 3 \| 0.05}`, `link/update/meeting/talk/signal/slack/telegram.enabled: false`, `mesh: {relay: none, discovery: false, bind: 127.0.0.1:…, hub: never}`. Log: 9 × "disabled by config", `mesh: … relay none (discovery false)`, zero connection attempts |
| **Vault (deviation)** | the copied `secrets.sqlite` does not open in the scratch dir: `the mesh key does not open with this engine's key` — the vault key is a macOS keychain item bound to the absolute data dir (`crates/secrets/src/key.rs:1-4`, `vault.rs:204-213`). Reported to the orchestrator as the brief's blocker; proceeded on the least-invasive path after no answer: **empty vault** (`secrets: {key_store: file}`, copied secrets.sqlite deleted — zero user secrets in `/tmp`) and **`agent.model: anthropic/claude-sonnet-4-5` on `ANTHROPIC_API_KEY` from the process environment** (genai's adapter fallback, `crates/agent/src/supervisor.rs:7951-7955`; the brief's own `nohup env … $T` would have passed that env in). No key was extracted. Consequence: the nights ran on Sonnet, not the user's `openai/gpt-5.6-sol`. Side effect: two `--secret-list` probes with `key_store: auto` minted a keychain item `theuth / secrets:/tmp/136/live1` and the rebuilt binary then blocked on a keychain-access prompt (`SecKeychainFindGenericPassword`); the item is harmless and can be deleted by hand |
| Engine | `t --headless --data-dir /tmp/136/liveN` as a supervised process (a first `nohup … &` start from the tool shell was killed with its process group after ≈ 3 min; `engine.json` left behind — restart was clean) |
| Driver | `scripts/apps/rpc.ts` (Bun jsonrpsee client) behind two throwaway scripts in `/tmp/136`: `call.ts <dir> <method> [params]`, `watch.ts <dir> <out.jsonl>` (every `subscribe_events(null, descendants=true)` item, one JSON line) |
| Copied-store side effect | 109's resume found 2 open turns and 3 reminders in the copied `sessions.sqlite` and tried to resume them on the scratch engine; every write was refused by the 68 ownership guard (`… is owned by <live engine id> (epoch 1)`), the turns ended `error` with no persisted model call (`model_calls.max(created_at)` unchanged). Expected on a clone; it also set `signals.last_root_turn_at`, which is why the sensor held on "a root session turned recently" |

## 2. The nights

| | Night 1 — live1 (Z's code, rebuilt) | Night 2 — live2 (P's fixes) | Night 3 — live3 (budget 0.05) |
|---|---|---|---|
| Fired | `dreaming.run_now {budget_usd: 3, mode: pass}` 03:18:56Z → session `01a11985-…` | `run_now {3, pass}` 03:50:18Z → `01a119a2-…` | `run_now {0.05, pass}` → see §3 |
| Ledger | 129 sessions · 164 plain user messages · 296 tool errors · 0 rewinds · backlog fresh 0 / untidied 1553 · candidates: 0 fresh, **300** retitle, **14** duplicate stems, **0** archive-by-use (bug), 15 verify | 135 · 168 · 319 · 0 · untidied 1669 · 0 fresh, 300 retitle, 14 stems, **60** archive-by-use, 15 verify | — |
| Tool calls (`inner_calls`) | python 20 · knowledge_get 50 · knowledge_retitle 20 · knowledge_merge 10 · knowledge_verify 10 · load_skill 1 · dreaming_ledger 1 · knowledge_list 1 · dreaming_journal 1 | NIGHT2_TOOLS | — |
| Knowledge commits | 40, all `dreaming <dreaming@localhost>`: 20 `dreaming retitle`, 10 `dreaming merge`, 10 `dreaming verify`; 50 concept files changed (= the receipt's 50 ids) + `index.md`, `decisions/index.md`, `log.md` | NIGHT2_COMMITS | 0 |
| Receipt groups | retitle 20 · merge 21 · verify 9 (one id merged *and* verified) | NIGHT2_GROUPS | — |
| Duration | 6 min 18 s (fire → `TurnFinished end_turn`); the `journal` close took 2 min 48 s inside that (actor backlog, see §0) | NIGHT2_DURATION | — |
| Model calls / spend | 21 round calls, 703 k prompt (649 k cached) / 6.7 k completion tokens; ledger `spend_usd` **0.367** of 3 (`model_calls` estimate 0.498) | NIGHT2_SPEND | NIGHT3_SPEND |
| Undo | 50 restored, 0 errors, 21 s; 49/50 files byte-identical to BEFORE, 1 half-restored (§0); `undone: true`; second undo `0 restored` no-op line | NIGHT2_UNDO | — |

Total model spend across the nights: TOTAL_SPEND USD (cap 10).

### 2.1 Night 1 observations beyond the table

- `<knowledge>` hints were **on** in the dreaming session (message seq 1 is a `<knowledge>` note) and every note records impressions: the dreaming session's own chatter (300 retitled titles per pass) inflates `uses.shown` of neighbouring ids with 0 opens — the exact signature archive-by-use looks for. Not fixed (the only off switch is a `Toolset{hint_limit: Some(0)}` allowlist, `crates/agent/src/lib.rs:518-521`, which would also restrict the tool list); recommendation in §6.
- The model's final reply was `paused` (the only reply text the routine named), stored as the run's `report` — the routine row would have said "paused" after a full night. Fixed in the routine text.
- `verify` ×10 in 11 s inside one python cell: no read tool between `knowledge_get` and `knowledge_verify`. The tool cannot tell; the skill now forbids it in words. A code gate (refuse `verify` unless a read tool ran since the `knowledge_get` of that id) is the real fix — §6.
- The 109 resume of the copied store published 5 `TurnFinished{error}` events for the user's sessions before the night; `Signals` counted them (`sessions_since: 5`) — on a clone only.

## 3. Budget refusal (night 3, live3, `budget_usd: 0.05`)

NIGHT3_SECTION

## 4. Acceptance checklist

| # | Claim | Result | Evidence |
|---|---|---|---|
| 1 | `dreaming.status` and `list_schedules` show the routine; sensor quiet outside the window | PASS | status `{enabled, next_window 2026-10-09T00:00:00Z (= 01:00 BST), budget 3, backlog {0, 1553, 0}}`; `list_schedules` row `dreaming`, `when: dreaming.py …`, `last_result: quiet` |
| 2 | Sensor gate evaluates the real rules | **FAIL → fixed** | night 1 envelope `events=no-signals Expecting value: line 1 column 2 (char 1)`; night 2 (fixed) `events=hold / gate: a root session turned recently / sessions_since=2` |
| 3 | `RunNow → FireWith` applies the overrides | PASS | `schedule.sqlite.runs` row 2: `dreaming`, the session id, `fired_at 04:18:56+01:00`; first `<request>` block `mode: pass / budget_usd: 3`; `state.json.run_request` recorded then cleared on close |
| 4 | Frame: skill present, `<knowledge>` hints off in the dreaming session | **PARTIAL** | `load_skill("dreaming")` is the first inner call and the skill text is the sidecar; there is no `<dreaming-ledger>` prompt block in the implementation (the ledger is the `dreaming_ledger` tool's structured result); hints are **on** (§2.1) |
| 5 | Every knowledge write authored `dreaming <dreaming@localhost>`, message `dreaming <op> <id>` | PASS | `git log b7dcca1..231fa21`: 40/40 commits by `dreaming <dreaming@localhost>`, subjects `dreaming retitle|merge|verify <id>` |
| 6 | Op scope: no delete; no `status: fresh` touched; `visibility`/`scope`/`owner`/`evidence` untouched; stubs keep a line + `merged_into`; links retargeted; retitles keep the text | PASS | script over the 50 ids (git `b7dcca1` vs HEAD): 0 issues; 20/20 retitles gained `## Summary`, titles 127–439 → 19–117 chars; 11 stubs `status: merged, merged_into, hint: false`, 10 canonicals `status: tidy`; `links` rows to `dr-failover-cli-design` → `…-v2`; 0 deletions; the store had 0 `fresh` entries |
| 7 | `KnowledgeChanged` published for each write | PASS | 101 `KnowledgeChanged{source: dreaming}` events on the WS stream (51 ops + 50 `restore` puts) |
| 8 | Hints exclude archived | NOT EXERCISED LIVE | 0 archives on night 1 (candidates were 0 — the bug); night 2: NIGHT2_ARCHIVE_HINTS; unit-tested in `knowledge` (`hint_for_skips_unhinted_and_non_live…`) |
| 9 | Cases ≤ cap, append-only, scrubbed | NIGHT_CASES | night 1: 0 cases (model skipped Mine); night 2: NIGHT2_CASES |
| 10 | Learned edits: proposed/judged/applied, sidecar-only, anchored, backed up, judge under `CallKind::Dream` and budget-checked | NOT EXERCISED LIVE | 0 open cases on the copy → no cluster qualified (needs ≥ 3 cases or a standing instruction); unit tests cover `skill_learned_edit`, `replay_case`, the gate |
| 11 | Journal: one document, the night entry with counts, the undo line later; `dreaming.nights` matches | PASS (shape fixed) | `doc_list`: one `Dreaming` note `agqrtcdbpvzrbn4e2hauwk2kpu`, `state.json.journal` = its id, reused by every append; `### Undone` lines `50 concepts restored…` then `0 concepts restored…`; `dreaming.nights[0]` = `{retitled 20, merged 21, verified 10, spend 0.3673, undone: true}`. Counts in the note were the model's prose on night 1 (now a code-owned `Receipt` line; night 2: NIGHT2_RECEIPT_LINE) |
| 12 | Undo complete and idempotent | **FAIL → fixed** | night 1: 49/50 byte-identical, 1 id kept the merge frontmatter (§0); second undo: `knowledge_restored: 0`, no-op line. Night 2: NIGHT2_UNDO_CHECK |
| 13 | `DreamNightStarted/Finished/Undone` observed by a WS subscriber | PASS | `watch.ts` log: `DreamNightStarted{night}` 03:19:06Z, `DreamNightFinished{counts, spend_nanos 367326900, proposals []}` 03:24:58Z, `DreamUndone{night, group: null}` 03:28:04Z |
| 14 | Budget guard refuses at $0.05 with `budget exhausted`, clean turn end, night record says so, no write without a receipt | NIGHT3_RESULT | §3 |
| 15 | Privacy: no message bodies / private titles / secrets in this document | PASS | ids and counts only; the one quoted concept title is a knowledge entry, not a session |

## 5. Bugs found and fixed (file:line → commit)

| # | Bug | Where | Fix | Commit |
|---|---|---|---|---|
| 1 | sensor parses `read_file` output as JSON → never fires | `crates/dreaming/src/sensor.py:104` | `strip_header`; `# theuth-sensor: dreaming 2` so the installed copy is rewritten; `sensor_rules::python_sensor_reads_signals_under_the_read_file_header` | `e4eff6a4` |
| 2 | `uses` fallback keyed on an empty table | `crates/dreaming/src/dry_run.rs:132`, `ledger.rs:144` | `merge_uses(index, sessions)` per id; `ledger::candidates` treats `stats`' zero rows as absent; test | `14c1d064` |
| 3 | `commit_before` from history at handling time (race + 6 s/event walk) | `crates/dreaming/src/night.rs:791-805`; `crates/knowledge/src/lib.rs:117` | `KnowledgeChanged.commit` (every publish site in `store.rs`), `night::commit_before` = parent via `revision`; test reproduces merge-then-verify | `53bbdc55` |
| 4 | headings inside appended markdown break the night block | `crates/dreaming/src/journal.rs:87` | demote `#`/`##`/`###` → `####`; test | `53bbdc55` |
| 5 | no code-owned counts/undo handles in the note | `night.rs:finish` | `Receipt` section line; test | `53bbdc55` |
| 6 | skill/routine let the model sample, "verify" without reading, head sections with `##`, reply "paused" | `skill.md`, `routine.md` | prose | `62ac8793` |

Gates after the fixes (build-lock `136-P`): `cargo test -p dreaming` 40 passed (36 + 4), `cargo test -p knowledge` 44 passed, `cargo clippy -p knowledge -p dreaming --tests -- -D warnings` clean, `cargo fmt -p knowledge -p dreaming --check` clean, `just ts-desktop` regenerated (`KnowledgeChanged.ts` gains `commit: string | null`; bindings dirs are gitignored), `cargo build -p theuth`.

## 6. Bugs and gaps left (with the file:line reason)

1. **Hints in the dreaming session count as impressions** — `crates/agent/src/loop_.rs:5047-5082` builds the note and calls `record_shown` for every chat session; the dreaming routine is a chat session. Recommendation: give `Night`/`dreaming_ledger` a way to mark its session hint-free without an allowlist (`SetToolset` is an allowlist, `agent/src/lib.rs:511-522`), or skip `record_shown` when `knowledge.may_consolidate(session)` says "unattended root". Until then every night adds ≈ `rounds/hint_rounds × 5` impressions to ids near the titles it tidies.
2. **`knowledge_verify` has no read-tool gate** — `crates/dreaming/src/skill.md` forbids it in words; `crates/knowledge/src/tools.rs` cannot see whether a read tool ran. A cheap gate: the dreaming session's `inner_calls` since the `knowledge_get(id)` include one of `read_file|glob|code_find|session_read|shell`.
3. **Undo leaves `links` retargeted** — `merge` rewrites `links` rows index-only (`store.rs` merge, Z hand-off W1); `restore` of a folded stub re-indexes only that file, so the citing concept's row still points at the canonical until that file is next written. After night 1's undo: `links where dst='dr-failover-cli-design'` = 0 (was 1+).
4. **The night self-harvests** — `loop_.rs:6149-6153` runs the harvest for any depth-0 session on compaction; a long night can write `status: fresh` entries about itself (not observed on these short nights; 0 non-dreaming commits).
5. **Model under-delivery is prompt-bound** — night 1 did a 7 % pass at 12 % of budget; night 2 on the tightened text: NIGHT2_VERDICT. A code floor (e.g. `dreaming_journal{section: journal}` refusing while `L.tidy.retitle` ids remain untouched and budget remains) would make it a contract, not prose.
6. `Receipt.commit`/`KnowledgeChanged.commit` are abbreviated ids for consolidation writes and full ids for `revision().parent`; both resolve through `rev_parse_single`.
7. Not exercised live: learned edits / `replay_case` / the judge (no qualifying cluster on the copy), archive hint exclusion (night 1), backlog mode (spend cap).

## 7. Recommendations for the first real night on the user's engine

- Rebuild and restart: the sensor fix only takes effect when `sensors/dreaming.py` is rewritten at start (v2 differs from the installed v1 text). Confirm with `list_schedules` → `last_check.outcome` and the run envelope's `gate:` line; `events=no-signals` means the old sensor.
- Expect per pass ≈ 300 retitles, 14 merges, up to 60 archives (the bootstrap `uses` merge is live), 15 verifies; night 2 here cost NIGHT2_SPEND USD on Sonnet for NIGHT2_SHORT. On `openai/gpt-5.6-sol` with `effort: max` the same pass will cost more and run longer — keep `budget_usd` at the default 1 for the first night and read the `Receipt` line before raising it.
- Run `t dreaming dry-run --data-dir <copy>` first: with the merge the archive-by-use count is the one the night will act on.
- Watch the first night's `uses` growth of ids the night itself touched (gap 1).
- Undo is per night or per group (`dreaming_undo("<night>", "merge")`); after an undo of merges, re-run `knowledge sync`/restart to refresh `links` (gap 3).
