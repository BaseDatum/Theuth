# POPOUT-2 as built — notes and meetings pop out; meetings deletable from the list; Open meeting from a chat; meeting page with embedded chat

Branch `feat/popout-notes-meeting-delete`, worktree `theuth-popout2`, base `d4473c5` (POPOUT merged). Five commits plus this report. Builds on `lib/popout.ts` from POPOUT (70-as-built-POPOUT.md): same `?window=<label>` mechanism, same `open_window` command, no host changes.

## Commits

### `621d58d` feat(ui): notes and meeting notes pop out (brief item 1)

- `lib/popout.ts`: `PopoutTarget` gains `{kind:'note', doc}` → `/notes/<doc>?window=popout-note-<doc>`, `{kind:'meeting', meeting}` → `/meetings/<id>?window=popout-meeting-<id>`, `{kind:'meeting-note', meeting}` → `/meetings/<id>?pane=note&window=popout-meeting-note-<id>`. `popoutLabel` is now `popout-<kind>-<safe id>` for every kind (chat/artifact labels unchanged). `popoutTarget(href)` parses `/notes/$id` and `/meetings/$id` (`?pane=note` picks the note form). New `popoutMeeting(t)` (the meeting a popped window is about) and `closePopout()` (Tauri `getCurrentWindow().close()` / browser `window.close()` — was inlined in `App.tsx`, now three call sites).
- `routes/meetings.$id.tsx`: `POPOUT?.kind === 'meeting-note'` → `PoppedMeetingNote`, else `ControlCenter`. A `kind:'meeting'` popout is the control center under the `Popped` root (no sidebar).
- `components/meeting/Note.tsx`: `MeetingNote` takes an optional `meeting`; with it (and not popped) a "Note" header row with the "Open note in new window" button. `PoppedMeetingNote({id})`: `meeting_get` → `MeetingNote` over `manifest.note` full height, `document.title` = meeting title; "This meeting has no note." when `note` is null.
- `components/notes/NotePage.tsx`: header action "Open in new window" (hidden when popped); popped hides the Chat action/panel (editor alone); a popped note's own Delete closes the window instead of navigating to `/notes`.
- `components/notes/NotesList.tsx`: row hover button "Open in new window" + the same item in the row's ⋯ menu (after Rename).
- `App.tsx`: in a popout about a meeting (`popoutMeeting(POPOUT)`), the meeting `Deleted` event closes the window (same shape as the chat popout's `SessionDeleted`).
- Two windows on one note: each webview is its own `useDocument` provider on the same `DocId` — the participant strip in the popped note showed "Chrome (you) · Chrome", exactly the two-devices case; edits in either landed in the other within ~1 s (see Verification). No shared provider, so nothing to fight.

### `90f282b` feat(meeting): delete a meeting from the list (brief item 2)

- `meeting_delete` already existed end to end (`crates/rpc/src/api.rs` → `meeting::DeleteMeeting` → `Store::delete` + `agent::DeleteSession` + `MeetingEvent::Deleted`); the detail page's `Summary.tsx` used it with a native `confirm('Delete this meeting and its recording?')`. Reused, not re-added.
- `components/meeting/MeetingList.tsx`: rows are a `Row` component. Hover actions at the trailing edge: Attach to chat… · **Open note in new window** · **Delete meeting** (trash) · ⋯. Right-click (non-touch) opens the same `DropdownMenu` as ⋯: Open · Open in new window · Open note in new window · Attach to chat… · Delete. Exported `deleteMeeting(api, id)`: one `confirm`, one RPC, `false` when declined — used by the row and by `Summary.tsx` (the confirm string now lives in one place). The list refreshes through the existing `Deleted` → `invalidate(queryKeys.meetings)` in `lib/rpc.ts`, plus `useAction`'s `keys`.
- `MeetingHeader` in popped mode: no sidebar toggle/search trigger, no macOS traffic-light inset (popouts have a normal title bar, as in POPOUT's chat header).
- `crates/meeting/src/lib.rs` `DeleteMeeting`: reads `manifest.note` alongside `session` and, after the store delete, asks the document actor `document::Delete { doc }` via new `note::delete` (`crates/meeting/src/note.rs`) — the owner path, like knowledge/artifacts. Without it the meeting's note lingered in `/notes` as an orphan `meeting_note` row. Failure is a `warn!`, never a failed delete. The event and session deletion are unchanged.
- Popped meeting / meeting-note windows close on `Deleted` (commit 1); the chat popout of the meeting's session closes on `SessionDeleted` as before.

### `3b7b3b2` feat(ui): Open meeting from a meeting's chat (Sky's item 3)

- No session→meeting RPC exists (`Store::by_session` is Rust-internal, used by the agent tool only), but `MeetingSummary.session` is already in `meeting_list`. `lib/meeting-link.ts`: `meetingForSession(list, session)` (pure) and `useMeetingForSession(session)` over the cached `queryKeys.meetings` query (deduped; invalidated by meeting events). `openMeeting(meeting, alt, here)`: ⌥ → `openPopout({kind:'meeting'})`, else `here()`.
- `components/chat/SessionMenu.tsx` (the pane header's ⋯): after Open parent, **Open meeting** (⌥-click pops out) and **Open meeting in new window**; both absent when no meeting names the session.
- `components/sidebar/SessionItem.tsx`: the same two items after "Open in new window".

### `1701a52` feat(ui): meeting page — transcript over the embedded chat, note beside (Sky's item 4)

- `components/chat/Panes.tsx`: `Divider` exported and decoupled from the pane tree — `{dir, onRatio}`; `Branch` passes `onRatio={(r) => usePanes.getState().resize(path, r)}`. Same 0.15–0.85 clamp, same pointer-capture drag.
- `lib/meeting-settings.ts`: `split` (default `DEFAULT_SPLIT = 0.6`) + `setSplit`, persisted in the existing `meeting-settings` localStorage blob beside the eye/hotkeys/model; a stored value outside 0.15–0.85 reads as the default.
- `routes/c.$session.tsx`: `LiveSession` exported (unchanged body; `first={false}` skips the sidebar toggle and inset; it only sets `document.title`/hotkeys when it is the focused pane, so embedding does not fight the meeting page).
- `components/meeting/ControlCenter.tsx`: left column = transcript area (live log + briefs while recording; summary/review/manifest + briefs after) at `flex: split`, `Divider dir="col"`, the meeting's chat (`LiveSession session={manifest.session}`) at `flex: 1-split`; right column = `MeetingNote` (45 %). Under `md` (`useIsMobile`, < 768 px — the same breakpoint the page already used for its tabs) the three stack in one scroll at 70 dvh each, no divider; that covers a small popped meeting window too. The old mobile transcript/note tab strip is gone (stacking replaces it). `document.title` = meeting title (popped window title).
- Meetings always have a session (`Manifest.session: SessionId`, created by `meeting_start` before the manifest is written), so the "no session yet" composer case does not arise; the chat block is simply skipped if `session` is somehow absent.

### `b30528f` fix(ui): ⌥ read from the item's click; meeting page titles its window

- Radix `DropdownMenuItem.onSelect` dispatches a bare `CustomEvent` (no `detail.originalEvent` in the installed `@radix-ui/react-menu`), so the first cut never saw ⌥. Now `onClick={(e) => openMeeting(meeting, e.altKey, …)}`: Radix routes Enter/Space to `click()` too, so keyboard selection still opens in place. Smoke-verified after the fix.

## Tests written

- `lib/popout.test.ts`: new test — `/notes/$id`, `/meetings/$id`, `/meetings/$id?pane=note` (and `?pane=chat` → whole meeting), no-id cases, url→target round trip for the three kinds, label charset, `popoutMeeting`.
- `lib/meeting-link.test.ts`: `meetingForSession` (hit, miss, list absent/empty); `openMeeting` ⌥ → named `window.open` with the meeting popout url, plain → `here`.
- `components/meeting/MeetingList.test.ts`: `deleteMeeting` — declined `confirm` → no RPC, `false`; confirmed → `meeting_delete(id)`, `true`; engine refusal propagates.
- `lib/meeting-settings.test.ts`: `split` round-trips through `meeting-settings` with the other keys intact; out-of-range and absent values read as `DEFAULT_SPLIT`.
- `components/meeting/ControlCenter.test.ts`: only a hoisted `localStorage` stub added — the page now imports the chat pane, whose `nudges` store reads storage at import.

## Verification

- `just ts-desktop`: ok (5 export tests passed; bindings copied — the worktree had none).
- `bunx tsc --noEmit`: 0 errors (after `bun install --frozen-lockfile` in `crates/desktop/ui` and `packages/apps`).
- `bunx vitest run`: 80 files, 487 tests. One full run had `src/link/pairing.test.ts` fail while `cargo test` compiled concurrently; alone it passes (6/6) and it is untouched by this branch. All other files green in every run.
- `bun run build` (`tsc -b && vite build`): exit 0.
- `cargo check -p meeting`: Finished dev, no warnings from the crate. `cargo test -p meeting --lib`: 56 passed. `cargo check -p desktop --tests`: Finished dev (only the pre-existing `block v0.1.6` future-incompat note).
- Browser smoke (browser fallback, not Tauri): `vite --port 1421` with `VITE_THEUTH_URL`/`VITE_THEUTH_TOKEN` at the running local engine (`~/.theuth/engine.json`, 127.0.0.1:53703), headless Chromium via omp's browser tool:
  - Notes: New note → header shows "Open in new window" → popup at `/notes/<id>?window=popout-note-<id>`, no sidebar, `document.title` = note title, actions without Chat/popout; participant strip "Chrome (you) · Chrome". Typed in the popout → main's editor showed the text; typed in main → the popout followed. Screenshot `/tmp/69/popped-note.png`. The smoke note was deleted afterwards through its header Delete.
  - Meetings list: rows show Attach · Open note in new window · Delete meeting · ⋯ on hover; right-click opens the menu (Open · Open in new window · Open note in new window · Attach to chat… · Delete; `/tmp/69/meetings-menu.png`). Trash → native confirm "Delete this meeting and its recording?" — **declined**; the three rows stayed. The three meetings on the engine are Sky's real recordings (one titled "Test", 1 min, in Review), so no meeting was actually deleted; the confirmed path is `meeting_delete`, the RPC the detail page already used, exercised by the unit test with a mock.
  - Row "Open note in new window" → popup at `/meetings/<id>?pane=note&window=popout-meeting-note-<id>`, title "Test", the note editor alone (`/tmp/69/popped-meeting-note.png`); main stayed on `/meetings`.
  - Meeting detail (Done meeting): status strip, then transcript/summary on top, the embedded chat (model picker · title · context · approvals off · ⋯, transcript, composer) below a horizontal separator; dragging the separator up moved it from y=512 to y=311, `meeting-settings.split` = 0.296, and it stayed there after reload (`/tmp/69/meeting-detail.png`, `/tmp/69/meeting-detail-dragged.png`). The "Test" meeting (has a note) showed the note column with its "Note" header and popout button (`/tmp/69/meeting-detail-note.png`).
  - Chat ⋯ → "Open meeting in new window" → popup `/meetings/<id>?window=popout-meeting-<id>`: no sidebar toggle, chat embedded, separator present at 1280 px; at 600 px viewport the separator is gone and the blocks stack (`/tmp/69/popped-meeting-narrow.png`).
  - The meeting's own chat at `/c/<session>`: ⋯ lists Open meeting · Open meeting in new window (a non-meeting chat's sidebar menu did not). Plain click → `/meetings/<id>`; Enter → same; ⌥-click → popup, main stayed on the chat. The meeting sessions are not in the sidebar's recent list on this engine, so the sidebar items were not clicked in the smoke (same hook and helper as the header menu).

## Deviations

- Report/paths: the brief's rules block names `/tmp/67/…` and `docs/design/67-as-built-…`; its item 3 (and the file's own location) name `/tmp/69/report-POPOUT-2.md` and `docs/design/70-as-built-POPOUT-2.md`. The latter used, as POPOUT did.
- The brief's "Verification is not your job" vs item 3's explicit verify list: item 3 followed (tsc, vitest, build, cargo check, `cargo test -p meeting --lib`, browser smoke).
- Meeting note document id: the brief says "the same editor over `meeting:<id>`". `meeting:<id>` is a server-side alias in `document/tools.rs` for the agent tools only; the UI gets the note's `DocId` from the manifest (`manifest.note`) and `MeetingNote` already opened it that way. The popped form fetches the manifest and opens that id — the same document, no alias plumbing added.
- The popped meeting-note URL is `/meetings/$id?pane=note`, not a new route: `popoutTarget` reads `?pane` once at boot (the route has no `validateSearch`, so the router drops it afterwards — same trick as `?window=`).
- `meeting_delete` existed, so no new RPC; the one gap against the brief's spec for a new one (removing the meeting's note document) was added to the existing handler (disclosed above; `crates/meeting/src/lib.rs`, `note.rs` — the meeting crate owns its note, in-plugin, via the document actor's owner `Delete`).
- Delete confirm is the native `confirm()` the detail page already used, not a dialog component; the prompt text is shared through `deleteMeeting`.
- Meeting rows also gained "Open in new window" (whole meeting) in the menu — the target existed for item 3 and the row is where a meeting is picked.
- Item 4 divider: no shared primitive existed (POPOUT-era `Panes.Divider` was bound to the pane tree; `NoteChat`/`ArtifactPanel` have inline pixel drags). `Panes.Divider` was generalised by one prop rather than adding a fourth drag.
- Item 4 narrow layout: stacked blocks at 70 dvh each in one scroll (the brief's "stack: transcript, chat, notes"); the previous mobile transcript/note tab strip was removed rather than extended to three tabs.
- Item 3's "sidebar row menu for that session" — implemented, not clicked in the smoke (see Verification).
- `Summary.tsx` refactored to the shared `deleteMeeting` (one line of behaviour: only navigates to `/meetings` when the delete actually ran).
- `SessionItem` rows each call `useMeetingForSession` → one shared `meeting_list` query for the whole sidebar; an engine without the meeting plugin fails that query quietly (items hidden).

## Not built

- `meeting_for_session` RPC: not needed — the list already carries `session`.
- A "no session yet" composer on the meeting page: meetings always have one.
- Closing a popped *note* window when the note is deleted elsewhere: the existing `closed · deleted` provider status already blanks it ("This note was deleted."); only the popped window's own Delete closes it.

## Files touched

`crates/desktop/ui/src/`: `lib/popout.ts`, `lib/popout.test.ts`, `lib/meeting-link.ts` (new), `lib/meeting-link.test.ts` (new), `lib/meeting-settings.ts`, `lib/meeting-settings.test.ts` (new), `App.tsx`, `routes/meetings.$id.tsx`, `routes/c.$session.tsx` (export only), `components/meeting/{ControlCenter,ControlCenter.test,MeetingList,MeetingList.test,Note,Summary}.tsx/.ts`, `components/notes/{NotePage,NotesList}.tsx`, `components/chat/{Panes,SessionMenu}.tsx`, `components/sidebar/SessionItem.tsx`. `crates/meeting/src/{lib,note}.rs`.

DONE POPOUT-2
