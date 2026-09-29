# 76 as built — TALK-UI-A (talk UI follow-ups from Sky's first mobile+desktop test)

Branch `fix/talk-ui-a` (worktree `theuth-talk-ui-a`), based on `main` @ db4fa2c3 (the brief named 31d0d710; the
worktree was cut later — no conflict with the allow-list). A first worker (an omp session lost in the 2026-09-29
provider outage) left uncommitted work on this branch; it was reviewed, kept where correct and finished here.
UI only — no Rust change (see A1, "Saying granularity").

## A1 Subtitles race ahead of the voice

**Cause (confirmed).** The agent line was set from `TalkEvent::Saying` and cleared by that utterance's
`SpeechFinished`. `Saying` is published by `crates/talk` on the voice actor's `SpeechStarted` — the start of
*synthesis* (`crates/voice/src/speak.rs` reports `Progress::Started` before the first frame). On the daemon's
speaker (`sink: device`) that is within the player ring's ~200 ms of playback, because `player.write` blocks and
paces synthesis to playback. On a client speaker (`sink: client`, the phone / a linked desktop) nothing paces it:
every sentence of a reply is synthesised, `Saying`-announced and `SpeechFinished` within a second or two while
the phone plays for many seconds — the line raced ahead and blanked while the voice was still talking.

**Saying granularity.** Already per sentence: `policy.rs speak_committed` splits each committed block into
sentences and issues one `Speak` (one `SpeechId`) per sentence; `SpeechStarted` of each yields one `Saying`. Not
too coarse, so `crates/talk/src/lib.rs` is untouched.

**Fix.**
- `speech.ts`: a talk's queued utterances are tracked in a schedule (`sched`: speech id → sources still to end).
  The head is what the speaker plays NOW; `setTalk(session, heard)` reports changes of it — the first packet of a
  sentence, the join to the next, `null` when the tail has played. An underrun (sources drained, packets still to
  come for a claimed utterance) is not an end. A stop (`Flush`, a read-aloud start, leaving) reports
  `null, cut=true`.
- `talk.ts`: the store turns that into a local `Playing { speech, cut }` event (the client-sink equivalent of
  `Saying`, per the brief). Reducer `applySubtitles`:
  - device sink: unchanged in spirit — `Saying` shows the sentence, its `SpeechFinished` (`done`) clears it;
  - client sink: `Saying` only records the text by id (`said`); once a `Playing` arrived the line follows
    `Playing` and ignores `SpeechFinished` (synthesis done, not playback) and the engine's early
    `TalkState listening`;
  - streamed/agent text is never on the line: only `Saying` (device) or `Playing` (client) put text there.
- **Interrupted freezes, then clears with the state.** A cut arrives as `SpeechFinished { stopped }` (device) or
  `Playing null, cut` (client, from `Flush` → `speech.stop()`), which marks the line `cut` instead of blanking;
  `Interrupted` sets `frozen`. Frozen, nothing moves the line (late `Saying`, `Playing`, `SpeechFinished`,
  `TalkState listening|holding|interrupted`); it clears when the talk leaves listening for the next turn
  (`thinking`/`speaking`/`idle`) or ends. The engine never publishes `TalkState interrupted` (commit goes straight
  to `listening`), so "clears with the state" = the next turn; a cut that no `Interrupted` claims (Esc from
  another frontend → `turn_finished cancelled`) clears on the next `TalkState`.
- The optional dimmed previous sentence on the second line was **not** added: the second line is the user's
  partial (76 §0.12) and `Subtitles.tsx` is outside the allow-list.

## A2 Phase line above the orb + turn cue

- `talk.phaseLine(state, tool, saying)`: `Listening`, `Thinking…` (turn started, nothing spoken), `Working…`
  (the session's `activity === 'tool'` — the talk state does not distinguish tool calls — while thinking, or while
  speaking with no sentence on the line), `Speaking`; `holding` keeps `Speaking` ("nothing new"); idle / no talk on
  this session → empty. Rendered muted above the orb in the hint's style (`text-xs text-fg-tertiary`, one line,
  150 ms fade, `aria-live=polite`).
- `lib/cue.ts`: `chime(ctx)` — two sine notes (E5 659 Hz 120 ms, A5 880 Hz 130 ms from +100 ms), 10 ms attack,
  exponential decay, peak gain 0.06, whole cue ≤ 250 ms, WebAudio only (no asset). Played on the talk's
  `AudioContext` (`speech.unlock('talk')`, taken inside the start gesture so it may play later).
- Trigger (`talk.ts handle`): `Submitted` sets `committed`; the next `TalkState thinking` *from* `listening` chimes
  and clears it (any state other than `listening` clears it too). So: never on a barge-in resume
  (`holding → speaking`), never on a typed turn from elsewhere (`TurnStarted` → thinking without `Submitted`),
  never mid-utterance (the gate closed before the submit); a committed barge-in utterance is a new turn and does
  chime. Only the client that owns the talk (`mine`) chimes; an attached second window stays silent.
- Setting `talk.cue`, default on: `lib/talk-settings.ts` (zustand, `localStorage['talk-cue']`, like the other
  client-only UI prefs). **Deviation:** there are no dictation/talk settings in the tree to sit beside; the toggle
  is a new *Talk* section in Settings › Chat ("Turn chime"), which is also the mobile settings sheet.

## A4 Orb hint matches the state

`talk.orbHint(state)`: idle (or no talk here) → "Tap the orb to speak"; listening → "Tap the orb to stop
listening"; thinking/speaking/holding → "Tap the orb to stop". One muted line below the orb/subtitles (the old
off-state caption "Tap the orb to start talking" in the subtitle caption slot is gone; that slot keeps the
`Degraded` caption). Hidden while a talk runs on another session (the "Talking in … — open" link shows instead).

## Simulator (`?sim=…`, DEV only)

- `?sim=1` (device script): now drives a tool call (`ToolCallPending`/`ToolCallFinished` dispatched to the
  session store → `Working…`) and a realistic cut (`SpeechFinished stopped` → `Interrupted`).
- `?sim=client` (new): the client-sink script — `Saying`/`SpeechFinished` seconds ahead of `Playing`, the engine
  back to `listening` while the phone still plays, then a second turn cut by `Flush` + `Playing null, cut` +
  `Interrupted`.
- `runSim(session, { hold, client })`; the route passes the whole flag.
- The chime is not exercised by the sim (the sim's talk is not `mine`, and WebAudio needs a gesture); it is
  covered by `talk.test.ts` and `cue.test.ts`.

## Files

`crates/desktop/ui/src/routes/talk.$session.tsx` (centre only: phase line, hint, `activity`, sim flag — header and
leave/Esc handler untouched), `src/lib/talk.ts`, `src/lib/talk.test.ts`, `src/lib/talk-sim.ts`,
`src/lib/speech.ts`, `src/lib/speech.test.ts`, new `src/lib/cue.ts` + `cue.test.ts`, new
`src/lib/talk-settings.ts`, `src/components/settings/Chat.tsx`. `speech.test.ts`, `talk-settings.ts` and `Chat.tsx`
are beyond the brief's literal allow-list: the speech test covers the new `speech.ts` behaviour, and the latter two
are "the settings store file for the toggle" and where it is shown.

## Verification

- On the final tree: `bunx tsc --noEmit` 0 errors; `bun run build` exit 0; `CI=1 bunx vitest run` → 118 files,
  666 tests passed (`bun install` in `crates/desktop/ui` and `packages/apps` first).
- New/changed tests: `talk.test.ts` — client-sink line follows `Playing` not `Saying`/`SpeechFinished`, a `Flush`
  cut holds the line, barge-in freeze until `thinking` (device and client), an unclaimed cut clears on the next
  state, only `Saying` puts text on the line, `phaseLine`/`orbHint` table, the cue fires on
  `listening → thinking` after `Submitted` only (not on resume, typed turns, setting off, or a non-owner);
  `speech.test.ts` — heard-now reporting incl. underrun and `cut` on stop (its ids made unique: `mine` is module
  state and an earlier test's `b` stayed claimed); `cue.test.ts` — two notes, ≤ 250 ms, ≤ peak gain.
- Simulator in headless Chrome (CDP, Vite dev against a scratch `t --headless` on `/tmp/talkuia-data`), sampled
  every 250 ms. Before: phase line absent, off-state caption only, the agent line blanked at the cut. After, 390×844
  `?sim=1`: `Listening` → `Thinking…` → `Working…` → `Thinking…` → `Speaking` with the sentence; at the barge-in the
  cut sentence stays through `Listening` + the user's words and clears at `Thinking…`; hints switch
  speak / stop listening / stop. 1440×900 `?sim=client`: the agent line stays on each sentence until its
  `Playing`, while the engine already says `Listening`. Layout at 390×700: phase 100–116, orb 156–390, subtitles
  430–565, hint 605–621, mobile Transcript button 656–688 px — no overlap.
- Not verified: the chime by ear and the client-sink path on a real phone (no device); the device-sink timing on
  real hardware (relies on the ring pacing described above). In the `?sim=client` run the app's `talk_status`
  refresh against the real scratch engine (no talk running) reset the simulated talk once — an artefact of
  running the sim against a live engine, not of this change.

DONE TALK-UI-A
