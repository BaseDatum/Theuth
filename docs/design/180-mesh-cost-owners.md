# 180 MESH-COST-OWNERS — a taken session's calls need their ledger owner on the taker

Status: Phase 1 (the fix) as built on `fix/mesh-cost-owners`, after the code-level correction of 17:33 (it replaced
the brief's D1). Working notes and live proof: `180-notes.md`.

## Report

Sky paired a new machine (xeus) with Mac A. A session S created on A showed up on xeus; sending in it on xeus failed
with `model: accounting: error returned from database: (code: 787) FOREIGN KEY constraint failed` under an "owner
changed" card; sending from A showed the same; so did "Continue here". Read-only field evidence: xeus had no
`cost_owners` row for S (A had one, `hlc` NULL), lacked ~97 more of A's rows, and held thousands of orphan
`model_calls` / `remote_meters` and sessions without ledger rows.

## Cause

- `model_calls.owner_id REFERENCES cost_owners(id)`. `model_calls`, `remote_meters`, `errors` and `sessions` ride
  the session log (`capture::ATTACHED`, `sessionsync`); `cost_owners` rides the stage-1 **record replica**
  (`theuth/records/1`, 68 decision 8) — a second, independent stream. Post-attach inserts are captured by its
  triggers; `hlc` NULL in the table is by design (the writers never stamp), so "hlc NULL = never sent" was wrong.
- A newcomer gets A's session rows in seconds, but A's record history is **replayed** batch by batch, oldest first
  (an engine that ran alone never prunes its outbox, so no snapshot is sent). A session's owner row lands when the
  replay reaches it. In between, any send takes the session, and the taker's next `INSERT INTO model_calls` fails
  the FK; the calls that did replicate land as orphans (`owner_wins` answers `SQLITE_CHANGESET_FOREIGN_KEY` with
  `OMIT`).
- Reproduced word for word on two scratch engines (`180-notes.md`, Phase A run 4): FK on B 1 s after pairing, six
  orphan calls, S's owner row landing 76 s later when the replay finished. In that run none of the three
  receive-side failures the correction suspected fired (the row was late, not lost); they stay real code-level risks
  and D1b removes them. [inference] The field's ~97 missing rows were rows xeus' replay of A's far longer history had
  not reached; the field stores were not readable from here.
- What A showed: the taker's FK error rows replicate into S's transcript (`errors` is attached), so A displays B's
  error as if it were its own. `errors` carries no engine column; naming the producing engine on the card needs a
  schema change and is not done here.

## Decisions as built

| | Decision | As built |
|---|---|---|
| D1a | The taker derives the owner locally (core fix, no replica dependency) | `accounting::ensure_owner_chain_in(conn, id)`: walks `sessions.parent` root first and `INSERT OR IGNORE`s each missing ledger row with `history_coverage = 'unknown'`, no bump, and `hlc = STAND_IN_HLC` (`0000000000000000-00000000-0`, it parses and sorts below every real stamp). A parent with neither a `sessions` nor a ledger row ends the chain (its child becomes a root). Called in `reserve_call_in` before the insert (both `reserve_call` and `reserve_journaled_call`), in `ensure_owner_in` for a missing parent (a subagent spawned in a taken session), and before the `remote_meters` and `remote_turns` writes. An id with no `sessions` row writes nothing and is refused: `session <id> has no row here` (the loop shows `model: accounting: session <id> has no row here`). |
| — | Stand-ins and the record replica | The stand-in is captured by the record replica's triggers like any row, but under per-column LWW the sentinel stamp loses everywhere: the origin keeps its row, and the origin's row overwrites the stand-in wherever it lands (`sentinel_stand_in_never_overwrites_the_origin_row`). Later revision bumps on the taker carry real stamps and win the `revision` column only, as any cross-engine bump always did. |
| D1b | Record replica robustness | `Engine::apply` applies each table in its own transaction under a `SAVEPOINT` on the bookkeeping: a table that cannot be written is rolled back (rows and shadow clocks) and its changes are **parked** (`replica.sqlite` `parked`, migration 0003) — the batch is recorded so the stream moves on, but nothing that could apply later is dropped: parked changes are retried (LWW makes a late apply safe) after every later batch touching the table and when the table attaches, and removed once they land. Before, the first failing table aborted the batch, ended the stream and failed again on every reconnect, starving every table behind it (`cost_owners` sorts after `agents`/`availability`). Child before parent: agent migration 0052 adds `BEFORE INSERT` / `BEFORE UPDATE OF parent_id` triggers on `cost_owners` that give a missing parent a stand-in root (`INSERT OR IGNORE`, sentinel stamp), so a child the replica applies first lands and the parent's real row overwrites the stand-in in place. Stream ends log at warn; per-batch problems (unattached table, rows skipped, table parked) warn once per `(origin, table)`, then debug. Not parked, by choice: changes of a table this engine never attached (it would park every batch of a disabled plugin forever) and partial rows of a pk never received (no clock is set, so the full row lands whenever it comes). |
| — | No cascade | Every write to `cost_owners` is `INSERT OR IGNORE` or an `UPDATE` in place (record apply: `UPDATE`, then `INSERT` only when absent); nothing deletes-and-inserts it, so `remote_turns … ON DELETE CASCADE` never fires (`stand_in_replaced_by_the_real_row_keeps_remote_turns`). `INSERT OR REPLACE INTO remote_turns` replaces a `remote_turns` row only; no table references `remote_turns`. |
| — | Coverage | Stand-ins and repaired roots are `unknown` but carry the sentinel stamp, and the costs page's H3 count (`cost_pre_ledger_sessions`) excludes them: they are owners whose row has not arrived, not sessions without usage data. The real row brings the origin's coverage and stamp. |
| D2 | A call never fails on a missing owner | D1a. |
| D3 | Boot repair, idempotent | `Store::open` runs `accounting::repair_owners_in`: a D1a stand-in for every `sessions` row without a ledger row (lineage root first), then a tombstoned root (`deleted_at` set, `unknown`, sentinel stamp) for every other id `model_calls`, `remote_meters`, `remote_turns` or a ledger row's `parent_id` names — no reference dangles. One info line when it wrote anything: `accounting: repaired N missing cost owner(s): D derived from sessions, T tombstoned root(s)`. Calls that had no owner were invisible to the cost cube's join; after the repair they count. |
| D4 | Replicated real rows win | By the sentinel stamp (record replica). The session log never carries ledger rows, and the changeset apply (FK `OMIT`) is unchanged. |
| D5 | Continue here works both ways | The taker derives (D1a); the origin always has its own row, untouched by anything the taker did (`origin_retake_keeps_its_row`, via log and via snapshot). Live in Phase C. |
| D6 | No change for a single engine | A healthy store has no session without a ledger row and no orphan (this Mac's store, read-only: 0 and 0): the repair writes nothing and `ensure_owner_chain_in` returns on its first lookup. Owners are added, never calls. |

## Tests

- agent: `store::accounting::tests::unknown_session_still_refuses`, `store::accounting::tests::repair_backfills_orphans`
  (empty `foreign_key_check`, the cube's join counts every call, H3 counts none of the repaired owners, a rerun is a
  no-op), `store::accounting::tests::record_child_owner_before_its_parent_still_lands`,
  `store::accounting::tests::stand_in_replaced_by_the_real_row_keeps_remote_turns`,
  `loop_::tests::removed_owner_row_still_reaches_the_transport_once`.
- sessionsync `owner::tests` (two stores, session log only — no record replica, the bug's precondition):
  `taken_session_reserves_without_replicated_owner` (both directions), `taken_child_chain_root_first`,
  `origin_retake_keeps_its_row`, `origin_retake_keeps_its_row_after_snapshot`.
- replica: `tests/sync_loopback.rs::sentinel_stand_in_never_overwrites_the_origin_row`,
  `tests::apply_with_one_failing_table_still_lands_cost_owners` (parked, then retried and landed).

## Live proof

Two scratch engines on loopback, the Phase A steps on the corrected build (`180-notes.md`, Phase C): B derives the
stand-in on its first call while its record replay is still running; send on B, send from A and Continue here on
either side all get the fake's reply; `foreign_key_check` is empty on both throughout; when the replay reaches S,
A's real row replaces B's stand-in and A's own row never moves. A store seeded with xeus-like orphans logs
`accounting: repaired 4 missing cost owner(s): 1 derived from sessions, 3 tombstoned root(s)` at start and checks
clean; the next start logs nothing.
