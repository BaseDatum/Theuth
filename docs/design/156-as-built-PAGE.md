# 156 MEETING-VIEW — as built, W1 PAGE (2026-10-09)

Branch `feat/156-page` off b3a1988c. Decisions D1–D7, D15, D16 of `docs/design/156-meeting-view.md`. UI only; nothing under `crates/` outside `crates/desktop/ui`.

## What was built, file by file

Paths under `crates/desktop/ui/src/`.

- **`components/notes/NotePage.tsx`** (D1, D2)
  - 31-35: `NotePage` is the route wrapper and renders `NoteView`.
  - 66-73: `NoteView({ id, isNew?, nb?, embedded?, meeting? })` exported — the former inner `Note`. Regular-note behaviour unchanged when neither `embedded` nor `meeting` is set.
  - 88-97: with `meeting`, reads the meeting's session from `queryKeys.meeting(meeting)` (the cache `ControlCenter` already holds; `enabled` only with `meeting`); `document.title` is left alone when embedded; a popped meeting note pins its `NoteChat` session to the meeting's session (`setChat(id, { session })` whenever it differs).
  - 125-139: `ask(block)`: embedded → `prefillDraft(meetingSession, 'about block …: ')` (the `LiveSession` composer renders the draft store, so the text lands at once) + `theuth:focus-composer`; otherwise the standard panel path.
  - 157-169: the title group is a `span` with `min-w-[180px]` when embedded so the actions wrap under it in a narrow pane.
  - 180-189: "Open in new window" opens `{kind:'meeting-note', meeting}` for a meeting note; Delete rendered only when `!ownedByMeeting(kind, meeting)`; the Chat toggle and the `NoteChat` panel (`below`) are omitted when embedded (the meeting chat is beside the note).
- **`components/editor/DocumentPage.tsx`** (D1)
  - 31-36: `ownedByMeeting(kind, meeting)` — the pure Delete rule (`kind === 'meeting_note' || meeting !== undefined`).
  - 47-49, 55, 65, 88-93: `Frame`/`DocumentPage` gain `embedded`: no `SectionBand`, no back link of either kind; the `SectionRow` alone is the pane toolbar, allowed to wrap (`h-auto min-h-11 flex-wrap py-1`).
  - 230: `embedded` on `DocumentPage`'s prop type.
- **`components/editor/DocumentPage.test.ts`**: `Page` takes `embedded` and renders an action; two new tests (80-94): embedded has no back link and keeps title + actions; `ownedByMeeting` by kind, by prop, and false for a plain note.
- **`components/meeting/Note.tsx`** (D1): `MeetingNote({ doc, meeting, className, style })` is a bordered box around `<NoteView id embedded meeting />` (15-21). `PoppedMeetingNote` renders the non-embedded `<NoteView id meeting />` (23-30); the popped `Frame` draws the drag header and `NoteView` owns the window title. The bare `Live … dense` strip and its "Note" label are gone.
- **`components/meeting/BriefColumn.tsx`** (new, D3): `hhmmss` (moved from `ControlCenter`), `useBriefs(id)` (the `meeting_briefs` query, shared by the column and the header count), `BriefColumn({ briefs, cursor, live, className, style })` — `flex min-h-0 shrink-0 flex-col overflow-y-auto`, oldest first, cursor ring, cached link, "No briefs yet."; while `live` it stays at the bottom when the reader was at the bottom (`atBottom` ref updated on scroll). No local `extra` state; the list is the query alone.
- **`components/meeting/MeetingActions.tsx`** (new, D5, D6)
  - 23-42: `RowAction` and pure `actionsFor(state, reviewedAt, cycle)`: Review → `done_reviewing` | `reviewed(at)`, then `summarize(enabled = !!reviewedAt)`; Done → `rerun`; Failed → the strip's fix (`retry_summary` when `cycle.action === 'summarize'`, else `rerun`; nothing when there is no strip); every other state → `[]`.
  - 44: `deletable(state, cycle)`: any lifecycle state, or `Failed`.
  - 50-54: `layout(actions, compact)`: compact keeps the first enabled button in the row and overflows the rest into the ⋯ menu.
  - 70-136: the component: `meetingTranscript(id)` query (`enabled` only in Review, same key as `Review`, so no extra fetch); `meeting_edit([{op:'Reviewed'}])` written into that cache; `meeting_summarize` / `meeting_post_retry` invalidating `meeting(id)`; the Re-run confirm text from the old `Summary`; `deleteMeeting` (its confirm) then `/meetings` (or `closePopout()` when popped); `children` render between the actions and the ⋯ menu; the menu holds the overflow and "Delete meeting…".
- **`components/meeting/MeetingActions.test.ts`** (new): the per-state actions, `deletable`, `layout`.
- **`components/meeting/Summary.tsx`** (D5): the Re-run/Delete block removed; renders `summary.md` only for a meeting without a note, else `null`.
- **`components/meeting/ControlCenter.tsx`** (D3, D5, D6)
  - 1-12 header comment updated.
  - 93-249 `ControlCenter`: the row is title · state (or oracle summary, `md` and up) · `<MeetingActions compact={mobile}>` with the Briefs toggle (`aria-pressed`, "Briefs · N", desktop only) and Session inside it, so the ⋯ sits last. Body row: desktop → `BriefColumn` at `briefWidth` + `ResizeHandle` (vertical, `aria-valuenow/min/max`, drag and arrow nudge by `PX_STEP`) as the first child when `briefsOpen`; phone → a "Briefs · N" disclosure block (`aria-expanded`), open while live, folded once ended (`useEffect` follows `isLive`), the column capped at `50dvh`. Centre column `flex: 5 1 0%`, note `flex: 4 1 0%`. The old `BriefLog` mounts (ex-159 and ex-188) and the `BriefLog` function are gone; `LiveLog` now stands alone in the top block and gained `min-h-0` so it never grows the page.
  - 252-290 `LifecycleStrip`, 292-348 `LiveLog`: unchanged apart from that class.
- **`lib/meeting-settings.ts`** (D3, D15): `DEFAULT_BRIEF_WIDTH = 272`, `MIN_BRIEF_WIDTH = 200`, `MAX_BRIEF_WIDTH = 480`, `clampBriefWidth`; `briefWidth` and `briefsOpen` saved next to `split`, clamped on load, `setBriefWidth` (clamped) and `setBriefsOpen`. **`lib/meeting-settings.test.ts`**: one new test (clamp on load, clamp on set, rounding, default on garbage, persisted whole).
- **`lib/rpc.ts`** 277-281 (D4): `TopicAdded`, `TopicRemoved`, `Brief` → `invalidateQueries(queryKeys.meetingBriefs(e.meeting))`. Nothing else in the file.
- **`routes/meetings.$id.tsx`** (D7): under `import.meta.env.DEV`, `?sim=meeting-live|review|done` dynamically imports `dev/meeting-page-fixture` and calls `install(scope.connection, mode)`; the page renders only once the fixture is installed so its first `meeting_get` never reaches the engine; the effect's cleanup uninstalls.
- **`dev/meeting-page-fixture.ts`** (new, D7): `meetingPageSim()`, `seed(mode)` (manifest, 30-segment transcript on both tracks with labels `me`/`s1`/`s2`, every word timed, 4 briefs, one profile, the live log, an empty `Loaded` page for the meeting session), `install(connection, mode)`: wraps the connection's `Api` for id `fixture-meeting` and re-wraps after a reconnect swaps it; live mode dispatches `Started`/`State Recording` and then, every 4 s, pushes a brief into its list and dispatches `TopicAdded` (plus one `Utterance`) through `store/sessions.dispatch`. **`dev/meeting-page-fixture.test.ts`**: the seed shape.

## Decisions not spelled out in the brief, and deviations

1. **Fixture seeding via the connection's `Api`, not `setQueryData`.** The brief said "seeds the React Query cache". A cache seeded with `setQueryData` goes stale after 30 s and every refetch — including D4's invalidate → refetch, the thing the live fixture exists to show — would have hit the real engine with an unknown id. Instead `install` swaps `connection.api` for a wrapper that answers `meeting_get`, `meeting_transcript`, `meeting_briefs`, `meeting_live`, `meeting_summary`, `meeting_speakers` for the fixture id and passes every other call through; the cache fills from it and refetches come back to it. `useApi` (`lib/rpc.ts`) was left alone because that file is "invalidations only".
2. **Writes never leave the page**: `meeting_edit` sets `reviewed_at` locally and returns the transcript; `meeting_summarize` flips the manifest to `Summarizing`, then `Done` after 3 s with `Summarized`/`State` dispatched; `meeting_post_retry` and `meeting_delete` resolve `null`. For the meeting's chat (`LiveSession`) the wrapper answers `load_session`, `list_agents`, `list_artifacts`, `get_todos` for `fixture-session` with empty data so no toast fires.
3. **Fixture note id** is the fake `fixture-note` (allowed by the brief). The engine answers `doc_open` with "no such document", which the provider reports as `offline · 0 queued` under an empty editor, so the pane shows the toolbar and the paper.
4. **Pop-out**: kept `{kind:'meeting-note', meeting}` (`/meetings/<id>?pane=note`), rendering the non-embedded `NoteView` with `meeting`. The list row's "Open note in new window" keeps working unchanged, and the popped note keeps its D2 chat pin.
5. **D2 popped**: the chat panel is pinned by an effect that re-pins whenever `chat.session !== meetingSession` — `NoteChat`'s "New chat" is therefore a no-op for a meeting note (it is not in W1's file list, so its button stays). Embedded: the note's Chat toggle and `NoteChat` are not rendered; "Ask theuth…" prefills and focuses the meeting chat's composer (`theuth:focus-composer`).
6. **Note width**: `w-[45%]` → `flex: 4 1 0%` beside a `flex: 5 1 0%` centre (the brief column now takes 272 px). At 1280 with the sidebar that is ~380 px of note; the paper (`max-width: 800px`, `width: 100%`) fits without horizontal overflow.
7. **Embedded toolbar wraps.** Eight 44 px actions plus the title did not fit 380 px: the title was squeezed to 0. `Frame` embedded uses a wrapping `SectionRow` and the title keeps `min-w-[180px]`; the ParticipantStrip and actions wrap under it.
8. **Oracle summary hidden under `md`** (`hidden md:flex`) so the 390 px row keeps the title, the compact action, Session and ⋯.
9. **Delete also for a failed start** (`state === 'Failed'` with no lifecycle strip): the engine refuses delete only while recording.
10. **Phone compact rule**: no measuring; `layout(actions, true)` keeps the first enabled button in the row and moves the rest into ⋯ (Review before reviewing: [Done reviewing] in the row, Summarize in ⋯; after: [Summarize] in the row, "Reviewed HH:MM" in ⋯). Done/Failed have one action, which fits.
11. **`Live`'s `dense` prop stays**: `CharterDoc` and `TaskDetail` still use it; only the meeting pane stopped.
12. **`deleteMeeting` after success in a pop-out** closes the window instead of navigating the pop-out to `/meetings`.

## Gates

- `BUILD_LABEL=156-PAGE nice -n 10 ~/.theuth/bin/build-lock ./node_modules/.bin/tsc -b --noEmit` → exit 0, 0 errors (tsc 7.0.2; `--force` and `-p tsconfig.json` also 0).
- `BUILD_LABEL=156-PAGE … vitest run src/lib/meeting-settings.test.ts src/components/editor/DocumentPage.test.ts src/components/meeting/MeetingActions.test.ts src/components/meeting/ControlCenter.test.ts src/dev/meeting-page-fixture.test.ts src/lib/no-emoji.test.ts src/components/meeting/MeetingList.test.ts` → 7 files, 18 tests passed.
- `git status`: clean except ignored files; nothing under `/tmp/156/proof-page` committed; bindings untouched.

## Proof (throwaway, `/tmp/156/proof-page/`, never committed)

Vite dev on 127.0.0.1:5261 against the live engine (`ws://127.0.0.1:56525`, token from `~/.theuth/token`), read-only: only `meeting_get`, `meeting_transcript`, `meeting_briefs`, `doc_open` on Sky's meeting; no clicks that write, no typing. Headless Chrome via raw CDP (`shot.ts`, `live.ts`, `dom.ts`), 2× DPR.

- `done-1280-light.png` — Sky's "Invesco Planning" (Done) at 1280×900: full-height brief column (6 cards, 272 px), row = title · Done · [Re-run post pipeline] · [Briefs · 6] · [Session] · ⋯, embedded full note (title "Invesco Planning", dictation, Rename · Attach · Snapshots · Export · Undo · Share, ParticipantStrip) on the paper.
- `done-1280-light-transcript-bottom.png` — the transcript scroller at its bottom (`scrollTop 11236/11668`): the last segment is the last thing in it; nothing below the transcript.
- `done-390-light.png` — same meeting at 390×844: title · Done · [Re-run post pipeline] · [Session] · ⋯ in one row, "Briefs · 6" folded above the transcript; `document.documentElement.scrollWidth === 390`; the only elements past 390 px are inside the off-screen sidebar `<aside>`.
- `live-1280-a.png` / `live-1280-b.png` — `?sim=meeting-live`, ~10 s apart, no reload: DOM brief count **5 → 8**, header toggle "Briefs · 5" → "Briefs · 8"; the column stayed at its bottom; MeetingBar at the foot.
- `review-1280-light.png` — `?sim=meeting-review`: row = Review · [Done reviewing] · [Summarize, disabled] · [Briefs · 4] · [Session] · ⋯.
- `review-390-dark.png` — same at 390×844 dark: [Done reviewing] compact in the row, Summarize in ⋯, "Briefs · 4" folded.
- `done-fixture-1280-menu.png` — `?sim=meeting-done` with ⋯ open: menu items `["Delete meeting…"]`.
- `regular-note.png` — `/notes/lnmketzhovkzvdswutqr7r7crq` as a control (see below).

## Seen and left alone

- **Pre-existing console error** `Cannot update a component (ParticipantStrip) while rendering a different component (Editor)` — reproduced on the plain `/notes/<id>` page (`regular-note.png` run) on the same base, so not introduced here; `ParticipantStrip`/`Editor` are outside this brief.
- **Vite's file watcher did not pick up edits in this worktree** (served modules stayed stale until the dev server was restarted). Proofs were taken after restarts; not a code matter.
- `Review.tsx` still renders its own Done reviewing / Summarize after the last segment (W2 removes them); mounted exactly as before, `<Review id readOnly={state !== 'Review'} />`.
- The brief column's `at_ms` for cached briefs shows "cached · HH:MM:SS" as before; untapped topic briefs are not marked (open question in recon, not decided).
- `hydrateMeeting` on a reconnect rebuilds the store from the real engine's `meeting_status`, which would end the live fixture's "live" view until the page reloads; dev-only, accepted.
