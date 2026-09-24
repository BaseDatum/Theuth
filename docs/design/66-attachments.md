# 66 — Attachments: reference any saved thing from a message, and a picker worth using

Status: design, 2026-09-24, for Sky's review before code. Extends 59 §5.4/§7.3
(the `[doc <id> "<title>"]` marker and the composer `+` → "Attach note…"),
45 §0.2 (marker-line convention), 62 §0.4 (the model, not the UI, is the reader
that needs the fact), 49 (unified `search`, `SearchCategory`), 35 (artifacts,
`show`), 53 (meetings), 10 (knowledge), 48 (`/shake` picker, 44 px rows),
51 §4 (private trees), 28 (untrusted content fence), 17/20 (file attachments
as `Binary` parts). Amends 59 §5.4 (the marker grammar generalises; `[doc …]`
stays readable), 59 §19.3 deviation 1 (the `notes` search category returns as
phase 5 here, plus `meetings`).

Scope: one gesture — attach a saved thing to the message you are typing — for
everything the harness can find: notes, knowledge concepts (and a past revision
of one), saved artifacts of every kind, past chats, meetings. The attachment is
a **reference**, not content: the agent gets a stable id, a human title and a
few facts, and pulls the whole thing or pieces of it with the read tool for that
kind. One picker, launched from `+`, the same on desktop and phone.

Not in v1 (seams named in §12): TUI picker (typed markers work day one), text
previews inside the picker, attaching to a *broadcast* (62), attaching from
Slack/Signal (a typed marker works there too), briefs/schedules/skills as kinds
(§3.2 says why), a `notes`/`meetings` search category in the backend (phase 5).

## 0. Decisions in one screen

1. **A typed reference `{kind, id, title, meta}`, five kinds:** `note`,
   `knowledge` (id may carry `@<commit>` for a past revision), `artifact`
   (by `call_id`, the row key — one immutable version), `session`, `meeting`.
   Files (images/PDF uploads) stay what 17/20 made them: `Binary` parts via
   `files`, not references. *Rejected:* briefs, schedules, skills, todos as
   kinds (§3.2).
2. **Serialised as leading marker lines in the user text, not a structured
   array on the message.** `[attached <kind> <id> "<title>" · <meta>]`, one
   per line, arrival order, then the typed body — the exact shape 59 §5.4
   already uses for documents, generalised. `[doc <id> "<title>"]` remains
   accepted as the legacy spelling (`doc <DocId>` = note, `doc knowledge:<id>`
   = knowledge). Zero RPC/schema change; survives reload because it *is* the
   stored text; indexed by session search because it is user text; works from
   the TUI/Slack/Signal by typing; `edit_turn`/fork/regenerate keep it. What
   the model sees is literally the line (§5). *Rejected:* an `attachments`
   column/array (§4.3 — three RPC methods, a migration, bindings, TUI and ACP
   all change for a fact the text already carries).
3. **Per-message lifetime.** A reference rides with the message it was attached
   to (Sky's ruling). The rendered history shows it as a clickable chip with an
   "Attach again" action, and the picker's empty state lists "Earlier in this
   chat" from the transcript's markers. The only pin is 59 §7.3's note-page
   panel (`pinned`), unchanged in semantics.
4. **Living things point to latest; immutable things are addressed exactly.**
   Notes and knowledge concepts are edited in place by the agent, so the
   reference is the id (latest). An artifact `call_id` *is* one version (35
   §3.3: "an immutable snapshot with a name and a version"). A knowledge
   revision is an explicit kind-variant (`knowledge facts/x@<commit>`), chosen
   from Knowledge history in the picker, never implied.
5. **Per-kind read tools, not a polymorphic reader.** `doc_read`, `read_file`,
   `knowledge_get`, `meeting_read` exist; two are missing — `session_read`
   (outline, then turn ranges) and `artifact_read` (by `call_id`, with
   `read_file`'s paging) — plus two small extensions: `knowledge_get` accepts
   `id@commit`; `doc_read` resolves `meeting:<id>` to the meeting's note.
   *Rejected:* `attachment_read(ref, range)` — a dispatcher over four unlike
   paging models (block ids, line ranges, ms windows, turns) whose args union
   is worse for the model than four tools it already knows (§6).
6. **One static prompt block, `attachments`,** registered by the agent plugin
   next to its `RegisterPrompt` (`supervisor.rs:6434-6449`): what a marker
   line means, the kind → tool → paging table, "refer to it by title". ~150
   tokens, pure function of nothing (cache-stable, `registry.rs:344-345`). The
   per-message reference stays in the user text (≤ ~50 tokens; ids dominate).
   No outline/summary hint inline: every kind has a cheap outline call.
7. **One picker: cmdk (`components/ui/command.tsx`, already the dependency
   behind `NotePicker`, `ChatPicker`, `ModelMenu`, `SearchDialog`) inside a
   radix `Popover` on desktop and `Sheet side="bottom"` on phones
   (`BroadcastDialog.tsx:42-45` is the precedent).** Query → the `search` RPC
   (49) for sessions/artifacts/knowledge/history, client-side filtering over
   `doc_list`/`meeting_list` for notes/meetings until phase 5 adds their
   categories. Empty query → recents and "in this chat". Exclusive type tabs
   (All · Notes · Knowledge · Artifacts · Chats · Meetings), multi-select,
   preview pane on wide windows, 44 px rows under a coarse pointer.
8. **Inline trigger: `@` yes, `#` no,** phase 4, with the slash rule's
   discipline (`slash.ts:157-163`: no match → plain text; `Composer.tsx:207-208`
   opens the list only for a lone `/word`). `@` at a word boundary opens the
   same picker anchored to the caret; the word is left as typed unless a row
   is picked. `#` collides with markdown headings and issue numbers (§7.8).
9. **Names come from one module, `lib/attach.ts`:** `title(ref-source)` /
   `subtitle(...)` used by the picker rows, the composer chips, the history
   chips and the marker itself — so the agent inherits exactly the name the
   user saw. Fallbacks in §8. The TUI phase ports the same table to Rust.

## 1. Problem and goals

Today a chat can hold a *note* (59 §7.3) and nothing else. The user who wants the
agent to "keep working on the report it showed me last week", "look at what we
decided in Tuesday's meeting", or "pick up where that other chat left off" must
paste text, quote an id from the sidebar, or point at a path. The `+` menu is
three static buttons (`Composer.tsx:637-664`); the one picker it has filters a
500-row note list by title (`NotePicker.tsx:21, 55-61`) and the chosen thing is
rendered raw as `[doc … ]` in the bubble (`MessageUser.tsx:100-104`).

Goals: (a) attach anything searchable from one fast picker, on desktop and
phone; (b) never inline content — the agent fetches what it needs, in pieces;
(c) human names everywhere, including in what the model reads; (d) reattach and
see history easily; (e) no protocol churn: the marker convention that 45, 59 and
62 established carries this too.

Non-goals: session-lifetime pinning (ruled out), a new store or index, content
previews in v1, an "AI summarise this attachment" button, attachments on
assistant messages.

## 2. Current state (survey, 2026-09-24, base `main` 23e5691)

### 2.1 Composer and chips

- `+` is a radix `Popover` (`Composer.tsx:626-666`, `w-48 p-1`) with three ghost
  buttons: **Attach file** (`:657`; Tauri `pickFiles` filter "Images and PDF" on
  the local desktop, else the hidden `<input type=file>`), **Attach note…**
  (`:660` → `NotePicker`), **Send to many…** (`:663` → 62's dialog). No camera
  path; phones get whatever the OS file sheet offers.
- Held things are `Paste`s keyed by arrival (`lib/pastes.ts:2-13`): `text`
  (large paste, `⟦paste n⟧` token inline, `:15-18`, merge 9f97d62), `file`
  (daemon path from `put_attachment`), `doc {ref, title}` (`:6-7`, 59 §7.3).
  `attachDoc` dedupes by `ref` (`:65-68`); `docMarker` writes
  `[doc <ref> "<title>"]` with `"` escaped (`:70-73`); `expand` puts every
  doc marker first, one per line, then the body with tokens replaced, and
  returns `files` separately (`:97-114`). `Composer.submit` prepends a `pinned`
  doc's marker (`:402-403`) and calls `api.send_input(session, body, files)` /
  `api.steer(...)` / `api.use_skill(...)` (`:401-438`).
- `HeldStrip.tsx`: `TextCard` (`:24-78`, opens a dialog editor), `DocChip`
  (`:80-88`, a plain `<span>` — not clickable, no preview), file thumbnails via
  `Attachments` (`:110-111`); corner `X` → `untoken` + `drop`
  (`Composer.tsx:620-623`).
- Slash rules (Sky, 2026-09-24): the list opens only when the whole box is one
  `/word` (`Composer.tsx:207-208`); `isCommand` = built-in or known skill, else
  plain text (`slash.ts:157-163`). `SlashList` is bespoke (`Composer.tsx:63-93`),
  not cmdk.
- Drafts keep `{text, pastes}` across remounts (`lib/drafts`; `prefillDraft`
  `Composer.tsx:54-56`). `ChatPicker.tsx:22-64` is the reverse direction —
  "Attach to chat…" from a note/knowledge page picks a session and prefills.
- Drag: the composer accepts file drops (`Composer.tsx:315-333`); nothing in
  the app is a drag *source* (grep `draggable|onDragStart`: only `Panes.tsx:35
  draggable: false`).
- Existing "attach this" actions: knowledge page **Ask theuth…** / **Attach to
  chat…** (`knowledge/Document.tsx:422-423`), note page **Attach to chat…**
  (`notes/NotePage.tsx:125,136`), editor slash "Ask theuth…"
  (`editor/Editor.tsx:137-141`). `ArtifactCard.tsx:157-193` has Copy / Save as
  / Open / Open in explorer / Copy path — no attach. Session rows and meeting
  rows: none.

### 2.2 History rendering

`MessageUser.tsx:79-104`: only the voice marker is parsed (`parseNote`,
45 §7.4); `text` is otherwise shown verbatim, so `[doc abc "Groceries"]` is the
first line of the bubble. Files render as `Attachments` from
`context.attachments` (48 §3.3). `EditBubble` (`:14-53`) edits the raw text,
marker included.

### 2.3 Engine side

- **Nothing in Rust parses `[doc …]`.** The whole contract is the `document`
  plugin's static prompt block, `crates/document/src/tools.rs:20-32`:
  > `doc` is a document id or `knowledge:<concept id>`. A user line starting
  > `[doc <id> "<title>"]` means that document is attached to the
  > conversation: pass `<id>` as `doc`. Several `[doc …]` lines mean several
  > documents are attached; ask which one when the request is ambiguous.
  Same convention as `[voice note, N s]` (`crates/voice/src/lib.rs:138-147`)
  and `[broadcast b7f3 · 4 sessions]` (62 §0.4).
- A user turn is `send_input(session, text, files: Vec<PathBuf>, delivery)`
  (`crates/rpc/src/api.rs:157-165`), `steer(...)` (`:168-177`),
  `edit_turn(session, turn, text, files)` (`:296-301`). `files` must be
  image/PDF under the size caps (`supervisor.rs:6003-6031`) and become
  `ContentPart::Binary` after the text (`loop_.rs:4950-4962`). Persisted as
  one `ChatMessage` JSON in `messages.body` (`store.rs:1778-1786`,
  `0001_init.sql:9-13`); the only typed side-columns are `excluded`
  (`0010_context.sql:7`) and `vision` (`0014_vision.sql:6`).
- History projection: `HistoryEntry::User { text, turn, context, peer, vision }`
  (`lib.rs:1302-1328`, `supervisor.rs:5830-5839`).
- System prompt: `frame::baseline` (`frame.rs:250-305`) joins the header,
  `prompt.md`, skills, `peer_line`, then plugin blocks from `ListPrompts`
  (`registry.rs:343-357`, sorted by plugin name) — every block "a pure function
  of config … anything dynamic busts the system cache" (`registry.rs:344-345`).
- The only custom URI in the tree is `theuth://notes/<id>` returned by
  `doc_create` (`document/src/tools.rs:448`); nothing resolves it.

### 2.4 Stores and identities

| Store | Identity | Human name | Date/size fields | Agent read tool |
|---|---|---|---|---|
| Documents (`documents/`, `docs` index `document/src/store.rs:28-31`) | `DocId`, 26-char base32 UUIDv7 (`meta.rs:12-45`); kinds knowledge/artifact/meeting_note/note (`:69-78`) | `title` (`title_of` `actor.rs:765-796`: frontmatter → first line ≤ 80 → `""`) | `created_ms`, `updated_ms`, `bytes` (`DocSummary`, `meta.rs:322-337`) | `doc_read {doc, format, blocks, depth}` (`tools.rs:133-198`); resolves DocId / `knowledge:<id>` / bare `a/b` (`:65-83`) |
| Knowledge (`knowledge/`, FTS5 `knowledge/src/store.rs:38-44`, git history) | concept id `facts/slug`; revision = `id@<commit>` (49 as-built prints it, `rpc/src/search.rs:754-770`) | `Meta.title`, "the id when absent" (`types.rs:62-82`) | `timestamp`; FTS `size`/`mtime` | `knowledge_get {id}` (`tools.rs:135-148`); revision read is RPC-only `knowledge_read_revision` (`api.rs:685-688`) |
| Artifacts (`artifacts/`, `artifacts.sqlite`) | **`call_id` PK** (`0001_artifacts.sql`); `id` = sha16 of bytes and blob stem; `version` per `(session,name)` (`lib.rs:136-164`) | `title` (arg → frontmatter → first `#` → `name`, `store.rs:401-403`) | `bytes`, `pages`, `created_at` (unix s), `kind` of 11 (`lib.rs:64-107`) | **none by id**; `read_file(<path>)` only. RPC `get_artifact(call_id)` `api.rs:1215`, `get_artifact_listing` `:1221` |
| Sessions (`sessions.sqlite`) | `SessionId` UUIDv7 | `title` = first input ≤ 60 chars (`loop_.rs:957-962`), later model-written with `tags` (07 §15), or `rename_session` | `created_at`, `updated_at`, `project`, `archived`, `retention` (`SessionSummary.ts`) | **none** (only `search`); `load_session` is UI RPC (`api.rs:375-378`) |
| Meetings (`meetings/<id>/`, `meetings.sqlite`) | `MeetingId` UUIDv7 | `title: Option` → `process_name` (`meeting/src/tools.rs:474`, `store.rs:430`); the summary turn sets it (`summary.rs:179-181`) | `started_at`, `ended_at`, `duration_ms`, `state` (`MeetingSummary.ts`) | `meeting_read {meeting, from_ms, to_ms, speaker}` ≤ 400 lines (`tools.rs:227-259`), `meeting_search`, `meeting_list`; the note is a document (`Binding::MeetingNote`, `meta.rs:101-114`) |

Unified `search` (`crates/search`, `rpc/src/search.rs:149-225`): exactly four
categories — `sessions | artifacts | knowledge | knowledge_history`
(`search/src/lib.rs:70-91`), nucleo fuzzy with fzf syntax, 10/50 per category,
blank query → no groups (`:157-159`). Hits carry what a picker row needs:
session `title, updated_at, archived` (`agent/src/search.rs:189-219`; no
`project`), artifact the full `Artifact` (`artifact/src/search.rs:54-61`),
concept `Meta`, history `{id, title, commit, time, …}` (`knowledge/src/search.rs:66-88`).
**Gaps:** no `notes` (59 §19.3 dev. 1) or `meetings` category; no browse mode
(empty query returns nothing); artifact hits are per `call_id` (each version a
row — which is what we want to attach anyway).

### 2.5 Platform

Phones run the same bundle: Tauri iOS shell (`tauri.ios.conf.json`) and the
relay serving `ui/dist` at `/d` (`relay/src/web.rs:32,107-113`,
`main.tsx:25-26`); a paired device speaks mode-0 JSON-RPC over Noise with the
owner's rights (31 §4.3, `31-link.md:316,521-522`). Detection: `touch()` =
`pointer: coarse` (`lib/platform.ts:23`), `useIsMobile()` = width < 768
(`hooks/use-mobile.ts`), `useKeyboardInset()` (`lib/mobile.ts:37-53`). Bottom
sheets are `SheetContent side="bottom"` (`components/ui/sheet.tsx:43-64`).

## 3. The attachment model

### 3.1 The reference

```ts
// lib/attach.ts
export type Kind = 'note' | 'knowledge' | 'artifact' | 'session' | 'meeting'
export type AttachRef = {
  kind: Kind
  id: string          // DocId | concept id[@commit] | artifact call_id | SessionId | MeetingId
  title: string       // what the user saw (§8); never empty
  meta: string        // "pdf v2 · 3 pages · 84 KB · Sep 21" — display-only, may be ""
}
```

`id` is the stable identity the read tool for the kind accepts — nothing is
resolved on attach. `title`/`meta` are frozen at attach time (history shows what
was attached; opening a chip fetches the live thing). No `locator` field: the
sub-address (block id, line range, turn, ms window) is the agent's business
after it reads the outline; the user can say "the second section" in words.
A knowledge revision is `kind: 'knowledge', id: 'facts/x@3f2a9c1…'` — the same
spelling the `search` tool prints and `knowledge_get` will accept (§6.3).

Versioning, per kind:

| Kind | Reference points to | Why |
|---|---|---|
| note, knowledge | latest | the user wants the agent to *continue working on it*; the doc tools edit the live Y.Doc (59 §0.2). A pin would make the agent edit a stale copy or refuse to edit. |
| knowledge `@commit` | that snapshot, read-only | chosen deliberately from history; `knowledge_get` says "revision of <date>, read-only; latest is `knowledge_get(<id>)`". |
| artifact | that `call_id` = one version | immutable by construction (35 §3.3). The picker shows the latest version of each `(session,name)` by default with `vN`; older versions are reachable from the Artifacts tab's version list. `artifact_read` reports "v2 of 3; latest is <call_id>". |
| session | the transcript as it is at read time | it grows; the reference is the id. |
| meeting | the meeting (immutable once summarised) | id. |

### 3.2 Kinds surveyed and excluded

| Candidate | Decision | Justification |
|---|---|---|
| Meeting notes (`DocKind::MeetingNote`) | covered by **meeting** | attaching the meeting gives transcript *and* note/summary through `doc_read("meeting:<id>")` (§6.4); a separate kind would make the user choose. |
| Markdown artifacts bound to a document (59 §0.7) | **artifact** | `artifact_read` mentions the bound doc id so the agent can edit via `doc_read`. |
| Apps (`Kind::App`, 46) | **artifact** | an app is an artifact row; `artifact_read` returns its source (`AppDoc.source`, `app/src/lib.rs:175-178`). |
| Knowledge revisions | **knowledge@commit** | Sky's list; `search` already surfaces them; pinning a version is the whole point. |
| Meeting briefs (`briefs.sqlite`, 05-brief-cache) | **no** | a brief is a few bullets the meeting toolset injects itself; `brief_search` reaches it by title; attaching one adds nothing over naming it. Cheap to add later (kind + tool exist). |
| Schedules/loops (`crates/schedule`) | **no** | identity is a unique `name`; typing it *is* the reference; `schedule_status {name}` reads it. |
| Skills | **no** | `/skill:<name>` already parks a skill on the next send; "attach a skill" would mean the same thing with a second gesture. |
| Todos (47) | **no** | no id; per-session; visible in the todo panel. |
| Decisions (`crates/decision`) | **no** | no tool, no content to read (RPC stats only). |
| Files (images/PDF) | stay `files` | 17/20 native `Binary` parts; a reference would be strictly worse for a picture. |

### 3.3 Serialisation: marker lines (recommended) vs a structured array

**Recommended: marker lines**, the 59 §5.4 grammar generalised.

```
[attached <kind> <id> "<title>" · <meta>]
```

- Leading lines only, one per attachment, arrival order (pinned first), then a
  newline and the typed body — exactly where `expand` puts doc markers today
  (`pastes.ts:113`). Parsers stop at the first non-marker line, so a bracketed
  line typed *after* the first sentence is never misread.
- `<kind>` ∈ the five; `<id>` is `\S+`; `<title>` has `"` and `\` escaped as
  `\"`/`\\`, no newlines; ` · <meta>` is optional and opaque (`[^\]]*`).
- Legacy accepted by every parser: `[doc <DocId> "<title>"]` → note,
  `[doc knowledge:<id> "<title>"]` → knowledge. Existing sessions render as
  chips; the document prompt block keeps one sentence saying both spellings
  mean the same. New sends use the new form (NoteChat's `pinned` included).
- One grammar, two implementations, one test table: `lib/attach.ts`
  `marker(ref)` / `parse(text) → {refs, body}` (vitest) and
  `agent::attach::{marker, parse}` (Rust, used by `session_read`'s outline to
  show attachments as chips-in-text, by the TUI later, and by any channel that
  wants to render). Same fixtures on both sides.

Why not a structured `attachments: Vec<AttachRef>` on `send_input`/`steer`/
`edit_turn` plus a `messages.attachments` column (the `vision` pattern)?

| | Marker lines | Structured array |
|---|---|---|
| Protocol | none | 3 RPC methods, ts-rs bindings, TUI `Command::Send`, ACP `prompt_parts`, Slack/Signal channels, `broadcast` |
| Persistence | none | migration + `HistoryEntry::User.attachments` |
| Model sees | the line (it is user text; §5) | text the engine renders from the array — identical bytes, one more place to keep in sync |
| Reload / fork / edit_turn / regenerate | free (it is the text) | each path must carry the array |
| Search | session index already covers user text ("where did I attach the Q3 report?" works) | needs indexing |
| Parsing risk | a user typing a well-formed marker on line 1 attaches — acceptable, it is what they wrote; malformed → plain text | none |
| Other clients (TUI, Slack, Signal, `t -p`) | can attach by typing today | need the field plumbed |
| Precedent | 45, 59, 62 all chose markers; 62 §0.4 argued it | — |

The array's one real advantage — unambiguous parsing — is bought by the strict
leading-line rule and escaping. If a future attachment must carry bytes or a
typed record no text can hold, `0014_vision.sql` is the template; nothing here
forecloses it.

## 4. What the client sends

`Paste` gains `{ kind: 'ref'; ref: AttachRef }` and loses `doc`
(`pastes.ts:6-7`); `attachDoc`/`docMarker`/`DocRef` become
`attachRef`/`marker`/`AttachRef` (dedupe by `kind+id`). Callers to migrate:
`Composer.tsx:23,54-56,402-403,605-608`, `HeldStrip.tsx:6,80-88,104-109`,
`NotePicker.tsx`, `ChatPicker.tsx`, `knowledge/Document.tsx:92-97,257`,
`NotePage.tsx:71,125,136`, `NoteChat.tsx`, `pastes.test.ts:85-91`. `expand`
emits `marker(ref)` lines in place of `docMarker`; everything else in
`pastes.ts` (tokens, `reconcile`, `untoken`) is untouched — refs are chips
without tokens, as docs are today.

The wire is unchanged: `send_input(session, "<markers>\n<body>", files)`.

## 5. What the agent receives

### 5.1 Per message — exactly the marker lines, in the user turn

```
[attached artifact toolu_01HZ3v7Q "Q3 pipeline report" · pdf v2 · 3 pages · 84 KB · Sep 21]
[attached session 0192a4c1-7e3b-7c0e-9c1f-2b6d1e8f4a10 "Fix relay reconnect loop" · chat · theuth · updated Sep 19]
[attached meeting 0192b0f2-0a1d-7e55-8d3a-6f2c9e1b7d44 "Weekly planning" · Sep 23 · 42 min]
[attached knowledge decisions/prod-dr-scale-to-zero@3f2a9c1 "ICD prod DR — scale to zero" · revision of Sep 12]
[attached note agqmlbgmk57ybm2rdgyj7kcery "Groceries" · updated Sep 24 · 1 KB]
can you fold the meeting's action items into the note and check the report's numbers against what that chat concluded?
```

Budget: 25–50 tokens per line, dominated by the id (a UUID is ~20 tokens; a
`DocId` ~12; a concept id ~5). Nothing else is inlined. No outline hint: each
kind's first read is one cheap call, and an inline hint would either be stale
(living docs) or paid on every message.

### 5.2 Standing prompt block (static, `RegisterPrompt { plugin: "attachments" }`)

> **Attachments.** A user message may begin with lines
> `[attached <kind> <id> "<title>" · <details>]` (older messages:
> `[doc <id> "<title>"]`, the same thing for notes and knowledge). Each is a
> reference the user attached to that message — none of its content is in
> your context yet. When the request concerns it, fetch what you need with
> the tool for its kind, and read big things in pieces:
>
> | kind | first look | then |
> |---|---|---|
> | note, knowledge | `doc_read(<id>)` outline | `doc_read(<id>, format="markdown", blocks=[…])`; edit with `doc_insert`/`doc_replace`. `knowledge <id>@<commit>` is a past revision: `knowledge_get("<id>@<commit>")`, read-only. |
> | artifact | `artifact_read(<id>)` (header: name, kind, version, size; first 2000 lines or the rendered PDF/document) | `offset`/`limit`/`ranges`/`tail` like `read_file`; images come back as pictures; archives as a listing; a markdown artifact names its document id for `doc_read`. |
> | session | `session_read(<id>)` outline: numbered turns, one line each | `session_read(<id>, turns=[a,b])` for the full text of those turns. |
> | meeting | `meeting_read(meeting=<id>)` transcript (400 lines per call, `from_ms`/`to_ms`); `doc_read("meeting:<id>")` for the notes and summary | narrow by time or speaker. |
>
> Call it by its title in your prose, never by its id. Several lines mean
> several attachments; ask which when the request is ambiguous. A reference
> belongs to the message it came with; things attached earlier are in the
> transcript above. If a tool reports the item missing, deleted or
> unreadable, say so plainly.

Registered from the agent plugin beside its existing block
(`supervisor.rs:6434-6449`), so it is present in every mode (TUI, `-p`,
headless, ACP) and independent of which stores are enabled — a missing tool is
simply not in the toolset, and the model learns that on the first call. The
`document` block (`document/src/tools.rs:27-29`) drops its marker sentences in
favour of one: "A leading `[attached note|knowledge …]` or `[doc …]` line means
that document is attached; see Attachments."

## 6. Tools: new and changed

### 6.1 `session_read` — new, `Risk::ReadOnly`, agent crate

```
session_read(session: SessionId, turns?: [from, to], with_tools?: bool = false)
```

- Default (no `turns`): **outline** — one line per turn from the stored
  messages: `#<turn> <date> <role-first-line ≤ 120> → <assistant first line ≤ 120> · <n> tool calls`, attachments of that turn shown as `⎘ <title>` (via `agent::attach::parse`). ≤ 200 turns per call; `turns=[from,to]` also pages the outline. Header line: `[session_read "<title>" <id>: N turns, <project>, <created> – <updated>; showing turns a–b]`.
- `turns=[a,b]`: full user and assistant text of those turns, tool cards as
  their one-line summaries (`HistoryEntry::Tool` text), or full tool output
  with `with_tools=true`; images as `[image: <name>]` (`loop_.rs:5058-5092`).
  Output capped at `tools::fs::MAX_BYTES` (64 KiB) with the usual "narrow the
  range" tail.
- Built on the same store read `load_session`/history projection uses
  (`supervisor.rs:5830-5839`); a `ReadSession` message on `Agents`,
  authorised like `load_session` for the caller's owner (51 §4.2): a session
  inside a **private** root is readable only from a session of that same root;
  archived sessions are readable; `Retention` is reported in the header.
- Untrusted content: rows with `peer: Some(_)` (50 §9) and every tool body
  under `with_tools=true` are wrapped in 28 §6's fence *inline* (same id
  scheme, `[external content from session_read …]`), without setting
  `out.untrusted` — reading one's own past chat must not taint the current one
  (28 §0.1 would otherwise force approvals). Open question 7.

### 6.2 `artifact_read` — new, `Risk::ReadOnly`, artifact crate (beside `show`)

```
artifact_read(artifact: call_id, offset?, limit?, ranges?, tail?)
```

- Resolves the row (`Get`/`get_artifact` path, `api.rs:1215`), then by kind:
  text kinds (markdown, text, csv, svg, mermaid, html, file-as-text) and
  **pdf** → `read_file`'s renderer over `path` (`tools/src/fs.rs:60-186`;
  `doc::to_markdown` for PDF/Office), its functions made `pub(crate)`-to-`pub`
  so the artifact crate can call them — no second paginator; **image** →
  `ToolOutput.images` (52's vision fallback applies); **app** → the source
  (`AppDoc.source`); **archive** → the listing `get_artifact_listing` already
  computes (`api.rs:1221`).
- Header: `[artifact_read "<title>" (<name> v<N> of <M>, <kind>, <bytes>, shown <date> in session "<session title>"): …]`; when a document is bound (`doc_resolve(Binding::Artifact{session,name})`), adds `editable as document <DocId> (doc_read)`.
- Missing `call_id` → error text `artifact <call_id> not found (deleted or from another data dir)`; missing blob with a live row → `blob missing`.

Why not `read_file("artifact:<call_id>")`: `tools` is below `artifact` in the
dependency graph (artifact implements `tools::Tool`); a scheme in `fs.rs` would
invert it or need a callback registry. One tool in the owning crate is smaller.

### 6.3 `knowledge_get` — accept `id@commit`

`knowledge_get {id}` (`knowledge/src/tools.rs:135-148`): `split_once('@')` →
`ReadRevision {id, commit}` (`knowledge/src/search.rs:123-127`, already behind
RPC `knowledge_read_revision`); output header `revision <commit[..7]> of <date>
(read-only; latest: knowledge_get("<id>"))`. `commit` may be a unique prefix
(git semantics via gix). Same spelling as the `search` tool prints
(`rpc/src/search.rs:754-770`), so the model never reformats.

### 6.4 `doc_read` — resolve `meeting:<id>`

`Docs::resolve` (`document/src/tools.rs:65-83`) gains one arm: `meeting:<MeetingId>`
→ `Binding::MeetingNote(id)` lookup (`meta.rs:101-114`); "no note" → the
error names `meeting_read` for the transcript. Since 59 §0.9 the summary is
appended into the note, so this one id reaches notes *and* summary; no
`meeting_summary` tool.

### 6.5 Rejected: `attachment_read(ref, range?)`

One tool would have to accept block ids, line ranges, ms windows, speakers and
turn ranges in one schema, dispatch to four stores, and re-explain each paging
model in its description — the model would still need to know which sub-args
apply to which kind. The prompt table (§5.2) gives the same one-glance mapping
with tools that already exist and have tests. Revisit only if a sixth kind
arrives with no natural reader.

## 7. The picker

### 7.1 Placement and shell

- Desktop: radix `Popover` anchored to `+` (`align="start"`, opens upward),
  560 × 440 px; ≥ 960 px viewport adds a 280 px preview pane on the right
  (total 840). cmdk `Command` inside, as `ModelMenu.tsx` does
  (`Command` in `Popover`).
- Phone (`useIsMobile()`): `SheetContent side="bottom"` at `max-h-[90vh]`, input
  pinned at top, list scrolls, `useKeyboardInset` keeps the Done bar above the
  keyboard; 44 px rows (`pointer-coarse:min-h-11`, `SearchDialog.tsx:685`).
- One component `AttachPicker` mounted once in `Composer` (the `ContextPicker`
  pattern, `ContextPicker.tsx:119-126`), opened by `+` → **Attach…**, by the
  `@` trigger (§7.8) and by a `theuth:attach-open` event. The `+` menu becomes:
  **Attach…** (opens the picker on All), **Attach file**, **New note** (creates
  `Untitled` and attaches, today's `NotePicker` "New note"), **Send to many…**.
  `NotePicker` is deleted; `ChatPicker` stays (it picks a *chat*).

### 7.2 Desktop wireframe

```
┌──────────────────────────────────────────────────────────┬──────────────────────┐
│ 🔍 Search notes, knowledge, artifacts, chats, meetings…   │ Q3 pipeline report   │
│ [All] [Notes] [Knowledge] [Artifacts] [Chats] [Meetings] │ ┌──────────────────┐ │
│──────────────────────────────────────────────────────────│ │  (preview.png)   │ │
│ RECENT                                                   │ └──────────────────┘ │
│ ▸ ⎘ Groceries                        note · updated 2 h  │ PDF · v2 of 3        │
│   ▣ Q3 pipeline report            pdf · v2 · 84 KB · Sep 21│ 3 pages · 84 KB      │
│ IN THIS CHAT                                             │ shown Sep 21 in      │
│   ▣ relay-timings.csv              csv · 12 KB · Sep 24  │ "Q3 planning"        │
│   ☷ Fix relay reconnect loop        chat · theuth · Sep 19│                      │
│ NOTES                                                    │ Attached to 2 chats  │
│   ⎘ Meeting notes — Weekly planning  note · Sep 23        │                      │
│ KNOWLEDGE                                                │ ⏎ attach   ⌘⏎ add    │
│   ◈ ICD prod DR — scale to zero     decision · Sep 12    │ → preview  ⇥ tabs    │
│ ARTIFACTS  ‥ show more                                   │                      │
│ CHATS                                                    │                      │
│ MEETINGS                                                 │                      │
│──────────────────────────────────────────────────────────│                      │
│ ☑ 2 selected  · Groceries ×  · Q3 pipeline report ×     Done │                      │
└──────────────────────────────────────────────────────────┴──────────────────────┘
```

With a query, RECENT / IN THIS CHAT disappear and the type groups show hits.

### 7.3 Phone wireframe (bottom sheet)

```
╭──────────────────────────────────────╮
│ ━━━                                  │  drag handle
│ 🔍 Search…                        ✕  │
│ All  Notes  Know…  Artif…  Chats  Mtgs │  horizontally scrollable tabs
│──────────────────────────────────────│
│ RECENT                               │
│ ⎘ Groceries                     ○    │  44 px rows, tap = toggle
│   note · updated 2 h                 │
│ ▣ Q3 pipeline report            ●    │
│   pdf · v2 · 84 KB · Sep 21          │
│ IN THIS CHAT                         │
│ ▣ relay-timings.csv             ○    │
│   csv · 12 KB · today                │
│ …                                    │
│──────────────────────────────────────│
│              Attach 1                │  sticky above the keyboard
╰──────────────────────────────────────╯
```

Long-press a row → a preview sheet (title, subtitle, artifact thumbnail, Open,
Attach). Tap toggles selection; **Attach N** commits and closes; one tap on a
row with nothing else selected still requires **Attach 1** (no accidental
attaches while scrolling).

### 7.4 Data: browse and search

Empty query (browse), all fetched in parallel with react-query and shown as
they arrive:

| Section | Source | Rows |
|---|---|---|
| Recent | `localStorage['attach-recents']` per harness: last 20 refs attached from this device, newest first | ≤ 8 |
| In this chat | artifacts of the current session (`list_artifacts(session)`, latest version per `(session,name)`), plus refs parsed (`parse`) from the loaded transcript's user rows, deduped, newest first | ≤ 6 |
| Notes | `doc_list('note', 30)` (`api.rs:698-703`), `updated_ms` desc | 5 (All) / 30 (tab) |
| Knowledge | `knowledge_list(limit 30)` by `timestamp` desc; history only under the tab, via `search` with a query | 5 / 30 |
| Artifacts | `list_artifacts(undefined, true)` newest first, latest version per `(session,name)` | 5 / 30 |
| Chats | `list_sessions` roots, not archived by default (toggle "Show archived" in the tab), excluding the current session | 5 / 30 |
| Meetings | `meeting_list(30)` (`api.rs:1027`) | 5 / 30 |

Non-empty query (≥ 1 char after trim; 150 ms debounce, `SearchDialog.tsx:50`):

- `search(query, [sessions, artifacts, knowledge, knowledge_history], 10)` —
  one request, server-ranked (nucleo), the `has_more` → "show more" flow of
  49 §2.3 reused (10 → 30 → 50). Rows keep 49's provenance: an artifact hit
  shows `vN`; a history hit is offered as `knowledge …@commit`.
- Notes and meetings: client-side over the browse lists above with cmdk's
  built-in `command-score` on `title` (the lists are ≤ 500 and ≤ a few hundred
  rows; `NotePicker` already does this). **Phase 5** replaces this with
  `SearchCategory::Notes | Meetings` (§10) and the picker stops holding lists.
- Groups in fixed order (49 §0.4: no cross-type score), `indexing` pages show
  the 49 status line, a failed group shows its reason and Retry — the
  `Section`/`Group` components of `SearchDialog.tsx:733-840` are reused, not
  copied (extract to `components/search/`).
- Type tab = which categories are requested; All requests all. Tabs are
  **exclusive** here (unlike 49's multi-select chips): the picker is a
  one-handed "find the thing" surface; 49's independent per-category limits
  serve research, not picking. Recommendation, open question 4.

### 7.5 Row anatomy and previews

`[icon] Title` (one line, truncated) / `kind · <kind-specific facts> · date`
(muted, one line). Icons: note `StickyNote`, knowledge `BookOpen`, artifact by
kind (`FileText`/`Image`/`Table`/`Braces`/`AppWindow`/`Archive`), chat
`MessageSquare`, meeting `Mic`. Right edge: selection dot (phone) or checkbox
on hover/⌘ (desktop); a `vN` badge for artifacts; `Archived` marker for chats
(49's `Archived()`); `project` badge on chats and on artifacts from another
project than the current session's.

Preview pane (desktop ≥ 960 px; `→` or hover 400 ms): the title, full
subtitle, and per kind — artifact: `preview` PNG or image thumbnail
(`Attachments`' lightbox path), page count; chat: tags, project, created/
updated, `Archived`/`Private`; meeting: date, duration, state, session title;
note/knowledge: kind, updated, bytes, "attached to N chats" (count of
sessions whose search index contains the marker — v2, needs `search`). Text
previews (first lines of a note, first turn of a chat) are **not** in v1: they
need one fetch per hovered row (`doc_open`, `load_session`) and 49 §2.3 ruled
out speculative viewer fetch per row for good reason. Open question 10.

### 7.6 Selection, chips, removal

- Enter → attach the highlighted row and close. ⌘/Ctrl+Enter (desktop) or the
  checkbox → toggle and stay; the footer shows `N selected` with removable
  mini-chips; **Done** attaches all. Attaching the same `kind+id` twice is a
  no-op (`attachRef` dedupe).
- Chosen items become `ref` pastes → `HeldStrip` renders `RefChip`: kind icon,
  title, muted kind; click → a small popover with the subtitle and **Open**
  (route per kind: `/notes/$id`, `/knowledge/$id` (`?commit=` for a
  revision, 49's historical mode), the artifact `Viewer`, `/c/$session`,
  `/meetings/$id`); corner `×` → `drop` (no token to `untoken`). Chips sit in
  arrival order beside `TextCard`s and file thumbnails — the strip already
  mixes the three; `⟦paste n⟧` tokens are unaffected because refs have no
  token. A `pinned` ref (NoteChat) keeps its no-`×` rendering.
- Recents are recorded on send (`attach-recents`), not on pick.

### 7.7 Keyboard and accessibility

| Key | Action |
|---|---|
| `↑` `↓` | move (cmdk); crosses section headers |
| `⏎` | attach highlighted, close |
| `⌘/Ctrl+⏎` | toggle highlighted, stay |
| `⇥` / `⇧⇥` | next / previous type tab (input keeps focus) |
| `⌘/Ctrl+1…6` | jump to tab |
| `→` / `←` | open / close preview pane (desktop) |
| `⌫` on empty query | back to All |
| `Esc` | close (selection discarded) |

cmdk provides `role=listbox`/`option` and `aria-activedescendant`; tabs are a
radix `ToggleGroup type="single"` with `aria-label="Attach from"`; a polite
live region announces `N results in M groups` once per settled response
(49 §5); rows expose title and subtitle as one accessible name; focus returns
to the textarea on close; `prefers-reduced-motion` disables the sheet slide.

### 7.8 Secondary entry points

- **`@` inline trigger (phase 4; recommended yes).** When the caret's word
  starts with `@` and the previous char is start-of-text or whitespace, the
  picker opens anchored to the caret in quiet mode: the word after `@` is the
  query; `⏎`/`⇥` on a highlighted row replaces `@word` with nothing and adds
  the chip; `Esc`, a space, or a query with zero results across all groups
  leaves the text exactly as typed — the `/`-rule discipline. No `#`: column-0
  `#` is a markdown heading, `#123` an issue number, and `#tag` prose;
  a second trigger buys nothing `@` does not.
- **"Attach to chat…" everywhere a record is shown:** `ArtifactCard` ⋯ menu
  (`ArtifactCard.tsx:157-193`) gains **Attach to this chat** (adds the chip to
  the card's session composer via `theuth:attach {session, ref}`) and
  **Attach to chat…** (→ `ChatPicker`, generalised to take an `AttachRef`);
  session rows' `SessionMenu` and the meeting page/list rows gain **Attach to
  chat…**; `knowledge/Document.tsx` and `NotePage.tsx` switch their existing
  actions to `AttachRef`. Search results (49's dialog) get a row action
  **Attach to current chat** when a chat is open.
- **Drag and drop:** `artifacts/List.tsx` rows, notes list rows, sidebar session
  rows and meeting rows become `draggable` with
  `dataTransfer.setData('application/x-theuth-attach', JSON.stringify(ref))`;
  `Composer.onDrop` (`Composer.tsx:330-333`) checks that type before files.
  Desktop only (no HTML5 drag on iOS Safari worth fighting); the Tauri native
  drop hook (`:315-328`) is for OS files and unchanged.

### 7.9 A sent message

`MessageUser` calls `parse(text)`: `{refs, body}`. Refs render as a chip row
above the bubble (like `Attachments` for files, `MessageUser.tsx:84`), each
chip: icon + title, click → **Open** (the §7.6 routes), hover/long-press →
**Attach again** (dispatches `theuth:attach` to this session's composer) and
**Copy id**. The bubble shows `body` only. `EditBubble` edits the body and
re-prepends the original marker lines on send, with the chips shown above it
with `×` to drop one from the edited turn. Own optimistic sends (`markSent`)
render identically because the marker is the text (45 §7.4's argument).
Legacy `[doc …]` lines render the same chips.

## 8. Naming: one module, `lib/attach.ts`

`title(src)` and `subtitle(src)` take the record the picker row was built
from (`DocSummary`, `Meta`/`HistorySearchHit`, `Artifact`, `SessionSummary`/
`SearchHit`, `MeetingSummary`) and return the strings used by the row, the chip,
the history chip and the marker (`title`, `meta`). Dates are local, `MMM d`
this year and `MMM d, yyyy` otherwise; recency in lists uses the existing
`ago()` (`sidebar/AgentTree`); sizes `KB`/`MB` with one decimal below 10.

| Kind | Title | Fallback when empty | Subtitle (`meta`) |
|---|---|---|---|
| note | `DocSummary.title` | `Untitled note · <created MMM d>` | `note · updated <ago or date> · <bytes>` |
| knowledge | `Meta.title` (already the id when absent, `types.rs:66-70`) | — (id) | `<type> · <id> · <timestamp date>`; revision: `revision <commit[..7]> · <time date>` |
| artifact | `Artifact.title` (`store.rs:401-403` never leaves it blank) | `name` | `<kind> · v<N> · <pages> pages · <bytes> · <created date>`; from another session: ` · in "<session title>"` |
| session | `SessionSummary.title` (first user line after any marker lines ≤ 60 until the title model runs, `agent::attach::{first_line, seed_title}`; a markers-only first message is named by its first attachment's title) | `Untitled chat · <created MMM d>` (as built: one rule, `agent::attach::session_title` / `lib/attach.ts` `chatName`, for search hits, `session_read`'s header, the TUI and the UI; a hit or `session_read` names a title-less session by its first user line before the date) | `chat · <project ?? "no project"> · updated <date>` + ` · archived` |
| meeting | `title ?? process_name` (`meeting/src/tools.rs:474`) | `Meeting · <started MMM d>` | `<started MMM d> · <duration min> min` + ` · <state>` if not summarised |

The same table is the fixture for `attach.test.ts`. Rust duplicates only the
fallback rules `session_read`/`artifact_read` need for their headers (they
have the record in hand) and, in the TUI phase, the whole table as
`tui::attach` with the same fixtures.

## 9. Permissions and safety

- **Untrusted text inside attached things.** Shared-document member blocks are
  already labelled by `doc_read` (`document/src/tools.rs:30-32`, §5.2 keeps
  the rule); `session_read` fences peer rows and tool bodies inline (§6.1);
  `artifact_read` of an artifact whose `origin` was a `read_url`/scrape output
  is ordinary file text — 28's taint applied when it was *fetched*, and the
  saved artifact is the owner's record; no re-taint on read. `meeting_read` is
  unchanged (transcripts are the user's own recordings).
- **Missing or deleted at read time.** Notes: `deleted_ms`/`.trash` → `note
  <id> was deleted <date>`; artifacts: row without blob → `blob missing`;
  sessions: unknown id → `not found`; meetings: manifest gone → `not found`.
  The chip in history stays; **Open** shows the store's "deleted" page
  (`NotePage` already has one, 59 §19.4 row 5).
- **Cross-project.** Attaching is harness-wide, like `search` (49 §4.4 "the
  caller's session is audit context, not a filter"); the picker badges rows
  from another project; the agent's read tools do not filter by project.
- **Private trees (51 §4).** The picker hides sessions of a private root unless
  the composer's session is in that root; `session_read` refuses across that
  boundary (§6.1). Artifacts and notes created from a private session are
  under its scratch root and listed only there (51 §4.5), so they never appear
  elsewhere.
- **Relay / phone.** A paired device is the owner (mode 0, 31 §4.3); the picker
  calls the same RPCs the sidebar already calls from the phone; nothing new
  crosses the relay. Guests (document members, 59 §8) have no chat and no
  composer. `search` over the relay returns snippets the phone already shows.
- **Size.** References are ≤ ~50 tokens; reads are bounded by the tools
  (`read_file` 64 KiB, `meeting_read` 400 lines, `session_read` 64 KiB /
  200 turns, images ≤ 5 MB by the store). The picker fetches lists of ≤ 500
  rows (`doc_list` cap) and `search` pages of ≤ 50.
- **No new write path.** Attaching writes nothing server-side; recents are
  client `localStorage`.

## 10. Data and protocol changes (minimal set)

| Phase | Change | Kind |
|---|---|---|
| 1 | none on the wire; `[attached …]` grammar in `lib/attach.ts` + `agent::attach` | text convention |
| 2 | tools `session_read`, `artifact_read`; `knowledge_get` accepts `@commit`; `doc_read` resolves `meeting:<id>`; prompt block `attachments`; `document` block shortened | tool registry only |
| 3–4 | none | UI |
| 5 | `SearchCategory::Notes`, `SearchCategory::Meetings` (`crates/search/src/lib.rs:70-91`), owners `document::search::SearchNotes` (title + derived export text, nucleo like the others) and `meeting::search::SearchMeetings` (title + segments FTS already exists — adapt to a `SearchPage`), `SearchGroup` arms (`rpc/src/search.rs:211-223`), `search` tool text lines, ts-rs bindings, `SearchDialog`/TUI exhaustive matches (59 §19.3 sized this at ~160–180 lines per category) | RPC enum extension, backward compatible (unknown category from an old client is already Invalid params, 49 §4.2) |
| 6 | TUI `/attach` (below) | TUI |

No migration. No change to `send_input`/`steer`/`edit_turn`, `HistoryEntry`,
the relay, ACP, or channels. `rpc::bridge` forwards nothing new.

**TUI equivalent (phase 6):** typed markers work from day one (the TUI sends
raw text). `/attach [query]` opens a `Menu::Attach` built on `tui::search::State`
(49's TUI menu: query, category filter, grouped rows, `Inspect` preview) whose
Enter inserts the marker line at the top of the input box as a held
`Paste::Ref` shown as `[⎘ attach n: <title>]` (the TUI's `paste_token` scheme,
`model.rs:1072-1077, 3015-3017`); the transcript view renders leading markers
as `⎘ <title> (<kind>)` lines. Same names via the ported table (§8).

## 11. Phases and verification

Each step ships alone and leaves the app working. Tests only where a plausible
bug would fail them; manual checks named per step.

| # | Scope | Verification |
|---|---|---|
| 1 | **Grammar and chips.** `lib/attach.ts` (`AttachRef`, `marker`, `parse`, `title`, `subtitle`); `pastes.ts` `ref` paste replaces `doc`, callers migrated (§4); `HeldStrip` `RefChip` with Open popover; `MessageUser` chip row + body-only bubble + `EditBubble` re-prepend; `agent::attach::{marker, parse}`; NoteChat `pinned` emits the new form. | vitest `attach.test.ts`: round-trip every kind, `"`/`\`/`·` in titles, legacy `[doc <DocId>]` and `[doc knowledge:…]`, a marker on line 2 is body, malformed line is body, `parse(marker+body).body` exact; §8 fixture table. Update `pastes.test.ts:85-91`. Cargo `agent::attach::tests` with the same fixtures. Manual: attach a note → bubble shows chip + words only; reload → same; `edit_turn` keeps the chip; old session with `[doc …]` shows a chip. |
| 2 | **Read tools.** `session_read`, `artifact_read`, `knowledge_get @commit`, `doc_read meeting:`, prompt block, document block trimmed. | Cargo: `agent/tests/session_read.rs` (outline paging, `turns` range cap, private-root refusal, peer row fenced, unknown id), `artifact/tests/read.rs` (text ranges header, pdf → markdown, image → `images`, archive → listing, app → source, missing `call_id`, bound document mention), `knowledge` unit (`id@prefix` resolves, unknown commit error), `document` unit (`meeting:<id>` → note, no note → error names `meeting_read`). Manual (`t -p`): attach each kind by typing the marker; the model's first call matches the §5.2 table; a 300-turn chat is read in two `session_read` calls. |
| 3 | **Picker v1.** `AttachPicker` (cmdk in Popover / Sheet), tabs, browse sections, `search` query path with 49's group states, client filter for notes/meetings, multi-select, keyboard map, preview pane (metadata + thumbnail), recents, `+` menu rewrite, `NotePicker` deleted. | vitest for the pure parts: browse merge/dedupe (latest artifact version per `(session,name)`, "in this chat" from transcript markers, recents ordering), tab → categories mapping, keyboard reducer. Manual checklist, desktop: open with `+`, type 2 chars → groups within 300 ms, `⇥` cycles tabs, `⌘⏎` twice → `2 selected`, Done → two chips, `×` one, send → one chip in history, click → opens; iPhone over the relay: sheet rises with the keyboard, tabs scroll, tap-toggle, long-press preview, **Attach 2**, chips visible above the box, send. |
| 4 | **Entry points.** `@` trigger; ArtifactCard / SessionMenu / meeting rows "Attach to (this) chat…"; `ChatPicker` over `AttachRef`; drag sources + `application/x-theuth-attach` drop; search-dialog row action; history chip "Attach again". | vitest: `@` word-boundary detector (`a@b` no, ` @q` yes, `@` at col 0 yes, `sky@x.com` no) and the leave-as-typed rule. Manual: type `@gro` → picker filtered → `⏎` → chip, text `@gro` gone; type `@` then space → text kept; drag an artifact card onto the composer → chip; "Attach again" on an old chip → chip in the box. |
| 5 | **Search categories `notes`, `meetings`.** Owners, enum, groups, tool lines, bindings, dialog and TUI arms; picker drops its client-side lists; "attached to N chats" preview count. | Cargo: owner page tests (title hit, body hit, deleted note excluded, meeting title/segment hit, `has_more`); `rpc/search` composition test with all six. vitest: `SearchDialog` exhaustive `ICON`/`Section` maps compile (tsc). Manual: 49's checklist item 4 for the two new categories; the picker's Notes tab shows body matches. |
| 6 | **TUI `/attach`.** | Manual in `t`: `/attach q3` → menu → `⏎` → `[⎘ attach 1: Q3 pipeline report]` in the box → send → transcript shows the `⎘` line; `t -p` with a typed marker unchanged. |

Steps 1–2 alone make every kind attachable by typing and readable by the agent;
3 delivers the product; 4–6 are polish in priority order.

## 12. Open questions for Sky (with recommended answers)

1. **Marker lines vs a structured `attachments` field?** Recommend markers
   (§3.3): zero protocol change, parity for every client, precedent in 45/59/62.
   Revisit only if an attachment must carry bytes.
2. **User-facing word for sessions: "Chat" or "Session"?** Recommend **Chat**
   in the UI (sidebar says chats; `/c/$session`), `session` in the marker and
   tool names (`session_read`, `search` output) — the model reads "session",
   the user reads "Chat".
3. **`@` inline trigger?** Recommend **yes for `@`, no for `#`**, phase 4, with
   the leave-as-typed rule (§7.8). If Sky prefers zero inline triggers, `+`
   and drag cover everything and phase 4 shrinks.
4. **Exclusive tabs or 49's multi-select chips?** Recommend exclusive tabs +
   All (§7.4). Consistency with the search dialog is the cost; one-handed
   picking is the benefit. Trivial to flip (`ToggleGroup type`).
5. **Pin notes/knowledge to a version at attach time?** Recommend **no**
   (§3.1): the agent edits the live document; a chosen revision is its own
   explicit reference (`@commit`).
6. **Briefs, schedules, skills as kinds?** Recommend **no for v1** (§3.2); the
   model is open — a kind is one enum value, one icon, one tool line.
7. **Does `session_read` taint the reader (28 §0.1)?** Recommend **no**: fence
   peer rows and tool bodies inline, no taint; the owner's own transcript is
   not external input. If Sky wants the conservative rule, set `untrusted`
   only when a peer row or `with_tools=true` output is actually included.
8. **Private sessions in the picker from a durable chat?** Recommend
   **hidden**, and `session_read` refuses across the boundary (§9); the
   private tree's own composer sees them.
9. **Attach artifacts from other projects?** Recommend **allowed with a
   project badge**; `search` is already harness-wide.
10. **Preview pane content in v1?** Recommend **metadata + artifact thumbnail
    only**; text previews once phase 5 gives the backend a snippet path per
    kind (then a preview is one `search` snippet, not a `doc_open`).
11. **Picker before or after the search categories (phase 3 vs 5)?**
    Recommend **picker first** with client-side notes/meetings filtering: lists
    are small and `NotePicker` already works this way; phase 5 removes the
    branch without changing the UI.
12. **Title staleness after a rename.** Recommend **history keeps the title as
    attached** (it is what was said); **Open** and the preview show the live
    title; the marker for a re-attach is regenerated from the live record.
13. **Should the engine validate marker ids on `send_input`?** Recommend
    **no**: the read tool reports missing items at read time (§9); a user who
    types a marker by hand gets the same behaviour as one who picked.
14. **Artifacts: attach `call_id` (one version) or `(session,name)` (latest)?**
    Recommend **`call_id`** — it is the row key, what `get_artifact` and
    `search` use, and the only immutable address; `artifact_read` tells the
    model when a newer version exists.

DONE DESIGN66
