# 70 TASK-NOTES-HIDDEN — task threads are not notes (as built)

Branch `fix/task-notes-hidden`. Brief: `/tmp/69/omp-TASK-NOTES-HIDDEN.md`. Read alongside `59-documents.md` §4.2/§5.3, `66-attachments.md` §3.2, `67-as-built-T.md`.

Sky: "task notes shouldn't be visible in my list of notes."

## Root cause

`tasks::thread::DocThread::create` asked the document actor `CreateNote`, so every task thread (67 §7.1) was minted `kind: note`, `binding: "note"`, derived `documents/<id>/note.md` — indistinguishable from a user note. Every notes surface keys on that one column: `doc_list('note', 500)` (`useNotes`, shared by `NotesList`, the Notes pop-out, `AttachPicker`, `Composer`, `NotePage`) and `search_notes` (`Store::list(Some(DocKind::Note))`, the `notes` group of the unified `search` RPC). No client-side filter anywhere — the fix is the kind.

## Built

### document crate

- `meta.rs` — `DocKind::TaskThread` (`"task_thread"`), `Binding::TaskThread(task_id)` → kind `TaskThread`, derived `documents/<id>/thread.md`. `DocId::for_binding` gives new threads a v5 id from the task id (one thread per task, mesh-convergent, `Store::create` refuses a second).
- `store.rs`
  - `Store::bind(id, &binding) -> Result<bool>`: `kind`, `binding`, `derived` follow `binding` in `meta.json`, the index row (`UPDATE docs SET kind = ?, binding = ? WHERE id = ?`) and the mirror; the old derived file is renamed to the new path (so `last_export` stays true). `Ok(false)` when already bound so; error when `binding` belongs to another document.
  - `migrate_ids` skips `TaskThread` like `Note`: `tasks.thread` holds the id, so a re-kinded thread keeps its v7 id instead of being converged to `for_binding`.
- `actor.rs` — `Bind { doc, binding }` ask → `Result<bool, String>`: `store.bind`, the open copy's meta refreshed, `DocumentChanged` published (the notes list drops the row live). Re-exported from `document`.
- Rename/DeleteNote gates (`kind != Note` → `title_owned_by_binding` / `delete_via_owner`) now apply to threads, which is the intent: a task owns its thread. `doc_read`/`doc_open`/`doc_markdown` by id are kind-blind — TaskDetail `ThreadDoc`, HUD/Attention, and the agent's `doc_read` are unaffected.

### tasks crate

- `thread.rs` — `Thread::create(task, title, body)` (task id added); `DocThread` asks `Create { binding: Binding::TaskThread(task), … }`. `adopt_threads(pool)`: `SELECT id, thread FROM tasks WHERE thread IS NOT NULL` → `Bind` each; idempotent, logs each re-kind at `info`.
- `lib.rs` — `TasksPlugin::start` spawns `adopt_threads` after the actor registers (document starts before tasks in `engine_plugins`; `Handle::lookup` is per call).
- `ledger.rs` — passes `t.id`; `MemThread` test double updated.

### UI

No code change. `crates/rpc/bindings/{DocKind,Binding}.ts` regenerated (`just ts-desktop`): `DocKind` gains `"task_thread"`, `Binding` gains `{ "task_thread": string }`. `NotesList`'s kind chip (`kind !== 'note'`) is still never shown — threads no longer arrive.

## The repair on Sky's data (dry run on `/tmp/69/dbcopy-notes/`, a `cp` of `~/.theuth/tasks.sqlite` + `~/.theuth/documents/`)

`tasks.sqlite` has 2 tasks, both with a thread; both thread documents are `kind: note` in `documents/index.sqlite` and their `meta.json`:

| task | thread doc | title | before → after |
|---|---|---|---|
| `01a0dd46-8411-7b80-b004-b32259fe054c` (done) | `agqn2ruecf5ybmaewm5oc6i4ee` | Task 01a0dd46: DR pre-drill verification: work checklist A–H and compile action items for Sky | `note` → `task_thread`, binding `{"task_thread":"01a0dd46-…"}`, `note.md` → `thread.md` |
| `01a0dd8a-05d7-7e30-a41f-a50ac09ad0ee` (todo) | `agqn3cqf257dbja7uuj2ypbcvq` | Task 01a0dd8a: Correct stale deleted-branch facts in DR A–H action-items artifact | same |

Per document, at first engine start on this build: `meta.json` rewritten (`kind`, `binding`, `derived`), `UPDATE docs SET kind='task_thread', binding='{"task_thread":"<task id>"}' WHERE id='<doc>'`, `documents/<doc>/note.md` renamed `thread.md`, `mirror/<doc>.meta.json` refreshed, `DocumentChanged` published. Ids do not change. The real notes (`agqn3vifufz4dkeaetb2uqqbqu` "DR Failover Findings"; `agqn4a3v2574dmwckfgtcx42q4` "DR Failover Procedure", created after the first copy), the 3 `meeting_note` docs and the 9 trashed notes are untouched. Notes list after: exactly the real notes.

Nothing else is written; the sweep re-runs at every start and is a no-op once the two rows are bound.

## Verification

- `cargo test -p document` (39 lib incl. new `bind_rekinds_a_note_into_a_task_thread` + `binding_key_and_derived_paths`; 10 integration), `cargo test -p tasks` (51), `cargo check --workspace --tests` (after `just ts` — `nodes` needs `bindings/methods.json`).
- Dry run: throwaway `cargo run -p document --example rekind_dryrun -- /tmp/69/dbcopy-notes` (not committed): both threads `note → task_thread (rebound)`, second run `unchanged`; `list(Note)` = `DR Failover Findings` only.
- Engine smoke: `target/debug/t --headless` on `THEUTH_DATA_DIR=/tmp/69/engine-smoke` (fresh dir seeded with a second copy of `documents/` + `tasks.sqlite`), `THEUTH_RPC__LISTEN=127.0.0.1:7799`. On start the sweep rebound both rows in `index.sqlite`, `meta.json`, `thread.md`. Over RPC: `doc_list('note')` → the 2 real notes; `doc_list('task_thread')` → the 2 threads; `search` notes group for `Task 01a0` → no thread hits; `doc_open(thread)` → `kind: task_thread`; `task_get` → same thread id. UI (`vite --port 5186` against it, headless Chromium): `/notes` shows the 2 notes; `/tasks/01a0dd46-…` THREAD renders the re-kinded document; Composer attach picker Notes group and its search for `Task 01a0dd46` show only the 2 notes.
- UI: `bunx tsc --noEmit`, `bunx vite build`, `CI=1 bunx vitest run` (82 files, 499 tests).

## Deviations / left out

- Design 59 §4.2's kind list is not edited (it already lacks `agent_charter`); this doc is the record.
- `DocThread` creates via `Create` (bound), so a task whose thread creation raced a duplicate would fail with `already document` — one thread per task is the 67 contract.
- Replica peers on an older build see an unknown `task_thread` kind and skip the row in `list`/`read_meta` — existing behaviour for every new kind.
