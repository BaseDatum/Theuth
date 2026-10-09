# 156 MEETING-VIEW — the meeting page as Sky asked for it (2026-10-09)

Status: contract for the 156 wave (orchestrator session 01a120f8). Base: origin/main f4834cf3.
Recon with every file:line: `~/.theuth/briefs/2026-10-09/156/recon.md` (read against f4834cf3, 21:30-21:50).

## The ask (Sky, 2026-10-09 21:29, verbatim)

> I just noticed that in the meetings view, a meeting note is not treated as full note the same way that a regular note is. it should be.
>
> also both during the meeting and after the meeting the briefs should be a full height column on the left hand side, right now they're like a half height column (that doesn't seem to update) during the meeting, and are then jammed in below the transcript in one area after the meeting.
>
> the controls for post review and summarization are also oddly in this same area and are hard to find if the transcript is long, these controls need to be moved to the meeting view header below the tabs.
>
> the transcript audio segments timestamps also when I click them say operation not supported, they are supposed to playback that segment of audio of the transcript, and the three dots menus on the end of the transcript segments do nothing where they should allow me to enhance an enrolled speaker or enroll a new speaker with that segment of speech, or selected segments of speech.
>
> let's plan and implement these fixes with omp subagents.

Earlier rulings that still stand (recon §0): transcript over the meeting's chat, note on the right (2026-09-26); the post-meeting status strip stays at the top (2026-09-24); controls for a view go in the row below the tabs, pop-outs have no tabs (97b, 2026-10-04); pause/stop stay in `MeetingBar`; notes look like paper (2026-09-22); "speaker enrollment profiles; playback by segment; segments usable for enrollment" (2026-09-19 interview).

## Root causes (verified by the orchestrator on f4834cf3)

1. **Note.** `components/meeting/Note.tsx` mounts the bare `Live` editor in `dense` paper with a static "Note" label. The meeting note is the same block document as a regular note (`DocKind::MeetingNote`, doc `meeting:<id>`), but none of `NotePage`'s title, action bar, ask, snapshots, export or pop-out page is mounted.
2. **Briefs.**
   - **Half height.** `BriefLog` (`ControlCenter.tsx` 312-354) is a `w-80` box inside the top block at `flex: split` (0.6) while live.
   - **Jammed after the meeting.** Once the meeting ends it is appended under Summary/Review in the same scroll div.
   - **Not updating.** It fetches `meeting_briefs` once and listens only to `Brief`. Topic briefs, the common case, publish only `TopicAdded` (`meeting/src/lib.rs` 3033-3040). On the 21:04 "Invesco Planning" meeting all 6 briefs were topic briefs, so the page received zero `Brief` events. `lib/rpc.ts` never invalidates `meetingBriefs`.
3. **Controls.** "Done reviewing" and "Summarize" render after the last segment (`Review.tsx` 54-67). "Re-run post pipeline" and "Delete" sit at the top of the same scroll (`Summary.tsx` 22-36).
4. **"Operation not supported".** This is WebKit's `NotSupportedError` from `audio.play()` on a `blob:` URL (`Review.tsx` 150-158).
   - The window CSP (`crates/desktop/tauri.conf.json` 49) has no `media-src`, so `default-src 'self' ipc: …` blocks blob/asset media.
   - The engine side is fine: `meeting_segment_audio` returns a base64 16-bit 48 kHz mono WAV of [t0, t1).
   - It does decode the whole Ogg Opus track on every click.
5. **⋯ does nothing.** The ⋯ is a "Words" toggle for splitting (`Review.tsx` 250-259), with every word disabled in read-only states. Sky's meeting is Done.
   - Enrollment exists only per speaker label (`EditOp::Enroll`).
   - There is no path from chosen segments to a voiceprint.
   - `speakers/` on Sky's Mac is empty, while the diarization models are installed.

## Layout (desktop >= md)

```
SectionBand (Library tabs, 52 px)
SectionRow (44 px): title · state · [MeetingActions by state] · [Briefs toggle] · Session · [⋯ Delete meeting…]
degradations line · LifecycleStrip (ended states; unchanged)
┌ BriefColumn ─────┬ centre ─────────────────────────────────┬ NoteView (embedded) ──────┐
│ full height      │ LiveLog (live) | Recording facts (post) │ note toolbar row          │
│ default 272 px,  │ | summary.md fallback + Review (ended)  │ (title, actions)          │
│ resizable,       │ ── Divider (split, unchanged) ──        │ BlockNote on note paper   │
│ collapsible      │ LiveSession (the meeting's own chat)    │                           │
└──────────────────┴─────────────────────────────────────────┴───────────────────────────┘
```

Phone (< md): one scroll as today. Briefs become a collapsible "Briefs · N" block above the transcript: open while live, collapsed once ended. The row keeps the actions, compact, and overflows into the ⋯ menu.

## Decisions (D1-D16; decided — workers do not re-litigate, deviations go in the as-built with the reason)

**Note (issue 1).**
- **D1. The meeting note is a full note.**
  - `NotePage.tsx` exports `NoteView({ id, embedded?, meeting? })`, today's inner `Note`. `NotePage` stays the route wrapper.
  - The meeting page's right column mounts `<NoteView id={manifest.note} embedded meeting={id} />`, which brings:
    - the note's own title (`TitleInput` → `doc_rename`);
    - the action bar: Rename · Attach to chat… · Snapshots… · Export · Undo agent action · Open in new window;
    - ParticipantStrip and Share;
    - the "Ask theuth…" slash item.
  - `embedded` means no `SectionBand` and no back link (`DocumentPage` gains `embedded`). Its toolbar is the pane's top row.
  - Paper: use the same paper as a regular note at that width, with no horizontal overflow at 1280 or 390.
  - **Delete is hidden for meeting notes**, embedded or popped. Deleting the note would orphan `manifest.note`, and the meeting's own Delete owns it.
  - "Open in new window" shows the full note page: `PoppedMeetingNote` renders a non-embedded `NoteView`, or the pop-out opens `{kind:'note', doc}`. Pick one and record which.
  - A meeting without a note keeps the existing `NoNote` text.
- **D2. A meeting note's chat is the meeting's own session.**
  - "Ask theuth…" and the note's Chat action go to the meeting's session; no separate note-chat session is created.
  - Embedded: the meeting chat is already beside the note, so focus its composer with the question, prefilled or sent the same way `NotePage`'s `onAsk` treats a regular note. Popped: the Chat panel shows the meeting's session.
  - If that routing proves impractical inside the wave, keep `NotePage`'s standard behaviour and write down why.

**Briefs (issue 2).**
- **D3. New `components/meeting/BriefColumn.tsx`.**
  - The first child of the body row in every state: live, post, failed, review, summarizing, done.
  - Full height: `flex min-h-0 shrink-0 flex-col overflow-y-auto`.
  - Default width 272 px, resizable with the shared `ResizeHandle` (138). The width persists in `lib/meeting-settings.ts` next to `split`.
  - Collapsible with a Briefs toggle in the header row; the collapsed state persists.
  - Oldest first. While live it sticks to the bottom if the reader is at the bottom.
  - Keeps the island cursor ring, the card markup and the cached link.
  - Removed from `ControlCenter.tsx` 159 and 188.
- **D4. Live updates.**
  - `lib/rpc.ts` invalidates `queryKeys.meetingBriefs(e.meeting)` on `TopicAdded`, `TopicRemoved` and `Brief`.
  - `BriefLog`'s local `extra` state goes.
  - No engine change: publishing `Brief` for topic briefs would push them onto the island's stack.

**Controls (issue 3).**
- **D5. New `components/meeting/MeetingActions.tsx`**, rendered in the `SectionRow` before "Session". By state:
  - **Review:** [Done reviewing], which becomes a muted "Reviewed HH:MM" once done, then [Summarize], disabled until reviewed.
  - **Done:** [Re-run post pipeline] with its existing confirm.
  - **Failed:** the strip's current fix action, [Retry summary] or [Re-run post pipeline].
  - **Post / Summarizing / live:** nothing; the strip spins, and pause/stop stay in `MeetingBar`.
  - **Delete** moves into a ⋯ menu at the end of the row ("Delete meeting…", existing confirm) in every ended state.
  - A popped meeting window shows the same actions in its drag header.
  - The old buttons are removed: `Summary.tsx` 22-36 (W1) and `Review.tsx` 54-67 (W2).
  - `reviewed_at` is read from the shared `queryKeys.meetingTranscript(id)` cache, with no extra fetch.
- **D6. Phone.** As in the Layout section.
- **D7. Dev fixture.** `dev/meeting-page-fixture.ts`, dev-only behind `import.meta.env.DEV` like `lib/talk-sim.ts`.
  - `?sim=meeting-live|meeting-review|meeting-done` on `/meetings/<any id>` seeds the React Query cache with:
    - the manifest;
    - a transcript of about 30 multi-word segments on both tracks with 3 labels;
    - 4 briefs;
    - one enrolled profile.
  - `meeting-live` also replays `TopicAdded` events through `dispatch` every few seconds, so D4 is visible.
  - Fixture RPCs that would write answer locally and never reach an engine.

**Playback (issue 4).**
- **D8. New `lib/segment-audio.ts`, playing through Web Audio**, which the CSP does not govern:
  1. Call `ctx.resume()` synchronously in the click handler, on `getSharedAudioContext()`.
  2. Fetch `meeting_segment_audio(id, seg.track, max(0, t0-150), t1+150)`.
  3. Decode base64 → `ArrayBuffer` → `decodeAudioData`, then `createBufferSource().start()`.
  4. Clicking the playing row again stops it; clicking another row stops the previous segment first.
  5. `onended` clears the playing marker.
  6. Errors toast in words, never a DOMException name.
  - Plays the segment's own track. Pure helpers (padding, base64 → bytes, state machine) get vitest tests.
- **D9. CSP.** `tauri.conf.json` gains `media-src 'self' blob: asset: http://asset.localhost`. This also unblocks the artifact `<audio>` and the notebook audio overview.
- **D10. Bounded decode.** `segment_wav` stops decoding past t1 and does not keep samples before t0, so memory per click is bounded by the segment, not the meeting.

**Enrollment (issue 5).**
- **D11. The per-row ⋯ becomes a `DropdownMenu`** on every segment in every ended state (Review, Summarizing, Done; not the live log). Items:
  - Play segment.
  - Split at a word… (Review only; opens today's words view).
  - Enroll as new speaker….
  - Add to enrolled speaker ▸: a submenu of `meeting_speakers` profiles, disabled with "No enrolled speakers yet" when empty.
  - When the row is in a multi-selection, the two enroll items act on the whole selection and say so ("Enroll 4 selected segments…").
- **D12. Selection** in every ended state (today Review only).
  - Row checkboxes, plus shift-click for a range.
  - The sticky bar shows "N selected · 23 s of speech", then Enroll as new… · Add to ▸ · Relabel and Merge (Review only) · Clear.
  - Enroll actions are disabled with the reason when the selection spans both tracks ("pick segments from one track") or has < 1 s of speech.
- **D13. Enroll dialog.**
  - Fields: name (required) and optional entity, as in the existing `SpeakerChip` enroll.
  - On the "already enrolled" error it offers an inline [Add to X instead].
  - On success:
    - `setQueryData(meetingTranscript(id), r.transcript)`;
    - invalidate `meetingSpeakers`;
    - toast "Enrolled Dana from 12 s of speech" or "Added 12 s to Dana (4 samples)", appending "(using 30 s of 74 s)" when capped.
- **D14. Engine `meeting_speaker_enroll`** (contract below).
  - **Speech:** the word spans of the chosen segments, one track, concatenated in segment order. At most the first 30 s is used; at least 1 s is required.
  - **New:** refuses a name already enrolled (case-insensitive) and creates a profile file.
  - **Existing:** folds the embedding into that profile by id, as today's running mean with samples + 1.
  - **Labels:** when every chosen segment carries one label, link it (`label.profile` and `name` set, `proposed` cleared) and save the transcript. Then refresh `proposed` (best_match >= 0.6) for the other unlinked labels that have embeddings.
  - **States:** refused in `Post(_)` and `Summarizing`, allowed in Review and Done. Runs off the actor mailbox (DelegatedReply + spawn_blocking), like `SegmentAudio`.
- **D15.** No migration and no new engine config keys. UI-only settings go in localStorage.
- **D16. Out of scope (v1):**
  - playback and enrollment from the live log while recording;
  - listing meeting notes in Library › Notes (asked of Sky separately);
  - renaming the meeting from the note title.

## Contract (engine ↔ UI; W3 implements, W2 codes against hand-written stubs)

Rust, in `crates/meeting` (re-exported at the crate root):

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind")]
pub enum EnrollTarget {
    /// A new profile; refused when that name (case-insensitive) is enrolled.
    New { name: String, entity: Option<String> },
    /// Fold the speech into this enrolled profile ("enhance").
    Existing { profile: ProfileId },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Enrolled {
    pub profile: Profile,
    /// Speech used for the voiceprint (<= 30 s).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub speech_ms: u64,
    /// Speech in the chosen segments before the cap.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub available_ms: u64,
    /// The transcript after linking (label.profile/name set, proposed refreshed); unchanged when not linked.
    pub transcript: Transcript,
}
```

RPC (`crates/rpc/src/api.rs`, beside `meeting_speakers`; types added to `export.rs`):

```rust
/// 156: a voiceprint from the speech of `segments` (one track): a new profile, or folded into an enrolled one.
#[method(name = "meeting_speaker_enroll")]
async fn meeting_speaker_enroll(&self, id: meeting::MeetingId, segments: Vec<u32>, target: meeting::EnrollTarget) -> RpcResult<meeting::Enrolled>;
```

The TS that ts-rs must emit, which W2's stubs copy:

```ts
// bindings/EnrollTarget.ts
export type EnrollTarget = { "kind": "New", name: string, entity: string | null, } | { "kind": "Existing", profile: ProfileId, };
// bindings/Enrolled.ts
export type Enrolled = { profile: Profile, speech_ms: number, available_ms: number, transcript: MeetingTranscript, };
// rpc/api.ts
meeting_speaker_enroll(id: MeetingId, segments: number[], target: EnrollTarget): Promise<Enrolled>
```

Use the type names the existing bindings use for `Profile`, `ProfileId`, `MeetingId` and the transcript. If ts-rs names the transcript type differently, W3's export wins and W2's stub follows it at integration.

Error strings: the UI shows them as they are. `{…}` marks a value:
- `segments span both tracks`
- `pick at least 1 s of speech (got {N} ms)`
- `speaker models not installed under {dir}`
- `a speaker named {X} is already enrolled`
- `no speaker profile {id}`
- `Summarizing: wait for it to finish`
- `still processing: wait for the post pipeline`

## Workers (files disjoint)

**W1 PAGE** (`feat/156-page`, `~/Develop/theuth-156-w1`), D1-D7. Files:
- `components/meeting/ControlCenter.tsx`, `Summary.tsx`, `Note.tsx`, `MeetingList.tsx` (header / popped header only).
- New `BriefColumn.tsx` and `MeetingActions.tsx`.
- `components/notes/NotePage.tsx`, `components/editor/DocumentPage.tsx`.
- `lib/rpc.ts` (invalidations only), `lib/meeting-settings.ts`, `routes/meetings.$id.tsx`.
- New `dev/meeting-page-fixture.ts`.
- Tests: `ControlCenter.test.ts`, `DocumentPage.test.ts`, and new `BriefColumn.test.ts` / `MeetingActions.test.ts`.

**W2 TRANSCRIPT** (`feat/156-transcript`, `~/Develop/theuth-156-w2`), D8, D9, D11-D13 and the `Review.tsx` half of D5. Files:
- `components/meeting/Review.tsx`, owned outright. It keeps the signature `Review({ id, readOnly })` and the `meetingTranscript(id)` cache.
- New `SegmentMenu.tsx` and `EnrollDialog.tsx`.
- New `lib/segment-audio.ts` (+ test).
- `rpc/api.ts`: add the one method.
- `crates/desktop/tauri.conf.json`: the CSP only.
- Tests: new `Review.test.ts` / `segment-audio.test.ts`.

**W3 ENGINE** (`feat/156-engine`, `~/Develop/theuth-156-w3`), D10 and D14. Files:
- `crates/meeting/src/lib.rs`: the `EnrollSegments` message and handler, plus the bounded `segment_wav`.
- `crates/meeting/src/speakers.rs`: `add_sample` and `create`.
- `crates/meeting/src/transcript.rs`: pure `speech_spans` and the label link / proposed refresh.
- `crates/rpc/src/api.rs`, `crates/rpc/src/export.rs`.
- Tests in those crates.

**Integration** (orchestrator, `integrate/156`, `~/Develop/theuth-156-int`):
1. Merge W3, then W1, then W2 `--no-ff`.
2. Export the bindings into the UI tree.
3. Run tsc, the full vitest, vite build, `mbx test -p meeting` and the rpc export.
4. Live check: playback on Sky's Done meeting (read-only), and the fixtures at 1280 and 390.
5. Enrollment end to end only on a copy of a meeting with the real models, never against `~/.theuth/meetings`.
