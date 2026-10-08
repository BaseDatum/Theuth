# 144 NOTEBOOKS — as built, W4: notebook page, source page, citation chips, Library tab

Contract: `144-notebooks.md` §6.3–§6.5, §8, §12, §13. Branch `feat/144-w4` off main `cb944b75`. UI only (`crates/desktop/ui`), no
cargo, no bindings regeneration. Paths below are under `crates/desktop/ui/src/` unless noted.

## What landed (file:line)

**Types and RPC (local mirror until `.bindings-ready`)**
- `lib/notebooks-types.ts` (92) — §13.1 shapes verbatim (`Notebook`, `NotebookItem`, `ItemKind`, `Source`, `SourceOrigin`,
  `SourceStatus`, `Citation`, `CiteKind`, `SourceEvent`, `ContextLevel`, `ContextEstimate`); i64/u64 as `bigint` like ts-rs.
- `lib/notebooks-rpc.ts` — `NotebooksApi` (the 22 §13.3 methods, positional params), `notebooksApi(client)` over `RpcClient.call`
  (:57), `useNotebooksApi()` (:66; the dev fixture may answer), `notebookKeys` (:75). `rpc/api.ts` untouched: its `METHODS` must
  equal the server's `methods.json` (`api.test.ts`), so the names live here until the contract worker extends `Api` (Hand-offs).
- `components/notebooks/useNotebook.ts` — `useNotebook`, `useNotebookItems` (live rows), `useSources` (:26; `SourceEvent`s
  patch cards in place, `Changed` also refetches), `useSource`, `useNotebookChats`, `useDescription` (`doc_markdown` of the
  Notebook document), pure `firstLine`, `sourceEventOf` (:71, tolerant of the tag's case), `applySourceEvent` (:82).
  `useNotebooks.ts` — `useNotebooks(archived)`, `useCreateNotebook`.
- `lib/notebook-columns.ts` — zustand + `localStorage['notebook-columns']` collapse state (`useNotebookColumns`), pure
  `layout(mobile, collapsed)` → tabs | `320px | 1fr | 320px` with 12 px rails (:35). Test `notebook-columns.test.ts`.

**Routes and Library** — `routes/notebooks.tsx` (`?archived`), `routes/notebooks_.$id.tsx` (`?new`, `?tab`), `routes/sources_.$id.tsx`
(`?nb`, `?tab`); `routeTree.gen.ts` regenerated (63 added lines, nothing else changed). `lib/nav.ts:9,22` — `Tab` gains
`'/notebooks'`, `GROUPS.library` gains `{ to: '/notebooks', label: 'Notebooks', icon: Notebook }` after Notes; sidebar untouched.
`components/editor/DocumentPage.tsx:37,153` — `backTo` unions widened with `'/notebooks'` (only change).

**Notebook page** (`components/notebooks/`)
- `NotebooksList.tsx` — `Page`/`Column` list: title, description first line, `N sources · M notes`, project chip, `ago`;
  **New notebook**, **Archived** filter; row menu Rename / Archive / Delete (preview → `Delete, keep sources` | `Delete with its N
  exclusive sources`).
- `NotebookPage.tsx` — header (inline rename → `notebook_update`, description, `project · …` / `shared · N members · live` from the
  Notebook document's `meta.members`, disabled **Ask across notebooks** / **Overviews ▾** `title="wave 3"`, `ShareAction` when the
  provider is up, collapse toggles); `layout()` grid (:111) with `SideColumn` rails (:155); phone tabs `Sources · N | Chat |
  Notes · M` (:122) with the composer under Sources.
- `ChatColumn.tsx` — `Composer` + `Transcript` (143's pieces) in column shape; `localStorage['notebook-chat']` remembers the open
  thread per notebook; `Chats · N ▾` from `notebook_chats`; **New chat**; `onCreated` → `session_set_notebook`; pinned
  `AttachRef { kind: 'notebook' }`; context line `in context: N sources · M notes` + wave-2 bar; `NotebookContext` around the
  transcript (`NotebookContext.ts`).
- `NotesColumn.tsx` — items ∩ `useNotes()`, newest added first; **+ New note** = `createNote` + `notebook_item_add` + pop-out
  (`noteOpenMode`); rows with `edited <ago>` or `AI from chat · N citations kept` (`isProvenanceLine` + `citationCount` over
  `doc_markdown`), `meeting note` / `board` badges; Library-note drop target (`DRAG_TYPE`); Overviews wave-3 placeholder.
  Test `NotesColumn.test.ts`.
- `NotebookGuest.tsx` + `components/share/GuestApp.tsx:154,186,215` — `/d` with `meta.kind === 'notebook'`: Sources read-only,
  centre "Answers the owner promoted" (notes whose blocks carry the provenance link) + "No composer here — ask the owner, or
  edit the notes.", Notes `live · you may edit|view only`; click opens the note/source in place; no composer/Add/dial/Overviews/Ask.

**Sources** (`components/sources/`) — `origin.ts` (pure: glyph, label, `originLine`, `statusWord`, `isMedia`, `detectUrl`,
`detectFile`; test `origin.test.ts`), `SourceCard.tsx` (mock `.src`; `changed since · re-fetch` and `failed · retry` →
`source_refresh`; disabled dial `title="wave 2"`; draggable `{kind:'source'}`), `SourcesColumn.tsx` (`Sources · N`, **+ Add ▾**,
disabled bulk row, empty copy; test `SourcesColumn.test.ts`), `AddSourceMenu.tsx` (File… → `putAttachment` → `source_add({path})`;
URL / Meeting… / Paste / Chat… / Confluence / Jira dialogs), `SourcePage.tsx` (`DocumentPage` read-only, tabs Content · Insights
(wave 2) · Details with Re-fetch, Open original, **In notebooks** + Remove, **Delete** with `also in N other notebook(s)`; media
scrubber placeholder).

**Citations** — `lib/citations.ts` (pure §6.1 grammar: `parseTheuthHref` :11, `collectCitations` :37, `routeOf` :54,
`locatorLabel` :71, `isProvenanceLine` :77; test `citations.test.ts`), `components/chat/Citations.tsx` (`CitationsProvider` :23
batches `cite_resolve` per message, `CitationLink` :46 chip / plain `<a>` otherwise, `References` :79, `usePulseBlock` :118,
`useOpenCite` :38; test `Citations.test.ts`), `MessageAssistant.tsx:59,299-313,325` (`a: CitationLink`, provider + footer,
`NotebookActions` :200 = **Save answer as note** / **Promote into … ▾**), `editor/Editor.tsx:172,206` (pulse on `#<block>`;
`theuth://` click → navigate), `board/BoardReader.tsx:402` (chip branch), `styles.css:808-` (`.theuth-cite`, paper chips numbered
by CSS counter, `.theuth-cite-pulse` 1.5 s).

**Dev fixture** — `dev/notebooks-fixture.ts`, on with `localStorage['notebooks-fixture'] = "1"` in dev: the mock's notebook,
seven sources, chats and notes borrowed read-only from the engine (`doc_list`, `list_sessions`); writes stay in memory.

## Proof

Bare `bunx vite --port 5244` (`VITE_THEUTH_URL` = the running engine, read-only), headless Chromium, fixture on. Nothing was
created, sent or deleted on the engine. Screenshots in `/tmp/144/w4/`:
- `library-notebooks.png` — Library tab **Notebooks** after Notes; rows with title, description first line, `7 sources · 3 notes`,
  `project · dr-2026`, `2h`. `library-archived.png` — `?archived=true` shows the archived one only.
- `notebook-desktop.png` (1600×1000) — header + three columns; `gridTemplateColumns` read back as `320px minmax(0px, 1fr) 320px`;
  seven cards with the mock's status words (`ready`, `transcribing 62 %`, `failed · retry`), origin lines, `changed since · re-fetch`,
  disabled dials; chat column `Chat · this notebook · Chats · 3 ▾ · New chat` with a real transcript (read via `load_session`) and
  the two notebook actions under the assistant message; `Notes · 3` with the engine's note titles; Overviews placeholder.
- `notebook-collapsed.png` — both sides collapsed: `12px minmax(0px, 1fr) 12px`, rails 12×904 px with `aria-label="Expand …"`.
- `notebook-add-menu.png` / `notebook-chats-menu.png` — the **+ Add ▾** items and the chats list.
- `source-content.png` / `source-details.png` — `/sources/$id?nb=` frame with `← notebook`, tabs, read-only paper (empty: the
  fixture source has no document on the engine), Details with origin/status/fetched/sha/changed since/size, Re-fetch, Open
  original, In notebooks + Remove, Delete.
- `phone-sources.png` / `phone-chat.png` (390×844) — tabs `Sources · 7 | Chat | Notes · 3`; composer pinned under Sources and Chat.
- Chips: no notebook-bound session exists on the engine, so no live `theuth://` text to render; proof is `Citations.test.ts`
  (plain anchor untouched; chip ordinal 1 with title; References with `moved`/dead rows) and the fixture's `cite_resolve`.
- Guest view: needs a relay share link to a notebook document — not reachable from this tree; proof is code + `promoted()` pure
  helper; the closer's run.

## Gates

- `BUILD_LABEL=144-W4 nice -n 10 build-lock bun run check` → exit 0, 0 errors (after `bun install` in `packages/apps`, whose
  `@openuidev/*` deps the fresh worktree lacked; generated bindings copied from the main checkout — both gitignored).
- `… build-lock bunx vitest run src/components/notebooks src/components/sources src/components/chat/Citations.test.ts src/lib`
  → 82 files / 496 tests passed (new: `citations.test.ts`, `Citations.test.ts`, `origin.test.ts`, `SourcesColumn.test.ts`,
  `NotesColumn.test.ts`, `notebook-columns.test.ts`, `useNotebook.test.ts`).

## Deviations

- `ChatColumn` composes `Composer` + `Transcript` directly instead of mounting `NoteChat`: NoteChat is a resizable bottom
  panel bound to `lib/note-chat`'s per-note localStorage; the notebook column is full-height and reads the thread list from
  the server (§8). Same two shapes (bar / transcript), same `Composer` props; `NoteChat.tsx` untouched.
- RPC param orders not written in §13.3 were chosen as: `notebook_create(title, description, project)`, `notebook_items_list(notebook)`,
  `notebook_item_add/remove(notebook, kind, item)`, `session_set_notebook(session, notebook)`, `source_add(notebook, spec, title)`
  with `spec = {path}|{url}|{text}|{meeting}|{session}|{mcp:{server,tool,args}}`, `source_list(notebook|null)`,
  `notebook_delete_preview → {notes, sources, exclusive_sources, chats}`. One file to fix if the contract differs (`lib/notebooks-rpc.ts`).
- Confluence/Jira add: `{ mcp: { server: 'confluence'|'jira', tool: 'get_page'|'get_issue', args: { id } } }` literals.
- `routeOf`: sessions drop the turn (`/c/$session` validates only `?seq`); meeting/source times go to the hash as `#t<ms>` (no `?t`
  search on `/meetings/$id`). Two lines to change when the routes gain them.
- Pulse uses BlockNote's own `data-id` (already on every `.bn-block`); no `data-block-id` added. Paper chips are numbered by a CSS
  counter per editor (same target ≠ same number inside a note; chat chips are exact).
- "In notebooks" on the source page and the delete preview are computed client-side from `notebook_list` + `notebook_items_list`
  (`ponytail:` O(notebooks)); no source→notebooks RPC exists in §13.3.
- Guest note titles come from one `EngineProvider` per note item (`doc_open`, `ponytail:` comment) — the mode-2 allow-list has no
  titled items call. Guest phone tabs default to Notes (no Chat tab for a guest).
- Awareness text (`Noel editing now`) on Notes rows is not rendered: it needs a provider per note; rows show `edited <ago>`.
- Collapse toggles sit in the page's `SectionRow` (right), not over the column headers (they would cover `+ Add ▾` / `+ New note`).
- `AttachChip.tsx:36-39,85-86` — tolerant of `notebook`/`source` refs (icon/label fallback, open → `/notebooks/$id` / `/sources/$id`)
  so the pinned notebook chip renders before `lib/attach.ts` gains the kinds (it crashed the page otherwise).
- `GuestApp` renders an empty pane while `meta` is unknown and the status is `connecting` (so a notebook never flashes a paper);
  a note/board still mounts `BlockEditor` the moment `doc_open` answers.
- Vite's file watcher did not fire in this worktree (shared machine); screenshots were taken after restarts.

## Hand-offs

- **Contract worker — `lib/attach.ts`**: `Kind` gains `'notebook' | 'source'`; `KINDS`, `KIND_LABEL` (`notebook: 'Notebook'`,
  `source: 'Source'`), the `ATTACHED` regex alternation, and `Source`/`title`/`subtitle`/`idOf`. Then `components/chat/AttachChip.tsx:13`
  `KIND_ICON` gains `notebook: Notebook, source: FileText` and the two `string` guards in `useOpenRef` can become `case`s; the
  `'notebook' as Kind` casts in `ChatColumn.tsx`, `SourceCard.tsx` go.
- **Contract worker — `rpc/api.ts`**: add the 22 methods of `lib/notebooks-rpc.ts` `NotebooksApi` to `interface Api` and `METHODS`
  (same signatures), then `useNotebooksApi()` can return `useApi()` and the wrapper shrinks to the keys.
- **Bindings**: when `.bindings-ready` exists, copy `…/bindings/rpc-bindings/*` over `src/rpc/bindings/` and `methods.json` to
  `bindings/`, switch the `@/lib/notebooks-types` imports (13 files; `grep -l notebooks-types src`) to `@/rpc/bindings/...`, delete
  the mirror, re-run tsc. Not done: the marker did not appear during this wave.
- **`packages/documents/src/schema.ts:34`** (outside this worker's tree) — without it BlockNote drops `theuth://` hrefs on
  import/paste, so paper chips in notes never appear:
  ```
  - isValidLink: (href: string) => !/^[a-z][a-z0-9+.-]*:/i.test(href) || /^(?:https?|ftps?|mailto|tel|callto|sms|cid|xmpp|code):/i.test(href),
  + isValidLink: (href: string) => !/^[a-z][a-z0-9+.-]*:/i.test(href) || /^(?:https?|ftps?|mailto|tel|callto|sms|cid|xmpp|code|theuth):/i.test(href),
  ```
- **Engine**: `SourceEvent` tags are read case-insensitively (`Changed`/`Progress`); confirm the bus frame shape is the flat
  `{ type, source, status | percent }` of §13.1.
