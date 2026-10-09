# 144 NOTEBOOKS wave 2 — as built, U1: the dial, the context line, Insights, chips into sources, the way back to the notebook

Contract: `144-wave2-contract.md` §1 (types, consumed), §3, §4; design `144-notebooks.md` §4.3, §5.1–5.2, §6.3–6.4, §12.1–12.2.
Branch `feat/144-u1` off `feat/144-w2-base` 6ab1905f. UI only (`crates/desktop/ui`), no cargo. Paths below are under
`crates/desktop/ui/src/` unless noted.

## What landed (file:line)

**Sky's ruling — the way back to the notebook (contract §4)**
- `routes/notes_.$id.tsx:9-12` — `validateSearch` gains `nb?: string` beside `new?: true`.
- `components/editor/DocumentPage.tsx:28,40-96,104-132,216` — `NotebookBack` type (`{ to: '/notebooks/$id', params, search: { tab }, label, always: true }`);
  `Frame`/`DocumentPage` take `back?: NotebookBack`: when set, the `SectionRow` gets no list chevron and the row leads with
  chevron + "Back to <title>" at EVERY width (label `hidden md:inline` → the phone sees the standard chevron). The popout
  header ignores it (143's rule). `useNotebookBack(kind, item, nb)` → `{ back, chip }`: with `?nb` the back goes there
  ("Back to notebook" until `useNotebook` answers); without it, to the item's first notebook (`useItemNotebooks`), and
  `chip` = `in <N notebooks> ▾` (`DropdownMenu`, each item a `Link` to `/notebooks/$id?tab=`). Popout → neither.
- `components/notes/NotePage.tsx:32-33,65,86-92,135,145` — `Note` takes `nb`; `back={wayBack.back}`, the chip after the title; the `?new` replace
  keeps `?nb`.
- `components/sources/SourcePage.tsx:55-67,227-229` — `back` with `tab: 'sources'` replaces W4's `← notebook` text link; chip after the title.
  `Details` uses `useItemNotebooks('source', id)` instead of its own `useNotebooks` + `useQueries` copy.
- `components/notebooks/NotesColumn.tsx:64,113,140` — row `Link` and **+ New note** carry `search: { nb }`; `<OverviewsSection notebook={notebook} />`
  replaces the wave-3 placeholder (U2's component).
- `lib/citations.ts:57-64` `routeOf(cite, nb?)` — notes/sources routes carry `{ nb }`; `components/chat/Citations.tsx:48-55`
  `useOpenCite` reads the notebook from `useNotebookChat()` so chips clicked in a notebook chat carry it.
- `components/chat/MessageAssistant.tsx:213-221` `NotebookActions` — the Save toast gains **Open** → `/notes/$id?nb=` (`SavedAnswer.doc`).
- `components/notebooks/useNotebook.ts:14,69-97` (added only) — `useNotebook(id, enabled = true)`, `useItemNotebooks(kind, item)`
  (`notebook_list` + one `notebook_items_list` per notebook, `undefined` while loading), `useContextEstimate(notebook, session)`
  (re-fetched on `SourceEvent::Changed`; callers invalidate `['notebooks','estimate',notebook]` after a dial write).

**The dial (§5.1) and the context line (§5.2)**
- `lib/notebooks-rpc.ts:43-71,88-89` — `ContextApi` (the seven contract-§1 methods, positional params in the contract's order),
  `contextApi(client)` over `RpcClient.call`, `useContextApi()`; `notebookKeys.estimate(notebook, session)`, `notebookKeys.insightSkills`.
- `lib/notebook-context.ts` (new, 97 lines) — `useContextDefaults` (:59) (zustand + `localStorage['notebook-context']`, notebook → item →
  `{ kind, level }`; `set`/`clear`/`setAll`), pure `remembered(saved, notebook)`; the §5.2 words: `fmtTokens` (`12.4k`),
  `kindWords`, `contextLine(estimate)`; `EstimateItem = ContextEstimate['items'][number]`.
- `components/notebooks/NotebookContext.ts:29,43-47` — `useOpenThread` moved here from `ChatColumn.tsx` (same key `notebook-chat`);
  `useNotebookSession(notebook)` = the remembered thread else the newest chat else `null` — the sources and notes columns are
  the chat column's siblings in `NotebookPage`, so they derive the session instead of receiving it.
- `components/sources/SourceCard.tsx:38,100-126,128-174` — `useDial(notebook)` (`set`/`clear`/`setAll`: the device default always, the session's
  row when a thread exists, then the estimate invalidates); `Dial` (radiogroup `off · mention · summary · full`, selected =
  the estimate's effective level, dot + `×` when `pinned`; without a session the remembered default shows pinned; right-click
  → clear; phone → `O M S F`); cards carry `id="item-<id>"`, a `truncated`/`omitted` pill, a dashed border when omitted.
  `context?: EstimateItem` prop; the guest's read-only card renders no dial (unchanged).
- `components/sources/SourcesColumn.tsx:14-47,50-79` — `BulkRow` (`all sources:` → `notebook_context_set_all(session, notebook, 'source', level)`
  + `setAll` on the device; `~<tokens> tokens` = the kind's non-omitted tokens); cards get their estimate item.
- `components/notebooks/NotesColumn.tsx` — `BulkRow` for notes under the header (`kind: 'note'`), a `Dial` per row, `id="item-<id>"`.
- `components/notebooks/ChatColumn.tsx:66-78,138-192` — `ContextLine` (exported): `in context: …` from `contextLine()`, the `N omitted` chip
  (`document.getElementById('item-<first omitted>').scrollIntoView`), the budget bar (`role="progressbar"`, amber ≥ 80 %,
  red when any item is `truncated`), `empty` → the sentence in amber; wraps on the phone (`basis-full md:basis-auto`).
  `onCreated` (a new thread): `session_set_notebook`, then `notebook_context_set` for every remembered item, then the
  estimate and chats refresh.
- `· 1 insight` in the origin line: already W4's `originLine` over `NotebookSource.insights` (no change).

**Insights tab (§4.3) and chips into sources (§6.4)**
- `components/sources/SourcePage.tsx:108-135,137-218` — `insightSections(blocks)` (pure, exported): the `###` sections under `## Insights`,
  `id` = the heading block (what `insight_promote` takes), name · model · date from the heading text. `Insights` reads the
  blocks the way the board reader does (`useBlocks(provider)` from `components/board/blocks`), **Run ▾** = `insight_skills()`
  (builtins labelled `Key insights · Table of contents · Open questions`, then a `Skills` group) → `insight_run(source,
  { builtin } | { skill })` with a `Running…` state (the section lands through the document stream); **Promote to note** →
  `insight_promote(source, headingBlock, nb ?? null)` → toast with **Open** → `/notes/$id?nb=`. Empty: "No insights yet — Run ▾ generates one."
- `components/chat/Citations.tsx:57-115,135` — a chip whose target is `missing` (or `!available`) is `data-dead` and opens a `Popover`
  card ("This passage changed or was removed" + **Open the source** without the locator; "This <kind> is unavailable")
  instead of navigating to a block that pulses nothing; `changed` adds "source changed since this was cited" to the chip's
  tooltip and `data-drift`; `moved` opens the document without the hash. References rows follow the same rule.

## Proof

Vitest render tests (static markup, as `NotesColumn.test.ts`/`SourcesColumn.test.ts` do; no DOM environment in this tree):
- `components/editor/DocumentPage.test.ts` — with `?nb`: `aria-label="Back to Vendor review"` + the label span, no `md:hidden`,
  link to `/notebooks/$id` `{ id: nb2 }` `{ tab: 'notes' }`; without `?nb`: back to the first notebook and the chip lists
  `DR failover`, `Vendor review`, every link `tab: 'sources'`; no membership → the phone-only list chevron, no chip.
- `components/notebooks/NotesColumn.test.ts` — every row link carries `nb`; a dial per row (pinned full → `data-pinned`,
  off → `omitted` pill), the bulk row with `~1.2k tokens`; the wave-1 ordering/AI-line test kept.
- `components/sources/SourcesColumn.test.ts` — live bulk row (`~12.4k tokens`, no `title="wave 2"`), `id="item-…"` anchors,
  default-rule `summary` selected without a dot, pinned `full` with dot + `×`, `truncated` pill; no estimate → `~— tokens`.
- `components/sources/SourcePage.test.ts` — `back` = `/notebooks/$id` `{ id: nb1 }` `{ tab: 'sources' }`; `insightSections` on
  a 10-block document; the Insights tab renders both sections, `Promote to note` ×2, Run ▾ with the three builtins; empty state.
- `components/notebooks/ChatColumn.test.ts` — `ContextLine` with a mocked estimate: the exact §5.2 sentence, `1 omitted` chip,
  `aria-valuenow=12400`, 31 % bar; amber at 33k/40k, red with a `truncated` item; `empty` → the sentence; loading/error rows.
- `components/chat/Citations.test.ts` — `missing` → popover card + **Open the source**; `missing` + `!available` → "unavailable",
  no open; `changed` → `data-drift="changed"` and the tooltip note; `same` → plain chip.
- `lib/notebook-context.test.ts` — set/setAll/clear per (notebook, item), localStorage round-trip; `contextLine` words; `fmtTokens`.
- `lib/citations.test.ts` — `routeOf(cite, nb)` carries `{ nb }` for notes/sources only.
Screenshots: the closer's run (no dev server from a worker).

## Gates

- `/tmp/144/w2/ready-ui` landed at 01:00 (the orchestrator's `bun install` ran in a directory without a `package.json`, rc=0
  but nothing installed), so `BUILD_LABEL=144-U1 build-lock bun install --frozen-lockfile` was run here in `crates/desktop/ui`
  (622 packages) and in `packages/apps` (484 — the `@openuidev/*` deps tsc needs, as W4 found). Binding stubs copied from
  `/tmp/144/w2/u1-stubs/` over the wave-1 `ContextEstimate.ts`/`NotebookContextItem.ts`/`ContextStatus.ts` (gitignored).
- `BUILD_LABEL=144-U1 build-lock bun run check` → rc=0, **0 errors** (`/tmp/144/w2/U1-tsc.log`).
- `BUILD_LABEL=144-U1 build-lock bunx vitest run src/components/sources src/components/notebooks src/components/editor
  src/components/notes src/components/chat/Citations.test.ts src/lib` → **93 files / 550 tests passed** (`/tmp/144/w2/U1-vitest.log`;
  one attribute-order assertion in `DocumentPage.test.ts` loosened after the first run, re-run green: 3/3).
  New: `DocumentPage.test.ts` (3), `ChatColumn.test.ts` (2), `SourcePage.test.ts` (3), `lib/notebook-context.test.ts` (2).
  Changed wave-1 tests: `NotesColumn.test.ts` (the Overviews placeholder assertion dropped — U2's section renders null; `nb` and dial
  tests added), `SourcesColumn.test.ts` (`title="wave 2"` → the live bulk row; two sources; dial/anchor/no-estimate tests),
  `Citations.test.ts` (drift test added; existing assertions untouched), `lib/citations.test.ts` (`routeOf(cite, nb)` cases).
- Prettier: the UI has no prettier config or `format:check` script (prettier is only a transitive dependency) — not run.

## Deviations

- **Binding names.** The contract's `ContextItem` collides with `agent::ContextItem` (`ContextItem.ts`, 48 §3.2); the base
  already exports the estimate item as `NotebookContextItem` (`notebook.rs:351-357`). UI code never names it: it uses
  `ContextEstimate['items'][number]` (`EstimateItem`), so whichever TS name E1 keeps compiles. Hand-written stubs
  (gitignored, `/tmp/144/w2/u1-stubs/`): `ContextStatus`, `NotebookContextItem` (contract fields `id, kind, title, level,
  pinned, tokens, status`), `ContextEstimate` (`items, total_tokens, budget_tokens, omitted, empty`), `InsightInstruction`
  (`builtin: string | null, skill: string | null`), `InsightRun`.
- **RPC module.** `rpc/api.ts` is not mine and `api.test.ts` pins `METHODS` to `methods.json`, so the seven methods live in
  `ContextApi` over `RpcClient.call` (W4's wave-1 shape), not in `Pick<Api>`; one hunk for the closer under Hand-offs.
- **Session for the dial.** `NotebookPage` (U2) renders the three columns as siblings; the session is derived in each column by
  `useNotebookSession` (store + `notebook_chats`) rather than passed down — no U2 file touched.
- **Device defaults on a new thread** are written in `Composer`'s `onCreated`, which fires after the first `send_input`
  (`Composer.tsx:808`); the first turn may run on the default rule — the same seam wave 1's `session_set_notebook` already has.
  Moving it before send needs a `Composer` hook (not mine; Hand-offs).
- **`useNotebook(id, enabled)`** — an optional second parameter added to W4's hook (the back control has no id until membership
  loads). **`useItemNotebooks`** rather than the brief's `useNoteNotebooks`: the same hook serves sources (the Details tab reused it).
- **`Dial`/`BulkRow` live in `SourceCard.tsx`/`SourcesColumn.tsx`** and `NotesColumn` imports them — no new component file outside
  the owned list.
- **Bulk sets are remembered per item** on the device (the store has no "all" row); a bulk set with no sources yet is disabled.
- **`· 1 insight`** comes from `NotebookSource.insights` (W4's `originLine`), not from the estimate.
- **Changed-since on chips** keys off `cite_resolve`'s `changed` (the block's text hash), not `sources.changed_since` (one
  `source_get` per chip otherwise). **`moved`** opens the document without a hash: `Citation` has no `block` for the new id.
- **Dangling-ref card** is a `Popover` on the chip (the chat flow keeps its text); the card opens the document, not the block.
- **Tests** use `renderToStaticMarkup` (the tree has no jsdom/Testing Library); Radix menus/popovers are mocked to plain elements
  where a test needs their content.
- `MessageAssistant.tsx` was touched for the Save toast's **Open** only (the brief's `grep -rln "theuth://"` names it).
- Long-press on the phone: the dial clears on `contextmenu` (what a long-press fires on Android; iOS fires it for links only), so
  the `×` next to a pinned dial is the reliable phone path.
- The 40-minute fallback for the bindings was not taken: the orchestrator's export was visibly compiling (mbx warnings in
  `bindings.log`) when the timer passed; a second `export_bindings` build on the shared machine would only have slowed it.

## Hand-offs

- **Closer — `rpc/api.ts`**: add to `interface Api` and `METHODS` (contract §1 order of params):
  `notebook_context_set(session: SessionId, item: string, kind: ItemKind, level: ContextLevel): Promise<null>`,
  `notebook_context_clear(session: SessionId, item: string): Promise<null>`,
  `notebook_context_set_all(session: SessionId, notebook: DocId, kind: ItemKind | null, level: ContextLevel): Promise<null>`,
  `notebook_context_estimate(notebook: DocId, session: SessionId | null): Promise<ContextEstimate>`,
  `insight_run(source: DocId, instruction: InsightInstruction): Promise<InsightRun>`,
  `insight_promote(source: DocId, block: string, notebook: DocId | null): Promise<DocId>`, `insight_skills(): Promise<string[]>`.
  Then `lib/notebooks-rpc.ts`: `ContextApi = Pick<Api, …>`, `useContextApi = useApi`, delete `CONTEXT_METHODS`/`contextApi`.
- **E1** — `Citation` could carry `block: Option<String>` for `moved` so the chip opens the found block (§6.4 "the chip opens that block").
- **Composer owner** — a pre-send hook (`onBeforeSend(session)`) would let the device defaults land before the first turn.
- **U2 (`NotebookPage.tsx`)** — nothing required; the columns derive the session themselves.
