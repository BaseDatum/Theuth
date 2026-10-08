# 136 — As-built W2 (AGENT)

Status: as-built · 2026-10-08 · worktree `~/Develop/theuth-136-w2`, branch `feat/136-w2`, cut from C's `f99f6d54` · owns `crates/agent/**` only. Everything below is unit-tested in `cargo test -p agent --lib`; the gates and the one pre-existing failure are in §6.

## 0. One screen

- **Budget guard** (`loop_.rs`): `Reserved::open` takes `budget: Option<&Budget>` and refuses before any `model_calls` row is written when `spent + estimate > cap` (`Store::budget_spent`, `store/accounting.rs`). `Turn::call` passes `SessionState.budget`; the refusal is `Abort::Budget` → `Turn::round` queues `SessionState.budget_note` and returns `Round::Stop("end_turn")`; the next turn's `round` persists the note as one user row before its first call. Side calls (title, escalate, `SideCall`) pass `None`: not refused, but counted.
- **Ledger** (`store/dreaming.rs`, `supervisor.rs`): `Store::ledger_since(&LedgerSince, redact) -> (Ledger, hinted ids)` fills every vector but `hints`/`rewinds` from the durable store with the §2.2 caps; `Agents::handle(LedgerSince)` runs it with the vault redactor and asks knowledge `Stats` for the ids the window's `<knowledge>` notes listed.
- **Harvest** (`harvest.rs`, `loop_.rs`): the instruction gets an appended amendment paragraph (verbatim §2); the parsed title is split at 120 chars (`split_title`), `Put.evidence = [session]`, `Put.status = Some(Fresh)`.
- **Impressions**: `record_shown` verified once per `<knowledge>` note with exactly the ids shown (`meeting_diet_tests`).
- **Sidecars** (`skills.rs`): `body(dir, meta)` appends `\n\n## Learned (dreaming)\n` + `<skills>/learned/<name>.md` when non-empty — the one read behind `load_skill`, `UseSkill`, the `always` inliner, ACP materialisation and the `LoadSkill` RPC; `learned/` and `retired/` are not skills; front matter `origin`; `index()` lists `origin: dreaming` skills last (so `prompt_index` does).

## 1. File by file

| File | What |
|---|---|
| `crates/agent/src/store/accounting.rs` | `Store::budget_spent(owner, currency, model) -> (spent, estimate)`: `SUM(returned ?? estimated)` of the owner's rows in `currency` (every status — a dispatched call is committed money); `estimate` = the newest `settled` row of `requested_model = model` (same measure), 0 when none or another currency. Owner's own rows only (`ponytail:` note: subtree if routines grow children). |
| `crates/agent/src/loop_.rs` | `SessionState.budget_note`; `Abort::Budget{spent, cap, currency}`; `Refused{Accounting, Budget}` + `Display`; `budget_note(spent, cap, currency)` (the note text); `Reserved::open(.., budget)` guard; `Turn::call` passes `self.state.budget.as_ref()` and maps `Refused` → `Abort`; `Turn::round` flushes `budget_note` before the pre-call notes and handles `Abort::Budget` (`info!`, queue, `Stop("end_turn")`); `continuation`/the summariser map `Abort::Budget` to an `Err("budget: …")`; harvest `Put` with `evidence`/`status` and `crate::harvest::{instruction, split_title}`; tests `budget_refuses_the_call_that_would_pass_the_cap`; `mod budget_tests` |
| `crates/agent/src/loop_/side.rs` | `Reserved::open(.., None)`, `.map_err(|e| e.to_string())` |
| `crates/agent/src/loop_/budget_tests.rs` | NEW: through a real `Turn` at the fake provider — a session whose spend ($0.25 settled) passes a 1-cent cap: turn 1 ends `end_turn`, no request, note queued; turn 2 (the handed-back state) carries one `<budget spent="250000000" cap="10000000" currency="USD">` row after its input, is refused again, no `model_calls` row either time |
| `crates/agent/src/store/dreaming.rs` | NEW: `Store::ledger_since`, `hinted_ids(note)`; test `ledger_counts_filters_and_scrubs` (two durable sessions + dreaming tree + desk tree + advisor; every vector; redactor), `clip_and_hinted_ids` |
| `crates/agent/src/store.rs` | `pub mod dreaming` |
| `crates/agent/src/supervisor.rs` | `impl Message<LedgerSince>`: `ledger_since` with the vault redactor, then knowledge `Stats` → `hints` |
| `crates/agent/src/harvest.rs` | NEW: `TITLE_MAX = 120`, `AMENDMENT`, `instruction(related, cwd)`, `split_title(title, body)`; tests |
| `crates/agent/src/lib.rs` | `pub mod harvest` |
| `crates/agent/src/loop_/vision_fallback_tests.rs` | `HARVEST_REPLY` is now a `type: record` with a ≈400-char title (the fixture every detached-harvest scenario uses) |
| `crates/agent/src/loop_/harvest_compact_tests.rs` | scenario A asserts the landed concept: `type: record`, title `Detached harvest landed on 2026-10-08`, `status: fresh`, `evidence = [session]`, body starts `Summary: The harvest instruction was answered…` |
| `crates/agent/src/loop_/meeting_diet_tests.rs` | after each run: `knowledge.stats(ids)` — `[1, 1, 1]` after the first note, sum `3 + 3 + 2` after the `Some(2)` run, unchanged after `Some(0)` (no note → no impression) |
| `crates/agent/src/skills.rs` | `NOT_SKILLS`, `LEARNED_HEADING`, `learned_path(dir, name)`; `FrontMatter.origin`; `index()` drops `learned/`/`retired/` files and sorts `origin: dreaming` last; `body(dir, meta)` appends the sidecar; `load`/`load_text`/`always_bodies(dir, skills)`/`materialise` thread `dir`; test `learned_sidecar_rides_every_read_and_dreaming_dirs_are_not_skills` |
| `crates/agent/src/frame.rs` | `always_bodies(&skills_dir, &skills)` |

## 2. The harvest prompt diff (verbatim)

`knowledge::harvest::HARVEST_INSTRUCTION` is in W1's crate and is **unchanged** by W2. The agent appends, after the rendered instruction and a blank line (`crate::harvest::instruction`):

```
Titles are at most 120 characters: the first clause only; the statement itself goes in the body.
`type` may also be `record` (what happened on a date — a landing, a machine's state that morning,
an incident) or `warning` (a one-line never/always whose title is the whole payload).
```

So the model sees the old template (`type: fact            # fact | entity | decision | procedure | project`, `title: one line`) followed by the paragraph above. Hand-off (§4) folds it into the template itself.

Parser side (agent, `harvest_frame` → `crate::harvest::split_title`): a title ≤ 120 chars passes; a longer one is cut at the last `. `, `; `, `: `, ` — ` or ` – ` inside the first 120 chars (trailing `.;:` trimmed from the title); the rest becomes `Summary: <rest>` as the body's first paragraph (`\n\n` before the original body, or alone when the body is empty). No clause boundary → cut at the last space before 120 chars, `…` appended, and the **whole** title becomes the Summary paragraph (a fragment in the body would read wrong). Then `Put { evidence: vec![session.to_string()], status: Some(Status::Fresh), .. }`. `type` is passed through as written (`record`/`warning` accepted because `Put.type_` is open).

## 3. Budget-guard semantics

- Where: `Reserved::open` — the one seam every native model call of the agent crate goes through (51 §6.1). The check runs before `reserve_call`/`reserve_journaled_call`, so a refused call leaves **no** `model_calls` row and no `turn_steps(model_call)` step.
- What is compared: `spent = Σ (returned_nanos ?? estimated_nanos)` over the owner's `model_calls` rows whose money is in the budget's currency (other currencies ignored; a row with `returned` in another currency counts 0 — a returned amount is never replaced by an estimate); `estimate` = the money of the newest `settled` attempt of the same `requested_model`, 0 when there is none (so the first call always goes out, and from then on a call is refused when the last one's cost would not fit). Refuse iff `spent + estimate > cap`.
- Who is checked: round, harvest and compaction calls made through `Turn::call` (they read `SessionState.budget`, which the running turn owns). Detached side calls (`Side::call`: title, escalate, `SideCall` — W3's `replay_case` with `CallKind::Dream` is one) are **not refused** but their cost lands in `model_calls` and is counted by the next check. Ceiling: a budget can be overshot by one side call.
- What happens: `Turn::round` logs `budget spent; turn ends`, stores the note on `SessionState.budget_note` and returns `Round::Stop("end_turn")` — `TurnFinished{stop_reason: "end_turn"}`, no error row. Harvest/compaction refusals surface as `Err("budget: spent … of … <CUR> nanos")` through their existing error paths (a harvest is logged and skipped; an auto-compaction logs `compaction: budget: …`).
- The note: the next turn's `round`, before the context-pressure/todo/knowledge notes, persists one user row

  ```
  <budget spent="250000000" cap="10000000" currency="USD">
  This session's money budget is spent: 0.25 USD of a 0.01 USD cap. The last model call was refused and the turn ended; further calls are refused until the budget is raised. Finish with what is already written.
  </budget>
  ```

  then makes its call — which, with the same budget, is refused again (another note is queued, one per turn). The model actually reads the note only once the cap is raised (`SetBudget` with more nanos or `None`) or the session's spend no longer counts (a new session: `memory: clear` routines start at zero).
- `SetBudget` (C): applied via `SetBudgetNow` to `SessionState.budget` (idle) or `Session.pending_budget` (mid-turn, applied at `TurnDone`). The schedule plugin sends it before `SendInput` on every fire, so a fire is covered from its first call.
- Unknown money: a model with no catalog rates and a provider that returns no cost settles with `unknown_reason` and no nanos — it never counts. A budget only bites on priced calls.

## 4. Ledger queries (`Store::ledger_since`)

Bounds `since`/`until` are re-rendered through `chrono` to the store's own RFC 3339 shape (`Utc::now().to_rfc3339()`, `+00:00`) so SQL text comparison orders like instants. Window = `[since, until)`.

| Vector | Query | Notes |
|---|---|---|
| `sessions` | `SELECT id, parent, desk, unattended, kind, created_at, updated_at, title, finished_at FROM sessions` (every row — the lineage walk needs parents outside the window) | listed when `created_at < until ∧ (created_at ≥ since ∨ updated_at ≥ since ∨ finished_at ≥ since)`; `kind = 'advisor'` never; a session with any ancestor in `exclude` never; `owner_only` drops every tree whose root has `desk`; `kind` = child (has parent) · desk (root with `desk`) · routine (root `unattended`) · root; sorted by `created_at, id`; past 500 → truncated, and the dropped sessions' rows vanish from every other vector (prefix semantics) |
| `turns`, `stops` | `turn_steps WHERE kind = 'turn_finished' AND at ∈ window` | `payload.stop_reason ≠ "end_turn"` → `stops` |
| `tool_calls`, `errors`, `tool_errors`, `skill_loads` (direct) | `tool_calls WHERE COALESCE(finished_at, started_at) ∈ window` | planned-never-started rows have no time and are not counted; `is_error` → `tool_errors` (first non-blank output line, redacted, ≤ 300 chars; past 2,000 → truncated); `name = load_skill` → `skill_loads[args.name]` |
| `inner_calls`, `skill_loads` (nested) | `inner_calls WHERE at ∈ window` | by `name`: calls, errors, distinct sessions; `load_skill` → `skill_loads[args.name]` too |
| `model_cost_nanos` | `model_calls WHERE created_at ∈ window` | `returned ?? estimated`, any currency summed (a session is one currency in practice) |
| `questions` | `questions WHERE asked_at ∈ window` | `answer`: a JSON string (`"deny"`) unwrapped, other JSON kept as text; redacted, ≤ 300 chars |
| `user_messages`, `app_presses`, hinted ids | `messages WHERE at ∈ window ORDER BY at, seq`, body decoded as `ChatMessage` | role `User`; text parts only (`ContentPart::Text`), trimmed; `<knowledge>…` → ids from its `` - `id` `` lines (distinct, in order) for `hints`; `[app ` → `app_presses`; any other leading `<` or `[` skipped; else `user_messages` (redacted, ≤ 2 KB on a char boundary; past 5,000 → truncated) |
| `hints` | supervisor: knowledge `Stats{ids}` → `LedgerHintUse{id, shown, opened}` | knowledge actor absent or erroring → empty, warned |
| `rewinds` | — | always empty: the corrected tail is deleted at the next write (29) and `resume_reports` holds interruptions, not rewinds; dreaming's `Signals` snapshots rewinds at event time (W3) |

Private sessions: never in the durable store (51 §4.5, `:memory:` per private root) — the handler reads `self.store` only, so they cannot appear. The fixture test therefore covers the filters that *can* drop a durable row (`exclude` tree, `owner_only`, advisor) rather than a private row.

## 5. Deviations from the contract / brief (and why)

1. **`HARVEST_INSTRUCTION` and `parse_harvest` not edited** — they live in `crates/knowledge/src/harvest.rs` (W1's crate; contract §1 lists the harvest prompt under W2 but the file under W1). Implemented as an agent-side appended paragraph + agent-side title split/`evidence`/`status` at the `Put`. Hand-off below to fold the amendment into the template.
2. **`Reserved::open` returns `Result<Self, Refused>`** (was `String`): a budget refusal must end the turn `end_turn`, an accounting refusal is still an error. `Side::call` stringifies.
3. **Side calls are not refused** (see §3): the budget lives on `SessionState`, which the turn owns; threading it into `Shared` would move C's field. Counted, not blocked. A one-call overshoot is the ceiling.
4. **Estimate = last settled same-model cost**, not a token forecast: nothing knows the prompt's price before the provider bills it; the previous call is the best available predictor and is zero for the first call.
5. **`origin` is not on `SkillMeta`**: adding a field breaks two `SkillMeta {` test literals outside the agent crate (`crates/tui/src/model.rs:15920`, `crates/slack/src/ui.rs:876`). The ordering is done in `index()` instead (dreaming-made skills after the others), which `prompt_index` preserves — and `ListSkills` shows the same order.
6. **The sidecar is appended in `body(dir, meta)`**, the single read behind all three contracted sites — so the `LoadSkill` RPC (UI skill viewer) shows the learned block too. `body`/`always_bodies` gained a `dir` parameter (`frame.rs` updated).
7. **`rewinds` empty** — see §4.
8. **The `<budget>` note is a raw user row** (`<budget spent cap currency>` with attributes), not `Turn::note(kind, text)` (which renders `<kind>\n…\n</kind>` without attributes).

## 6. Verified / not verified

Verified (gates in §7).

Not verified: the engine running with a `budget:` routine end to end (no `t` against `~/.theuth`); the ledger over the live 829 MB store (fixture only — the `messages` scan decodes every windowed row's `ChatMessage`; a heavy day is thousands of rows, fine for a nightly job, but a window of weeks would be slow: `ponytail:` add `json_extract(body, '$.role')` pre-filtering if Z's dry run shows it); knowledge `Stats` through the real actor from the handler (unit-tested at the store seam; `meeting_diet_tests` covers `record_shown` → `stats` through the real actor).

## 7. Gates (through `build-lock`, target `~/Develop/theuth-136-target/W2`)

- `cargo test -p agent --lib --no-fail-fast`: **470 passed, 1 failed, 2 ignored** — the failure is the pre-existing `loop_::tests::prompt_parts_in_order_with_blank_lines` (C's hand-off, not 136). Run three times: runs 2 and 3 as stated; run 1 died with `SIGSEGV` inside the parallel test binary after 364 passes (the last tests started were `store::tests::*` / `supervisor::tests::*`, none of W2's), not reproduced in the two following runs — noted for Z as a flake to watch, not attributed.
- `cargo clippy -p agent -- -D warnings` and `cargo clippy -p agent --tests -- -D warnings`: green.
- `cargo fmt -p agent --check`: green.
- `cargo check -p agent --tests`: green. No workspace-class run (none allowed to W2); `crates/tui` / `crates/slack` `SkillMeta {` literals untouched and unaffected (no field added).

## 8. Hand-offs

- **W1 / Z (knowledge)**: fold the §2 amendment into `HARVEST_INSTRUCTION` itself — `type: fact            # fact | entity | decision | procedure | project | record | warning` and `title: one line, at most 120 characters (the first clause; the statement goes in the body)` — then delete `crate::harvest::AMENDMENT` and make `crate::harvest::instruction` a plain re-export (or move `split_title` into `knowledge::harvest::parse_harvest` and drop `crates/agent/src/harvest.rs`). Keep the first line of `HARVEST_INSTRUCTION` unchanged: two agent tests key on it.
- **W1**: `retitle` prepends "a Summary paragraph with the old title text"; the harvest's split writes `Summary: <rest>` as the first paragraph — align the two shapes (one of them should move).
- **W3**: `replay_case`'s model call under the routine's owner is counted but not refused by the guard (deviation 3); the skill should stop passes when a `<budget>` note arrives, as the contract already says. `dreaming_ledger` gets `rewinds: []` from the agent — fold `ledger/pending/*` as planned.
- **W3**: `LedgerSince.exclude` should list the dreaming session only — descendants are dropped by lineage here.
- **Z**: `cargo test -p agent --lib` has the pre-existing failure `loop_::tests::prompt_parts_in_order_with_blank_lines` (C's hand-off; not 136).
- **Z**: the two `SkillMeta {` literals outside `crates/agent` are untouched and still compile (no field added) — nothing to do; noted so the `origin`-on-`SkillMeta` idea is not re-opened without fixing them.
