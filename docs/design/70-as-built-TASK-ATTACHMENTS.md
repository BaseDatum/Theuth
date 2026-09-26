# 70 TASK-ATTACHMENTS — attach notes, artifacts, chats, meetings, knowledge to a task (as built)

Branch `feat/task-attachments`. Brief: `/tmp/69/omp-TASK-ATTACHMENTS.md`. Sky: "just like I can attach notes and artifacts and anything to a session I should be able to also attach to tasks". Read alongside `66-attachments.md` (the reference, marker grammar, picker, chips) and `69-as-built-TASKS.md` / `70-as-built-DESK-PROTOCOL.md` (the task record, `done_when`, the wake text).

## Decisions

| Question | Built |
|---|---|
| Shape | one column `tasks.attachments_json`, a JSON array of design 66's ref `{kind, id, title, meta}` — the Rust `agent::attach::AttachRef` reused as-is (not a tasks-local copy), now `JsonSchema` + `ts(export)` (`AttachRef.ts`, `AttachKind.ts`; the enum is renamed on the wire because `artifact::Kind` already owns `Kind.ts`). `title` gained `#[serde(default)]` so a model may omit it (`{kind, id, title?}`); renderers fall back to the id. `external_json` untouched (links belong in the brief). |
| Replace vs attach/detach | **replace whole** (`TaskPatch.attachments: Option<Vec<AttachRef>>`), the `blockers`/`external` pattern; the ledger diffs old vs new and writes one `attached` / one `detached` event per changed ref, so the thread reads as if there were verbs. No `attach`/`detach` tools: one more field on `task_update` is less for the model to learn than two tools. |
| Dedupe | by `(kind, id)`, first wins, blank ids dropped — `ledger::dedupe_attachments`, applied on create and update. |
| How the desk sees them | `[attached <kind> <id> "<title>" · <meta>]` lines (the 66 §5.1 grammar, via `agent::attach::marker`) under the brief in the wake text and under the JSON in `task_checkout`, so the model uses the tools the `attachments` prompt block already teaches. |
| Where the picker lives on a task | the task detail page has an **Attachments** section (chips + `+ Attach…` + drop target) — there is no separate "edit brief" surface in this UI (the brief is read-only `Streamdown`), so that section *is* the edit surface. The create sheet has the same editor as a field. |
| Context refs in `task_draft` | the tool reads the calling session's latest user turn (`agent::ReadSession`) and unions its `[attached …]` refs into every draft (`draft::offer`); the card shows **Attached:** … and the tool result tells the model to pass `attachments` to `task_create`. |

## tasks crate

- `migrations/0005_attachments.sql` — `ALTER TABLE tasks ADD COLUMN attachments_json TEXT NOT NULL DEFAULT '[]'`. Replication needs nothing else: `replica::attach` rebuilds `_replica_ai/au/ad_tasks` from `PRAGMA table_info` on every attach; the HLC is the row's (`Store::put` rewrites every column with `hlc(updated_ms)`).
- `store.rs` — `Row.attachments_json`, `From<Row>` (`unwrap_or_default` on bad JSON), `COLS`, `put` (?24).
- `types.rs` — `Task.attachments: Vec<AttachRef>` (`serde(default)`), `TaskCreate.attachments` (default), `TaskPatch.attachments: Option<Vec<AttachRef>>`.
- `ledger.rs` — `attachment_label(ref)` = `<kind> "<title>"`; `dedupe_attachments`; `create` dedupes; `update` diffs and pushes `("detached", None, label)` then `("attached", None, label)` into `changes` → `task_events` rows + thread lines `- <stamp> **actor** attached: note "Groceries"`. Test `attachments_round_trip_and_events` (sqlite round trip, dedupe, event order, thread lines, no-op patch writes nothing).
- `tools.rs` — `task_create.attachments`, `task_update.attachments` (replace whole); `task_inbox` appends ` — attached: note "X", artifact "Y"`; `task_checkout` appends `--- attachments ---` + marker lines and says so in its description.

## roster / app

- `roster/src/desk.rs` `wake_text` — after `External:` a block `Attached (read with the tool for the kind, see Attachments):` + one marker line per ref; `wake_text_snapshot` extended with a note ref.
- `app/src/draft.rs` — `Draft.attachments`; `source()` renders `**Attached:** note "X" · artifact "Y"` under the brief; `TaskDraftTool::call` unions the current turn's refs (`turn_attachments` → `ReadSession` → last `HistoryEntry::User` → `attach::parse`); description and result text name `attachments`; structured result carries them. Test `drafts_carry_attachments`.
- Struct-literal additions `attachments: vec![]` in `roster/{attention,desk,lib,proposals}.rs`, `signal/channel.rs`, `slack/channel.rs`, `tui/model.rs`, `tasks/{ledger,place,types}.rs`.

## UI (`crates/desktop/ui`)

- `lib/attach.ts` — `droppedRef(dataTransfer)` (parse a `DRAG_TYPE` drop, validate kind/id, `null` otherwise) and `addRefs(current, added)` (merge by `kind:id`). The composer keeps its inline parse (Composer.tsx is the other worker's file).
- `components/chat/AttachChip.tsx` — `popoutOf(ref): PopoutTarget | null` (note/artifact/chat/meeting; knowledge has no popped route); the popover gains **Pop out** when `canPopout()` and a target exists (both composer and task chips get it); new `onRemove` prop puts a × on the pill.
- `components/tasks/Attachments.tsx` (new) — `AttachmentsEditor { value, onChange, disabled }`: chips with ×, `+ Attach…` anchoring the composer's `AttachPicker` (`session={null}`, same kinds and search), drop target for sidebar/notes/artifacts/meetings rows (`dragSource`); `AttachmentChips`; `AttachmentCount` (paperclip + N for the board card).
- `TaskDetail.tsx` — **Attachments** section under Brief → `task_update {attachments}`; events with `before === null` (attached/detached/created) render `after` only.
- `NewTaskSheet.tsx` — Attachments field, sent in `task_create`.
- `Board.tsx` `TaskCard` — `<AttachmentCount>` badge in the meta row.
- Fixtures: `tasks/mock.ts`, `tasks/helpers.test.ts`, `hud/model.test.ts` gain `attachments: []`.
- Tests: `components/tasks/Attachments.test.ts` — chips (title, kind label, × only when editable), count badge, `addRefs`/`droppedRef` (merge, foreign/invalid drops), `popoutOf`.
- Mobile: the editor is a flex-wrap row; the picker is the composer's (bottom sheet under 768 px); the pill × is 24 px under a coarse pointer.

## Verification

See `/tmp/69/report-TASK-ATTACHMENTS.md`.

## Deviations / not built

- No `attach`/`detach` tools (replace-whole on `task_update`, see Decisions).
- `task_draft` unions context refs into every draft; a model cannot opt a draft out of them (the user removes on the task afterwards). Cheap to add a `no_context: true` flag if it ever matters.
- Browser smoke ran against a scratch engine built from this branch (`target/debug/t --headless`, own data dir), not the user's live engine: the live binary predates the column, would drop `attachments` on the floor and return tasks without the field.
