# 136 — As-built W3 (DREAMING)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-w3`, branch `feat/136-w3`, cut from C's `f99f6d54`. Owns `crates/dreaming/**` and this file. Read with `136-contract.md` §2.4–2.7/§3.3 and `136-as-built-C.md` (C's TS contract is unchanged: no contract type, message or tool signature was altered).

## 0. One screen

- Every `// 136 W3` stub of C's skeleton is behaviour now: `Signals` (desks, power, rewind snapshots), `Night` (every ask), the nine tools, `ledger` / `tidy` / `cases` / `edits` / `replay` / `preferences` / `journal` / `undo`, `dry_run`, the `dreaming` skill prose with both prompts, the routine prompt.
- New modules: `scrub.rs` (the vault redactor + token shapes + the design-37 placeholder, run on every case / ledger / journal / preference / proposal write), `tidy.rs` (the rule-only Tidy candidates shared by `dreaming_ledger`, `dreaming.status.backlog` and `dry_run`), `dry_run.rs`.
- A night is **opened by `dreaming_ledger`** (`BeginNight`) and **closed by `dreaming_journal{section: "journal"}`** (`Commit{phase: "journal"}`). While open, the `Night` actor records every `KnowledgeChanged{source: dreaming}` as `(id, commit_before = history(id)[1], group = op)` in `ledger/<night>.night.json` — the undo receipt — and counts the ops. Undo groups are the knowledge ops (`merge | supersede | archive | verify | retitle | put`), `skill:<name>` and `preferences`.
- `skill_learned_edit` has two passes behind one signature: every edit `gate: null` → candidate text in memory, **no write** (what `replay_case` tests); every edit with a gate verdict → accepted ones written (backup first), all logged to `edits.jsonl`, accepted edits close their cases, rejected ones enter `state.rejected` with the gate numbers.
- Gates green: `cargo test -p dreaming` 36/36 (incl. the 30-concept / 2-session dry-run fixture with a byte-level "nothing changed" check and a `python3` lockstep run of `sensor.py` over the Rust rule table), `cargo clippy -p dreaming --tests -- -D warnings`, `cargo fmt -p dreaming --check`.

## 1. File by file (`crates/dreaming/`)

| File | What |
|---|---|
| `Cargo.toml` | + `roster` (desks), `genai` (`ChatRole::Assistant` on the agent store's rows), `regex`, `sha2` (split hash), `uuid` (v7 ids; no ulid crate in the workspace), `sqlx` (read-only `dry_run`) |
| `src/lib.rs` | `pub mod dry_run, scrub, tidy`; `dry_run(data_dir)` → `dry_run::dry_run`; tests: routine parses, install semantics, **skill carries `replay::JUDGE_PROMPT` verbatim and stays < 24 KB**, **dry-run fixture** (30 concepts: 10 fresh, 8 long tidy, a 3-file dated series, 2 expired records, a 61-shown/0-opened fact, a warning, an archived long one, 4 verified procedures; a 2-session store with an excluded advisor) asserting `Report {fresh: 10, retitle: 8, duplicate_stems: 1, archive_by_use: 3, verification_sample: 15, sessions: 2, friction: 0}` and that every file's bytes are unchanged |
| `src/scrub.rs` | `scrub(text)`: `tools::secrets::redact` then `{{secret:NAME}}` → `[secret]`, `sk-…`/`gh?_…`/`xox?-…`/`AKIA…`/bearer tokens/PEM blocks → `[redacted]`; `scrub_value(&mut Value)`; `clip(text, n)` |
| `src/tidy.rs` | `stem(id)` (strips `-YYYY-MM-DD`, `-YYYYMMDD`, `-vN`); `candidates(metas, uses, cfg, now) -> Candidates {fresh, retitle (> 120 chars, live), duplicate_stems (live groups ≥ 2), archive_by_use (record past `lifetimes.record`; ≥ 50 shown ∧ 0 opened ∧ ≥ 14 d old ∧ type ≠ warning), verify_sample (no/old `verified` on fact/procedure/entity/hosts/project/tool/architecture; checkable claims first, then most shown; ≤ `caps.verifications`)}`; `Candidates::untidied()` = `backlog.untidied` |
| `src/cases.rs` | `cases.jsonl` as an append-only log folded by id (`all`), `split_for` = `sha256("136" + id) % 100 < 34 → val`, synthetic never val; `append` fills id/night/at, scrubs, clips `context` to 1,200; `list`, `get`, `transition` (closed → `closed_by`, reopened → `reopened_at`, settled), `settle_old` (180 d without a reopen) |
| `src/edits.rs` | sidecar `skills/learned/<skill>.md`, bullets `- <content> [helpful: n; harmful: m; evidence: …; since: date]`; `parse`/`render`/`apply_one` (add/replace/delete with exact-content anchors; 12 × 140 caps; duplicates refused); `apply` (candidate pass vs record pass, backup `backups/<night>/<skill>.md` once per night — an absent sidecar backs up as an empty file so undo removes it —, `edits.jsonl` log folded by id, case closing, `state.rejected` ≤ 200) |
| `src/replay.rs` | `JUDGE_PROMPT` (verbatim below; the skill test pins it), `target_system` (the learned text as the live frame appends it), `judge_user` (ideal + user words + answer, nothing else), `parse_judge` (JSON inside prose, clamped), `replay(data_dir, cfg, session, case_id, candidate_text?, target_model?)` → two `agent::SideCall`s (`CallKind::Dream`, `session` = the dreaming session as cost owner; target `ModelRole::Named` or `Session`, judge `cfg.models.judge` or `Session`), `gate(baseline, candidate, cfg) -> GateResult` (§7.2 step 4, tested) |
| `src/preferences.rs` | `preferences.md` lines `- <text> [evidence: …; since: …; confirmed: …]`; `block()` renders texts only; `set(data_dir, knowledge?, op, line, evidence, cap)`: `add` (re-add = confirm + evidence union; full block refuses and names the oldest-confirmed line), `replace` (`"<old> => <new>"`), `demote` (removed; written as knowledge `decisions/preference-<slug>` when the store is up); `republish` re-registers / unregisters the prompt block |
| `src/journal.rs` | text of record `dreaming/journal.md` (`# Dreaming` / `## <night> · <mode>` / `### <section>`, newest night first, sections append); `append` scrubs and mirrors the whole text into the `Dreaming` document (`CreateNote` once → `state.journal`, then `ImportMarkdown{Replace}`; best effort) |
| `src/ledger.rs` | `DayLedger` (+ `night`, `mode`, `tidy: Candidates`, `backlog`, `sessions`); `build(data_dir, cfg, knowledge?, session?, since?)`: `BeginNight` → `LedgerSince{since: state.last_dream_at ∨ 24 h, exclude: session + descendants, owner_only}` → knowledge `stats` for the hint ids → pending rewinds → desk journals copied to `ledger/<night>/journals/` → candidates capped by `caps` → scrub → shrink to ≤ 2 MB (`truncated`) → `ledger/<night>.json` → pending files removed |
| `src/night.rs` | `NightRecord` (`ledger/<night>.night.json`: summary, session, `Touched{id, commit_before, group, undone}`, backups, phases, proposals, sidecars_undone), `persist` (whole `dreaming:` block of config.yaml, double's pattern), `update_routine_file` (budget / when / run_timeout lines follow a settings change, `enabled:` untouched), `add_proposals` (≤ `rows_per_week` per 7 days over waiting + acted rows), `file_counts`; asks: `BeginNight` (idempotent; refuses while `paused_until`; a finished record of the same night reopens for a heavy-day second window), `Commit` (`journal` → finish: counts, spend via `LedgerSince` over the night for the session + children, `last_dream_at`, `nights += 1`, `run_request` cleared, `settle_old`, `Dreamt` to Signals, `DreamNightFinished`), `KnowledgeChanged` (receipt + counts), `Undo`, `Export` (`laya` \| `fixture` JSON under `dreaming/export/`), `RunNow` (`state.run_request`, `FireWith{budget_nanos, run_timeout_secs: req ∨ 4 h in backlog, payload "mode: …\nbudget_usd: …"}`), `Status` (live backlog), `Nights`, `Settings`, `SetSettings` (validates, persists config + routine file + `paused_until`, applies live), `Proposals`, `Propose`, `SetMetaNote`, `ProposalAct` (routine → `schedules/<name>.md` from the ```markdown fence with `enabled: <start>`; remember → knowledge `decision`; double_class → journal line naming the 126 path; "never …" → `state.rejected`), `MetricsOver` (§13 series) |
| `src/undo.rs` | `undo(data_dir, knowledge?, rt?, night, group?)`: restore each touched id to `commit_before` (created-that-night ids → an error line), restore sidecar backups (`edits`/`skills`/`skill:<name>`), mark applied edits `undone_at` + reopen their cases, restore `backups/<night>/preferences.md`, `summary.undone` when everything is undone, "Undone" journal line, `DreamUndone` |
| `src/signals.rs` | + `desks_doing` (roster `ListAgents`, `status == Working`, 2 s timeout, on every `TurnFinished`), `probe_power`/`parse_pmset` (`pmset -g batt`, ≤ once a minute, macOS only), the rewind cache (last assistant text per attended root at `TurnFinished`) → `ledger/pending/<id>.json` on `ContextChanged{reason: "rewind"}` (≤ 2 KB, scrubbed) |
| `src/tools.rs` | real bodies; `CaseDraft` (serde defaults; an existing `id` = transition); `dreaming_journal` section protocol (`proposals`, `meta-note`, `journal`); `dreaming_undo`/`dreaming_state` go through the actor when it runs |
| `src/dry_run.rs` | `config_of`, `read_metas` (front matter parsed directly), `read_uses` / `read_sessions` (read-only sqlx), `dry_run`, `render` |
| `src/sensor_rules.rs` | the table is `rows()`; `rule_table` (Rust) and `python_sensor_answers_the_same_table` (runs `sensor.py`'s `gate` over the same rows through `python3`, skipped when absent) |
| `src/sensor.py` | unchanged from C (in lockstep, proven by the test) |
| `src/routine.md` | C's front matter; the prompt names the order, the paused stop, the budget stop and the backlog loop |
| `src/skill.md` | 19.7 KB: budget order + backlog loop, one section per phase with exact calls, caps, stop rules, `decide_many` recipes, both prompts, the tool contract |
| `src/rpc.rs` | unchanged (asks the real handlers) |
| `src/types.rs` | `Default` on `CaseKind` (correction), `CaseOrigin` (real), `CaseState` (open), `ProposalKind` (remember) for the tool drafts — serde shapes unchanged |

## 2. The prompts (verbatim from `skill.md`)

### Proposer

You are the proposer of the dreaming routine. You see the TARGET's current text, the TRAIN CASES of one cluster (intent, context, what happened, the user's words), the REJECTED buffer for this target (edits a replay gate refused before, with the gate's numbers) and the META-NOTE (which kinds of edits have helped or hurt). Propose at most 4 bounded edits to the target's learned block only — never to the skill file, the prompt or any human-written text — as a JSON array of EditRecord objects: {"op": "add" | "replace" | "delete", "content": "<≤ 140 chars>", "anchor": "<exact text of the existing bullet to replace, delete or add after; null to append>", "rationale": "<one line>", "cases": ["<case ids this edit should close>"]}. Each edit MUST be a short, GENERAL, reusable rule or preference: never task-specific, never an answer to a single case, never a name, path, date or number copied from one case. Fix user-visible behaviour, do not imitate an evaluator: NEVER quote, paraphrase or reconstruct evaluator or judge text, rubric or scoring language. NEVER write an instruction to run, execute, call, install or fetch anything — a learned bullet describes how to behave, not what to do next. Do not re-propose an edit in the REJECTED buffer or one that differs from it only in wording. If no general rule follows from the cases, answer an empty array. Answer with the JSON array only.

### Judge (`replay::JUDGE_PROMPT`)

You are the judge of one replayed case. You see the IDEAL (what right looked like), the USER_WORDS (what the user said when it went wrong) and the ANSWER an assistant gave to the same situation. Score how close the ANSWER comes to the IDEAL in the spirit of the USER_WORDS: 1.0 = the user would have had nothing to correct; 0.5 = half right or right for the wrong reason; 0.0 = the same mistake again or worse. Judge substance, not wording; an answer that merely promises to behave is not a behaving answer. Reply with exactly one JSON object and nothing else: {"score": <number 0-1>, "reason": "<one line, under 140 chars>"}

## 3. The sensor table (`sensor_rules::tests::rows`, mirrored by `sensor.py`)

Default config (window 01:00–06:00, idle 20, heavy 300 concepts / 40 sessions), self engine `me`, a snapshot of `last_root_turn_at` 01:00Z, 12 concepts, 3 sessions, `last_dream_at` yesterday 03:00Z, on AC:

| Row | now (UTC) | tz | change | answer |
|---|---|---|---|---|
| fires at 02:00 local, idle 60 min | 02:00 | 0 | — | fire |
| another engine | 02:00 | 0 | `engine: other` | `another engine owns dreaming` |
| outside: 00:30 local | 00:30 | 0 | — | `outside the window` |
| outside: 06:00 local (end exclusive) | 06:00 | 0 | — | `outside the window` |
| tz: 02:00 UTC is 22:00 local at −240 | 02:00 | −240 | — | `outside the window` |
| tz: 23:00 UTC is 02:00 local at +180 | 23:00 | +180 | — | fire |
| idle: turned 10 min ago | 01:10 | 0 | — | `a root session turned recently` |
| idle: no turn ever recorded | 01:10 | 0 | `last_root_turn_at: null` | fire |
| desk doing | 02:00 | 0 | `desks_doing: 1` | `a desk is doing` |
| already dreamt at 01:30 this window | 03:00 | 0 | `last_dream_at: 01:30Z` | `already dreamt this window` |
| heavy day allows a second window | 03:00 | 0 | + `new_concepts_since: 300` | fire |
| nothing new | 02:00 | 0 | 0 concepts, 0 sessions | `nothing new since the last dream` |
| one session is enough | 02:00 | 0 | 0 concepts, 1 session | fire |
| battery 39 off mains | 02:00 | 0 | `on_ac: false, battery_pct: 39` | `on battery below 40%` |
| battery 40 off mains | 02:00 | 0 | `on_ac: false, battery_pct: 40` | fire |
| off mains, battery unknown | 02:00 | 0 | `on_ac: false, battery_pct: null` | `on battery below 40%` |
| power unknown (not macOS) fires | 02:00 | 0 | `on_ac: null, battery_pct: null` | fire |

Plus `window_crossing_midnight` (23:00–05:00: a 23:30 dream counts at 00:30; the previous night's does not). No row was added: `paused_until` is enforced by `BeginNight` (below), not the sensor.

## 4. Deviations from the contract / design (and why)

1. **Ids are uuid v7 hex (32 chars), not ulids** — no ulid crate in the workspace (C's hand-off offered `uuid::now_v7`). Shapes unchanged (`id: String`).
2. **`BeginNight` gained `session: Option<SessionId>`** (the dreaming session: spend owner, ledger exclusion). An actor message of my crate, serde default, not in the TS contract.
3. **Night records live in `ledger/<night>.night.json`**, the day ledger in `ledger/<night>.json` (contract §2.5 names only the latter). `dreaming.nights` reads the records. `NightReceipt` is derived from the record (`receipt()`), so the contract type is unchanged.
4. **Undo groups are knowledge ops**, not phases: `merge | supersede | archive | verify | retitle | put | skill:<name> | preferences`. The design's "each changed group with its undo handle" reads naturally this way and needs no phase bookkeeping; the receipt gets its `(id, commit_before)` from `KnowledgeChanged{source: dreaming}` + `history(id, 2)` at event time (C's hand-off suggested `HistoryOf` at `BeginNight`, when the touched ids are not yet known).
5. **Three `dreaming_journal` sections are protocols** (`proposals` → the inbox rows, `meta-note` → `state.meta_note`, `journal` → close the night) because the contract's nine tools have no `proposal_put` / meta-note / finish tool and the model must reach them. Z may split them into tools if the dotted sections prove brittle; the contract table stays at nine.
6. **`skill_learned_edit` candidate pass vs record pass** (§0): the contract args have no `dry_run` flag; `gate: null` on every edit is the candidate pass. Documented in the skill and the tool description.
7. **`preferences_set{op: replace}`** takes `line = "<old> => <new>"` (one `line` arg in the contract).
8. **`case_put` takes a `CaseDraft`**, not a full `Case`: serde defaults for `id`, `night`, `at`, `split`, `origin`, `state`, `closed_by`, `source_session` (the contract's `Case` has no defaults, so the model could not call it). The stored line is a full contract `Case`.
9. **`replay_case`'s judge default** is `ModelRole::Session` when `dreaming.models.judge` is unset (the design's "agent.model when its vendor differs from propose" needs the agent section, which the plugin does not read). The target default is also the session's model.
10. **Spend** comes from `LedgerSince` over the night's window with no exclusions (sum of `model_cost_nanos` of the dreaming session + children) — the only seam that reads `model_calls`; it is `0` until W2 fills the handler.
11. **`paused_until`** is enforced by `BeginNight` (`Err("paused until …")`; the skill and the routine prompt stop with no writes), not by the sensor: the sensor reads only `signals.json` and the clock (brief 2) and `SignalsSnapshot` has no pause field. Cost: one cheap model turn per fire while paused.
12. **`dreaming.set_settings.enabled`** is written to `config.yaml` only (no `schedule::SetEnabled`): the contract says the switch is `enable_schedule("dreaming", …)`; writing both from one call would fight the routine file's `enabled:`.
13. **The journal's text of record is `dreaming/journal.md`**; the `Dreaming` document mirrors it (`ImportMarkdown{Replace}` of the whole text) so a night without the documents plugin still journals and the model never needs to read the document body back.
14. **`dry_run` parses knowledge files itself** and opens the two SQLite files read-only (not `Knowledge::open`, whose `sync()` writes the index). SQLite may create a `-shm` beside a WAL database it reads; the fixture test's fingerprint allows exactly `-shm`/`-wal`.
15. **`Export` answers under `dreaming/export/cases.<format>.json`** with 57-shaped rows (`laya`: state/question/gold/label; `fixture`: batches with the user's words as the published turn); not reachable by tool or RPC (contract lists it as an actor ask only).

## 5. Hand-offs

- **Z — `t dreaming dry-run --data-dir <copy>`**: `dreaming::dry_run(&path).await?` then print `dreaming::dry_run::render(&report)`. Run it on the APFS copy with `knowledge/`, `knowledge.sqlite` and `sessions.sqlite` (copy `sessions.sqlite-wal` too, or checkpoint first). Expected per contract §4: ≈ 1,250 retitle candidates, ≈ 19 duplicate stems, several hundred archive-by-use, verification sample 15.
- **Z / decision owner — the correction detector**: `decide_many` refuses any family but `tool` / `tool.<task>` (`decision/src/tool.rs:384-396`), so the skill runs the double's `double.correction` **statement** as a `Bool` under the default tool family. To use the double's calibrated family from the night, either let `decide_many` accept `double.correction` or add a `tool.dreaming-correction` sub-family in config.
- **Z — the propose tier**: the proposer runs on the routine session's model (`model:` = `dreaming.models.tidy`); `dreaming.models.propose` is persisted and shown in Settings but not used by a call. The seam is one `agent::SideCall{role: Named(propose), system: <proposer prompt>, kind: Dream}` behind a tool — not added to keep the contract's nine.
- **W1 → Z**: the five knowledge ops are stubs on C's tree; `Night`'s receipt and counts need their `KnowledgeChanged{source: dreaming, op}` events and one gix commit per op (`history(id, 2)[1]` is the commit before). `knowledge.restore(id, commit)` of a merged stub is W1's.
- **W2 → Z**: `LedgerSince` is `Ledger::empty` on C's tree: `dreaming_ledger` returns an empty agent part (the tidy candidates, pending rewinds and desk journals are real) and the night's `spend_usd` is 0 until the handler lands. The budget guard at `Reserved::open` is what makes `replay_case`'s `SideCall`s count.
- **W2 — learned sidecar append**: `load_skill` must append `skills/learned/<skill>.md` under `## Learned (dreaming)`; the file holds bullets only (format in `edits.rs`).
- **W4**: `DreamStatus.backlog` is live from the actor (`fresh`, `untidied` = retitle + duplicate-stem members + archive-by-use, `open_cases`); `Proposal.body` of a `routine` proposal carries the draft in a ```markdown fence; `proposal_act` choices are matched by words (`start` / `paused`; `not` / `no` / `never` = decline).
- **Z — attention rows**: proposals are rows in `proposals.jsonl` + `DreamNightFinished.proposals`; the attention plugin's subscription (design §9.3) is not in this crate.
- **Schedule (optional)**: a permanent watcher still relies on `max_runs: 100000` (C's open item).
- **Z — `Cargo.lock`** (outside W3's list, left uncommitted): the only diff is the six new dependency edges of `dreaming` (`genai`, `regex`, `roster`, `sha2`, `sqlx`, `uuid`; no new external crate versions). `cargo check -p dreaming` on the merged tree regenerates it.

## 6. Not verified

The engine running with the plugin (no `t` against `~/.theuth`, no restart); `replay_case` against a live model (`SideCall` path compiled, not exercised: the agent actor is absent in tests, so the tool answers `no case x` / `agent actor is not running`); the journal mirror into a live documents actor; `RunNow` → `FireWith` against a live schedule actor; `ListAgents` from a live roster; `pmset` output on a machine on battery (parser tested on captured shapes); W1's ops and W2's ledger behind the seams.
