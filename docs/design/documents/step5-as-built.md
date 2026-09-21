# 59 — As built, step 5 (2026-09-21): markdown artifacts as living documents; one note per meeting

Step 5 of `59-documents.md` §14 on `feature/documents-owners` (worktree off
`feature/documents` 97df6b2 = steps 1–3). Commits: 3d1fad5 (artifact +
document), 8a60f34 (helper `move` + `doc_move`), ace9495 (meeting +
document), 35bc386 (one-line `export` in `notes/NotePage.tsx`, outside the
allow-list), 00ac619 (desktop: artifact page), 3a862d0 (desktop: meeting
panes), 2d57229 (fix: import entry lifetime), 494bf56 (fix from the smoke).
Not merged, not pushed. §17–§19 deviations stand; where §7.2/§7.4 and this
file differ, this is what exists. Written to its own file because another
omp edits `59-documents.md` in a sibling worktree.

## 1. What was built

### 1.1 Artifacts (§7.2)

- `Binding::derived`: `Artifact{..}` → `documents/<id>/artifact.md`,
  `MeetingNote(m)` → `meetings/<m>/notes.md` (were empty strings).
- `artifact::State::show` and `::commit`, `Kind::Markdown`, durable store
  only: after the row insert, `bind_document` — `Lookup(Binding::Artifact
  {session, name})`; `Some(doc)` → `ImportMarkdown{Diff, Principal::File}`,
  `None` → `Ensure` (mode `replace`) — then `Snapshot{"artifact-v<N>"}`.
  Failures are logged; the record stands without a document. A private tree
  (51 §4.5) never binds: its rows die with the process, documents do not
  (`State::binds_documents` = the owner lock is held).
- `ArtifactActor` subscribes to `document::Exported`; for an `Artifact`
  binding it reads the export and calls `State::edit_version(session, name,
  bytes)`: `Ok(None)` when the lineage is not markdown or the bytes equal the
  latest version's blob; else a new content-addressed blob
  `artifacts/<sha16>.md`, a row with `call_id = "edit:<unix ms>-<sha16>"`,
  `origin = None`, version `COUNT+1`, title from the text's own heading else
  the latest version's, `ArtifactShown` published (lists refresh, the chat
  panel's latest mode adopts it), then `Snapshot{"artifact-v<N+1>"}`.
  `Export`/`export_artifact` of that id copies the edited text.
- `ArtifactActor::new(stores)` for tests.
- Desktop `artifacts/Viewer.tsx`: for a `markdown` record, `doc_resolve
  (Binding::Artifact{session, name})`; when a document exists the body is
  `useDocument({doc}) → Live` (the BlockNote editor) instead of the static
  render; actions gain **Attach to chat…** (`ChatPicker`, `ref` = the DocId)
  and **Snapshots…** (`Snapshots` from `notes/NotePage.tsx`, now exported:
  `doc_snapshots` + Restore = `doc_restore`). The version select, Reveal,
  Open chat, the menu (Download) and the details rail are unchanged; other
  kinds and markdown lineages with no document render as before. The chat
  card already opens `/artifacts/$callId`.

### 1.2 Helper and tools

- `packages/documents` protocol **59.5**: `Edit` gains `{op: "move", blocks,
  after?|before?|at?}` — `take` + `place` in the one transaction; ids kept;
  the moved ids are **not** in `touched` (nothing was authored, so the
  attribution map and the reveal leave them alone); undo = `restore` at the
  original anchor. `EditResult` unchanged (`inserted`/`removed` empty).
- Rust `BlockEdit::Move`; tool `doc_move(doc, blocks, after|before|at) →
  {moved: [id]}` (`Mutating`, one of the three anchors required, the anchor
  must not be a moved block). The prompt block names it and now says
  "meeting notes" among the document kinds.

### 1.3 Document crate (additive)

- `Create{binding, title, markdown, by} → DocSummary`: a document bound to
  any binding, headed `# <title>` (`with_heading`), body as one `replace`
  import stamped `by`. `CreateNote` shares `create_doc`.
- `SetDerived{doc, derived}`: meta + index, the old derived file removed,
  the new path exported before the reply (`export()` takes an optional
  completion sender).
- `Restore` imports as `Principal::Owner{device: None}`, not `File`: a
  restore changes the document, so the derived file must follow it (File
  imports never export — a restored knowledge concept left its file stale
  since step 1). For artifacts a restore is therefore an edit version.
- `Import` entries live until both the helper's reply and its `ev: update`
  arrived (`done_at`, `applied`); a replied import whose update never comes
  (a no-op diff) is forgotten after 10 s on `Tick`; an update matching no
  import or edit warns. Before: the update could land after the reply had
  removed the entry, be stamped `Helper` and schedule an export the `File`
  import never should — that was §17's "on-demand creation exports once",
  and in this step it made a second `show` write `artifact.md`.

### 1.4 Meetings (§7.4)

- `Manifest.note: Option<DocId>` (`#[serde(default)]`, ts-rs; `None` for
  meetings recorded before this). `start_task` creates the note right after
  the meeting directory: `Create{binding: MeetingNote(id), title: <user
  title or "Meeting <date>">, by: Owner}` → `notes.md` from the first export.
  Documents off or refused → `None`, the meeting goes on as before.
- `crates/meeting/src/note.rs`: `summary_form(doc)` (the §7.4 contract,
  starts with `SUMMARY_HEAD`), `create`, `snapshot`,
  `restore_before_summary` (newest `meeting-before-summary` → `Restore`),
  `markdown`, `set_summary_path` (`SetDerived` → `meetings/<id>/summary.md`),
  labels `BEFORE_SUMMARY`/`SUMMARY`.
- `Summarize`: with a note, `Snapshot{"meeting-before-summary"}` then the
  note form; without, the legacy `SUMMARY_FORM` (kept for old meetings).
- `summary::finish`, with a note: `Snapshot{"meeting-summary"}`, the note's
  markdown (helper) is the summary, `SetDerived` writes `summary.md`
  (`notes.md` removed), title = the reply's one line (`reply_title`: first
  non-empty line, ≤ 120 chars, no backticks) else `title_of(markdown)`;
  then `summarized_at`, session rename, harvest, compaction with the
  markdown as head — unchanged. Without a note: the reply is written to
  `summary.md` as before. `GetSummary`/`meeting_summary` unchanged.
- `PostRetry`: `restore_before_summary` (a diff import that exports) before
  the pipeline re-runs; the next `Summarize` snapshots again.
- Desktop: `meeting/Note.tsx` (`MeetingNote`: `useDocument({doc}) → Live`
  in a bordered pane). `ControlCenter` live branch: transcript + briefs
  left, the note right (`md:w-[45%]`); under `md` a Transcript/Note tab
  row. `Summary` takes `note`: with one it renders `MeetingNote`
  (`min-h-[60vh]`) instead of the `meeting_summary` Streamdown; the retry
  confirm text says the note goes back to before the summary.

## 2. Deviations from §7.2 / §7.4 and the brief

1. **No `Artifact.doc` field.** The page resolves `Binding::Artifact` with
   the existing `doc_resolve` + `useDocument`. A field needs a migration,
   `COLS`/`Row` changes and four `Artifact {..}` literals in `tui`, `slack`,
   `signal` tests — all outside the allow-list — for something one RPC
   already answers.
2. **`call_id = "edit:<ms>-<sha16>"`, not `"edit"`**: `call_id` is the
   table's PRIMARY KEY. `ponytail`: two artifacts exporting identical text in
   the same millisecond collide → logged, no row.
3. **The show import is `Principal::File`**, not a `Show{call_id}`
   principal: File imports never export (step-1 `last_export` logic), which
   is the loop guard the brief asked for. Blocks a `show` wrote are
   attributed `file`, not to the agent's session.
4. **Derived path is a stable file** (`documents/<id>/artifact.md`), not
   the latest blob path: blobs are content-addressed and immutable; the
   artifact store copies each export into a new blob.
5. A user edit's version keeps the previous version's title unless the text
   names its own heading; `bytes` and `created_at` are the export's.
6. **Old meetings keep `SUMMARY_FORM`** (the brief said "replace its body"):
   a meeting without a note has nothing for `doc_*` to edit.
7. The `Review`-state page does not show the note; only `Done` does (and
   the live page). The manifest title is still only set from the reply when
   the user gave none (existing behaviour).
8. `doc_move` undo: one anchor per edit, so a non-contiguous run comes back
   contiguous at the first gap (same ceiling as `delete`'s undo). Marked
   `ponytail:` in `convert.ts`.
9. Two behaviour fixes in `crates/document` beyond "additive": `Restore` as
   Owner (§1.3) and the import-entry lifetime (§1.3). Both are bugs this
   step exposed; both are covered by the existing document tests plus the
   new artifact test ("a show is not an edit").
10. One file outside the allow-list: `components/notes/NotePage.tsx`,
    `function Snapshots` → `export function Snapshots` (35bc386).
11. `useEffect(() => end.current?.scrollIntoView(..))` in `LiveLog`
    (pre-existing) threw "destroy is not a function" once the editor was on
    the page (a scroll polyfill makes `scrollIntoView` return a Promise);
    braces now (494bf56).

## 3. Checks

| # | check | outcome |
|---|---|---|
| 1 | `cargo check --workspace` (5m01s), `cargo build -p theuth`; `cargo test -p artifact -p meeting -p document`; helper `bun test`; UI `tsc -b --noEmit`, vitest, `vite build`; `cargo clippy -p document -p artifact -p meeting --no-deps -D warnings`; `just ts-desktop` (5m11s) | green. artifact 36 unit + 1 integration (new `tests/document.rs`), document 15 + 3, meeting 39 + 1 (new `tests/note.rs`) + 1 (oracle); helper 77 (edit +2); vitest 198; clippy clean |
| 2 | Smoke (1): session `show` `plan.md` twice (99 B → 120 B) | one document `agqmmin57r6xdn4zbasa4i4n44`, snapshots `artifact-v1`, `artifact-v2`, two rows, **no `artifact.md`** after the shows (before 2d57229 the second show wrote one). `/artifacts/<v2 call>` rendered the editor (h1 "Launch Plan", footer `live`, actions Version · Attach to chat… · Snapshots…). Typing a line → `edit:1790030557488-…` version 3 listed **655 ms** after the last keystroke (second run 776 ms), title kept "Launch Plan", `artifact-v3` snapshot; `doc_restore` of `artifact-v2` → version 4 (`artifact-v4`) within 1.6 s; `export_artifact(v5.id)` returned the typed text. 5 versions, 5 snapshots at the end |
| 3 | Smoke (2): `meeting_start{title:"Step 5 smoke", process:null}` (mic + system tap, no degradations), three lines typed in the live note pane, `meeting_stop`, real post pipeline (transcribe → diarize → align, `diarized: true`, ~15 s, empty transcript — nobody spoke), `meeting_summarize` | `Manifest.note` set at start, `meetings/<id>/notes.md` = `# Step 5 smoke` at once, the three lines in the file **462 ms** after the last keystroke. Summary turn (sonnet-4-5): `meeting_read`, `doc_read`, one `doc_insert(at=end)` of the five sections; **turn 11.5 s**, `Harvested` (0/0), `Compacted` (16558 → 1938, style summary, head = the note), `Summarized{title: "Step 5 smoke: rollout budget and vendor coordination"}`, state `Done`. `notes.md` gone, `summary.md` present, `meeting_summary` byte-identical to the file (690 chars), snapshots `meeting-before-summary` + `meeting-summary`. The three user lines are verbatim (the model appended without regrouping — nothing to group). `/meetings/<id>` (Done) shows the note in the editor with the sections |
| 4 | Smoke (3): `meeting_post_retry` | `summary.md` back to the three lines within one 200 ms poll of the reply (the restore export), `Review` after ~16 s (pipeline re-run), `meeting_summarize` again: `doc_read` showed the same four block ids as before the first summary, sections re-appended (turn 13.7 s), 4 labelled snapshots, `Done` |
| — | live page layout | first render threw (deviation 11) and, fixed, gave the note most of the width (`flex-1` beside a `flex-1` editor); `md:w-[45%] md:flex-none` — screenshot `/tmp/theuth-59-smoke5/meeting-live-typed.png` |

Harness: `t --headless` on `127.0.0.1:7811`, `THEUTH_DATA_DIR=/tmp/theuth-59-step5`
(config from the step-3 smoke dir, three concepts, `models` → `~/.theuth/models`
symlink), `THEUTH_SECRETS__KEY_STORE=file`, `THEUTH_TOOLS__APPROVALS=false`;
Vite on 1441; headless Chromium via the omp `browser`; scripted jsonrpsee
client `/tmp/theuth-59-smoke5/{rpc,turn,watch5}.ts`. Load average 50–180
throughout. Screenshots in `/tmp/theuth-59-smoke5/`.

Not observed: a summary turn that actually *moves* blocks (the model had
nothing to regroup; `doc_move` is covered by helper tests and the meeting
integration test), the Transcript/Note tabs under `md`, `Attach to chat…`
from the artifact page end to end (same `ChatPicker` as notes), hover
attribution on the finished note (the map is in-memory and the page had
been reloaded).

## 4. Left

- `Artifact.doc` if a list ever needs it without a resolve per row;
  `DocSummary.attached_meeting` and the notes-list meeting badge (§19.5).
- The `Review`-state meeting page could show the note (the user may want to
  fix it before summarizing).
- `doc_move` undo of a non-contiguous run (deviation 8); `EditResult.moved`.
- Show-import attribution (deviation 3): a `Principal::Show{call_id}` would
  need a log tag, a `Who` variant and UI copy.
- `edit_version` skips only byte-identical exports; a lossy normalisation
  (`- ` → `* `) after a restore or a stray editor transaction is a version.
- The artifact page stays on the route's version (`v2` chip) while the
  editor is live: the chip is the record, the editor the lineage.
- `summary.md` stays the derived path after a retry restore (the file then
  holds the user's notes only until the next summary).
