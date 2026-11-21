# 201 as built — P6a (the state ledger)

D10, §9, §12.1 `ledger` / `ledger_bytes` / `ledger_suggestions`, phase row P6.

## Landed

- **`endless/ledger.rs`** (new, pure): `Section` (Goals, Decisions, Constraints, OpenTasks, Artefacts; lenient `parse`), `State` (open/closed/superseded), `Entry {id "L<n>", section, text, state, unit, created, closed, todo}`, `Suggestion {id "<unit>+1", unit, section, text, created}`, `Artefact {path, action, generation}` (≤ `MAX_ARTEFACTS` 40, newest kept), `Ledger` (JSON). `Op {section, op, text?, id?}` is the tool's argument schema. `Ledger::apply` (add | close | replace (old → superseded, new entry) | accept (suggestion → Goals/Decisions/Constraints, keeps the unit id) | drop (suggestions only)) returns the change note and queues it; `take_notes`; `mirror_todos` (todo-sourced Open tasks follow the list: done/dropped/missing → closed, new → added; model-added tasks untouched); `trim(units, gen, suggestions)` (artefacts from `Kind::Tool` units via `artefact_of`, suggestions from `Kind::User` units via `cue`, lapse after `LAPSE_TRIMS` = 2, notes cleared); `render(budget)` (`<ledger_snapshot>` block; over budget hides, in order: oldest closed tasks, oldest superseded decisions, other closed entries, oldest artefacts; all stay stored); `counts`.
- **`migrations/0072_endless_ledger.sql`**: `endless_ledger(session_id PK, body, updated_at)`; `endless_generations.ledger TEXT` (NULL = off for that generation). Node-local, `SESSIONS_WIRE_SCHEMA` unchanged. Store: `endless_ledger`, `endless_ledger_put`, `GenerationRow.ledger`.
- **`endless/pump.rs`**: `Generation.ledger: Option<String>` (persisted with the frozen view); `Generation::framed()` = ledger block (unmarked) + view blocks. `start_generation` calls `ledger_at_trim` (off or meeting → `None`, nothing read or written; `again` keeps the previous snapshot); it scans units since `Ledger.scanned`, mirrors `store.todos`, lapses, persists, renders within `ledger_bytes`. `ledger_listed`, `ledger_op` (refused without a generation or when the generation has no ledger), `ledger_notes`, `ledger_status`; one global `LEDGER_LOCK` serialises read-modify-write.
- **`endless/mod.rs`**: `pub mod ledger`; `Endless::{ledger_listed, ledger_op, ledger_notes}`; wire: `SessionMemory.ledger: Option<LedgerStatus>`, `LedgerStatus {goals, decisions, constraints, open_tasks, artefacts, suggested, bytes}` (counts are open entries; `bytes` = the frozen snapshot).
- **`endless_ext::ledger_note`** + `units.rs`: a bare `<ledger>…</ledger>` user message is no unit.
- **`loop_/endless_tools.rs`**: `LEDGER` in `is_local`/`specs()`/`run`. **`loop_.rs`**: in `rebase` the `listed` filter drops `ledger` unless `ledger_listed` and not `meeting_filter`; dispatch runs `ledger` without `served`; the three `g.render` frame sites (`endless_tail`, `shrink_view`, `trim_in_turn`) use `g.framed()`; `Turn::ledger_note` persists `<ledger>` notes right after the turn's input (only with `self.view`).

## Deviations

- Suggestions use the deterministic cue list for every profile (P3: `user` lines never reach a model). The standard-profile marker variant is the seam `ledger::marker_of` (strips `[decision]`/`[constraint]`); nothing calls it.
- Suggestions, artefacts and todo mirroring are computed at the trim (deterministic, in `start_generation`), not per tool call. They show in the next snapshot; they produce no notes. Only tool ops produce `<ledger>` notes.
- The tool list follows the generation's frozen switch (`Generation.ledger.is_some()`), so a config change lists/unlists `ledger` at the next trim. Before the first generation the config decides. A pre-P6a generation row has NULL → no ledger until its next trim.
- `ledger` is `Risk::ReadOnly` like zoom/recall/date (it writes only harness memory).
- Entries hidden by the budget are reachable by `recall` only through the `ledger` tool-call units that created them (no separate index).
- `/endless on` (generations cleared) keeps the ledger.

## Seams / unfinished

- `ledger::marker_of` for a later phase that sends `user` units to the standard compactor (prompt sentence not added; `compact_prompt.txt` untouched).
- No loop-level test (real turn with the note row); covered at pump level and by `units::tests::ledger_notes_are_not_units`.
- Bindings: regenerate with `just ts-desktop` (see report for whether it ran). New TS: `LedgerStatus`, `SessionMemory.ledger?: LedgerStatus | null`.
