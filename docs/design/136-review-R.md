# 136 R — review of Dreaming as merged (`675e95fd..aa9696fe`, tree detached at aa9696fe)

Read: design v0.2 (incl. §14.1/§15), contract v1, as-builts C/W1/W2/W3/W4/Z; then the diff file by file through the six lenses. Recorded deviations are not findings unless they are themselves wrong or unsafe. Every line number is this tree's.

## Summary

| Severity | Count |
|---|---|
| BUG (wrong behaviour or safety hole) | 16 |
| RISK (likely to fail live or under load) | 18 |
| POLISH (quality/UX/wording) | 15 |

Top five, in order: the per-night budget is a lifetime cap (night 2 never runs) · the gate is model-asserted (an edit is applied with whatever `verdict` the proposer sends) · the proposer's `EditRecord` JSON cannot be deserialised by `skill_learned_edit` · `approvals: false` leaves `shell`/`knowledge_delete` callable and the skill tells the model to use a non-existent `delete_file` · the tidy-mark put flips a desk's private entry to `owner: user, visibility: shared`.

---

## BUG

### R-1 · Per-night budget is a lifetime cap on the reused routine session
- where: `crates/agent/src/store/accounting.rs:456-460`; `crates/schedule/src/lib.rs:1245-1271, 1274-1281`; `crates/agent/src/lib.rs:455-459` (`Budget{nanos, currency}`); `crates/agent/src/loop_.rs:2571-2583`
- what: `"SELECT COALESCE(SUM(CASE … END), 0) FROM model_calls WHERE owner_id = ?1"` — no time bound. The schedule fires the same session every time (`Some(s) => { … Compact{Clear} … s }`), only a root delete removes `model_calls` (`store.rs:2413`), and `SetBudget{nanos: 1 USD}` is re-sent each fire.
- why: night 1 spends ≈ $1; night 2's first `Reserved::open` sees `spent ≈ 1e9 + estimate > cap` → `Abort::Budget` → `end_turn`, no tool runs, no journal — every night thereafter, and every `Dream now` compares its override against the cumulative total. `dreaming.status.budget_left_tonight_usd` meanwhile reports $1.00 (derived from `last_night.spend_usd`, `night.rs:140-153`). `budget_tests.rs:23-134` demonstrates exactly this shape (a prior $0.25 settled row blocks a later 1¢-cap turn) and calls it the intended behaviour. W2 §3's premise "`memory: clear` routines start at zero" is not what `fire_with` does.
- fix: window the sum from the moment the cap is set. `Budget` gains `since: String` (server-stamped in `Agents::handle(SetBudget)`, `supervisor.rs:3129`; `#[serde(skip)]`/`#[ts(skip)]` so the wire type is unchanged); `budget_spent(owner, currency, model, since)` adds `AND created_at >= ?` to both queries.
  ```rust
  // supervisor.rs SetBudget
  let b = m.nanos.map(|nanos| Budget { nanos, currency: m.currency, since: chrono::Utc::now().to_rfc3339() });
  // accounting.rs
  "… FROM model_calls WHERE owner_id = ?1 AND created_at >= ?3"
  ```
- test: `budget_tests`: settle a $0.90 row, then `SetBudget{1 USD}`; the next call must go out; settle it at $0.20; the call after is refused. Today the first assertion fails.

### R-2 · The replay gate is model-asserted; nothing checks `gate` against replay results
- where: `crates/dreaming/src/edits.rs:291, 310-335`; `crates/dreaming/src/replay.rs:116-168, 174-208`; `crates/dreaming/src/skill.md:85-86`
- what: `let accepted = e.gate.as_ref().is_some_and(|g| g.verdict == Verdict::Accepted);` then `if record_pass { if report.text != before { … write_atomic(&sidecar_path(..)) } }`. `replay()` returns `{answer, score, reason}` and persists nothing; `replay::gate` is called only from its own tests. The skill: "Gate (the Rust `replay::gate` rule, mirrored here) … Set the same `gate` … on every edit".
- why: design §7.2/§11 row 6 make the held-out gate the one thing that may not be skipped; here it is enforced by the proposing model doing arithmetic and reporting honestly. A miscount, or `verdict: accepted` with zero replays, writes a live sidecar bullet. §0.11 "judge ≠ proposer" is void at the acceptance step: the night's model both proposes and decides.
- fix: persist replays and recompute server-side. `replay()` appends `{night, case_id, candidate_sha256: Option<String>, score}` to `dreaming/replays.jsonl`; in `edits::apply` record pass, ignore the caller's numbers: collect tonight's rows for `sha256(report.text)` (candidate) and `None` (baseline), pair by `case_id`, `gate = replay::gate(&b, &c, cfg)`; no rows → every edit `Error("no replays for this candidate")`, nothing written. Skill step 5 becomes "read the verdict from the record pass".
- test: record pass with `gate.verdict = Accepted` and no logged replays → nothing written; 3 logged pairs 0.4→0.7 → applied; 2 pairs → `unverified`, not applied.

### R-3 · The proposer's `EditRecord` JSON cannot be deserialised by `skill_learned_edit`
- where: `crates/dreaming/src/types.rs:172-178`; `crates/dreaming/src/tools.rs:375-382`; `crates/dreaming/src/edits.rs:303-309`; `crates/dreaming/src/skill.md:83, 92`
- what: `pub struct EditRecord { pub id: String, pub night: NightId, pub target: String, … }` — no `#[serde(default)]`; `SkillLearnedEditArgs { edits: Vec<EditRecord>, … }`. The skill's proposer prompt produces `{"op","content","anchor","rationale","cases"}` and step 3 passes "the records, gate: null" straight to the tool. `edits.rs:303-309` fills empty `id`/`night`/`target` — a path that is never reached.
- why: every Rehearse call fails with `missing field `id``; a model that reads the schema and invents ids persists them into `edits.jsonl` and `closed_by: edit:e1`.
- fix: `#[serde(default)]` on `id`, `night`, `target` (and `applied_at`, `undone_at`, `gate` are already `Option`).
- test: `serde_json::from_str::<EditRecord>(r#"{"op":"add","content":"x","anchor":null,"rationale":"r","cases":[]}"#)` parses and `apply` assigns a 32-char id and the night.

### R-4 · `approvals: false` leaves Destructive tools callable; the skill names `delete_file` (absent) and `git log`
- where: `crates/dreaming/src/routine.md:11`; `crates/schedule/src/lib.rs:1285-1301`; `crates/tools/src/lib.rs:457-459, 518-522`; `crates/tools/src/shell.rs:266`; `crates/knowledge/src/tools.rs:366`; `crates/dreaming/src/skill.md:49, 103`; no `SetCap` in `crates/dreaming`
- what: `SetApprovals{session, enabled: false}` → "every `ask` (policy or MCP gate) runs as `allow`"; `destructive: PolicyAction::Ask` by default; `shell` and `knowledge_delete` are `Risk::Destructive`. Skill §5: "move it to `skills/retired/<name>.md` … (`write_file` + `delete_file`; never a human skill)" — there is no `delete_file` tool in the tree; §2 Verify: "cites the fix commit when `git log` shows it" (= `shell`).
- why: design §9.4 "the tools above are the whole blast radius" and §15 (34) "`approvals: false` is permitted for it because its tool surface is the bounded set" are false: the night runs `shell`, `knowledge_delete`, `write_file`, MCP tools unattended, and the skill itself points the model at `shell` for retirement. Knowledge bodies are "data, not instructions" by prompt only (§10 row 4).
- fix: cap the session at `Mutating` at fire time (before the first model turn): schedule `file.rs` gains `cap: mutating` (→ `tools::SetCap{session, cap: Some(Risk::Mutating)}` next to `SetApprovals`, `lib.rs:1285`), set in `routine.md`. Skill: drop `git log` (use `knowledge_history`/`read_file`), replace retire-by-move with `write_file("skills/retired/<name>.md", text)` + `write_file("skills/dreaming-<name>.md", "---\nretired: true\n---\n")` or defer retirement of dreaming-made skills.
- test: registry test — bypassed session + `SetCap{Mutating}` → `shell` is denied, `knowledge_merge` allowed; `routine_text` parses with `spec.cap == Some(Mutating)`.

### R-5 · Tidy-mark `knowledge_put` rewrites `owner`/`visibility` from the dreaming session's scope — private → shared
- where: `crates/knowledge/src/store.rs:829-837`; `crates/dreaming/src/skill.md:35-39`; `crates/dreaming/src/ledger.rs:134` (`k.list(None, …)` lists every owner's entries); `crates/dreaming/src/tidy.rs:120-150` (no owner/source filter)
- what: `let owner = scope.map_or(USER_OWNER, ..); let visibility = p.visibility.unwrap_or(if scope.is_some() { Private } else { Shared }); fm.set("owner", …); fm.set("visibility", …);` — unconditional on an existing file. The recipe passes no `visibility`.
- why: the dreaming session has no `AgentScope` → every tidy put on a desk's `owner: agent:<name>, visibility: private` fresh entry (≤ 300 a night) lands as `owner: user, visibility: shared`: a visibility leak and the opposite of §12 "desks' entries are tidied under their visibility and never rewritten".
- fix: `put_for` keeps the file's identity when it exists: set `owner` only on create; set `visibility` only on create or when `p.visibility.is_some()`. Belt: skill recipe passes `visibility=meta["visibility"]` and skips `meta["owner"] != "user"`/`meta["source"] == "user"`.
- test: put `facts/p` with `AgentScope{owner: "agent:a"}` (private); `put_for(Put{status: Some(Tidy), ..}, Dreaming, None)`; assert `owner == "agent:a"`, `visibility == Private`.

### R-6 · `may_consolidate` admits isolated roots and every routine, not "the owner's own root"
- where: `crates/agent/src/store.rs:1255-1258`; `crates/agent/src/lib.rs:1778-1794` (`scope = 'isolated'` for `NewSession`/root-creating `SendInput`); `crates/schedule/src/lib.rs:1651-1670` (every fired session is `unattended`)
- what: `"SELECT unattended OR (parent IS NULL AND desk IS NULL) FROM sessions WHERE id = ?"` — `scope` never read.
- why: any ACP/channel/`SendInput` root passes and may `knowledge_merge/archive/supersede/retitle` the shared store or flip `status`/`hint` through `knowledge_put`; every user routine with `approvals: false` is a consolidator. Design §0.3: only dreaming consolidates. C deviation 3 records the predicate's *location*, not this width.
- fix: `… OR (parent IS NULL AND desk IS NULL AND scope = 'owner')`; narrow routines by registering the dreaming session: `Night` already receives `BeginNight{session}` — keep `dreaming_sessions` and make the consolidator `unattended AND id IN dreaming_sessions OR owner root`.
- test: `NewSession{unattended:false}` root → `false`; owner root → `true`; a non-dreaming routine session → `false`.

### R-7 · Every consolidation op restamps `timestamp`, resetting the age the rank and the archive rules read
- where: `crates/knowledge/src/store.rs:301-306` (`stamp`), `1703, 1716, 1753, 1777, 1811, 1832` (each op), `862` (`put_for`); `1508-1512, 1534-1540` (`age_days` from `timestamp`; record past lifetime → 0); `crates/dreaming/src/tidy.rs:73-77, 120-125`
- what: `fn stamp(fm, source) { fm.set("source", …); fm.set("timestamp", now) }`; `age_days` parses `timestamp:` only, on both sides.
- why: `retitle` (≈ 1,500 backlog, 300 a pass), the tidy put, `verify ok` and merge stubs all set `timestamp = now`: a 60-day `record` retitled tonight is hinted for 14 more days; archive-by-age cannot fire on any retitled record for 14 days; the ≥ 14-day / 50-shown noise rule re-arms. §5.2 means the event date; §5.1 says the same of `valid_until` ("not the save time").
- fix: split `stamp` into `stamp_source` for the five ops and a `Source::Dreaming` put (keep `timestamp` when present; git has the write time).
- test: `record` with `timestamp` 20 d ago → `retitle` → `hint_for` still excludes it and `meta.timestamp` unchanged.

### R-8 · The budget stop is unobservable in-turn; a capped night never journals, never closes, re-fires
- where: `crates/agent/src/loop_.rs:5174-5182` (refusal → `Round::Stop("end_turn")`, note queued), `4949-4952` (note flushed at the *next* turn); `crates/dreaming/src/skill.md:13, 15, 88`; `crates/dreaming/src/routine.md:17`; `crates/dreaming/src/night.rs:538-560` (`last_dream_at`, `run_request = None`, `Dreamt` only in `finish`); `crates/dreaming/src/sensor_rules.rs:67-75`
- what: the skill's two stop signals are "a `<budget …>` note in your input" (arrives one turn later — a routine fire is one turn) and `replay_case` answering `budget exhausted:` (after which the next round call is refused too: same guard, `estimate` = last settled cost > 0).
- why: when the cap is hit during Tidy/Mine the turn ends silently; `Commit{journal}` never runs: the record stays `finished_at: None`, `last_dream_at` and `sessions_since` do not move, a Dream-now `run_request` stays and makes every later nightly fire `mode: backlog` (`ledger.rs:192-199`). Once R-1 is fixed the sensor re-fires after `cooldown: 3h` with a fresh $1 (window/30 m × budget worst case); today R-1 masks it. Backlog mode "repeat passes … until no `<budget>` note" cannot end any other way than mid-pass.
- fix: close the night from the actor, not the model: `Night` subscribes `agent::AgentEvent::TurnFinished{session}`; if `active.is_some()` and `r.session == Some(session)` → `finish(r)` with a phase line `stopped: budget|turn end`. Also have `Signals` set `last_dream_at` on `DreamNightStarted` (not resetting counters, so a heavy day keeps its second window).
- test: `BeginNight{session S}` → deliver `TurnFinished{S}` without `Commit{journal}` → `finished_at.is_some()`, `state.last_dream_at` advanced, `run_request == None`, one `DreamNightFinished`.

### R-9 · `preferences_set` is not undoable: no backup is ever written; the undo branch is not idempotent
- where: `crates/dreaming/src/preferences.rs:278`; `crates/dreaming/src/undo.rs:131-146`
- what: `write_atomic(&path(data_dir), render(&lines).as_bytes())?` with no backup anywhere (`grep backup preferences.rs` → none); `undo.rs:137` reads `backups/<night>/preferences.md` and restores with no `undone` marker.
- why: rung 3 — the system-prompt block, the highest-blast-radius rung (§7.1) — cannot be undone by `dreaming_undo`, contrary to §9.4/§3.3 item 5 and the W3 as-built; if a backup ever existed, every repeat undo re-clobbers and re-reports.
- fix: in `preferences::set` before the write: `let b = night::dir(d).join("backups").join(ledger::tonight()).join("preferences.md"); if !b.exists() { write_atomic(&b, &std::fs::read(path(d)).unwrap_or_default())?; }`; in `undo` guard with `record.sidecars_undone.contains("preferences")`, empty backup → remove the file.
- test: `set(add)` on night N, `undo(N, None)` → block equals the pre-set text; second `undo` restores nothing.

### R-10 · `dreaming_ledger` is `ReadOnly`-labelled and ungated, yet it opens the night, writes and consumes inputs
- where: `crates/dreaming/src/tools.rs:106, 117-125`; `crates/dreaming/src/ledger.rs:201-209, 252, 296-300`
- what: `const RISK: Risk = Risk::ReadOnly;` and `call` goes straight to `ledger::build(.., Some(cx.session), ..)` — no `self.0.mutating(..)`. `build` asks `BeginNight{session}` (sets `active`, `r.session`, publishes `DreamNightStarted`), copies desk journals, writes `ledger/<night>.json`, deletes every `ledger/pending/*.json`.
- why: any session — a desk, a child, a private session, the user in chat — can consume the pending rewind snapshots so the night never sees them, become `r.session` (the spend owner: `spend_nanos` then sums that session's calls into `spend_usd`), exclude itself from the ledger, or open `active` so unrelated dreaming-source ops are attributed to a night. §9.4 says the tools are visible only to the dreaming session and owner roots; the registry has no such filter.
- fix: `tools.rs:117` add `self.0.mutating(Self::NAME, &cx).await?;` (the same gate the other mutating tools use) and `RISK = Mutating`.
- test: `LedgerTool` with a `ToolCx` whose session fails `may_consolidate` → `Err`; `ledger/pending/*.json` intact; no `.night.json`.

### R-11 · A second `dreaming_journal{section: "journal"}` re-runs `finish`
- where: `crates/dreaming/src/night.rs:744-757, 538-575`
- what: `if m.phase == "journal" { self.finish(r).await … }` with no `finished_at.is_some()` guard; `finish` does `nights += 1`, `last_dream_at = now`, `Dreamt` tell, `publish(DreamNightFinished)`.
- why: the model calling the close twice (or `Journal` + `journal`: `section.to_lowercase()`) double-counts nights, resets the signals counters twice, emits two `DreamNightFinished` (two digest rows once attention lands), moves `last_dream_at`.
- fix: at the top of the journal branch `if r.summary.finished_at.is_some() { write_record(..)?; return Err("night already closed".into()); }`.
- test: two `Commit{phase: "journal"}` → `state.nights == 1`, one event.

### R-12 · Settle's helpful/harmful recipe is a no-op and demands a fabricated gate
- where: `crates/dreaming/src/skill.md:72, 102`; `crates/dreaming/src/edits.rs:192-206, 291, 310-316`; `grep harmful crates/dreaming/src` → parse/render only
- what: skill: "`harmful +1` (`skill_learned_edit` with `op: replace`, the exact bullet as `anchor`, the same content — the counters live in the metadata the tool rewrites)". `apply_one` `Replace` sets `content`, unions `evidence`, resets `since`; it never touches `helpful`/`harmful`. The record pass needs every edit to carry a gate and writes only `Accepted` ones.
- why: §0.7/§12 name "helpful/harmful counts" and reopen detection as the safety net replacing the trial state; nothing ever increments a counter, so `helpful − harmful ≤ −2 → delete` never fires. Following the recipe means inventing `{verdict: accepted}` for a counter bump — logged in `edits.jsonl` as an accepted edit, and resetting `since`.
- fix: counters server-side: on `cases::transition(.., Closed, Some("edit:<id>"))` bump `helpful` on the bullet whose `evidence` contains the case; on the `Reopened` transition (`tools.rs` case_put path) find the edit in `edits.jsonl` with that case and bump `harmful`, deleting at ≤ −2. Delete skill:102's tool instruction.
- test: sidecar bullet with evidence `[c1]`; `case_put{id: c1, state: reopened}` → `harmful: 1`; twice more → bullet removed.

### R-13 · `dreaming.enabled: false` leaves the installed routine armed, with approvals off and no dreaming tools
- where: `crates/dreaming/src/lib.rs:165-168, 115-140`
- what: `if !cfg.enabled { tracing::info!("dreaming disabled by config"); return Ok(()); }` — `install_files` only ever adds; nothing removes or disables `schedules/dreaming.md`, `sensors/dreaming.py`, `skills/dreaming.md`.
- why: design §8/§9.2 and the UI copy (`Dreaming.tsx:112` "no routine, no sensor, no signals") promise removal. After one enabled run, disabling in config leaves the routine polling every 30 m; a frozen `signals.json` (stale `last_root_turn_at`, non-zero `sessions_since`) passes the gate inside the window → a session fires with approvals off, the `knowledge_*` consolidation tools (registered by knowledge regardless, predicate passes for unattended) and no `dreaming_*` tools or Signals writer. Not recorded in any as-built.
- fix: before the early return, remove the three files (ignore NotFound); the schedule reconcile drops a removed file.
- test: install into a tempdir, run the disabled branch → none of the three paths exist.

### R-14 · The model tiers do not reach the run: Settings' Tidy pick never edits `model:`; the default night runs on `agent.model`; judge/target default to the same model
- where: `crates/dreaming/src/lib.rs:90-94, 108` (`model:` emitted only at install, only when `tidy` is `Some`); `crates/dreaming/src/night.rs:243` (`update_routine_file` patches `budget | when | run_timeout` only); `crates/dreaming/src/config.rs:69-75` (all four `None` by default); `crates/dreaming/src/replay.rs:133-135, 147-152` (`ModelRole::Session` fallbacks); `crates/desktop/ui/src/lib/dreaming.ts` `resolvedDefault` (shows `Default (<title_model>)`)
- what: with the default config the routine has no `model:` line → `new_session(.., model: None)` → the session runs on `agent.model` (the live config's `openai/gpt-5.6-sol`, effort max). Picking a Tidy model later persists `cfg.models.tidy` but the file is never patched (`grep models.tidy` → settings get/set only). `replay_case` target and judge default to the session model → one model proposes, answers and judges.
- why: design §9.2 defaults `tidy` to `agent.title_model` (haiku) and §0.11 requires judge ≠ proposer; at frontier prices a $1 night affords a fraction of Tidy. The Settings pane shows "Default (anthropic/claude-haiku-4-5)" for a tier that actually runs gpt-5.6-sol — the UI lies about the model spending the money. W3 §4.9/Z record the `propose` tier as unused and the judge fallback, not the install-time `model:` gap or the `agent.model` default.
- fix: `routine_text` resolves `tidy` server-side (`rt.section("agent")` → `title_model` ?? `subagents.model`) and always emits `model:`; `update_routine_file` adds `"model"` to its key set (insert/replace/remove); `replay` resolves judge to `agent.model` when its vendor differs from the target; expose `resolved: DreamModels` on `DreamSettings` so the pane shows the server's answer (W4 hand-off).
- test: install with `tidy: None` → routine `spec.model == Some(title_model)`; `SetSettings{tidy: Some("x/y")}` → `Some("x/y")`; back to `None` → resolved default again.

### R-15 · Budget render can produce an unparseable routine file
- where: `crates/dreaming/src/lib.rs:106`; `crates/dreaming/src/night.rs:960-962`; `crates/schedule/src/file.rs:48-56, 65-68`
- what: `.replace("{{budget}}", &format!("{} USD", cfg.budget_usd))` — `f64` Display; `SetSettings` checks only `is_finite() && >= 0.0`; `Budget::parse` refuses `frac.len() > 9` and overflow.
- why: `0.1234567891` or `1e20` from the Settings number input writes a `budget:` line the schedule's reconcile rejects → the `dreaming` entry becomes a parse error and stops firing until the file is repaired by hand. The UI reducer (`lib/dreaming.ts:122`) only checks `>= 0`.
- fix: render from nanos: `let n = (usd * 1e9).round() as u64; format!("{}.{:09} USD", n / 1_000_000_000, n % 1_000_000_000)`; bound `budget_usd` in `SetSettings` (e.g. ≤ 1e6).
- test: `routine_text` with `budget_usd: 0.1234567891` and `1e12` parses via `schedule::file::parse`.

### R-16 · The "auto-promote preferences" switch controls nothing
- where: `crates/dreaming/src/config.rs:145`; `grep auto_from_explicit crates/dreaming/src` → config, settings get/set only; `crates/dreaming/src/skill.md:104`; `crates/dreaming/src/tools.rs` `PreferencesSetTool`
- what: `preferences.auto_from_explicit` is persisted and shown (`preferences_auto`), never read by `preferences_set` or the skill; the skill promotes "automatically and reported" unconditionally.
- why: design §8 ("preferences: auto-promote from my explicit instructions (default on)") and §11 row 12 ("human veto on the always-on rung") give the user this switch; off, the night still writes rung 3 without a press.
- fix: `PreferencesSetTool` with `op: add` and `!cfg.preferences.auto_from_explicit` → `bail!("auto-promote is off: file it as a `remember` proposal")`; skill §5 says so.
- test: `auto_from_explicit: false` → `preferences_set(add)` errors and `preferences.md` is unchanged.

---

## RISK

### R-17 · Signals ignores `TurnStarted`: a long in-flight root turn passes the idle rule
- where: `crates/dreaming/src/signals.rs:207-221`; `crates/agent/src/lib.rs:2196` (`TurnStarted` exists)
- what: only `TurnFinished` stamps `last_root_turn_at`.
- why: a root turn started 25 min ago and still running leaves `last_root_turn_at` ≥ idle → the gate fires while the user's turn runs (§0.2 "no root session has turned for 20 min" means active, not finished).
- fix: add the `TurnStarted { session, .. }` arm (attended root → stamp + `flush()`); or keep `roots_running: u32` in the snapshot and a `roots_running > 0` rule in both gates.
- test: `TurnStarted` at T, gate at T+25 min → `Err("a root session turned recently")`.

### R-18 · `sessions_since` counts the dreaming session's own turn (and every routine): "nothing new" never holds
- where: `crates/dreaming/src/signals.rs:214-216, 264-269`; `crates/dreaming/src/night.rs:556-560`
- what: `if self.seen.insert(session) { self.snap.sessions_since = self.seen.len() }` on every `TurnFinished`; `Dreamt` is sent inside `finish`, *before* the dreaming routine's own turn finishes → the next `TurnFinished` is the dreaming session's → `sessions_since = 1`.
- why: every night fires even on an empty day (§0.2 rejected "dreams on empty days"); the 30-min `every:` loops of other routines also count.
- fix: count only attended roots: wrap the `seen.insert` in `if self.attended_root(session).await`.
- test: `Dreamt` then `TurnFinished{dreaming session}` → `sessions_since == 0`.

### R-19 · Stale `signals.json` is never detected; no heartbeat
- where: `crates/dreaming/src/signals.rs:114-116` (`written_at` written); `crates/dreaming/src/sensor_rules.rs:36-89`, `crates/dreaming/src/sensor.py:54-96` (never read)
- why: with the Signals actor dead or the plugin disabled (R-13) the file freezes and the gate keeps passing. The two rule tables are otherwise line-for-line identical (engine, HH:MM, `[start,end)` with midnight crossing, tz offset, idle floor, desks, already-dreamt, heavy, nothing-new, power matrix).
- fix: a 10-min `interval` tell in `Signals` that calls `flush()`; both gates refuse `written_at` older than 30 min (`"signals stale"`); add the row to `rows()` so the Python lockstep test covers it.
- test: table row `written_at: now − 45 min` → `signals stale` on both sides.

### R-20 · `commit_before` is read at event-handling time; writes outside an open night are never receipted
- where: `crates/dreaming/src/night.rs:767-769, 791-805`; `crates/knowledge/src/store.rs:1859-1876` (publish after commit, via the bus `tell`)
- what: `k.history(Some(&e.id), 2) … .changes.get(1)` when the event is handled; first touch wins.
- why: two commits on one id before Night handles the first event (the skill's own Tidy recipe does `knowledge_retitle` then `knowledge_put` on each fresh id; the Night mailbox may be busy — `Status` runs `ledger::candidates`, which lists 20,000 concepts and may run the ≈ 10 s `uses_from_sessions` scan) pins `commit_before` to the first *dreaming* commit, so undo leaves the id half-changed. A `KnowledgeChanged{source: dreaming}` while `active` is `None` (owner-root `knowledge_archive` in chat, `ProposalAct::Remember`) is not receipted at all.
- fix: choose the first entry of `history(id, 20)` whose message does not start with `dreaming ` (and, for `put`, is older than `started_at`); cleaner: carry `commit` on `KnowledgeChanged` (the store has it in `Receipt`).
- test: two consolidate commits on one id, then deliver both events → `Touched.commit_before` is the pre-first commit.

### R-21 · `LedgerSince` runs multi-second full scans inside the supervisor's mailbox
- where: `crates/agent/src/supervisor.rs:3155-3193`; `crates/agent/src/store/dreaming.rs:81-87, 364-370`; `crates/agent/migrations/0026_message_at.sql:4` (no index on `messages.at`; none on `tool_calls.finished_at`, `inner_calls.at`, `model_calls.created_at`)
- why: on the 829 MB live store this blocks every `SendInput`/`SetBudget` for the scan, twice a night (`dreaming_ledger`, `spend_nanos`) and on every Dream now; `SideCall` already uses `DelegatedReply` + `tokio::spawn` (`supervisor.rs:3475-3497`).
- fix: same pattern (clone store + vault, spawn, delegated reply) plus one migration `CREATE INDEX messages_at ON messages(at)` (and `model_calls(created_at)`).
- test: smoke timing on the APFS copy, as Z did for the `uses` fallback.

### R-22 · The budget is in-memory only; a durability-resumed turn runs uncapped after a restart
- where: `crates/agent/src/loop_.rs:692-695, 837, 4501`; `crates/agent/src/lib.rs:466-468`
- why: an engine restart during a 45-min night resumes the dreaming turn with `budget = None` until the next fire — the rest of the night and its `replay_case` side calls are unmetered. Recorded as in-memory; the uncapped-resume consequence is not.
- fix: persist `(nanos, currency, since)` on the session row (or a side table) in `SetBudgetNow`; load into `Shared.budget` when the actor is built.
- test: set a budget, rebuild the actor from the store, assert `shared.budget.is_some()` before the first resumed call.

### R-23 · Harvest re-put replaces `evidence` and resets `status` on an existing concept
- where: `crates/knowledge/src/store.rs:842-854`; `crates/agent/src/loop_.rs:6616-6618`
- what: `if let Some(s) = p.status { fm.set("status", …) }`, `if !p.evidence.is_empty() { fm.set("evidence", …) }` — set, not union; the harvest always sends `status: Some(Fresh)`, `evidence: [session]`.
- why: a harvest that re-emits an id dreaming has merged/tidied overwrites the evidence union with one session and flips a `merged` stub to `fresh` while keeping `merged_into` (and `hint: false`: a fresh entry that never hints). §4 Tidy: `evidence` = union; §0.3: the harvest writes raw *new* entries.
- fix: union evidence in `put_for`; set `Fresh` only when the file has no `status` or `status: fresh`.
- test: file `status: merged, evidence: [a,b]`; harvest put with `[c]` → `evidence == [a,b,c]`, `status == merged`.

### R-24 · `learned_path` trusts the skill's front-matter `name` (path traversal read)
- where: `crates/agent/src/skills.rs:37-39, 118-127, 178-182`
- what: `dir.join("learned").join(format!("{name}.md"))` with `name` from any parsed skill (repo-cloned skills included); `check_name` applies only to `add`.
- why: `name: ../../knowledge/<owner>/secret-note` splices that file into every session that loads the skill (every turn if `always`). Dreaming's writer rejects such names (`edits.rs:218-224`); the reader does not.
- fix: gate the read on `tools::git::valid_name(&meta.name)` (or refuse such names in `index()` with the existing warn path).
- test: skill `name: ../x`, `<dir>/x.md` beside `learned/` → `body()` does not contain `x.md`.

### R-25 · `source: user` protection is prompt-only; the ops overwrite `source`
- where: `crates/knowledge/src/store.rs:1703, 1716, 1753, 1777, 1811, 1832` (`stamp(.., Source::Dreaming)` unconditionally); `crates/dreaming/src/skill.md:7`; `crates/dreaming/src/tidy.rs` (no `source` filter in any candidate list)
- why: §7.1 "any `source: user` concept" is out of bounds; `merge` replaces bodies, `retitle` the title, then every op stamps `source: dreaming`, erasing the marker the prose rule depends on. Not recorded in W1/Z.
- fix: `Knowledge::load` (store.rs:1845, the reader every op uses): `ensure!(fm.source != Some("user"), "{id} is human-authored (source: user); dreaming never rewrites it")`; same in `PutTool` when `consolidates`; `tidy::candidates` skips `source: user`.
- test: `k.write("facts/h", …)` (stamps `source: user`) → `archive`/`retitle`/`merge` bail, file byte-identical.

### R-26 · The hint window is spent on dead rows (filter after the cut)
- where: `crates/knowledge/src/store.rs:1097-1104, 1113-1118, 1423-1426, 1472`
- what: `ranked(.., limit + exclude.len(), ..)` truncates by raw bm25, then `hint_for` drops `!hint || !status.is_live() || score <= 0`.
- why: merge stubs keep title/description/tags and match beside their canonical; after night 1 hundreds of archived/`hint: false` rows match too — a 5-line note can show 2 lines while live matches sit below the cut. §5.3 says filtered *before* ranking.
- fix: `AND hint = 1 AND status IN ('fresh','tidy')` in the MATCH query for the hint path (UNINDEXED columns are filterable), or filter in `ranked`'s `filter_map` before `truncate`.
- test: 6 concepts matching two terms, 4 archived with better bm25 → `hint_for(limit 5)` returns both live ones.

### R-27 · In-flight calls count 0 toward spend; concurrent side calls overshoot the cap
- where: `crates/agent/src/store/accounting.rs:456-460` vs `533-537` (`estimated_nanos` written only at settle); `crates/agent/src/loop_.rs:1971-1974` (`SideCall` spawned)
- why: python can `await` several `replay_case`s concurrently; each passes the guard against the same `spent`. Ceiling is one call per concurrent stream, not one call (W2 §3 says one).
- fix: count `dispatched` rows at the last estimate in `Reserved::open` (`+ COUNT(dispatched) × estimate`), or record it in the as-built.
- test: two dispatched-unsettled rows + cap just above one settled cost → next `open` refused.

### R-28 · The night's own `knowledge_get` reads count as opens and defeat archive-by-use
- where: `crates/knowledge/src/tools.rs:183-187` (`record_opened` unconditional); `crates/dreaming/src/skill.md:31, 36`; `crates/dreaming/src/tidy.rs:127-131`
- why: Tidy reads up to 300 fresh + 300 retitle + 15 verify entries a night; each gets `opened += 1`, so `opened == 0` can never hold again for anything the night touched — the 53 % never-opened noise (§13) becomes undetectable; §11 row 1 (the reader confirming itself).
- fix: `GetTool`: skip `record_opened` when `may_consolidate(cx.session)` (one guard).
- test: get from a consolidating session → `stats(id).opened == 0`; from a plain session → 1.

### R-29 · Proposer anchors do not match the matcher
- where: `crates/dreaming/src/skill.md:81, 92`; `crates/dreaming/src/edits.rs:160-164, 87-128`
- what: the prompt asks for "exact text of the existing bullet"; `apply_one` matches `b.content == a` (the text between `- ` and ` [`); `read_file("skills/learned/<skill>.md")` returns numbered lines by default.
- why: a model quoting `- rule [helpful: 0; …]` or `3\t- rule […]` gets `skipped_unmatched` on every replace/delete — an all-unmatched night per $0.40 cluster.
- fix: normalise the anchor in `apply_one` (strip leading `\d+\t`, `- `, trailing ` […]`); skill: `numbers=False`, and say "anchor = the bullet text without `- ` and metadata".
- test: `apply_one(replace, anchor "- old [helpful: 0; harmful: 0; evidence: ; since: 2026-10-08]")` on bullet `old` → Applied.

### R-30 · `prompt:standing-preferences` clusters have no candidate/record path
- where: `crates/dreaming/src/skill.md:83, 86`; `crates/dreaming/src/edits.rs:218-224, 286-289` (`valid_skill_name` rejects `prompt:standing-preferences`)
- why: steps 3–6 cannot run for the preferences target; `replay::target_system` can render it from a `candidate_text` but the skill does not say how, and `preferences_set` writes without a gate — rung 3 edits bypass §7.2 entirely.
- fix: skill step 3: "for `prompt:standing-preferences` the candidate is the current block + the new line; skip `skill_learned_edit`"; step 6: "accepted → `preferences_set(add)`; rejected → journal". With R-2's server-side gate, `preferences_set(add)` should require a passed gate for inferred lines.
- test: n/a (prose) + a replay test rendering the preferences target.

### R-31 · `run_request` is persisted before `FireWith` succeeds
- where: `crates/dreaming/src/night.rs:893-895, 912-918`
- why: `FireWith` can answer `busy`/`disabled`/`check broken`; the request then stays until some later night's journal clears it, and the next *nightly* fire runs `mode: backlog` with the override budget it was never given (the skill repeats passes).
- fix: write `run_request` after a successful `ask`, or clear it on `Err`.
- test: `RunNow` with no schedule actor → `Err`, `read_state().run_request.is_none()`.

### R-32 · `tidy::stem` byte-slices ids — panic on a non-ASCII knowledge file name
- where: `crates/dreaming/src/tidy.rs:42-43`; `crates/dreaming/src/dry_run.rs:79-80` (ids from arbitrary `.md` paths)
- why: `&id[id.len() - 11..]` inside a multibyte char panics `t dreaming dry-run` and `ledger::candidates` (run by `Status` and every night).
- fix: `if !id.is_ascii() { return id; }` at the top of `stem`.
- test: `stem("facts/café-2026-10-05")` returns.

### R-33 · `engine ≠ self` is enforced only when `home_engine` is configured
- where: `crates/dreaming/src/lib.rs:95, 176-177`; `crates/dreaming/src/sensor_rules.rs:43-48`
- what: `engine_arg` comes from `cfg.home_engine`, not the running engine id; default `None` → no `engine=` arg.
- why: §10 / contract §3.3.2 "refuses when `signals.json.engine ≠ self`"; two mesh engines on one data dir both dream. `config.rs:40` says "null = the engine that installed the routine".
- fix: `install_files(.., home.or(Some(&engine)))`.
- test: `routine_text(&cfg, dir, Some(id))` with `home_engine: None` carries `engine=`.

### R-34 · Clearing the Settings budget field saves $0
- where: `crates/desktop/ui/src/components/settings/Dreaming.tsx:141`; `crates/desktop/ui/src/lib/dreaming.ts:122`; `crates/dreaming/src/night.rs:960`
- what: `onBlur={(e) => change({ kind: 'budget', usd: Number(e.currentTarget.value) })}` — `Number('') === 0` passes `>= 0` on both sides.
- why: select-all → Delete → Tab writes `budget: 0 USD`; every night's first call is refused (silently, see R-8). `DreamNow` guards the same path (`amount > 0`, L279); the pane is inconsistent with itself.
- fix: `if (e.currentTarget.value === '') return` before `change`.
- test: reducer/DOM-less `budgetFromInput('') === null`.

---

## POLISH

### R-35 · `merged` counts rewritten ids, not merges
- where: `crates/dreaming/src/night.rs:782-790`; W1 deviation 3 (one `KnowledgeChanged` per rewritten id)
- what/why: `Merge => counts.merged += 1` per event — a 4-into-1 merge counts 5; the Routines subtitle and journal say "5 merged".
- fix: count distinct `into` (the first event of a merge commit) or events whose id is not a stub; or publish one `op: merge` for the canonical and `op: put` for stubs.

### R-36 · `edits.jsonl`, `state.rejected`, night `phases` and copied desk journals skip the scrubber
- where: `crates/dreaming/src/edits.rs:339, 355-364`; `crates/dreaming/src/night.rs:746-750` (`clip` without `scrub`); `crates/dreaming/src/ledger.rs:104-105`
- why: §10 "the spill redactor runs on every ledger/case/journal write"; a model-written `rationale` with a pasted token lands in `edits.jsonl` and `state.json`.
- fix: `scrub_value` the record before `log`; `scrub(&m.summary)` before `clip`; scrub the desk journal bytes.

### R-37 · Undo on a no-op call still journals and publishes; created ids keep `summary.undone` false; unknown `group` is silent
- where: `crates/dreaming/src/undo.rs:44-50, 147-152, 176-184`
- fix: skip journal/event when nothing was restored and no errors; treat `commit_before == ""` as undone after its error line; `bail!` on a group that selects nothing.

### R-38 · `cases::transition` enforces no state machine; `choice_is_no` misreads "Now"
- where: `crates/dreaming/src/cases.rs:140-153`; `crates/dreaming/src/tools.rs:300-312`; `crates/dreaming/src/night.rs:1023-1026`
- fix: `match (from, to) { (Open|Reopened, Closed) | (Closed, Reopened) | (_, Settled) => {}, _ => bail!() }`; `c == "no" || c.starts_with("no ")`.

### R-39 · `sensor.py` `int()` raises instead of answering `window malformed`; dry-run SQLite open errors become silent zeros
- where: `crates/dreaming/src/sensor.py:23-31`; `crates/dreaming/src/dry_run.rs:98-108, 118-120`
- fix: `try/except ValueError → None`; `tracing::warn!` on `open_ro`/query errors.

### R-40 · Mine/Settle signals the model cannot observe
- where: `crates/dreaming/src/skill.md:27, 55, 103`; `crates/agent/src/dreaming_ledger.rs:86-97, 176-204`
- what: hint-miss needs per-id `knowledge_search`+`knowledge_get` rows (`inner_calls` carries counts only, `hints` lifetime totals); "0 loads in 45 days" / "trigger terms absent 30 days" over a one-day ledger; `L.sessions < min_sessions` with `min_sessions` in neither `L` nor `DreamStatus`.
- fix: drop hint-miss from v1 (or add `opened_after_search` in W2's query); compute the 45/30-day rules in `Night`; put `min_sessions` into `DayLedger`.

### R-41 · `decide_many` recipe gaps
- where: `crates/dreaming/src/skill.md:33, 57`; `crates/decision/src/tool.rs:47-57, 309-311, 476-481`
- what: `Choice` requires `instructions`; the arg is `questions`; `> 2000` rows errors while the ledger carries ≤ 5,000 user messages.
- fix: add `"instructions": "What kind of knowledge entry is this?"`; `decide_many(questions=rows[i:i+2000])`.

### R-42 · `budget exhausted:` should be matched with "contains"; `case_id=c` passes a dict
- where: `crates/dreaming/src/skill.md:13, 84, 88`; `crates/dreaming/src/replay.rs:100-111` (kameo `SendError` wrapped; tool errors are prefixed)
- fix: "an error containing `budget exhausted:`"; `case_id=c["id"]`.

### R-43 · A second `dreaming_ledger()` (backlog pass) rewrites the night file without the pending rewinds
- where: `crates/dreaming/src/ledger.rs:249-251, 296-300`
- fix: keep `ledger/<night>/rewinds.json` from the first call and fold it back, or skip the rewrite when the file exists.

### R-44 · `retitle` stacks `## Summary`; `merge` leaves `hint: false`/`superseded_by` on a previously archived canonical
- where: `crates/knowledge/src/store.rs:1834, 1697-1702`; `crates/knowledge/src/harvest.rs:75-77` (post-136 harvest bodies already start `## Summary`)
- fix: insert under an existing `## Summary`; merge sets `hint: true` and clears `superseded_by`/`valid_until` on the canonical.

### R-45 · `split_title` no-space branch yields 121 chars
- where: `crates/knowledge/src/harvest.rs:67-70` (`unwrap_or(cap)` then `…` appended)
- fix: cut at `TITLE_MAX − 1` chars when there is no space.

### R-46 · Dream now is enabled when it cannot run; `RunNow` ignores `paused_until`
- where: `crates/desktop/ui/src/components/settings/Dreaming.tsx:90`; `crates/dreaming/src/night.rs:886-895`
- fix: `disabled={s?.enabled === false || routine?.enabled === false}`; `RunNow` either refuses while paused or clears the pause (an explicit press is the user's ask).

### R-47 · A hand-edited `config.yaml` never resyncs the routine file
- where: `crates/dreaming/src/lib.rs:134-139` (write only when absent); `update_routine_file` is called only from `SetSettings`
- fix: when the routine exists, `install_files` calls `update_routine_file` (patch in place, `enabled:` preserved).

### R-48 · Three-decimal money (`$1.000`, `$4.100`)
- where: `crates/desktop/ui/src/lib/dreaming.ts:30-32`; W4 deviation 6 (recorded) — design §8 shows `$4.10`.
- fix: `Intl.NumberFormat(undefined, {style:'currency', currency:'USD'})` or a `decimals` option on `formatMoney`.

### R-49 · Unfinished nights are listed forever with `finished_at: null`
- where: `crates/dreaming/src/night.rs:693-719` (`BeginNight` only reopens a *finished* record); `Nights` reads every record
- what/why: with R-8 a night cut off before `journal` never closes; Recent nights shows a perpetual "running" row and `budget_left_tonight_usd` is computed from the previous finished night. Folded into R-8's fix (close on `TurnFinished`); listed so the UI state is checked after it.

---

## Conformance

Legend: ✅ implemented · ⚠️ deviated (as-built named) · ❌ missing / not working · 🔗 see finding.

### Design §13 series (`dreaming.metrics`, `night.rs:1157-1231`)

| Series | State | Note |
|---|---|---|
| Hint open rate (7-day) | ⚠️ unrecorded | lifetime `uses` totals, not a 7-day window |
| Impressions on never-opened ids | ✅ | `never / shown` over lifetime; 🔗 R-28 makes the number meaningless after night 1 |
| Entries hinted with `status: fresh` > 24 h | ❌ | no field in `Metrics` |
| Median hint title length | ✅ | live titles |
| Repeat-correction rate | ✅ (shape) | `reopened / closed` in window; 🔗 R-12: reopen is manual and no counter moves |
| Active knowledge growth vs raw puts | ⚠️ unrecorded | `active_concepts` count only, no ratio |
| Skill loads per 100 sessions | ✅ | `LedgerSince` over `days` |
| Standing preferences block size | ✅ | `preference_lines` |
| Night spend within budget | ✅ (series) · ❌ (enforcement) | 🔗 R-1, R-8, R-27 |
| Undo actions per month | ✅ | nights with `undone` or any touched id undone |
| Attention rows per week | ❌ | no attention subscription (Z §1 "not this wave"); digest never produced |

### Design §16 "done when" and contract seams

| Item | State | Note |
|---|---|---|
| S0 contract committed | ✅ | `136-contract.md` pre-existed the diff |
| S1 §5.1 keys round-trip in `put_for`; unknown keys verbatim | ✅ | `store.rs:813-860`; 🔗 R-23 (set-not-union), R-5 (owner/visibility) |
| S1 `type` vocabulary `record`/`warning` | ✅ | open `type`, harvest template lists them |
| S1 `uses` table durable, `Stats`, `record_shown`/`record_opened` | ✅ | never touched by `sync`; `knowledge_get` records an open — 🔗 R-28 |
| S1 `hint_for` filter + §5.3 rank | ✅ (formula) | `rank_weight` exact; filter after the bm25 cut — 🔗 R-26; age from `timestamp` — 🔗 R-7 |
| S1 `Merge/Supersede/Archive/Verify/Retitle` commands + tools, no delete | ✅ | one commit per op as `dreaming`, stubs, link retarget (index-only, W1 §5); author always `dreaming` (W1 dev 1) |
| S1 `search` marks `[archived]` | ✅ (+ merged/stale/retired, W1 dev 5) | |
| S1 owner-root/unattended predicate on mutating tools | ⚠️ C dev 3 · 🔗 R-6 | `Consolidator` in knowledge; width wrong |
| S2 `HintsShown` → direct `record_shown` once per note | ✅ (contract) | `loop_.rs:5074-5082`, `meeting_diet_tests` |
| S2 `SetBudget` + `Reserved::open` guard + `<budget>` note | ✅ (mechanics) · ❌ (semantics) | 🔗 R-1 lifetime sum, R-8 note unobservable in a one-turn routine, R-22 in-memory |
| S2 side calls refused | ✅ (Z) | `Shared.budget`, `budget_tests` |
| S2 harvest: title ≤ 120 + `## Summary`, `evidence`, `status: fresh`, `record|warning` | ✅ | `harvest.rs:47-80`; 🔗 R-23, R-45 |
| S2 standing-preferences block in `frame::baseline` after the skills index, capped | ✅ | `preferences::block`, `RegisterPrompt`; placement not re-verified by this review |
| S2 `LedgerSince` handler, caps, `truncated`, private excluded, redacted | ✅ | `store/dreaming.rs`; 🔗 R-21 (mailbox scan) |
| S3 `Signals` + `Night` actors, state/ledger/cases/edits files | ✅ | 🔗 R-17/18/19 (signals), R-10/11 (night) |
| S3 tools §9.4 | ⚠️ W3 dev 5–8 | nine tools; three journal sections as protocols; `case_put` takes a draft; 🔗 R-3 (`EditRecord` shape), R-10 (ledger gate) |
| S3 `undo` restores to `commit_before` explicitly, sidecar backups | ✅ (knowledge, sidecars) · ❌ (preferences) | `undo.rs:61`; 🔗 R-9, R-20, R-37 |
| S3 RPC §9.5 nine methods | ✅ | dotted names (C dev 11, kept by Z); `NOT_RUNNING` except status/settings |
| S3 `dry-run` zero writes | ✅ | read-only sqlx, front matter parsed directly; only `-shm` (W3 dev 14); 🔗 R-32 |
| S3 bundled `dreaming.md` routine | ⚠️ C dev 6 | `fire: while_true` + `cooldown: 3h` + `max_runs: 100000`; 🔗 R-14 (`model:`), R-15 (`budget:` render), R-4 (`approvals: false` surface) |
| S3 `sensors/dreaming.py` ↔ Rust table in lockstep | ✅ | 17 rows + python lockstep test; 🔗 R-19 (no staleness), R-17 (no in-flight rule), R-33 (engine only with `home_engine`) |
| S3 `dreaming` skill with recipes + prompts | ⚠️ | judge prompt pinned verbatim; proposer prompt forbids the right things; 🔗 R-2/R-3/R-4/R-12/R-29/R-30/R-40/R-41/R-42 |
| S3 `run_now` → `FireWith` with budget + 4 h backlog timeout + `<request>` | ✅ | 🔗 R-31 |
| S3 `paused_until` enforced | ⚠️ W3 dev 11 | at `BeginNight`, one model turn per fire while paused; 🔗 R-46 |
| S3 `Export{format}` | ⚠️ W3 dev 15 | actor ask only, not reachable by tool/RPC/CLI (design §6 `t dreaming export`) |
| S4 learned sidecar on `load_skill`/`always`/ACP/`LoadSkill` RPC | ✅ | one read `body(dir, meta)`; 🔗 R-24 |
| S4 `skills/retired/` not indexed; `origin: dreaming` last | ✅ | `NOT_SKILLS`, `index()` sort (W2 dev 5: not on `SkillMeta`) |
| S4 an edit targeting the skill file itself refused | ✅ | writer only knows `skills/learned/<name>.md`; `valid_skill_name` |
| S4 retire a dreaming-made skill | ❌ | skill text points at `write_file` + non-existent `delete_file` — 🔗 R-4 |
| S5 Settings › Dreaming states, pickers, Dream now, Undo confirm, proposals | ✅ (fixture) | never met the real RPCs (W4/Z not verified); 🔗 R-14 (resolved default lies), R-34, R-46, R-48 |
| S5 Knowledge badges + "changed by dreaming" filter | ✅ | + `merged`/`retired` (W4 dev 5) |
| S5 Routines row subtitle | ✅ | `DreamingSentence`; 🔗 R-35 counts |
| S5 `Dream*` events through RPC stream and UI union | ✅ (Z) | bridge/client/events.ts/queries.ts |
| S5 mobile layout | ✅ | `WideRow` below `md` |
| S5 Dreaming note per-group Undo buttons | ⚠️ W4 dev 4 | per-night undo only; the note is a document mirror (W3 dev 13) |
| S6 first night on real data at a named budget | ❌ (not this diff) | Z ran the dry run only; no live night (Z §6) |
| §8 `dreaming.enabled: false` removes file/sensor/block/signals | ❌ | 🔗 R-13 |
| §8 weekly digest row, "undo > 2/month → offer to pause" | ❌ | `digest` persisted, nothing produces it |
| §8 preferences auto-promote toggle | ❌ | 🔗 R-16 |
| §4 phase 6 attention rows | ⚠️ Z §1 | `proposals.jsonl` + event only; no inbox rows |
| §9.2 model tier defaults (`tidy` = title model, `judge` other vendor) | ❌ | 🔗 R-14; `propose` unused recorded in Z/W3 |
| §9.4 tools visible only to dreaming + owner roots | ⚠️ C dev 3 | gate inside the tools; 🔗 R-10 one tool ungated |
| §10 secrets scrubbed on cases/ledger/journal | ✅ | `scrub.rs` patterns + redactor; 🔗 R-36 for edits/state/phases |
| §10 `home_engine` / `engine ≠ self` | ⚠️ | 🔗 R-33 |
| Contract §2.6 binding JSON for `Case`/`EditRecord` | ✅ (serialise) · ❌ (deserialise) | test pins the output shape; the input shape the skill prescribes does not parse — 🔗 R-3 |
| Contract §2.7 TS exports | ✅ | 14 `Dream*` + knowledge/agent types; names renamed per C dev 1 |
| Contract §3.1.4 commit author `dreaming` only for `Source::Dreaming` | ⚠️ W1 dev 1 | always `dreaming` |
| Contract §3.3.3 ledger ≤ 2 MB, desk journals, pending rewinds | ✅ | 🔗 R-43 |

## What is good

The seams were cut cleanly: one write lock + `spawn_blocking` in the knowledge store, one commit per op with every rewritten id in it, `undo.rs:61` never calls `restore(id, None)`, the dry run is genuinely zero-write (read-only sqlx, front matter parsed by hand, fingerprint test), the two sensor tables are line-for-line identical and proven so through `python3`, the judge prompt is pinned byte-for-byte, the judge sees only `ideal`+`user_words`+answer, the scrubber covers the token shapes the brief lists and is wired into every case/ledger/journal/preference/proposal write, the `<budget>` guard refuses before any `model_calls` row and now covers side calls, and the ledger handler redacts every free-text field with the right caps. The failures are in the joints — a lifetime sum where a per-fire one was meant, a gate trusted to the model that was supposed to be gated, a tool-surface promise the schedule never kept.
