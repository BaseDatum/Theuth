# Recon — theuth notes / documents substrate (2026-10-08)

Repo: /Users/Sky.Moore/Develop/theuth, branch `main`. The brief named HEAD cd4c8b5c; by the time this
recon ran (12:39–13:00) `git log -1` showed main at 84548287 (2026-10-08 12:37) — the notes seam
files cited here were read from the working tree at that commit; nothing in `crates/document`,
`crates/desktop/ui/src/components/notes` or `docs/design/59-documents.md` changed between the two
(last commit touching `crates/document` is 2026-10-07, see §17). Read-only recon on the shared
axes (AXES.md). Cites are `path:line` (code, repo-relative) or `path` + heading (design docs).
Line numbers for designs are given as `path:line` where a section was read by line range.

Legend: **built** = in the tree on main; **designed** = in a design doc, not found in code;
**not found** = grep/outline returned nothing (the greps are named).

## 1. Container model

- **Notes are a flat list; there is no container above a note.** The store row is
  `docs(id, kind, title, binding, created_ms, updated_ms, bytes, members_n, deleted_ms, hlc)` —
  no folder, tag, project or notebook column (`crates/document/src/store.rs:35-38`); the only index
  besides the primary key is on `binding` (`store.rs:39`). `DocMeta` (meta.json) carries id, kind,
  title, binding, derived path, fragment, members, links, agent style, last_export, authors,
  created/updated — nothing hierarchical (`crates/document/src/meta.rs:308-342`).
- **Grouping that does exist is by *kind* and *binding*, not by user choice.** `DocKind` has seven
  variants: `Knowledge`, `Artifact`, `MeetingNote`, `Note`, `AgentCharter`, `TaskThread`, `Board`
  (`meta.rs:83-101`); `free_standing()` is true only for `Note | Board` and is what makes a document
  "list among the notes" (`meta.rs:116-120`). `Binding` says which *other owner* a document belongs to
  (a concept id, an artifact `(session, name)`, a meeting id, an agent name, a task id) or `Note` =
  "no other owner" (`meta.rs:133-151`). So the only "containers" are owners: a meeting owns one note
  (`crates/meeting/src/note.rs:1-4`), a task owns a thread, an agent owns a charter.
- **The notes list = `doc_list('note', 500)` ∪ `doc_list('board', 500)`, sorted by `updated_ms`
  desc** (`crates/desktop/ui/src/components/notes/useNotes.ts:5-13`). No grouping, no filter, no
  pinning, no search box on the list page (`NotesList.tsx:26-65`).
- **Knowledge has a weak container-like field: `scope`** (a path prefix that boosts/penalises hits by
  cwd — `docs/design/10-knowledge.md:299-301`) and `tags`/`type` in frontmatter (`knowledge_list(prefix,
  type, tag)` in the runtime index). Neither applies to notes.
- **Tasks and schedules have a `project` field** (`task_create(project=…)`, `schedule(project=…)` in
  the runtime index) — a container for *work*, not for documents; a task can attach notes as
  `{kind, id, title}` refs (`crates/tasks/src/tools.rs:172`), which is the closest thing to "a note
  belongs to a project", and it is one-way (task → note).
- **Many-to-many membership: none.** A document has exactly one `binding` (`meta.rs:316`).
- **What a "container page" shows:** there is none for notes. The meeting page shows the meeting's one
  note beside the transcript (`docs/design/59-documents.md` §0 decision 9, lines 106-113). The Library
  section groups Notes as a *tab* beside Artifacts/Knowledge/Meetings/Skills/Secrets/Tools
  (`NotesList.tsx:40` `SectionBand group="library"`; design 97 per knowledge hint
  `decisions/theuth-sidebar-simplify-design-97-2026-10-04`).
- Designed, not built: wikilinks / backlinks from notes to knowledge (`59-documents.md` §15 "Open",
  lines 1093-1094); grep `wikilink|\[\[` over `crates/document/src` and `packages/documents/src`
  hits only a test fixture string (`crates/document/src/tools.rs:962`) — **not found**.

## 2. Source ingestion

What a "source" is today: a *reference the agent reads on demand*, never a persisted object. The
inputs and the pipeline per input:

| input | mechanism (built) | persisted as a reusable source? |
|---|---|---|
| PDF, Word, PowerPoint, Excel, ODF, RTF, EPUB | `read_file` renders them to GFM via `anydoc`, synchronous on the blocking pool under a deadline (`crates/tools/src/doc.rs:1-4`; design `20-documents.md`) | no — the markdown is returned into the turn and dropped |
| URL / web page | `read_url`, `scrape_url` (rendered or not), `crawl`, `map_site` (runtime index; `crates/scrape` 3,498 LOC, `crates/firecrawl-native` 4,102 LOC, §17) — design 122 added caching/change tracking per knowledge hint `projects/theuth-wave-122-web-reading-build-2026-10-07` | no store the agent can address later; only the turn text |
| YouTube / video | grep `youtube|YouTube` over `crates/scrape/src`, `crates/firecrawl-native/src`, `crates/web/src` → **not found** | — |
| audio file | `transcribe(path)` RPC → `voice::Transcript` (`crates/rpc/src/api.rs:1323-1324`, `:5278`); Slack voice notes go the same way (`crates/slack/src/channel.rs:2005`) | the transcript becomes the *message text* of a voice note (45-voice-notes), not a source |
| live meeting audio | meeting crate: STT + diarisation (`crates/meeting/src/{stt,diar,transcript,speakers}.rs`, §17) | yes — the transcript is stored and FTS5-indexed (`crates/meeting/src/store.rs:413`) and one note per meeting is a document |
| pasted text | the chat composer; or `doc_create(markdown=…)` | a note, if the agent/user makes one |
| images | `artifact_read` returns images as pictures; `computer_screenshot`; documents **cannot** hold images — the `image` block is disabled (`59-documents.md:31-33`, §15 line 1089) | artifacts only |
| a past chat | `[attached session …]` → `session_read` (`crates/agent/src/attach.rs:29-30`) | sessions are already stored |

- **Chunking / embedding: none anywhere on this seam.** grep `embed` over `crates/*/src/*.rs` hits
  only `crates/decision` (the local GLiNER/classifier oracle's optional pooled embedding gate,
  `crates/decision/src/engine.rs:85-87`, `gate.rs:103`) and unrelated strings (`browser`, `codeindex`
  import fixtures). `crates/agent/src/store/dreaming.rs` is a *ledger* reader (`ledger_since`,
  `dreaming.rs:79`) with no embedding. The knowledge design itself names the embedding column as a
  future upgrade only: "the upgrade is an embedding column in the same table (sqlite-vec)"
  (`docs/design/10-knowledge.md:333-334`).
- **Async jobs and status:** document conversion is bounded by a deadline (`doc.rs:2-3`); web reads
  by `2×TIMEOUT` budgets (122). There is no ingestion job queue with per-source status — nothing to
  track because nothing is persisted.
- **Dedupe:** none for sources (nothing persisted). Knowledge has `knowledge_merge` /
  `knowledge_supersede` for concepts (`crates/knowledge/src/tools.rs:529`, `:580`).
- **Failure handling:** `read_file` of a bad PDF "names why" (`crates/tools/src/doc.rs:92` test
  name); web reads return `ISSUE:` walls (122 as-built; runtime prompt).

## 3. Source representation

- **There is no source entity.** Nothing in `crates/document`, `crates/knowledge`, `crates/artifact`
  stores "an ingested external document with its full text, chunks, metadata and insights".
- The closest persisted things, and how each is addressed afterwards:
  - **Artifact** — a file the agent `show`ed: content-addressed blob + version row, addressed by
    `call_id` (`crates/artifact/src/tools.rs:104`), read with `artifact_read(offset/limit/ranges/tail)`
    (`crates/agent/src/attach.rs:26-28`). Markdown artifacts are live documents (59 decision 7,
    `59-documents.md:94-99`; `Binding::Artifact{session,name}` → `documents/<id>/artifact.md`,
    `meta.rs:137-142`, `:178`). An artifact is agent output, not an ingested source.
  - **Knowledge concept** — frontmatter + body, FTS5 row (`crates/knowledge/src/store.rs:44`),
    addressed by id (`facts/foo`) and by `id@commit` for history (`attach.rs:46-47`). Harvest writes
    these from sessions (`10-knowledge.md` §5). Scope/tags/type are the only metadata.
  - **Meeting** — transcript segments + speakers + one `MeetingNote` document whose derived file is
    `meetings/<id>/notes.md` → `summary.md` (`meta.rs:171-179`; `crates/meeting/src/note.rs:1-5`).
  - **Session** — the chat itself, addressed by `SessionId` (`attach.rs:50-51`).
- **Derived summaries/"insights" per source:** only the meeting summary turn (Summary / Decisions /
  Action items / Open questions appended into the meeting note — `crates/meeting/src/summary.rs:25`;
  `59-documents.md:106-113`) and knowledge harvest. Nothing per PDF/URL.
- **Address grammar that exists:** `[attached <kind> <id> "<title>" · <meta>]` with kinds `note |
  knowledge | artifact | session | meeting` (`crates/agent/src/attach.rs:1-8`, `:43-54`). A "source"
  kind does not exist in `Kind::ALL` (`attach.rs:58`).

## 4. Notes model

- **One editor, one truth.** Every document (knowledge, artifact, meeting note, note, charter, task
  thread, board) is a Yjs CRDT whose authority is the harness (`yrs::Doc` per open document); the
  markdown file is a derived export re-written after each settled transaction and re-imported
  block-diffed when the file changes (`59-documents.md:52-62` decision 2; `:63-70` decision 3;
  `crates/document/src/watch.rs` exists for the file watcher). The editor is BlockNote (pinned,
  `packages/documents` Bun helper; `59-documents.md:39-51`).
- **Manual vs AI-written:** the same document. The agent edits through block tools (`doc_create`,
  `doc_read`, `doc_insert`, `doc_replace`, `doc_delete`, `doc_move`) with a `Principal::Agent{session}`
  stamp (`crates/document/src/tools.rs:508-794` NAME constants; `meta.rs:446-468` `Principal`);
  agent transactions are revealed typewriter-style (decision 5, `59-documents.md:81-87`) and are
  undoable per agent action (`doc_undo_agent` RPC, `crates/rpc/src/api.rs:992`; UI `UndoAgentAction`
  in `NotePage.tsx:152`). Member-written blocks are labelled untrusted via `DocMeta.authors`
  (`meta.rs:334-337`).
- **Kinds:** the seven `DocKind` variants (`meta.rs:83-101`). A *board* is a note with a frontmatter
  `root:` for `code:` anchors (`meta.rs:97-100`), opened in read mode with an Edit toggle
  (`NotePage.tsx:121-127`, `:146-147`).
- **What a note can link to:** ordinary markdown links; boards add `code:path#La-Lb` links and
  `Excerpt` blocks validated at insert time (`tools.rs:208-271` `check_excerpts`, `:376-402`
  `excerpt_anchors`, `:407-447` `link_anchors`). Links *to other notes/knowledge* have no special
  grammar (wikilinks are §15 open — see §1). Notes can be attached to chats and tasks as references.
- **Versioning/history:** snapshot + update log on disk (`store.rs:28-30` `snapshot.yjs`,
  `updates.log`, `snapshots/`); `doc_snapshots` / `doc_restore` RPC (`api.rs:986-989`); the UI's
  Snapshots dialog says "Restoring imports the snapshot's text over the current one; nothing is
  rewound" (`NotePage.tsx:209-227`). `daily` snapshots deliberately not built (`59-documents.md:1125`).
  Trash: `.trash` dir + `deleted_ms` (`store.rs:31`, `:38`).
- **Title rules:** `DocMeta.title` is "knowledge: the frontmatter `title`; else the first heading"
  (`meta.rs:313-314`); a new note is created as `Untitled` and opens with the title selected
  (`create.ts:8-16`; `NotePage.tsx:83-90`); renames arrive on the bus as `DocumentChanged`, not on the
  document stream (`title.ts:4-12`). Free-standing kinds keep the title they were created with
  (`meta.rs:116-117`); other kinds take theirs from the owner.
- **Export:** `doc_export(doc, format=pdf|markdown, dest?)` — PDF default (A4, diagrams as text,
  excerpts as numbered code) or markdown; to a card or a path under `$HOME`
  (`crates/artifact/src/tools.rs:340-358`, `:372-386`); UI `ExportMenu` (`NotePage.tsx:151`).

## 5. Context control

- **Nothing is injected automatically; the model fetches.** An attached note reaches the model only
  as a one-line marker — `[attached note <DocId> "<title>" · <meta>]` — and the standing prompt says
  "none of its content is in your context yet… fetch what you need with the tool for its kind"
  (`crates/agent/src/attach.rs:17-21`). First look = `doc_read(<id>)` outline (one line per block,
  text truncated to 120 chars, `59-documents.md:520-532`), then `doc_read(format="markdown",
  blocks=[…])` for the pieces (`attach.rs:24-25`). The same rule holds for artifacts (first 2000
  lines), sessions (outline then turns) and meetings (400 lines per call) (`attach.rs:26-32`).
- **User-side toggles (off / summary / full): none.** The only user controls are *whether* to attach
  (composer chip; `NotePage.tsx:149` "Attach to chat…", `NotesList.tsx:119` drag source) and, in the
  note's own chat panel, every message leading with the note's marker (`NoteChat.tsx:21-22`).
- **Budgets:** grep `MAX_BYTES|budget|BUDGET` over `crates/document/src/tools.rs` → **not found**;
  `doc_read` has no byte cap beyond the outline truncation and the model's own `blocks=` selection.
  Knowledge *hints* are budgeted and ranked (bm25 × scope × type × freshness × use, ≥2-term gate,
  `10-knowledge.md:294-312`); 137 endless has `view_bytes` 64,000 for the session view
  (`docs/design/137-endless.md` §0 D5) — neither covers notes.
- **How a turn's context is assembled:** standing prompt blocks (documents `PROMPT`,
  `crates/document/src/tools.rs:24-36`; attachments `PROMPT`, `attach.rs:17-35`) + `<knowledge>`
  hints + the message with its marker lines. No per-source summaries exist to toggle to (§3).

## 6. Chat

- **A note's chat is an ordinary session with the note attached.** Decision 8: "the note page has a
  chat panel that opens a session with the note attached"; rejected: every note owning a session
  (`59-documents.md:100-105`). Built: `NoteChat` is a resizable bottom panel (default 320 px, min 160,
  `lib/note-chat.ts:8-9`) with a compact `Transcript` and a `Composer` "whose every message leads with
  the note's `[attached note …]` marker; the first message creates a session titled like the note
  and the panel keeps it" (`NoteChat.tsx:19-24`). The binding note→session is **client-side only**:
  `localStorage` + zustand, "nothing is pinned server-side" (`note-chat.ts:1-2`, `:20-36`).
- **Threads per note:** one remembered session at a time; "New chat" forgets it and the session stays
  in the normal chat list (`NoteChat.tsx:22-23`, `:86`); "Open in the chat list" navigates to
  `/c/$session` (`NoteChat.tsx:76`). No list of past chats about this note.
- **Model choice per chat:** the session's model (global `/model`; composer picker per knowledge hint
  wave-88); nothing note-specific found in `NoteChat.tsx` or `ChatPicker.tsx` (64 lines; the picker
  chooses *which session to attach to*, `NotePage.tsx:11`, `:149`).
- **"Ask theuth…" from a block:** the slash item prefills the composer with `about block <id>: ` and
  opens the panel (`NotePage.tsx:107-115`).
- **"Save this answer as a note": not found** as a UI action (grep of `NoteChat.tsx`/`NotePage.tsx`
  actions: Rename, Attach to chat…, Snapshots…, Export, Undo agent, Open in new window, Delete —
  `NotePage.tsx:144-154`). The agent can do it on request with `doc_create(markdown=…)` or
  `doc_insert` into the attached note (tools §13); the empty-state copy says "the agent's
  `doc_create` lands here too" (`NotesList.tsx:57`).
- **Streaming:** agent edits land whole and are *painted* progressively (decision 5,
  `59-documents.md:81-87`); the chat transcript streams as every session does (`crates/markdown`
  streaming renderer, `crates/markdown/src/lib.rs:1-9`).
- **Persistence:** the session is a normal `sessions.sqlite` session; the only note-specific state is
  the localStorage record above.

## 7. Ask / research across containers

- **Cross-note "ask" = the agent's ordinary tool loop; there is no dedicated pipeline.** The agent
  has `search(query, categories)` (fuzzy, across sessions / artifacts / knowledge / knowledge history
  / notes / meetings — `crates/rpc/src/search.rs:586-598`, categories `crates/search/src/lib.rs:71-92`),
  `knowledge_search` (FTS5 bm25, `crates/knowledge/src/tools.rs:115-132`), `meeting_search` (FTS5,
  `crates/meeting/src/tools.rs:341`), `session_read` (`crates/agent/src/session_read.rs:18`) and
  `doc_read`. Decomposition, retrieval strategy and synthesis are the model's, per turn; the only
  "graph" is the python runtime cell the model writes.
- **Limits that shape it:** `search` is bounded by a poll deadline and the turn's cancellation
  (`crates/rpc/src/search.rs:560-568`, `:608-623`); the notes category reads **every** live note file
  per query, ≤ 500 notes, nucleo-scored title/body, 160-char snippets (`crates/document/src/search.rs:14-16`,
  `:53-57` "ponytail: reads every note file per query"). `knowledge_search` limit 1–50
  (`tools.rs:132`). No vector retrieval anywhere (§2, §9).
- **What 136 / 137 add:** 137 *endless* gives one session a summary tree with `recall(query, limit)`
  = FTS5 over level-0 summary lines (`docs/design/137-endless.md` §0 D9; `crates/agent/src/store/endless.rs:2`,
  `:374`) — recall *within a session*, not across notes. 136 *dreaming* is a nightly routine over the
  knowledge bundle and the session ledger (hint rank, lifetimes, cases, skills gate — `136-dreaming.md`
  §5, §7, §9; `crates/agent/src/store/dreaming.rs:79` `ledger_since`); it re-ranks knowledge hints,
  it does not index notes (grep `DocKind|note` in `dreaming.rs` → not found by outline; the file
  reads `sessions`, `model_calls`, hint impressions and questions, `dreaming.rs:27-37`, `:211-220`,
  `:311-318`).
- **Boards** are the one built "research artefact across sources": the agent writes sections, one
  diagram per idea, `Excerpt`s and `code:` anchors over a repo and runs a read-back
  (`crates/document/src/tools.rs:42-64` BOARDS_PROMPT + `board.md` skill; design 71). Sources there
  are files in a git tree, not notes/PDFs.

## 8. Citations

- **Grammars that exist (built):**
  1. Attachment markers `[attached <kind> <id> "<title>" · <meta>]` for note | knowledge | artifact |
     session | meeting (`crates/agent/src/attach.rs:1-11`, `:43-54`); the same grammar in
     `lib/attach.ts`, both checked against `tests/fixtures/attach_markers.json` (`attach.rs:10-11`).
     The model is told to "call it by its title in your prose, never by its id" (`attach.rs:33`).
  2. Knowledge ids `facts/foo` and past revisions `id@<commit>` (`attach.rs:46-47`); the
     `knowledge <id>@<commit>` form is read-only (`attach.rs:25`).
  3. Block ids inside documents: `doc_read(format="markdown")` prefixes each block with
     `<!-- b3f2 -->` so the model can quote the id it is about to replace (`59-documents.md:534-536`).
  4. Board anchors over code: `[label](code:path#L10-L24)` in prose, `ref` on Graph nodes / Sequence
     steps, `Excerpt(path, text, start, symbol)`; `doc_insert` validates them and refuses dead anchors
     (`crates/document/src/tools.rs:208-271`, `:325-372` `ExcerptAnchor::refusal`, `:376-447`).
  5. Search identities: `search()` returns "exact identities (session id, artifact call_id+version,
     concept id, concept id@commit, note document id, meeting id)" (`crates/rpc/src/search.rs:596-597`).
- **"Target moved or deleted":** solved only for code anchors. `codeindex::LocateStatus` =
  `Same | Changed | Moved | Missing` ("Range exists and its text hashes to `text_sha`" / "text
  differs and `symbol` still starts there" / "`symbol` now starts on another line; start/end/text are
  the new range" / "file, range or symbol is gone") — `crates/codeindex/src/locate.rs:45-59`. This is
  the ready-made model for a "source chunk moved" status. For documents, a deleted note shows
  `closed · deleted` in the provider status and the page says so (`NotePage.tsx:116-117`); a tool
  reading a missing item is told to "say so plainly" (`attach.rs:34-35`).
- **Citation grammar for notes/sources *in chat answers*: not found.** There is no inline token the
  UI resolves to a note block or a source chunk; answers cite by title in prose (the rule above) or by
  markdown links to web URLs (`search_web` prompt). Chat → note deep link (open note at block id) is
  listed as needing "the outline ids in the index" (`59-documents.md:1103-1105`).

## 9. Search

Per store, what is indexed and how:

| store | engine | what | where |
|---|---|---|---|
| notes (+ boards) | nucleo fuzzy, no index; reads each `note.md` per query, ≤ 500 docs, title-or-body, 160-char snippet | title + derived markdown | `crates/document/src/search.rs:1-3`, `:14-16`, `:71-96` |
| knowledge | SQLite FTS5 `concepts` (id, type, title, description, tags, body), `bm25(…, 0, 3.0, 10.0, 5.0, 4.0, 1.0)`, snippet from body; Rust re-rank by scope ×1.5/×0.5 and the ≥2-term gate for hints | concept text; history by walking git commits | `crates/knowledge/src/store.rs:44`; `docs/design/10-knowledge.md:294-312`, `:324-330` |
| meetings | FTS5 over `speaker`, `text` (one or all meetings, ≤ SEARCH_CAP) | transcript segments | `crates/meeting/src/store.rs:413`, `:488` |
| sessions / artifacts | the `search()` owners (49/66/80); fuzzy, per-owner lazy snapshots answering `indexing` when cold | titles, turns, artifact text | `crates/rpc/src/search.rs:593-598`; `10-knowledge.md:328-330` pattern |
| endless (137) | FTS5 `endless_lines_fts` over level-0 summary lines | one session's summaries | `crates/agent/src/store/endless.rs:2`, `:374` |
| code | FTS5 `symbols_fts` + trigram `symbols_tri` | symbols | `crates/codeindex/src/store.rs:58-65` |

- **Vector / hybrid: none.** grep `embed` over `crates/*/src/*.rs` → only `crates/decision`
  (classifier oracle; `engine.rs:85-87`, `gate.rs:103`), plus noise. grep `sqlite-vec|sqlite_vec|
  hnsw|faiss` → not run beyond `embed`; the design names sqlite-vec as the *future* upgrade
  (`10-knowledge.md:333-334`). **Embedding model role: none** (§12).
- **Chunking parameters:** none (no chunks). Snippets: 160 chars (notes), FTS5 `snippet(…, 12)`
  tokens (knowledge, `10-knowledge.md:296`), outline lines 120 chars (`59-documents.md:521`).
- **Index rebuild:** knowledge `search_refresh` runs one `sync` and rebuilds lazily off the mailbox
  (`10-knowledge.md:321-324`); notes have nothing to rebuild (no index) — the written upgrade is "a
  cached index keyed on DocumentChanged" (`crates/document/src/search.rs:56-57`).
- **Block-level hits** that open the editor at a block: not built (`59-documents.md:1103-1105`).
- **Chats searchable:** yes (`Sessions` category, `crates/search/src/lib.rs:71`). Insights: none exist.

## 10. Transformations / reusable actions

No "named prompt action applied to a source or note" exists as a first-class object. Mechanisms
that would host one, each with where its output lands:

| mechanism | what it is | output lands | user-defined? | background? | cite |
|---|---|---|---|---|---|
| skills | markdown with front matter under `~/.theuth/skills/`, loaded with `load_skill` into the turn | the turn (the model then acts) | yes (`add_skill`, skill repos) | no | `crates/agent/src/skills.rs:28`; `crates/agent/src/lib.rs:1976-1992` `SkillMeta/SkillBody` |
| schedules / routines | prompt files in `~/.theuth/schedules/` on cron / every / at / sensor; `until` loops; `knowledge_put` for memory | chat channels, knowledge, any tool | yes (`schedule()`, `routine_draft()`) | yes | runtime index `schedule(…)`; design 136 §9.7 installs its own routine file |
| meeting summary turn | deterministic turn with a fixed instruction: regroup the user's blocks, append Summary / Decisions / Action items / Open questions via `doc_insert` | the meeting's note (document) | no | yes (after the meeting) | `crates/meeting/src/summary.rs:25`, `:176`; `59-documents.md:106-113` |
| knowledge harvest | `HARVEST_INSTRUCTION` sent as one user message at epoch boundaries; parsed into concepts | knowledge bundle | no (prompt is code) | yes | `crates/knowledge/src/harvest.rs:9`, `:87`; `10-knowledge.md` §5 |
| dreaming (136) | nightly routine: rank, lifetimes, cases, gated skill edits | knowledge frontmatter, `dreaming/` data, routine | config only | yes | `136-dreaming.md` §9.2-9.7 |
| `decide_many` | local classifier over many items, labels you give it | the python cell | per call | no | runtime index; `crates/decision` |
| endless compactor (137) | fixed COMPACT prompt per turn's units, guards G1–G5 | `endless_nodes` | model only (`agent.endless.model`) | yes | `137-endless.md` §5.4, §6, D7–D8 |
| `doc_export` | note/board → PDF or markdown card/file | artifact or `$HOME` path | — | no | `crates/artifact/src/tools.rs:340-358` |

Verdict: the *routine* (schedule file + prompt + `knowledge_put`/`doc_*`) is the closest host for
"apply transformation X to source Y and put the result in note Z"; a *skill* is the closest host for
a reusable prompt. Neither is bound to a document or triggered from a note's page.

## 11. Audio & voice

- **STT (built, local):** Parakeet TDT via `parakeet-rs` for dictation / voice notes
  (`crates/voice/src/stt.rs:2`, `:14`, `:170-179`; checkpoint `<data_dir>/models/parakeet-tdt`,
  `crates/voice/src/lib.rs:53-55`, `:408-409`); meetings run a streaming STT plus diarisation
  (`crates/meeting/src/{stt,diar,speakers}.rs`; knowledge hint `facts/theuth-stt-model-tiers-recon-2026-09-29`:
  nemotron-speech-streaming 0.6B for live, Parakeet TDT v3 for dictation). `transcribe(path)` RPC
  returns a `voice::Transcript` (`crates/rpc/src/api.rs:1323-1324`).
- **TTS (built, local):** Pocket TTS through the `ptts` crate — "markdown → sentences → `ptts` PCM →
  one Opus packet per Mimi frame" (`crates/voice/src/speak.rs:1-4`, `:10`); checkpoint
  `<data_dir>/models/pocket-tts` (`crates/voice/src/lib.rs:406`, `:645`); device playback through
  `cpal` at 24 kHz (`crates/voice/src/out.rs:1-5`). Used by *talk* (design 76, `crates/talk` 3,244 LOC)
  and spoken replies.
- **Voice notes in:** composer hold-to-record → `api.transcribe` → the text is submitted as the
  message (knowledge hint `facts/theuth-desktop-composer-voice-note-code-map-2026-10-03`;
  `lib/voice-note.ts` 12 lines); dictation into any field (`DictationButton`, `NotesList.tsx:13`,
  `:112`; `NotePage.tsx:141`).
- **Podcast / audio overview generation (outline → script → multi-speaker TTS): not found.** grep
  `podcast|audio overview|speaker profile` not run as code grep; `crates/voice/src` has one voice
  (`SpeechOptions`, `speak.rs:10`), no script stage, no speaker profiles — the only multi-speaker
  machinery is *input* diarisation in `crates/meeting`. A note *can* be read aloud: the talk/live path
  speaks any reply (`Live talk` prompt block), not a document.

## 12. Models & providers

- **Catalog:** bundled models.dev `data/models.json` merged with `<data_dir>/models.json`, resolved
  `provider/model` → genai adapter ids, no network (`crates/models/src/lib.rs:1-3`); per model:
  limits, cost, reasoning, tool_call, image input, reasoning options, protocol
  (`lib.rs:33-60`). `list_models()` lists what has credentials (runtime index).
- **Roles that exist (config keys):** `agent.model` (default; the user switches with `/model`),
  `agent.subagents.model` (`crates/runtime/src/lib.rs:353-359` config fixtures), `agent.title_model`,
  `agent.vision_model`, `agent.escalate_model` (persisted via RPC asks,
  `crates/agent/src/lib.rs:608`, `:620`, `:635`, `:653`); side-call fallback chain "`agent.escalate_model`
  → `agent.title_model` → the session's model" and "`agent.title_model` → the session's model"
  (`crates/runtime/src/side_call.rs:81-83`); 137 compactor `agent.endless.model` → title_model →
  session model (`137-endless.md` D8; `side_call.rs:34`). Per-session model = the session's own
  (`crates/agent/src/lib.rs:1002` `pub model: String`).
- **Local model roles (ONNX/candle, not in the catalog):** STT (Parakeet, Nemotron), TTS (Pocket
  TTS), diarisation (pyannote per knowledge hint `facts/theuth-local-models-state-2026-09-27`),
  decision/classifier (GLiNER + browser model, `crates/decision/src/gliner/candle.rs:52-80`), with
  one process-wide ORT initialiser and an `models.accelerator` knob (`crates/models/src/lib.rs:5-7`).
- **Embedding role: none.** grep `embedding_model` over `crates/runtime/src` → not found; `embed` hits
  only the decision oracle (§9). **"Transformation" role: none** (the meeting summary and harvest use
  the session/title model paths). **"Large context" role: none** named; 137 is the long-context
  answer by summarisation rather than a bigger model.
- **How a role is picked at call time:** the side-call chain above (`side_call.rs:81-83`), otherwise
  the session's model; subagents take `spawn_subagent(model=…)` or the configured default.

## 13. API / agent surface

- **Agent tools (python runtime functions; gate risk):** `doc_read` ReadOnly; `doc_insert`,
  `doc_replace`, `doc_delete`, `doc_move`, `doc_create` Mutating (`crates/document/src/tools.rs:508-510`,
  `:579-580`, `:639`, `:682`, `:722`, `:794`); `doc_export` Mutating, lives in `artifact` because
  `document` cannot depend on it (`crates/artifact/src/tools.rs:340-351`). `doc` resolves a `DocId`,
  `knowledge:<id>` or `meeting:<id>` (`tools.rs:106-167` `resolve`; standing prompt `tools.rs:24-36`).
  No `doc_share` tool by decision 15 — "Tools go through the gate; sharing never does"
  (`59-documents.md:160-164`). Plus `knowledge_*` (13 tools, `crates/knowledge/src/tools.rs:115-754`),
  `artifact_read`, `search`, `session_read`, `meeting_*` (§7).
- **RPC (jsonrpsee, ts-rs bindings; names from the `#[method]` trait):** `doc_list`, `doc_resolve`,
  `doc_ensure`, `doc_markdown`, `doc_export`, `doc_open`, `doc_update`, `doc_awareness`,
  `doc_subscribe`, `doc_snapshots`, `doc_restore`, `doc_undo_agent`, `doc_create`, `doc_rename`,
  `doc_delete`, `doc_stats`, `doc_members`, `doc_member_set`, `doc_member_revoke`, `doc_link_create`,
  `doc_link_revoke`, `doc_join`, `doc_set_name` (`crates/rpc/src/api.rs:932-1058`), `doc_send`,
  `doc_forward` (`api.rs:2551`, `:2576`); the desktop client mirrors them (`crates/desktop/ui/src/rpc/api.ts:320-343`).
  Sync is three calls — `doc_open`, `doc_update`, `doc_subscribe` — for every editor including the
  helper (`59-documents.md:289-291`).
- **REST / HTTP:** none for documents; the harness speaks JSON-RPC in-process and Noise over the relay
  (`59-documents.md:63-70`). Auth = the harness's own device pairing / bearer token; guests get a mode-2
  allow-list of `doc_*` only (`59-documents.md:1021`).
- **Driving ingestion by API:** `read_file`, `read_url/scrape_url/crawl/map_site`, `transcribe` RPC
  (`crates/rpc/src/api.rs:1323`) — all return text to the caller; nothing to "add a source to".

## 14. UI

- **Where Notes sits:** Library section, tab band `SectionBand group="library"`
  (`crates/desktop/ui/src/components/notes/NotesList.tsx:40`), route `/notes` and `/notes/$id`
  (`NotesList.tsx:26`, `NotePage.tsx:27-29`); from `md` up the list pane sits beside the document
  (`crates/desktop/ui/src/components/editor/DocumentPage.tsx:25-26`).
- **List page:** one column, max-width 768 px; rows = kind icon (sticky note / board), title or
  "Untitled", a kind badge for non-`note` kinds, relative "updated" time; hover reveals Open-in-new-
  window and a ⋯ menu (Rename inline, Delete one click); rows are drag sources that drop as attachments;
  New note button in the row; empty state "No notes yet. New note starts one; the agent's `doc_create`
  lands here too." (`NotesList.tsx:38-65`, `:97-150`). Sorting: `updated_ms` desc only
  (`useNotes.ts:11`). No search field, filter, tags or folders.
- **Note page:** `DocumentPage` frame = SectionBand + SectionRow (back · title input with dictation
  · actions) over the BlockNote editor (`DocumentPage.tsx:20-27`, `NotePage.tsx:129-154`); actions:
  Rename, Attach to chat…, Snapshots…, Export (md/pdf), Undo agent, Open in new window, Delete; a
  board adds a `board` badge and Read/Edit toggle (`NotePage.tsx:144-154`). Below the editor the
  `NoteChat` panel (resizable, 160 px–pane) with the remembered session's compact transcript and a
  composer (`NoteChat.tsx:19-24`, `:51-66`). Share dialog and participant strip come from the frame
  (`DocumentPage.tsx:8-9`). Popped-out window = editor only, no chat (`NotePage.tsx:118-120`).
- **Editor:** BlockNote shadcn flavour, curated slash menu, no image block, no AI menu
  (`59-documents.md:71-80`, `:156-159`); agent edits revealed over ≤ 1.5 s (`:81-87`); member cursors
  coloured from a 12-colour palette (`meta.rs:221-224`). Editor components: `Editor.tsx`,
  `FrontmatterBlock.tsx`, `CodeBlock.tsx`, `MobileToolbar.tsx`, `reveal.ts`, `toolbar.ts`
  (dir listing `crates/desktop/ui/src/components/editor/`).
- **Mobile:** under `pointer: coarse` drag handles and side menu hidden, a fixed formatting bar pinned
  above the keyboard via `visualViewport` with bold/italic/strike, heading, lists, indent, undo/redo
  and a block-type sheet (`59-documents.md:146-155` decision 13; `Editor.tsx:135`, `:150`
  `usePointerCoarse`); 44 px touch targets (`CodeBlock.tsx:92`).
- **Keyboard:** Enter/Escape commit/cancel the inline rename (`NotesList.tsx:90-96`); the resize
  handle nudges by `PX_STEP` (`NoteChat.tsx:65`); slash "Ask theuth…" (`NotePage.tsx:107-115`).
- **Source page:** none — there is no source entity (§3).

## 15. Storage & sync

- **On disk:** `<data_dir>/documents/` with `index.sqlite` (table `docs`, §1), per document
  `<id>/meta.json`, `snapshot.yjs`, `updates.log`, `snapshots/`, derived `note.md | artifact.md |
  charter.md | thread.md`; `.trash/`; flat `mirror/` (`crates/document/src/store.rs:25-33`;
  `meta.rs:175-184`). Knowledge concepts derive to `knowledge/<id>.md`, meeting notes to
  `meetings/<id>/notes.md` (`meta.rs:177-179`). Migrations: `CREATE TABLE IF NOT EXISTS` + `ALTER
  TABLE docs ADD COLUMN hlc` (`store.rs:35-41`) + an id migration (`store.rs:208-265` `migrate_ids`).
- **Truth and export/import loop:** Y.Doc is truth; export after each settled transaction with a
  400 ms debounce; a foreign file edit is imported as one block-diffed transaction; "our own write"
  check via `last_export` sha (`59-documents.md:52-62`; `meta.rs:329-333`; config
  `59-documents.md:1033-1043`: `export_debounce_ms 400`, `max_doc_bytes 8 MiB`, `snapshots_keep 30`).
- **Multi-device sync:** paired devices (macOS, iOS, browser build) are Yjs clients over Noise via the
  relay; the relay sees ciphertext only (`59-documents.md:20-25`, `:63-70`); step 4 as-built: link
  devices over mode 0 with an IndexedDB queue (`59-documents.md:1116-1117`).
- **Multi-engine (mesh, design 69):** the yrs server is a *hub duty* — one lease-holding engine opens
  docs, accepts updates, serves subscribers, admits guests; every other engine holds the replicated
  mirror and serves `doc_read`/`Outline`/`Markdown` from it, answering writes with `DocError::NotHub`
  (`crates/document/src/hub.rs:1-5`); the mirror is flat `<id>.meta.json` + `<id>.snapshot.yjs`
  because the replica's file mirror is last-writer-wins on whole bytes (`crates/document/src/mirror.rs:1-5`);
  the `docs` table is a replicated table with an HLC column (`store.rs:40-44`).
- **Offline:** documents are not served while the harness is offline — the relay stores nothing,
  accepted in v1 (`59-documents.md:28-30`, §15 lines 1097-1100); a device's own queued edits
  converge when it reconnects (step 4). Export/import = `doc_export` (md/pdf), file import via the
  watcher (`watch.rs`), plus `knowledge_put`/`write_file` through the derived file (`59-documents.md:56-59`).

## 16. Sharing & privacy

- **Roles:** `owner` (implicit, the harness's devices), `editor`, `viewer`; live demotion closes the
  subscription (`59-documents.md:809-815`; `meta.rs:191-196` `Role`, `:201-218` `Member`).
- **Links:** `https://<relay>/d#1.<harness_id>.<doc_id>.<secret>`; the harness stores only
  `sha256(secret)` sealed with the mesh data key; optional preset name and expiry; revoking a link
  stops new joins, members stay until revoked individually (`59-documents.md:817-829`;
  `crates/document/src/share.rs:1-6`; `meta.rs:231-249` `ShareLink{id, role, secret_hash_sealed,
  name, created_ms, expires_ms, revoked_ms}`). Join proof `HMAC-SHA256(sha256(secret), handshake_hash
  ‖ guest_pub)` (`share.rs:5-6`). **Password protection: none** — the secret in the fragment is the
  capability.
- **Guests:** a browser with no pairing loads `/d`, Noise IK with an unknown static → guest connection
  (mode 2), 5 s to join, 10 handshakes/min, 48 KiB per update, 8 MiB per doc, three viewer-write
  refusals close the connection (`59-documents.md:831-846`, `:1020-1025`). Limits in config
  (`:1038-1040` `max_members 64`, `guest_joins_per_minute 10`, `sharing: on|off|disabled`).
- **Untrusted text:** member-authored blocks are labelled `doc-member:<pub8>` in `doc_read` output
  and the approval card shows the label on the first tool call after such a read (`59-documents.md:1020`;
  `meta.rs:334-337` `authors`; standing prompt rule in `tools.rs:24-36`).
- **Knowledge visibility:** `knowledge_share(id, private | team | shared)` (`crates/knowledge/src/tools.rs:341`;
  runtime index). Notes have no visibility field beyond members/links (`meta.rs:321-326`).
- **What leaves the machine:** ciphertext to the relay (document updates, awareness) for paired devices
  and guests (`59-documents.md:20-25`, `:1027`); model calls to the configured providers; nothing
  else for documents (no cloud store, decision 12 `:139-145`).

## 17. Numbers

Counted with `wc -l`, `find`, `git log` on 2026-10-08 12:40 (`main` 84548287):

| unit | number | how |
|---|---|---|
| `crates/document/src/*.rs` | 10,744 LOC, 12 files (+ `tests/` dir, 13 `.rs` files total) | `wc -l`, `find` |
| `packages/documents/src/*.ts` | 1,017 LOC; tests 883 LOC in 4 files (`edit` 32, `yaml` 7, `helper` 6 `test(` calls) | `wc -l`, `grep -c` |
| `crates/knowledge/src` | 9,050 LOC | `wc -l` |
| `crates/artifact/src` | 8,740 LOC | |
| `crates/meeting/src` | 15,046 LOC | |
| `crates/search/src` | 503 LOC (one file) | |
| `crates/scrape/src` / `crates/firecrawl-native/src` | 3,498 / 4,102 LOC | |
| `crates/voice/src` / `crates/talk/src` | 4,372 / 3,244 LOC | |
| `crates/models/src` | 1,921 LOC | |
| `crates/markdown/src` | 1,423 LOC | |
| `crates/dreaming/src` | 10,817 LOC | |
| `crates/desktop/ui/src/components/notes/` | 10 files, 833 LOC (NotePage 243, NotesList 159, NoteChat 128, ChatPicker 64, ExportMenu 63, create 40, title 13, useNotes 14, 2 test files 109) | `wc -l` |
| `#[test]`/`#[tokio::test]` in `crates/document/src` | 45 | `grep -c` |
| commits touching `crates/document` | 43, first 2026-09-21 ("feat(document): store, yrs authority, actor, helper client (59 §3–4)"), last 2026-10-07 | `git log` |
| design docs | 465 files in `docs/design/`; 59-documents.md 2,449 lines; 66-attachments 783; 49-search 1,177; 53-meetings 547; 71-whiteboard-visuals 341; 10-knowledge 1,088 | `ls | wc -l`, `wc -l` |
| 59 build steps | 8 steps, 2026-09-21 → 2026-09-22 (§17–§24 headings at `59-documents.md:1128`, `:1282`, `:1477`, `:1674`, `:1953`, `:2142`, `:2211`, `:2313`) | grep `^## ` |
| branches / worktrees | 45 refs; 16 worktrees (110-repin, 125-f2, 125-r1, 126-w0, 140 ×7, 141, 143-w1/w2, bindings) | `git branch -a | wc -l`, `git worktree list` |
| RPC `doc_*` methods | 25 names (`api.rs:932-1058`, `:2551`, `:2576`) | grep |
| `DocKind` variants / `Binding` variants | 7 / 6 (`meta.rs:83-101`, `:133-151`) | read |
| config knobs (`documents:`) | 8 (`59-documents.md:1033-1043`) | read |

## 18. What the live designs already plan on this seam

Open items and in-flight work that touch notes / documents / knowledge, with status as of 12:45:

- **59 §15 Open (designed, not built, cites `docs/design/59-documents.md:1087-1105`):** images in
  documents via a `doc_blob_get` over the attachment store; wikilinks as inline links + backlinks
  notes → knowledge; comments/threads (`xl-comments`) with "the agent answering a comment thread" as
  the next AI surface; offline availability through an opt-in encrypted-snapshot relay mode;
  recompiled PDFs from edited markdown; **search: "notes as a category over derived files is trivial;
  block-level hits that open the editor at a block id need the outline ids in the index"** — the
  category shipped (`crates/document/src/search.rs:1-3`, 66 §10 phase 5), block-level hits did not.
- **59 §16.1 "Deliberately not built"** (`:1125-1126`): `daily` snapshots, owner display name, notes
  search category (since built), `Artifact.doc`. Step as-builts each carry a "Left … (and noticed on
  the way)" list (`:1266`, `:1457`, `:1655`, `:1938`).
- **10 knowledge:** the written upgrade path for noisy hints is "an embedding column in the same table
  (sqlite-vec) ranked alongside — not a new store" (`docs/design/10-knowledge.md:333-334`); 136 §15
  amends 10 §3/§6 so hints are ranked by use/freshness and frontmatter keys round-trip
  (`136-dreaming.md:432-436`).
- **136 dreaming (landed main 589a72f2 2026-10-08 10:59 per knowledge hint `projects/theuth-dreaming-design-136`):**
  still open — the reading of "No trial", gate margins as config, `home_engine` on the mesh
  (`136-dreaming.md:426-430`). It curates *knowledge*; notes are not in its corpus (§7).
- **137 endless (landed 68b9fc26 2026-10-08 08:01 per `projects/theuth-wave-137-endless-memory-2026-10-08`):**
  §12 "Meetings — the next wave (recorded, not built)": a summary tree over the older transcript,
  "one endless log per recurring meeting series gives continuity across meetings"
  (`137-endless.md:290-291`); §16 open: child inherits parent view, per-agent `view_bytes`, importing
  other sessions' history into an endless session, the meetings hybrid (`:325-331`). `recall` is
  FTS5 over summaries (D9) — a per-session long memory, not cross-note retrieval.
- **140 autoresearch (worktrees `theuth-140`, `-140-c/-ui/-w1/-w2/-w3/-w6`, branch `feat/140-autoresearch`):**
  experiment notes live in its own SQLite (`experiment_notes`, `ideas` table with FTS match —
  `docs/design/140-autoresearch.md:257`, `:280`); it does not use `doc_*` or notes (grep
  `doc_create|doc_insert|notes\b` over the doc hits only its own "notes" concept).
- **141 ACP subagents** (worktree `theuth-141`): no document/notes surface (grep → only "note" as a
  verb in `141-acp-subagents.md:14`, `:118`).
- **143 (`theuth-143-w1`, `-w2` on `feat/143-w1/w2` at 84548287):** no `docs/design/143*` on main
  (`git log --all -- docs/design/143*` empty) — cannot say whether it touches this seam; **unverified**.
- **Boards (71, as-builts `70-as-built-BOARDS-step-1…9`):** the one shipped "research document" model:
  anchored prose + Excerpts + diagrams + read-back, over code only.
- Not found as a design anywhere in `docs/design/`: a source/notebook container, per-source
  context toggles, podcast generation, vector search over notes (`ls docs/design | grep -i
  'notebook\|source\|podcast\|embed'` → nothing; the brief's list of load-bearing docs has none).

## 19. Gaps against a notebook model

Only what was verified absent (grep / outline named in the axis):

1. **No container above a note** — no notebook/project/folder/tag on `docs` or `DocMeta`
   (`store.rs:35-38`, `meta.rs:308-342`); the list is flat and sorted by update time (`useNotes.ts:11`).
   Tasks/sessions carry a `project` label (`crates/agent/src/lib.rs:1061-1063`) that documents do not.
2. **No persisted source** — PDFs, URLs, audio become turn text and vanish (`crates/tools/src/doc.rs:1-4`;
   §2 table); no "add to notebook", no per-source metadata, no insights/summaries per source, no
   `source` attachment kind (`attach.rs:43-54`).
3. **No context control** — no off/summary/full per attached item, no budget on `doc_read`
   (grep `MAX_BYTES|budget` in `tools.rs` → none); the model decides what to read (`attach.rs:17-21`).
4. **No citation grammar in answers** — nothing resolves a chat token to a note block or source chunk;
   deep link chat → note block is explicitly unbuilt (`59-documents.md:1103-1105`); the only resolved
   anchors are code anchors on boards (`locate.rs:45-59`).
5. **No vector / hybrid search over notes** — notes are nucleo-fuzzed from files per query
   (`search.rs:56-57`, `:80-96`); no embeddings anywhere outside the classifier (`crates/decision/src/engine.rs:85-87`);
   no embedding model role (§12).
6. **No "ask across notes" pipeline** — `search()` + `doc_read` by hand each turn (§7); nothing like a
   retrieval → synthesis graph with a fixed shape and limits.
7. **No audio overview / podcast** — single-voice TTS for replies only (`speak.rs:1-10`); no script
   stage, no speaker profiles; multi-speaker exists only on the input side (`crates/meeting/src/diar.rs`).
8. **No "save answer as note" affordance** — not in `NotePage.tsx:144-154` actions nor `NoteChat.tsx`;
   agent `doc_create` on request only.
9. **No YouTube / video ingestion** (grep § 2) and no images inside notes (`59-documents.md:31-33`).
10. **No per-note chat history** — one remembered session per note in localStorage
    (`note-chat.ts:1-2`), no server-side list of chats about a note.
11. **No transformations bound to a document** — skills/routines/harvest/summary exist (§10) but none
    is triggerable from a note or writes back to a chosen note by default.
12. **No block-level search hits / open-at-block** (`59-documents.md:1103-1105`), no wikilinks or
    backlinks (`:1093-1094`; grep `\[\[` → test string only).

## 20. Strengths a notebook product does not have

1. **The agent is a first-class co-editor with block tools under a risk gate** — `doc_read` outline →
   `doc_insert/replace/delete/move` with stable block ids, one Yjs transaction, typewriter reveal,
   per-action undo (`tools.rs:24-36`, `:508-794`; `59-documents.md:71-87`; `api.rs:992`
   `doc_undo_agent`).
2. **One CRDT truth for knowledge, notes, artifacts, meeting notes, charters, task threads and boards**
   with a derived markdown file that keeps grep/git/`read_file`/harvest working (`meta.rs:83-101`;
   `59-documents.md:52-62`).
3. **End-to-end encrypted collaboration** — paired devices and guests over Noise via a relay that
   sees ciphertext; roles, links with expiry/revocation, HMAC join proof, 48 KiB/8 MiB limits,
   member text labelled untrusted for the agent (`share.rs:1-6`; `59-documents.md:807-846`,
   `:1016-1030`; `meta.rs:334-337`).
4. **Mesh replication** — documents replicate across the user's engines with a hub lease and a flat
   mirror; `doc_read` works on any engine (`hub.rs:1-5`; `mirror.rs:1-5`; `store.rs:40-44`).
5. **Boards** — anchored explanations over code with `Excerpt`s validated at insert, `code:` links,
   drift statuses Same/Changed/Moved/Missing (`tools.rs:208-271`; `locate.rs:45-59`).
6. **Knowledge harvest + hints + dreaming** — durable facts written from sessions automatically,
   FTS5 bm25 with scope/type/freshness/use ranking, nightly curation (`crates/knowledge/src/harvest.rs:9`;
   `10-knowledge.md:294-312`; `136-dreaming.md` §5, §9).
7. **Meetings → notes** — live STT + diarisation, one note per meeting shown beside the transcript,
   a summary turn that regroups the user's blocks and appends Summary/Decisions/Actions/Open
   (`crates/meeting/src/note.rs:1-5`; `summary.rs:25`; `59-documents.md:106-113`); transcripts FTS5-searchable
   (`crates/meeting/src/store.rs:413`).
8. **Local voice both ways** — Parakeet/Nemotron STT for dictation, voice notes and meetings; Pocket
   TTS for spoken replies and live talk (`stt.rs:2-14`; `speak.rs:1-10`; `out.rs:1-5`).
9. **Multi-channel** — the same notes are attachable from the desktop, phone, browser build, Slack,
   Telegram, Signal and the TUI (`attach.rs:1-11` shared grammar; `crates/tui/src/attach.rs:235`;
   `crates/slack/src/channel.rs:2005`).
10. **Unified `search()` across sessions, artifacts, knowledge (+history), notes and meetings with
    exact identities and honest `indexing/unavailable` states** (`crates/rpc/src/search.rs:594-598`;
    `crates/search/src/lib.rs:71-92`).
11. **Export to PDF/markdown from the agent or the UI** (`crates/artifact/src/tools.rs:340-358`;
    `NotePage.tsx:151`) and snapshots with restore-as-import (`NotePage.tsx:209-227`).
12. **A python runtime with every tool as a function** — the agent can compose read_file → doc_insert →
    knowledge_put in one cell (`attach.rs:20-21`; runtime index) — the raw material for
    transformations without a new subsystem.

## Self-check

Re-opened with `read_file(path, offset, limit)` at 12:49 and matched against the claim (35 checked,
all present; two needles straddled a line break and were confirmed by eye):

- `crates/document/src/store.rs:35-38` — ok (`CREATE TABLE IF NOT EXISTS docs (id, kind, title, binding, …, hlc)`)
- `crates/document/src/meta.rs:83` — ok (`pub enum DocKind`)
- `crates/document/src/meta.rs:118` — ok (`free_standing` = Note | Board)
- `crates/document/src/meta.rs:308` — ok (`pub struct DocMeta`)
- `crates/document/src/search.rs:56-57` — ok ("reads every note file per query (≤ 500 small files)")
- `crates/document/src/search.rs:86` — ok (reads `note.md` per doc)
- `crates/document/src/tools.rs:508-509` — ok (`doc_read`, `Risk::ReadOnly`)
- `crates/document/src/tools.rs:24` — ok (documents `PROMPT`)
- `crates/agent/src/attach.rs:17-21` — ok ("none of its content is in your context yet")
- `crates/agent/src/attach.rs:43-54` — ok (`Kind` = Note, Knowledge, Artifact, Session, Meeting)
- `crates/rpc/src/search.rs:594-597` — ok ("Fuzzy-search … sessions, shown artifacts, … notes and meetings")
- `crates/search/src/lib.rs:79` — ok (`Notes` category)
- `crates/knowledge/src/store.rs:44` — ok (`CREATE VIRTUAL TABLE … concepts USING fts5`)
- `crates/codeindex/src/locate.rs:50-58` — ok (`LocateStatus` Same/Changed/Moved/Missing)
- `crates/desktop/ui/src/lib/note-chat.ts:1-2` — ok ("nothing is pinned server-side")
- `crates/desktop/ui/src/components/notes/NoteChat.tsx:21-22` — ok (every message leads with `[attached note …]`)
- `crates/desktop/ui/src/components/notes/useNotes.ts:10-11` — ok (`doc_list('note',500)` + `doc_list('board',500)`, sort by `updated_ms`)
- `crates/desktop/ui/src/components/notes/NotePage.tsx:226` — ok ("Restoring imports the snapshot's text over the current one; nothing is rewound")
- `crates/desktop/ui/src/components/notes/NotesList.tsx:57` — ok (empty-state copy names `doc_create`)
- `crates/artifact/src/tools.rs:349-350` — ok (`doc_export`, `Risk::Mutating`)
- `crates/tools/src/doc.rs:1-2` — ok (PDF/Word/PowerPoint/Excel/ODF/RTF/EPUB via `anydoc`)
- `crates/voice/src/speak.rs:1-2` — ok (markdown → sentences → `ptts` PCM)
- `crates/voice/src/stt.rs:2` — ok (Parakeet TDT via `parakeet-rs`)
- `crates/voice/src/lib.rs:406` — ok (Pocket TTS checkpoint dir)
- `crates/runtime/src/side_call.rs:81` — ok (escalate_model → title_model → session model)
- `crates/document/src/hub.rs:1-2` — ok (yrs server is a hub duty)
- `crates/document/src/mirror.rs:3-4` — ok (flat mirror, last-writer-wins)
- `crates/document/src/share.rs:5-6` — ok (HMAC-SHA256 join proof)
- `crates/meeting/src/note.rs:1-2` — ok (one `MeetingNote` per meeting)
- `docs/design/59-documents.md:100-102` — ok (decision 8 "Notes are documents with no other owner")
- `docs/design/59-documents.md:1103-1105` — ok (§15 Search: block-level hits need outline ids)
- `docs/design/59-documents.md:1125-1126` — ok (§16.1 "Deliberately not built")
- `docs/design/10-knowledge.md:333-334` — ok (sqlite-vec named as the upgrade)
- `docs/design/137-endless.md:290` — ok (§12 Meetings — next wave, not built)
- `docs/design/136-dreaming.md:426` — ok (§14.2 Still open)

Not verified: whether worktrees `theuth-143-w1/w2` touch this seam (no design file on main); the
exact per-category engines of the `Sessions`/`Artifacts` search owners (read only through
`crates/rpc/src/search.rs` and the design, not their owner crates); runtime behaviour of anything —
nothing was built or run.
