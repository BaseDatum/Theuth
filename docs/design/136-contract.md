# 136 — Contract: Dreaming build wave

Status: contract v1 · 2026-10-08 01:05 · binds the omp workers of design 136 (docs/design/136-dreaming.md v0.2, whose §14.1 rulings override its earlier text). Where this contract and the design disagree on a name or a signature, this contract wins; where the contract is silent, the design wins; where both are silent, the contract worker (C) decides and writes it in `136-as-built-C.md`, and every later worker reads that as-built before touching code.

## 0. Working rules (paste verbatim into every brief)

- The spec is `docs/design/136-dreaming.md` in YOUR worktree, including §14.1 which overrides v0.1 text, plus this contract and `docs/design/136-as-built-C.md` once it exists. Never change a contract type, message or tool signature; if one does not fit, implement against it and write the mismatch as a hand-off in your as-built.
- Shell setup for every cargo call: `export PATH="$(/opt/homebrew/bin/just toolchain):$PATH"`, `export CARGO_TARGET_DIR=<your target dir, given in the brief>`, `export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0`.
- Build coordination (one shared MacBook, several waves): crate-level gates only — `cargo check/test/clippy -p <crate> --no-fail-fast`; never `--workspace`, `just check/test`, `--release` or a full vitest from a worker. Wrap EVERY cargo / just / tsc / vitest / vite call: `BUILD_LABEL=136-<TAG> /Users/Sky.Moore/.theuth/bin/build-lock <cmd>`. A `[build-lock] waiting (Ns)` line every 60 s is normal, not an error; run gates in the foreground and wait. Rules: knowledge `procedures/theuth-shared-macbook-build-coordination-2026-10-07`.
- Never `cargo fmt --all`; `cargo fmt -p <crate>` only on crates you own. Never push, merge, rebase, add worktrees, touch `~/Develop/theuth` (the user's checkout) or any sibling worktree, run `t`/`theuth` against `~/.theuth`, or restart the engine. Your worktree and your target dir are the only places you write.
- Commit as you go: a checkpoint commit at least every 20 minutes, messages `136 <TAG>: …`. New commits only — never amend or squash a commit that exists when you start.
- Before you finish: your crate's `cargo test -p <crate>`, `cargo clippy -p <crate> -- -D warnings`, `cargo fmt -p <crate> --check` green; write `docs/design/136-as-built-<TAG>.md` (what you built, file by file; every deviation from the contract; hand-offs for the closer; what you did not verify); commit it; print exactly `DONE <TAG> <sha>` as the last line of your final message.
- If this brief is delivered to you a second time, reply exactly `DUPLICATE` and do nothing else.
- You own only the paths in your "You own" list. Everything else — especially the sibling worktrees named in your brief — you must not touch. A change you need outside your list is a hand-off line in your as-built, not an edit.

## 1. Waves, ownership, merge order

| TAG | Worktree / branch | Owns (may create and edit) | Must not touch | Done when |
|---|---|---|---|---|
| **C** contract | `~/Develop/theuth-136` · `feat/136-dreaming` (integration branch) | every seam in §2: `crates/knowledge` (types + store schema + command enum + tool stubs), `crates/agent` (`CallKind::Dream`, `SetBudget`, `LedgerSince` message + `Ledger` types + handler stub, `record_shown` call site), `crates/schedule` (`budget:` key, `FireWith`), **new crate `crates/dreaming`** (Cargo.toml, lib.rs with `DreamingPlugin`, config, types, actor stubs, tool stubs, rpc stubs, `skill.md`, `routine.md`, `sensor.py`), `crates/theuth/src/main.rs` (register the plugin, bundle the skill/routine/sensor), `crates/rpc` (method registrations + ts-rs exports for §2.7), workspace `Cargo.toml` | nothing else; no behaviour beyond stubs and tests of the seams | `cargo test -p knowledge -p agent -p schedule -p dreaming -p rpc` green + ONE `BUILD_SLOTS=1 build-lock cargo check --workspace --tests`; as-built with a **"TS contract (verbatim)"** section listing every exported type and RPC method; `DONE C <sha>` |
| **W1** KNOWLEDGE | `~/Develop/theuth-136-w1` · `feat/136-w1` | `crates/knowledge/**` only | everything else | §3.1 behaviour; `cargo test -p knowledge` incl. the new rank tests; `DONE W1 <sha>` |
| **W2** AGENT | `~/Develop/theuth-136-w2` · `feat/136-w2` | `crates/agent/**` only (harvest prompt + Put fields, hint impressions call, budget guard + `<budget>` note, `LedgerSince` handler, learned sidecar append in skills.rs, `skills/retired/`, index ordering) | `crates/knowledge`, `crates/dreaming`, `crates/schedule`, UI | §3.2; `cargo test -p agent --lib`; `DONE W2 <sha>` |
| **W3** DREAMING | `~/Develop/theuth-136-w3` · `feat/136-w3` | `crates/dreaming/**` (actors, state, ledger, cases, edits, undo, journal, tools, rpc impls, `skill.md`, `routine.md`, `sensor.py`), `docs/design/136-as-built-W3.md` | every other crate; `main.rs` (C already registered everything) | §3.3; `cargo test -p dreaming`; a `dry-run` over a fixture data dir writes nothing; `DONE W3 <sha>` |
| **W4** UI | `~/Develop/theuth-136-w4` · `feat/136-w4` | `crates/desktop/ui/**` only; hand-written ts-rs stubs for the §2.7 types under the gitignored bindings dir (never committed) | every crate | §3.4; `tsc`, `vitest` for the touched files, `vite build`; `DONE W4 <sha>` |
| **Z** closer | integration tree `~/Develop/theuth-136` | cross-seam hand-offs listed in the as-builts; the dreaming skill's prompts tuned against a snapshot; `just ts-desktop`; workspace gates (`BUILD_SLOTS=1`); the first `dry-run` on a copy of the live data dir; `docs/design/136-as-built-Z.md` | the user's checkout (the orchestrator lands) | gates green on the merged tree; `DONE Z <sha>` |

Merge order into `feat/136-dreaming`: C (already there) → W1 → W2 → W3 → W4 → Z. Workers are cut from C's `DONE` commit. The orchestrator lands `feat/136-dreaming` into the user's checkout with `merge --no-ff` and pushes origin + basedatum.

Section ownership of the design (each §4 phase and each §9 item belongs to exactly one wave): §5.1 keys, §5.2 types, §5.3 rank, §9.3 knowledge commands, §9.4 knowledge_* tools → W1. §4 Recall data (`LedgerSince`), §7.1 sidecar append, §9.3 `SetBudget` guard + `<budget>` note, §9.3 `HintsShown` (as a direct `record_shown` call), §15 07/04 amendments in code → W2. §4 phases 0–7 as the routine + skill + tools `dreaming_ledger`, `case_*`, `skill_learned_edit`, `replay_case`, `preferences_set`, `dreaming_journal`, `dreaming_undo`, §9.3 `Signals`/`Night` actors, §9.5 RPC impls, §9.6 files, §9.7 routine → W3. §8 screens → W4. §16 S6 first run → Z with the orchestrator.

## 2. The seams C lands (names are binding; field types may be adjusted by C only, and only in `136-as-built-C.md`)

### 2.1 `crates/knowledge` — types and schema
```rust
// types.rs — new on Meta (all optional in files; absent = the default named)
pub enum Status { Fresh, Tidy, Merged, Archived, Stale, Retired }   // serde lowercase; default Tidy when absent
pub struct Meta {
    /* existing: id, type_, title, description, tags, scope, source, timestamp, owner, visibility */
    pub status: Status,                 // `status:`
    pub hint: bool,                     // `hint:`; default true
    pub evidence: Vec<String>,          // `evidence:` session ids or "user"
    pub verified: Option<String>,       // `verified:` YYYY-MM-DD
    pub valid_until: Option<String>,    // `valid_until:` RFC 3339
    pub superseded_by: Option<String>,  // `superseded_by:` concept id
    pub merged_into: Option<String>,    // `merged_into:` concept id
}
// Put gains the same seven fields, all Option/Vec with serde(default); put_for writes every one that is set
// and PRESERVES any unknown frontmatter key of an existing file verbatim (round-trip test required).
// Source gains Dreaming (`source: dreaming`). The `type` field stays open; the harness-known list is
// fact | procedure | decision | project | entity | architecture | tool | record | warning.
pub struct Uses { pub id: String, pub shown: u64, pub opened: u64, pub last_shown: Option<String>, pub last_opened: Option<String> }
```
```sql
-- store.rs: knowledge.sqlite gains a DURABLE table (never dropped by sync(); the doc 10 rule "the index is deletable"
-- is amended: deleting the file loses usage counts, concepts rebuild from files)
CREATE TABLE IF NOT EXISTS uses (id TEXT PRIMARY KEY, shown INTEGER NOT NULL DEFAULT 0, opened INTEGER NOT NULL DEFAULT 0,
  last_shown TEXT, last_opened TEXT);
-- concepts (fts5) gains UNINDEXED columns: status, hint, verified, valid_until, superseded_by, merged_into
```
Store API (C adds signatures + unit tests on empty bodies where behaviour is W1's; W1 fills them):
`record_shown(&self, ids: &[String], at: DateTime)`, `record_opened(&self, id: &str, at)`, `stats(&self, ids: &[String]) -> Vec<Uses>`, `merge(&self, ids: &[String], into: &str, body: &str, evidence: &[String], scope: &AgentScope) -> Receipt`, `supersede(&self, loser, winner, valid_until, note) -> Receipt`, `archive(&self, id, reason) -> Receipt`, `verify(&self, id, outcome: VerifyOutcome, note) -> Receipt`, `retitle(&self, id, title, summary) -> Receipt`. `hint_for` and `ranked` take the §5.3 rank; `hint_for` filters `hint = false` and `status ∉ {Fresh, Tidy}`; `search` does not filter (archived stays searchable) but marks `[archived]`.

Commands (kameo `ask` on the knowledge actor, mirroring the existing `Put`): `Merge`, `Supersede`, `Archive`, `Verify`, `Retitle`, `Stats`. Events: `KnowledgeChanged` gains `op` values `merge | supersede | archive | verify | retitle`.

Tools (W1 fills; C registers with the Risk shown and a one-line description): `knowledge_stats{ids}` ReadOnly · `knowledge_merge{ids, into, body, evidence?}` Mutating · `knowledge_supersede{loser, winner, valid_until?, note?}` Mutating · `knowledge_archive{id, reason}` Mutating · `knowledge_verify{id, outcome: "ok"|"stale", note?}` Mutating · `knowledge_retitle{id, title, summary}` Mutating. All five mutating tools require `writable()` and additionally `cx.unattended || cx.is_owner_root` (C adds the predicate to `ToolCx` if absent: "the calling session is the dreaming routine's or the owner's own root").

### 2.2 `crates/agent` — budget, ledger, impressions, harvest, sidecars
```rust
// runtime side_call.rs: CallKind gains `Dream` (model calls made by dreaming's tools: replay, judge, propose).
// agent messages (supervisor `ask`):
pub struct SetBudget { pub session: SessionId, pub nanos: Option<u64>, pub currency: String } // None clears
pub struct LedgerSince { pub since: String /*RFC3339*/, pub until: String, pub exclude: Vec<SessionId>, pub owner_only: bool }
pub struct Ledger {   // crates/agent/src/dreaming_ledger.rs — one struct, serde, ts(export)
    pub since: String, pub until: String,
    pub sessions: Vec<LedgerSession>,            // id, title, kind (root|child|desk|routine), parent, created_at, turns, tool_calls, errors, model_cost_nanos
    pub user_messages: Vec<LedgerUserMessage>,   // session, seq, at, text (plain typed messages only: no leading `<` or `[`), ≤ 2 KB each, secrets scrubbed
    pub inner_calls: Vec<LedgerCount>,           // name, calls, errors, sessions
    pub tool_errors: Vec<LedgerToolError>,       // session, turn_id, name, attempt, at, first line of output (≤ 300 chars)
    pub questions: Vec<LedgerQuestion>,          // session, kind, state, answer ("allow"|"deny"|…), answered_by, asked_at
    pub stops: Vec<LedgerStop>,                  // session, turn_id, stop_reason, at
    pub app_presses: Vec<LedgerUserMessage>,     // user messages beginning `[app `
    pub hints: Vec<LedgerHintUse>,               // id, shown, opened (from knowledge stats for ids shown in the window)
    pub skill_loads: Vec<LedgerCount>,           // name = skill name
    pub rewinds: Vec<LedgerRewind>,              // session, at, dropped_assistant_text (≤ 2 KB; captured by dreaming's Signals at event time — agent only returns what it has in `resume_reports`/journal, may be empty)
}
```
Handler: `Agents::handle(LedgerSince)` runs read-only queries on the agent store (sessions, messages, inner_calls, tool_calls, turn_steps, questions, model_calls ⟕ cost_owners) and asks knowledge for `Stats`. Caps: 500 sessions, 5,000 user messages, 2,000 tool errors; anything past a cap sets `truncated: true` on the struct. Private sessions never appear.

Budget guard: `SessionState.budget: Option<Budget>`; `Reserved::open` (the per-call limiter seam) refuses when `spent_nanos(owner) + estimate > budget.nanos`, the turn ends, and the model sees one `<budget spent="…" cap="…">` note on the next input; `SetBudget` is also applied from the schedule plugin on every fire when the file has `budget:` (C wires the message; W2 implements the guard and the note).

Impressions: where `Turn::round` builds the `<knowledge>` note (loop_.rs ≈ 4926–4973), call `knowledge.record_shown(&ids, now)` after the note is appended (C adds the call against the stub; W1 makes it persist). Opens are recorded inside `knowledge_get` (W1).

Harvest (W2): `HARVEST_INSTRUCTION` — title ≤ 120 chars ("the first clause; the statement goes in the body"), `type` list gains `record` ("what happened on a date") and `warning` ("a one-line never/always whose title is the whole payload"); `parse_harvest` sets `Put.evidence = [session id]` and `Put.status = Some(Fresh)`; a title over 120 chars is split by the parser (first clause → title, rest → body "Summary" paragraph) rather than rejected.

Learned sidecars (W2, skills.rs): `load_skill(name)` → body + `\n\n## Learned (dreaming)\n` + contents of `<data_dir>/skills/learned/<name>.md` when the file exists and is non-empty; the same append for `always` inlining and for ACP materialisation (`spawn_subagent.skills`); `skills/retired/` is skipped by the registry walk; dreaming-made skills (front matter `origin: dreaming`) are listed after the others in `prompt_index`. Front matter gains optional `origin` (string), ignored otherwise.

### 2.3 `crates/schedule` (C)
Front matter `budget: <number> <CUR>` (e.g. `budget: 1 USD`) → on every fire `agent::SetBudget{session, nanos, currency}`; absent → `SetBudget{nanos: None}`. New `ask` `FireWith { name: String, overrides: FireOverrides { budget_nanos: Option<u64>, run_timeout_secs: Option<u64>, payload: Option<String> } }` → fires like `fire_schedule` with the overrides applied to that run only; `payload` is appended inside the `<schedule>` envelope as `<request>…</request>`.

### 2.4 `crates/dreaming` — the plugin (C: skeleton, types, stubs, config, registration; W3: behaviour)
```text
crates/dreaming/
  Cargo.toml            # deps: runtime, tools, knowledge, agent (types + ask), schedule (FireWith), document (journal note), serde, serde_json, tokio, kameo, chrono, ulid, anyhow, schemars, ts-rs (feature ts)
  src/lib.rs            # DreamingPlugin { } impl Plugin: name "dreaming"; start(): read DreamConfig; install routine/sensor/skill files when enabled; spawn Signals + Night actors; register tools; register RPC
  src/config.rs         # DreamConfig (serde defaults = design §9.2 with budget_usd 1.0; `enabled`, `window`, `idle_minutes`, `min_sessions`, `heavy_day`, `budget_usd`, `run_timeout`, `models{triage,tidy,propose,judge}`, `caps`, `lifetimes`, `gate`, `preferences{auto_from_explicit,cap_lines}`, `digest`, `home_engine`)
  src/types.rs          # Case, CaseKind, Split, CaseState, EditRecord, EditOp, ApplyReport, ApplyStatus, NightId, NightReceipt, NightCounts, Proposal, ProposalKind, SignalsSnapshot, RunRequest{budget_usd, mode: Pass|Backlog, run_timeout_secs}, DreamStatus, NightSummary, Metrics — all serde + schemars + ts(export)
  src/signals.rs        # Signals actor: subscribes TurnFinished, ContextChanged{reason: rewind}, roster task events; writes <data_dir>/dreaming/signals.json atomically; snapshots rewound tails to ledger/pending/<ulid>.json
  src/night.rs          # Night actor: BeginNight/Commit/Undo/Export/RunNow; owns state.json, ledger/, cases.jsonl, edits.jsonl, backups/, the journal document id
  src/ledger.rs         # dreaming_ledger: asks agent LedgerSince + knowledge Stats, folds pending rewinds and desk journals, writes ledger/<night>.json (W3)
  src/cases.rs          # cases.jsonl append/read, hash split (sha256(seed+id) % 100 < 34 → val), state transitions (W3)
  src/edits.rs          # learned sidecar apply with anchors + ApplyReport; backups/<night>/<skill>.md; edits.jsonl (W3)
  src/replay.rs         # replay_case: render case + candidate text, call target model (CallKind::Dream), call judge model, return {answer, score, reason} (W3)
  src/preferences.rs    # <data_dir>/dreaming/preferences.md (≤ cap_lines × 140 chars) + the ListPrompts block "## Standing preferences (learned)" (C: block plumbing returning empty; W3: content)
  src/journal.rs        # the `Dreaming` document: create once (document crate), prepend a night section (W3)
  src/undo.rs           # revert knowledge commits tagged dreaming/<night>[/<group>] via knowledge Restore per id, restore sidecar backups, mark edits undone (W3)
  src/tools.rs          # Tool impls (table below); C registers all with stub bodies returning anyhow::bail!("not built: W3")
  src/rpc.rs            # RPC method impls (table below); C registers stubs
  src/sensor.py         # the `when:` sensor (C: reads signals.json + clock per design §4 phase 0; a pure function + doc test in Rust mirrors the rule)
  src/routine.md        # the bundled schedule file (design §9.7; budget: 1 USD)
  src/skill.md          # the `dreaming` skill: per-phase recipes + proposer/judge prompts (C: headings and the contract of each tool; W3: the prose)
```
Registration (C, `crates/theuth/src/main.rs`): `Box::new(dreaming::DreamingPlugin)` after `agent::AgentPlugin`; bundled files written on start when `dreaming.enabled` (default true): `<data_dir>/skills/dreaming.md` (binary-owned like `double.md`), `<data_dir>/schedules/dreaming.md` (only if absent — the user's toggle state must survive restarts), `<data_dir>/sensors/dreaming.py` (binary-owned).

Tools registered by `dreaming` (name · Risk · args → returns; visible to the dreaming routine's session and the owner's root sessions; every mutating one refuses elsewhere):
`dreaming_ledger{since?}` ReadOnly → `Ledger` (+ pending rewinds, desk journals, `run_request`) · `case_put{case}` Mutating → id · `case_list{state?, skill_hint?, split?, limit?}` ReadOnly → `Vec<Case>` · `skill_learned_edit{skill, edits, night}` Mutating → `ApplyReport` · `replay_case{case_id, candidate_text?, target_model?}` ReadOnly → `{answer, score, reason}` (judge fixed by config) · `preferences_set{op, line, evidence?}` Mutating → the block text · `dreaming_journal{night, section, markdown}` Mutating · `dreaming_undo{night, group?}` Mutating → `UndoReport` · `dreaming_state{}` ReadOnly → `DreamStatus`.

Events published: `DreamNightStarted{night}`, `DreamNightFinished{night, counts, spend_nanos, proposals}`, `DreamUndone{night, group}`.

### 2.5 Files under `<data_dir>/dreaming/` (W3 writes; formats binding)
`state.json` `{last_night, nights: u64, last_dream_at, meta_note: String (≤ 1200 chars), rejected: Vec<RejectedEdit> (≤ 200), run_request: Option<RunRequest>, engine: String}` · `signals.json` `SignalsSnapshot {last_root_turn_at, desks_doing, new_concepts_since, sessions_since, last_dream_at, on_ac, battery_pct, engine}` · `ledger/<night>.json` · `ledger/pending/*.json` · `cases.jsonl` (one `Case` per line, append-only) · `edits.jsonl` (one `EditRecord` + outcome per line) · `backups/<night>/<skill>.md` · `preferences.md` · `proposals.jsonl`. The journal is a document in the documents store titled `Dreaming` (id kept in state.json).

### 2.6 Case and edit shapes (binding JSON; Rust mirrors)
```json
{"id":"01J…","night":"2026-10-08","kind":"correction|re-ask|disapproval|rewind|stop|denial|tool-chain|hint-miss|task-returned",
 "intent":"…","context":"≤1200 chars","happened":"…","user_words":"verbatim","ideal":"…","tags":["…"],
 "skill_hint":"<skill name> | prompt:standing-preferences | knowledge:<id>","source_session":"<uuid>","at":"RFC3339",
 "split":"train|val","origin":"real|synthetic","state":"open|closed|reopened|settled","closed_by":null,"reopened_at":null}
{"id":"01J…","night":"2026-10-08","target":"skill:<name>|preferences","op":"add|replace|delete","content":"≤140 chars","anchor":null,
 "rationale":"…","cases":["01J…"],"gate":{"baseline":0.42,"candidate":0.71,"min_delta":-0.05,"val_cases":6,"verdict":"accepted|rejected|unverified"},"applied_at":null,"undone_at":null}
```

### 2.7 RPC (crates/rpc registers; impls in `dreaming::rpc`) and TS contract
`dreaming.status() → DreamStatus {enabled, last_night: Option<NightSummary>, next_window, budget_usd, budget_left_tonight_usd, backlog: {fresh, untidied, open_cases}, proposals_pending, paused_until}` · `dreaming.nights(limit) → Vec<NightSummary {night, started_at, finished_at, counts: NightCounts {triaged, retitled, merged, superseded, archived, verified, cases, edits_accepted, edits_rejected, proposals}, spend_usd, mode, undone: bool}>` · `dreaming.run_now(RunRequest) → SessionId` · `dreaming.undo(night, group?) → UndoReport` · `dreaming.settings() / dreaming.set_settings(DreamSettings {enabled, window, budget_usd, models, preferences_auto, digest, paused_until})` (writes config.yaml's `dreaming:` section through the existing settings path and the live config) · `dreaming.proposals() → Vec<Proposal {id, kind: routine|double_class|remember, title, body, options: Vec<String>, created_at}>` · `dreaming.proposal_act(id, choice) → ()` · `dreaming.metrics(days) → Metrics` (hint open rate, impressions on never-opened share, repeat-correction rate, active concepts, median title length, skill loads per 100 sessions, preference lines, spend, undos). `enable` is the existing `enable_schedule("dreaming", bool)`.
ts-rs exports (feature `ts`, as every other plugin): `DreamStatus, NightSummary, NightCounts, RunRequest, UndoReport, DreamSettings, DreamModels, Proposal, Metrics, Case, EditRecord, ApplyReport, Uses` — C's as-built lists each with its TypeScript shape verbatim so W4 can hand-write identical stubs.

## 3. Behaviour per wave (what "done" means beyond the seams)

### 3.1 W1 KNOWLEDGE
1. Frontmatter round-trip: `put_for` writes the seven new keys when set and preserves unknown keys of an existing file; every one of the 1,598 live files parses unchanged (test over a fixture dir with the live shapes: no `status`, pasted `Cited by:` lines, 3-segment ids absent, `architecture`/`tool` types).
2. `uses` table: `record_shown`/`record_opened` upsert; `stats` returns zeros for unknown ids; `sync()` never touches `uses`; the FTS row carries the new UNINDEXED columns.
3. Rank (§5.3): `score = bm25 × scope × type_weight × freshness × use`; `type_weight` decision 1.3 · warning 1.3 · procedure 1.1 · fact 1.0 · entity/hosts/project 0.9 · record 0.6 (0 past `lifetimes.record` days — the knowledge crate reads the lifetime from its own config section `knowledge.lifetimes`, default record 14, fact 30, procedure 90, entity 60; dreaming's config mirrors these and the as-built says which wins: knowledge's); `freshness = 0.5 + 0.5·exp(−age_days/45)` for record and fact, 1.0 otherwise; `use = 1 + ln(1 + opened) − 0.4·[shown ≥ 50 ∧ opened = 0 ∧ type ≠ warning]`. Tests: more opens → higher; shown ≥ 50 & 0 opens → lower; warning exempt; `hint: false` and non-live status never returned by `hint_for`; `search` still returns archived items with an `[archived]` marker.
4. Commands/tools: `merge` writes the canonical (body given, `evidence` = union, tags = union, scope = canonical's), rewrites each folded id as a stub `status: merged, merged_into: <into>, hint: false` with a one-line body "Merged into [<into>](/<into>.md)"; `links` rows pointing at a folded id are retargeted; `supersede` sets `superseded_by`, `valid_until`, `hint: false` on the loser; `archive` sets `status: archived, hint: false` and appends `reason` to the body under "## Archived"; `verify` sets `verified` (ok) or `status: stale` + note (stale); `retitle` sets `title` and prepends a "Summary" paragraph with the old title text to the body. Each is one gix commit `dreaming <op> <id>` with author `dreaming <dreaming@localhost>` when `Source::Dreaming`, else `theuth`. Each publishes `KnowledgeChanged`.
5. `knowledge_get` records an open; the `<knowledge>` note text changes to "Concepts that may be relevant; read one with `await knowledge_get(id)` in python before relying on it" (unchanged) — nothing else in the note changes.
6. The owner-root/unattended predicate on the five mutating tools; refusal text names the rule.

### 3.2 W2 AGENT
1. Budget guard at `Reserved::open` + `<budget>` note; `SetBudget` handler; unit test with a 1-cent budget: the second model call is refused, the turn ends `end_turn` with the note queued for the next input; spend read from the owner's `model_calls` (returned or estimated nanos).
2. `LedgerSince` handler with the caps and `truncated` flag; a test over a fixture store with two sessions (one private, excluded) asserting counts; secrets scrubbed through the existing redactor.
3. Harvest changes (§2.2); fixture test: a 400-char title splits into ≤ 120 + Summary paragraph; `evidence` and `status: fresh` set; `type: record` accepted.
4. `record_shown` call where the note is built (C placed the call; W2 verifies it fires once per note with the ids shown).
5. Learned sidecar append in `load_skill`, `always` inlining and ACP materialisation; `skills/retired/` skipped; `origin: dreaming` skills listed last; tests for each.
6. `docs/design/136-as-built-W2.md` with the prompt diff verbatim.

### 3.3 W3 DREAMING (the behaviour; the routine; the skill)
1. `Signals`: `signals.json` within 1 s of a `TurnFinished`; rewind snapshot ≤ 2 KB scrubbed; `on_ac`/`battery_pct` from `pmset -g batt` parsed at most once a minute (macOS), `None` elsewhere.
2. `sensor.py` (design §4 phase 0): exit 0 only when every rule holds; the heavy-day second window; refuses when `signals.json.engine ≠ self`; unit-tested rule function in Rust with the same table (`sensor_rules.rs`) and the Python kept in lockstep.
3. `dreaming_ledger`: asks `LedgerSince` (since `state.last_dream_at` or 24 h, excluding the dreaming session and children), asks knowledge `Stats` for the shown ids, folds `ledger/pending/*`, copies desk journals (`<data_dir>/roster/journal/*.md`) into `ledger/<night>/journals/`, writes `ledger/<night>.json` ≤ 2 MB, returns it plus `run_request`.
4. `cases.rs`, `edits.rs`, `replay.rs`, `preferences.rs`, `journal.rs`, `undo.rs` per design §4–§7 and §2.5–2.6 here. `replay_case` calls the target model with `CallKind::Dream` under the routine's cost owner (so the budget guard counts it) and the judge model from config; the judge sees `ideal` + `user_words` + the answer, never the proposer's rationale; returns `{answer, score (0–1), reason}`.
5. `dreaming_undo(night, group)`: for every knowledge change of that night/group, `Restore` the id to its pre-night commit (the night's receipt lists `(id, commit_before)`); for every learned edit, restore `backups/<night>/<skill>.md`; mark edits `undone_at`; publish `DreamUndone`; the journal gets an "Undone" line.
6. `routine.md` (design §9.7 with `budget: 1 USD`, `when: sensors/dreaming.py`) and `skill.md`: one section per phase with the exact tool calls, caps and stop rules, the budget order (§4.1), and the proposer and judge prompts verbatim (the proposer prompt forbids task-specific rules, quoting evaluator text, and instructions that run anything; it receives the rejected buffer and the meta-note; ≤ 4 edits; JSON array of `EditRecord`). `run_now` with `mode: backlog` sets `run_request` and asks schedule `FireWith` with the budget and a 4 h run_timeout override; the skill tells the model to repeat passes while `dreaming_ledger()` reports backlog and no `<budget>` note has arrived.
7. `t dreaming dry-run --data-dir <copy>` (a CLI subcommand in `crates/theuth/src/main.rs` is NOT W3's — W3 exposes `dreaming::dry_run(data_dir) -> Report` as a library fn and the closer wires the CLI): runs Recall + the deterministic parts of Tidy (triage by rules only, duplicate-stem detection, archive-by-use candidates, retitle candidates) with **zero writes** and prints the counts the journal would show. Test over a fixture data dir with 30 concepts and a 2-session store.
8. `docs/design/136-as-built-W3.md` with every file's purpose, the prompts verbatim, and hand-offs.

### 3.4 W4 UI (`crates/desktop/ui`)
1. **Settings › Dreaming** (new settings group, same shell as Settings › Agent loop): status card (last night counts, next window, budget left, backlog, proposals pending), Enabled switch (calls `enable_schedule("dreaming", …)` and reflects it), Window (two time inputs), Budget per night (number, currency label from the existing formatter), **Models**: four Radix `Select`s over `list_models` labelled triage / tidy / propose / judge with "default" entries showing the resolved default, Preferences auto-promote switch, Digest (day + time), Pause for 7 days button, **Dream now** button → dialog with Budget (default 5) and Mode (One pass | Backlog) → `dreaming.run_now`; **Recent nights** list (counts, spend, mode) with an Undo button per night → confirm → `dreaming.undo`; **Proposals** list with the option buttons → `dreaming.proposal_act`.
2. Library › Knowledge: status badges (`archived`, `superseded`, `verified <date>`, `stale`, `fresh`) from the list RPC's new fields; a "changed by dreaming" filter (`source: dreaming` or `status ≠ tidy`).
3. Routines: no new component — the row's subtitle shows the last night's one-line summary when the schedule name is `dreaming` (from `dreaming.status`).
4. Mobile layout per 113; tests: vitest for the settings reducer/formatting, storybook-style fixtures for each state (no nights yet · a night with undo · paused · disabled).
5. `docs/design/136-as-built-W4.md`; hand-written binding stubs listed but never committed.

### 3.5 Z CLOSER
Merge W1 → W2 → W3 → W4 into the integration tree; close every hand-off; wire `t dreaming dry-run` CLI; `just ts-desktop`; `BUILD_SLOTS=1` workspace `cargo check --workspace --tests`, `cargo test` for the five touched crates, `bun run check`/vitest for the UI; run `dry-run` against an APFS copy of `~/.theuth` (knowledge + sessions.sqlite only; never the vault) and paste the counts into `136-as-built-Z.md`; tune the skill's prose where the dry run shows a cap or a rule misfires; `DONE Z <sha>`.

## 4. Numbers the wave must not break
Design §13 baseline: 1,584 concepts; hint open rate 2.2 %; impressions on never-opened ids 53 %; median title 332 chars. The dry run must report: fresh entries, retitle candidates (expect ≈ 1,250), duplicate stems (≈ 19), archive-by-use candidates (expect several hundred records + the 361/0, 331/0 kind of never-opened ids), verification sample size 15.

## 5. Risks the briefs name
- Two waves touching `crates/agent/src/loop_.rs` (C places the `record_shown` call and the `SetBudget` arm; W2 fills behaviour) — W2 is cut after C lands, so there is one owner at a time.
- `main.rs` is touched only by C (registration + bundles) and by Z (the CLI subcommand); other waves (134, shared-hw) may touch it on main — the orchestrator resolves at landing.
- Secrets: cases, ledger and journal pass through the spill redactor; a test asserts a 37-style secret-placeholder string and a `sk-…` token are scrubbed from a case.
- The `uses` table makes `knowledge.sqlite` partly durable — the as-built of W1 amends design 10's "deletable" sentence.
