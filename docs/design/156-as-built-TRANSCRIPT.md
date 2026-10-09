# 156 MEETING-VIEW — as built, W2 TRANSCRIPT (2026-10-09)

Branch `feat/156-transcript` off b3a1988c. Scope: D8, D9, D11, D12, D13 and the `Review.tsx` half of D5 from `docs/design/156-meeting-view.md`. Engine side (D10, D14) is W3's; W2 codes against hand-written stubs of the contract.

## What was built, file by file

**`crates/desktop/ui/src/lib/segment-audio.ts`** (new, D8)
- 1-4: header. 6-10: `PAD_MS = 150`, `padRange(t0, t1, pad)` → `[max(0, t0-pad), t1+pad]`.
- 12-18: `base64ToBytes` → `Uint8Array<ArrayBuffer>` in its own buffer (what `decodeAudioData` takes).
- 20-24: `PlayerContext` (the `AudioContext` slice used, so tests can stub it) and `FetchWav`.
- 31-100: `SegmentPlayer`. `toggle(id, t0, t1, fetch)`: same id → `stop()`; else `stop()` the previous source, mark `id` (`onChange(id)`), call `ctx.resume()` synchronously before any `await`, fetch the padded range, decode, `createBufferSource` → `connect(destination)` → `start()`; `onended` clears the marker only if that source is still current; a sequence counter drops a fetch that a later click overtook; failure clears the marker and rethrows.
- 103: `playError(e)` → `"Could not play this segment: <message>"`.

**`crates/desktop/ui/src/lib/segment-audio.test.ts`** (new): padding/clamp, base64 → bytes, the state machine with a stubbed context and sources (resume-in-click, marker set/clear, stop-then-play, late `onended` ignored, stale fetch dropped, failure path, no-context error).

**`crates/desktop/ui/src/lib/segment-select.ts`** (new, D12/D13 pure parts)
- 10: `MIN_SPEECH_MS = 1000`.
- 16-27: `selectClick(selected, order, anchor, id, shift)`: plain click toggles; shift with an anchor adds the inclusive range in `order` (either direction, keeps earlier picks); shift without an anchor is a plain toggle.
- 30-38: `speechMs(segments, ids)`: sum of word spans of the chosen ids — the engine's measure.
- 41-47: `enrollBlock(segments, ids)` → `"pick segments from one track"` | `"pick at least 1 s of speech"` | `null`.
- 50: `secondsText(ms)` → `23 s`, or one decimal under 10 s (`0.8 s`).
- 53-57: `enrolledToast(r, target)`: `Enrolled Dana from 12 s of speech` / `Added 12 s to Dana (4 samples)`, plus ` (using 30 s of 74 s)` when `available_ms > speech_ms`.
- 60: `alreadyEnrolled(message)` → the name in `a speaker named X is already enrolled`, else `null`.

**`crates/desktop/ui/src/lib/segment-select.test.ts`** (new): ranges incl. shift-click, the speech-ms sum, the both-tracks and < 1 s rules, the toast builder, the refusal parser.

**`crates/desktop/ui/src/components/meeting/SegmentMenu.tsx`** (new, D11)
- A `DropdownMenu` on a `Button size="icon-xs"` trigger with `aria-label="Segment options"` (`MoreHorizontal`), `align="end"`; keyboard-reachable through Radix.
- Items (40-72): **Play segment**; **Split at a word…** / **Hide words** only when `onWords` is given (Review, multi-word rows); **Enroll as new speaker…**; **Add to enrolled speaker ▸** as a `DropdownMenuSub` listing profiles by name with their sample count. With no profiles the sub becomes one disabled item whose second line reads "No enrolled speakers yet" ("Loading speakers…" before the first `meeting_speakers` reply).
- `count > 1` (row inside a multi-selection) relabels both enroll items: "Enroll 4 selected segments as new speaker…", "Add 4 selected segments to". `block` disables both and shows the reason as a second line.

**`crates/desktop/ui/src/components/meeting/EnrollDialog.tsx`** (new, D13)
- Fields: Name (required, autofocus) and "Knowledge entity (optional)", as `SpeakerChip`'s enroll. Description: "A voiceprint from N segments (12 s of speech)."
- 39-55 `run(target)`: awaits the raw RPC the parent passes; on success closes and calls `onDone(r, target)`; on the "already enrolled" refusal sets `taken` and invalidates `meetingSpeakers`; any other error → `toast.error(<engine string>)` and the dialog stays open.
- 90-98: the inline notice "X is already enrolled." with **[Add to X instead]**, which calls `run({ kind: 'Existing', profile })` for the profile whose name matches case-insensitively.

**`crates/desktop/ui/src/components/meeting/Review.tsx`** (owned)
- 1-8: header rewritten. 10-31: imports (`SegmentPlayer`, `playError`, the select helpers, `getSharedAudioContext`, `DropdownMenu*`, `toast`, the two new components, the stub types).
- 42-68 `Review`: unchanged signature `Review({ id, readOnly })`, same `meetingTranscript(id)` query and `meeting_edit` action. **The "Done reviewing" / "Reviewed at" / "Summarize" block (old 54-67) and the `summarize` action are deleted** (D5; W1's `MeetingActions` owns them).
- 134-331 `SegmentList`:
  - 142-145: `playing` marker state; one `SegmentPlayer` per list (`useRef`, context from `getSharedAudioContext`), stopped on unmount.
  - 146-156: selection (`selected`, `anchor`), `editing`, `words`, `enrolling` (the ids the dialog is for), `profiles = useQuery(meetingSpeakers)`, `order`/`ids`/`speech`/`block`.
  - 158-161 `play(seg)`: `player.toggle(seg.id, t0, t1, (a, b) => api.meeting_segment_audio(id, seg.track, a, b))`, failure → `toast.error(playError(e))`.
  - 162-171: `enrolled(r, target)` = `setQueryData(meetingTranscript(id), r.transcript)` → invalidate `meetingSpeakers` → `toast.success(enrolledToast(...))` → clear selection; `addTo` = `useAction` calling `meeting_speaker_enroll(id, segments, { kind: 'Existing', profile })` (errors toast as the engine wrote them).
  - 173: `targets(seg)` = the selection when the row is inside a multi-selection, else `[seg.id]`.
  - 190-241: the sticky bar, now in **every** ended state: "N selected · 23 s of speech" · [Enroll as new…] · [Add to ▸] (DropdownMenu of profiles; "No enrolled speakers yet" when empty) · the block reason in muted text · Relabel/Merge (Review only, as before) · Clear. Both enroll controls are disabled with `title=<reason>` when `block` is set.
  - 244-317: rows. Checkbox on every row in every state (`aria-label="Select segment at mm:ss"`, `onClick` with `e.shiftKey` → `selectClick`, anchor updated). Timestamp button titled Play/Stop with the `Volume2` marker while that row plays (also tinted). Text / words / edit as before, with the words view only when `onEdit`. `SegmentMenu` on every row.
  - 319-329: `EnrollDialog`, mounted only while `enrolling` is set so its fields start fresh each time.

**`crates/desktop/ui/src/rpc/api.ts`**: 79-80 type imports; 790-791 `meeting_speaker_enroll(id, segments, target): Promise<Enrolled>` after `meeting_speaker_delete`; 1370 METHODS entry between `'meeting_speaker_delete'` and `'meeting_speakers'`.

**`crates/desktop/tauri.conf.json`** line 49: `media-src 'self' blob: asset: http://asset.localhost` inserted after `img-src`; nothing else changed.

**Not committed (gitignored stubs, replaced by W3's ts-rs export at integration)**
- `crates/desktop/ui/src/rpc/bindings/EnrollTarget.ts`:
  ```ts
  import type { ProfileId } from "./ProfileId";
  export type EnrollTarget = { "kind": "New", name: string, entity: string | null, } | { "kind": "Existing", profile: ProfileId, };
  ```
- `crates/desktop/ui/src/rpc/bindings/Enrolled.ts`:
  ```ts
  import type { MeetingTranscript } from "./MeetingTranscript";
  import type { Profile } from "./Profile";
  export type Enrolled = { profile: Profile, speech_ms: number, available_ms: number, transcript: MeetingTranscript, };
  ```
  The transcript binding is `MeetingTranscript.ts` (the Rust `Transcript` is renamed on export; `bindings/Transcript.ts` is the STT reply). `Profile`/`ProfileId`/`MeetingId` names are as in the existing bindings.
- `crates/desktop/ui/src/rpc/bindings/methods.json`: `"meeting_speaker_enroll"` added in sorted position. `api.test.ts` actually reads `<repo>/bindings/methods.json` (gitignored by `/bindings`), which did not exist in this tree, so a copy was placed there too; the equality test then runs and passes rather than skipping.

## Decisions not spelled out, and deviations
- **Add to X is confirm-free** (menu, bar and the inline [Add to X instead]); one click, undo is W3's `meeting_speaker_delete` / Settings › Meetings. Recorded as the brief asked.
- **"No enrolled speakers yet"** is rendered as a disabled item with a second muted line, not a disabled sub-trigger: Radix cannot open a disabled submenu, so the hint would never be seen. Same treatment for the block reason on the two enroll items.
- **Words view toggle**: "Split at a word…" becomes "Hide words" while open, so the words view can be closed again (the old ⋯ toggled it).
- **Marker**: today's `Volume2` marker kept; the timestamp is tinted while playing and its title flips Play/Stop. The marker is set at click time (while fetching) so a slow `meeting_segment_audio` is visible; a second click during the fetch cancels it.
- **Seconds formatting**: one decimal under 10 s (`5.0 s`), integer above (`12 s`), in both the bar and the toast. The design's examples are all ≥ 10 s and read as written.
- **Dialog mount**: `EnrollDialog` is mounted only while open (fields reset per use; found by the stub proof where the previous name persisted). Costs the close animation.
- **Checkbox events**: `onClick` + `readOnly` instead of `onChange`, because the change event carries no `shiftKey`.
- **Debug counter**: not added to product code. The live proof wraps `AudioBufferSourceNode.prototype.start` and the `AudioContext` constructor via CDP `Page.addScriptToEvaluateOnNewDocument` instead, so nothing ships for the proof.
- **`Review.test.ts`** (recon's `createElement` smoke render) was not written: vitest here is NODE env with no DOM, and the pure logic that can break is in the two `lib/*.test.ts` files. Rendering is covered by the stub-app proof.
- No `ponytail:` corners cut with a ceiling worth naming.

## Gates
- `BUILD_LABEL=156-TRANSCRIPT nice -n 10 ~/.theuth/bin/build-lock ./node_modules/.bin/tsc -b --noEmit` → **0 errors** (with the stubs present).
- `… build-lock ./node_modules/.bin/vitest run src/lib/segment-audio.test.ts src/lib/segment-select.test.ts src/rpc/api.test.ts src/lib/no-emoji.test.ts` → **4 files, 14 tests passed** (6 + 5 + 2 + 1).
- `git status`: clean apart from ignored files; no stubs, `bindings/`, or proof files committed.

## Proofs (all under `/tmp/156/proof-transcript/`, throwaway)
1. **Live engine, read-only** — `live.ts` (driver), `live.log`, `live-playing.png`, `live-menu.png`. Vite dev from this tree on :5262 against `ws://127.0.0.1:56525`; headless Chrome 1280×900 with `--autoplay-policy=no-user-gesture-required`, `/meetings/01a12243-f793-7921-beb7-04de9329c73f` ("Invesco Planning", Done). Results in `live.log`: 167 rows, 167 ⋯ menus, 167 checkboxes, 0 old Done-reviewing/Summarize buttons; CDP mouse click on the first timestamp → `window.__plays` 0 → 1, `AudioContext.state` `running`, decoded buffer 11.998 s, Stop marker shown, **no toast**, the one RPC `meeting_segment_audio(id, "System", 10, 12008)` (t0 160 ms padded and clamped to 10, t1 + 150); clicking the Stop row clears the marker without a second start. `live-menu.png`: the ⋯ menu open on a segment with Play segment, Enroll as new speaker…, and "Add to enrolled speaker / No enrolled speakers yet" disabled (`speakers/` is empty). Every RPC the page sent is listed in `allRpc`; `writes` is empty.
2. **CSP control** — `csp/serve.ts` (bun server on :5263 serving `/old` and `/new`, the CSP read from this tree's `tauri.conf.json` and placed in a `<meta http-equiv>`; `/new` is the exact string, `/old` is it with the `media-src` clause removed), `csp/app.js` (external script, since the CSP has no `'unsafe-inline'` for scripts), `csp/run.ts`, `csp/csp.log`, `csp/old.png`, `csp/new.png`. Under the ORIGINAL CSP the blob `<audio>` path fails — console: "Loading media from 'blob:…' violates the following Content Security Policy directive: default-src … Note that 'media-src' was not explicitly set" — surfacing as `MediaError` code 4 "Media load rejected by URL safety check" (Chrome's spelling; WebKit's is `NotSupportedError`), while the Web Audio path plays (`state: running`, 0.5 s decoded, `onended` fired). Under the NEW CSP both paths play.
3. **Enroll flow on a stub API** — `stub/` (vite app on :5264; `node_modules` symlinked from this tree, `@` aliased to this tree's `src`, the full page CSS taken from the :5262 dev server since Tailwind's scanner would not pick up sources outside the stub root), `enroll.ts`, `enroll.log`, screenshots:
   - `enroll-1-selection.png`: click row 1, shift-click row 3 → "3 selected · 12 s of speech" with the sticky bar; `enroll-1b-row-menu.png`: that row's ⋯ says "Enroll 3 selected segments as new speaker…" / "Add 3 selected segments to".
   - `enroll-2-dialog.png`: the dialog ("A voiceprint from 3 segments (12 s of speech).").
   - `enroll-3-already-enrolled.png`: name "Dana" → stub refuses → "Dana is already enrolled. [Add to Dana instead]"; no toast.
   - `enroll-4-success-toast.png`: [Add to Dana instead] → `{ kind: 'Existing', profile: 'p-dana' }` → toast "Added 12 s to Dana (4 samples)", dialog closed, selection cleared, legend shows the linked label "Dana · enrolled".
   - `enroll-5-new-toast.png`: one row's ⋯ → Enroll as new → "Pat" + entity → toast "Enrolled Pat from 5.0 s of speech".
   - `enroll-6-both-tracks.png` / `enroll-6b-both-tracks-menu.png`: rows on System + Mic → "pick segments from one track", Enroll/Add to disabled in the bar and in the row menu.
   - `enroll-7-readonly.png`: `readOnly` — checkboxes, bar (no Relabel/Merge), menus without Split.
   - Captured payloads (`enroll.log` → `calls`): `meeting_speaker_enroll("fixture", [1,2,3], {kind:"New", name:"Dana", entity:null})`, `(…, [1,2,3], {kind:"Existing", profile:"p-dana"})`, `(…, [4], {kind:"New", name:"Pat", entity:"pat-entity"})`, `(…, [5], {kind:"New", name:"fail", …})` → engine string toasted as is ("speaker models not installed under /tmp/models", dialog stays open), `(…, [5], {kind:"Existing", profile:"p-dana"})` from the bar, and `meeting_segment_audio("fixture", "System", 0, 4950)` from the timestamp. One `meeting_edit Merge` in the log is the driver clicking the wrong bar button (index 3 is Merge in Review), not a product action.

Dev servers and every headless Chrome were killed afterwards.

## Seen but left alone
- `useApi()` in dev wraps the api with `endlessFixture`; irrelevant here.
- `SpeakerChip`'s label-level Enroll (`EditOp::Enroll`) still exists beside the new segment-level enroll; the design keeps both.
- The stub proof reads the page CSS from the :5262 server because Tailwind v4's `@source` pointing outside the vite root produced no utilities in this setup; not investigated further.
- `bindings/methods.json` at the repo root is what `api.test.ts` reads, not `ui/src/rpc/bindings/methods.json`; the brief names the latter. Both are gitignored; I kept both in sync locally.
