# 144 NOTEBOOKS — as built, U2 (wave 2): notebook pop-out, ask across notebooks, overviews, audio overview player

Contract: `docs/design/144-wave2-contract.md` §2 (engine names, E2) and §5 (this worker); design `144-notebooks.md`
§9, §10, §12.1; Sky's ruling 2026-10-09 00:02 (a notebook pops out into its own window like a note). Branch
`feat/144-u2` off `feat/144-w2-base` 6ab1905f. UI only (`crates/desktop/ui`); paths below are under
`crates/desktop/ui/src/` unless noted.

## What landed (file:line)

**Pop-out (Sky's ruling, built first)**
- `lib/popout.ts:15-16` — `PopoutTarget` gains `{ kind: 'notebook'; notebook }`: path `/notebooks/<id>`, label
  `popout-notebook-<id>` (`idOf`/`pathOf` :37, :54), parsed back from the URL (`popoutTarget` :90) so a reload of the
  popped window re-derives the target from the URL alone. `popoutSize` (:121) = 1100 × 720 clamped to
  `screen.avail*` for the notebook kind; `openPopout` (:127) passes it as `window.open` features in a browser (the Tauri
  host sizes by label — Hand-offs). Tests `lib/popout.test.ts:50-63`.
- `components/notebooks/NotebookHeaderActions.tsx:24` `popOutNotebook(id)` → `openPopout({ kind: 'notebook', notebook })`;
  the **Pop out** icon button (`PictureInPicture2`, label on hover) in the header group (:84) and the ⋯ menu (:61); hidden
  inside a pop-out and where no window can open (`canPopout()`, iOS). `NotebooksList.tsx:245-249` — **Pop out** in the
  Library row menu.
- `NotebookPage.tsx:81-86` — 143's rule: `isPopout()` → no back control (`SectionBand` already returns null); the three
  columns, chat, header actions and Share render as in the shell. Route state is `/notebooks/$id` + search only.
- Notes have no pop-out keyboard shortcut (`NotePage.tsx:153` is a button only), so none was added.

**Header actions** — `NotebookHeaderActions.tsx` (rendered by `NotebookPage.tsx:98` with Share as its `share` prop):
**Ask across notebooks** (opens `AskDialog`), **Overviews ▾** (`DropdownMenu`: Briefing · FAQ · Timeline · Mind map ·
Audio overview → `notebook_overview_run(notebook, kind)`, an item reads `running…` and is disabled while a row of that
kind is `running`), **Pop out**, then Share. Under 768 px (`useIsMobile`) one `⋯` menu (`aria-label="Notebook actions"`)
with Ask, an Overviews sub-menu and Pop out; Share stays its own button (it owns its dialog). The Ask dialog is URL state:
`?ask` on `/notebooks/$id` (`routes/notebooks_.$id.tsx`) — `true`, or the question typed after `/ask`.

**Ask across notebooks** (§9; contract §2)
- `lib/ask-rpc.ts` — `AskApi` (`notebook_ask`, `notebook_ask_list`, `notebook_overview_run`, `notebook_overviews`) bound on
  the connection's `RpcClient` (`askApi` :37, `useAskApi` :44 — see Deviations), `askKeys` (:50), `useAskList` (polls 5 s while
  a row is not `done`), `useOverviews` (5 s while `running`, else mount + focus), the pure `askEventOf` (:87; frame
  `{ type: 'NotebookAsk', session, event }`), `applyAskEvent` (:97), `useAskEvents` (:113; `on('NotebookAsk')` on the existing
  bus like `useSources` does for `SourceEvent`), `hitHref` (:127). Test `lib/ask-rpc.test.ts`.
- `components/notebooks/AskDialog.tsx` — scope checkbox list from `useNotebooks(false)` (default all; `scopeOrder` :17 puts
  the current notebook first, tagged `this notebook`), question `Textarea` (⌘/Ctrl+Enter sends), **Save the answer as a note
  into ▾** (`Select`, default don't save) → `notebook_ask(scope, question, save_into)` → `/ask/$session`. Test `AskDialog.test.ts`.
- `routes/ask_.$session.tsx` + `components/notebooks/AskView.tsx` — `SectionRow` with the question (from
  `notebook_ask_list`), `N notebooks · when · done|running`, **Open in the chat list** (`/c/$session`); `AskStructure` (:107):
  the plan as numbered steps (`planSteps` :59 — notebooks' titles + query), one `Collapsible` row per branch (`Branch n ·
  query`, `searching…` → `N hits`), hits as §6.1 chips (`hitsMarkdown` :54 → the chat renderer: `Streamdown` +
  `markdownComponents.a = CitationLink` + `References`, imported from `MessageAssistant`/`Citations`, not edited), the
  partial answer streaming into the row, the synthesis (`Answer.markdown`) through the same renderer until the session's
  own assistant turn is in the transcript, `Done.error` as a `role="alert"` danger box with **Retry** (re-runs
  `notebook_ask` with the row's scope and question). Underneath: the session's `Transcript` + `Composer` (loaded like
  `ChatColumn` does) — the user keeps asking; after a reload the structure is empty and the transcript carries the
  answer. Test `AskView.test.ts`.
- `components/chat/slash.ts:102,349-356` — `/ask [question]`: on `/notebooks/<id>` → `?ask` there (current notebook
  pre-checked), elsewhere → `/notebooks?ask` (all notebooks). Nothing else in the file touched.
- `routes/notebooks.tsx` + `NotebooksList.tsx:42-56,117-144` — sub-tabs **Notebooks · Ask** in the `SectionRow`
  (`?view=ask`); Ask = `notebook_ask_list()` rows (question, `N notebooks · when · done|running`) → `/ask/$session`, plus
  **Ask across notebooks** in the row; `?ask` opens the dialog over all notebooks. Test `NotebooksList.test.ts`.

**Overviews** (§10, §12.1; contract §2)
- `components/notebooks/OverviewsSection.tsx` (`{ notebook }`, rendered by U1's `NotesColumn`) — `OVERVIEWS` (:17, the five
  kinds with glyphs, shared with the header menu), `versionsOf` (:32), `useRunOverview` (:37); header `Overviews`; one row
  per kind, newest version first: glyph, `name · v<N> · <ago>`, status (`running…` / `failed · <error>` / `ready`), **Open**
  (`/artifacts/$callId`, design 59's viewer), **Regenerate** (same kind), older versions under `▾ versions · N`. The Audio row
  shows `<audio controls>` over `overview.mp3`'s bytes (`get_artifact(call_id).path` → `useFileUrl`: `asset://` on this Mac,
  else `get_attachment` → blob URL, 31 §9), the `note` ("one voice available — …") and the `overview-script.md` artifact of the
  same version beneath. Empty copy per the brief. Polling in `useOverviews`. Test `OverviewsSection.test.ts`.
- `components/artifacts/Viewer.tsx:302-312,348` — a `file` artifact with an `audio/*` mime renders `AudioBody`
  (`<audio controls>` + name · mime) instead of the download button; `lib/files.ts:21` types `.mp3` blobs `audio/mpeg`
  so the RPC path plays on WebKit.

**Routes** — `routeTree.gen.ts` gains `/ask_/$session` (hand-merged in the generator's order; the plugin rewrites it
identically on the next `vite` run — Gates).

## Proof

No dev server from a worker; render tests with `renderToStaticMarkup` (the `NotesColumn.test.ts` way) and pure tests:
- `lib/popout.test.ts` "144 wave 2: a notebook pops out …" — URL/label/parse round trip, size clamp.
- `NotebookHeaderActions.test.ts` — desktop buttons; `popOutNotebook` calls the helper with `{ kind: 'notebook', notebook }`;
  no Pop out inside a pop-out or without a window manager; the ⋯ menu under 768 px; the dialog follows `?ask`.
- `NotebookPage.test.ts` — shell: band + phone back; popped: neither, all three columns and the actions still render.
- `AskView.test.ts` — nothing before the plan; steps, branch rows, hit chips (`theuth://sources/<id>#<block>`), partials,
  answer (hidden once the transcript has the assistant turn), error alert with Retry.
- `ask-rpc.test.ts` — the frame filter, the fold, the chip href, positional params.
- `OverviewsSection.test.ts` — version grouping, statuses, Open hrefs, `▾ versions`, the audio row with `<audio controls
  src="blob:…">`, note and script, the empty copy.
- `NotebooksList.test.ts` — sub-tabs, the Ask rows, `?ask`.
- `AskDialog.test.ts` — scope order.
The closer takes the live screenshots (pop-out window at 1100 × 720, Ask run, Overviews with the player).

## Gates

(filled after the runs)

## Deviations

- **`lib/ask-rpc.ts` is not `Pick<Api, …>`** yet: the four §2 methods are not on `rpc/api.ts`'s `Api` (its `METHODS` is
  asserted equal to `bindings/methods.json`), so the module binds them on the connection's `RpcClient` (`client.call`,
  positional). Hand-off: the closer adds them to `Api`/`METHODS` and flips `AskApi = Pick<Api, …>`, `useAskApi = useApi`.
- **Tauri pop-out size** is set by label in `crates/desktop/src/commands.rs::open_window` (outside this worker's tree): a
  notebook currently opens at the generic 960 × 720 (landscape, the three columns fit at their minimum). The browser path
  gets 1100 × 720 clamped. Hand-off hunk below.
- The pop-out's `SectionRow` has no `data-tauri-drag-region` (the component takes no extra props and is not mine); the
  window keeps its native title bar (`open_window` does not undecorate), so it is draggable.
- **Synthesis rendering**: `Answer.markdown` is rendered from the event through the chat renderer only until the session's
  own assistant turn appears in the transcript (the ask lands in the session, §9), so the answer is never shown twice.
- **Retry** re-sends the row's `scope` + `question` from `notebook_ask_list` (reload-safe) with `save_into: null` —
  `AskSummary` carries no `save_into`.
- The Ask dialog is URL state (`?ask`) on both routes so `/ask` in the composer, the header button and the Library button
  share one path; `/ask` outside a notebook page lands on `/notebooks?ask` (all notebooks).
- `OverviewsSection` dates are `ago()` (`2h`, the codebase's row convention) rather than a calendar date.
- The hit kind for the chip (`sources` vs `notes`) is decided by membership in `source_list(null)` (U1's `useSources`,
  imported); `AskHit` has no kind field.
- Overview "Open" hides until `artifact` is set (a `running` row has none).
- `lib/files.ts` (`MIME.mp3`) and `components/artifacts/Viewer.tsx` were touched beyond the brief's file list only as the
  "artifact viewer's audio element" the brief names — without the MIME the RPC blob is `application/octet-stream`.

## Hand-offs

- **Closer — `crates/desktop/src/commands.rs:39-44`** (Tauri default size per Sky's ruling):
  ```
  -    let (w, h): (f64, f64) =
  -        if label.starts_with("popout-note-") || label.starts_with("popout-meeting-note-") {
  -            (880.0, 1000.0)
  -        } else {
  -            (960.0, 720.0)
  -        };
  +    let (w, h): (f64, f64) =
  +        if label.starts_with("popout-note-") || label.starts_with("popout-meeting-note-") {
  +            (880.0, 1000.0)
  +        } else if label.starts_with("popout-notebook-") {
  +            (1100.0, 720.0) // 144 wave 2: three columns, landscape; the OS clamps to the screen
  +        } else {
  +            (960.0, 720.0)
  +        };
  ```
- **Closer — `rpc/api.ts`**: add `notebook_ask(scope: DocId[], question: string, save_into: DocId | null): Promise<AskStarted>`,
  `notebook_ask_list(limit: number | null): Promise<AskSummary[]>`, `notebook_overview_run(notebook: DocId, kind: OverviewKind):
  Promise<OverviewRun>`, `notebook_overviews(notebook: DocId): Promise<Overview[]>` to `Api` and `METHODS`; then in
  `lib/ask-rpc.ts` replace `AskApi`/`askApi`/`useAskApi` with `Pick<Api, …>` / `makeApi` / `useApi` (the keys and callers stay).
- **U1 — `NotesColumn.tsx`**: renders `<OverviewsSection notebook={id} />` under the notes list (prop `{ notebook: string }`,
  export name unchanged — confirm). `useSources(null)` is imported read-only by `AskView` for the hit kind.
- **E2 — event shape**: `AskView` subscribes by top-level tag `NotebookAsk` and reads `{ session, event }` with `event.type ∈
  plan|branch|partial|answer|done` (snake_case, as the contract). `partial.answer` is treated as the whole text so far
  (replace, not append) — if E2 streams deltas, change `applyAskEvent`'s `partial` arm to append. The synthesis is
  expected as the session's assistant turn (design §9 "lands in the ask session"); `AskSummary.save_into` would let Retry
  keep the save target. The audio mp3 should arrive as artifact `kind: file`, `mime: audio/mpeg` (the viewer keys on the
  mime); `Overview.artifact` for the script row is its own `show` call id.
- **Design 148** (editing `slash.ts` tonight): only `COMMANDS` row `/ask` (:102) and `case '/ask'` (:349-356) were added.
