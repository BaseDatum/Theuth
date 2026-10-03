# 83 — As built: FX-VOICE — the talk band shows the agent's voice; SearchDialog uses VoiceWave; LevelMeter deleted

Brief: `/tmp/mc-2026-10-03/omp-fx-voice.md`. Base `2cb6a14e` (main), branch `fix/voice-wave-followups`. UI only (`crates/desktop/ui`); no Rust.

## 1. What changed (file:line)

### §1 Shared output node in `speech.ts`
- `src/lib/speech.ts:25-42` — `out: GainNode | null`, `outSubs`; `outputNode(): AudioNode | null` (`:33`); `onOutputReady(cb)` (`:36`): calls now if the node exists, else once, returns the unsubscribe.
- `:85-92` `unlock`: the context and the gain are made together (`createGain`, `gain.value = 1`, `connect(destination)`, waiting subscribers called once and cleared). Nothing else in the file creates a context.
- `:190, :199` `push`: every scheduled `AudioBufferSourceNode` connects to `out` instead of `ctx.destination`. That was the only `destination` connection in the file, so read-aloud in chat flows through the same node (tappable, no UI). Scheduling (`next`), `cut`'s `stop()`/`disconnect()`, `unlock`/`release` session ownership, iOS behaviour: untouched.

### §1 Analyser on a node, the band
- `src/lib/visual-source.ts:32-35` — `analyserNodeSource(ctx: BaseAudioContext, node)`: `createAnalyserTap` with the wave shape (32 bands, fft 1024, smoothing 0.5); `dispose` = the tap's (detaches only the node→analyser edge). `analyserSource(ctx, stream)` (`:16-26`) now builds its `MediaStreamSource` and delegates to it, then disconnects its own node on dispose.
- `src/routes/talk.$session.tsx:54-68` — `speakerSource()`: a `createFrameRelay` fed by `analyserNodeSource(node.context, node)` when `speech.onOutputReady` fires (now, or when the context appears); `dispose` unsubscribes and drops the tap. `:107` `voice = useVoiceSource(here && wave === 'speaking', speakerSource)`; `:198` the band takes `mic` while listening, `voice` while speaking, `null` otherwise. Listening/thinking/idle as built in 82.
- A `device`-sink talk (engine speaker) or a talk whose context has no audio yields a silent tap → the band draws the same flat line a `null` source does (`wave-line.ts` idle is `static`), so nothing regressed visually.

### §1 Simulator
- `src/lib/talk-sim.ts:112-139` — `tone()`: a 180 Hz oscillator with a 4 Hz tremolo through `speech.unlock('speak')` → `speech.outputNode()`; `runSim` (`:142-173`) starts it on `TalkState speaking`, stops it on any other `TalkState` and on the sim's stop (`release('speak')`). The module is already DEV-only (imported behind `import.meta.env.DEV`); no production path changed.

### §2 SearchDialog
- `src/components/sidebar/SearchDialog.tsx:6, :22, :169-170, :517` — `LevelMeter` → `<VoiceWave state="listening" source={dictSource} label="Dictation level" className="h-6 w-16" />` inside the existing `h-8 w-20` pill; `dictSource = useVoiceSource(dict.dictating && dict.dict.phase === 'dictating', () => tapOrLevels(dictation.tap(), dictation.onLevels))` — the composer pill's source line (`Composer.tsx:315`), not duplicated logic.
- `src/components/chat/LevelMeter.tsx` deleted (64 lines). It had no test; grep shows no other importer.

### §3 Docs
- `docs/design/82-as-built-ELECTRIC-WAVE.md:73, :75` — the two follow-ups struck through, pointing here.
- `docs/design/76-talk.md:904` — amendment line.

## 2. Behaviours and their tests

| Behaviour | Test |
|---|---|
| Every scheduled source connects to the one gain; gain = 1, gain → `destination`; `onOutputReady` registered before any `unlock` fires exactly once with that node; a late subscriber is called at once | `src/lib/speech.test.ts` "every scheduled source plays through the one output gain (83 §1), announced once the context exists" (+ `FakeGain`, `FakeSource.to`, module-level `outputReady`/`outputNode() === null` assertion) |
| `analyserNodeSource` taps the given node with fft 1024 / smoothing 0.5, creates no `MediaStreamSource`, and on dispose disconnects only the node→analyser edge | `src/lib/visual-source.test.ts` "analyserNodeSource › taps the given node … leaves the node connected on dispose" |
| `analyserSource` still disconnects its own source node fully | existing "analyserSource" test (unchanged, passes through the refactor) |

SearchDialog has no render harness (cmdk + router + query); the swapped piece is `VoiceWave`, covered by `VoiceWave.test.ts`, and the source hook is the composer's.

Smoke (headless Chromium against `vite` dev, throwaway `smoke.html` + `src/__smoke.tsx`, both deleted):
1. `VoiceWave state="speaking"` + `speakerSource()` mounted **before** any context existed; a click ran `speech.unlock('speak')` and scheduled a looping buffer source on `speech.outputNode()` exactly as `push` does → the band's `data-active` went `false → true` and the line moved (screenshot taken). Proves the gain tap, `analyserNodeSource` and the late `onOutputReady` attach.
2. `runSim('s', { hold: 'speaking' })` with the band keyed on `talk.useTalk().state`: sampled every 1.2 s — `listening`/`thinking` inactive, `speaking` active from the first sample on (the DEV tone through the output node).

The app itself cannot be opened without a daemon (the web build gates on pairing), same as 82; a real `/talk/$session?sim=speaking` run is a manual check for a desktop session.

## 3. Gate results (verbatim counts)

- `bun install` in `crates/desktop/ui` (617 packages) and `packages/apps` (484 packages); `just ts-desktop` once (bindings were missing; 464 files).
- `bun run check` → `tsc -b --noEmit`, clean.
- `CI=1 bun run test < /dev/null` → `Test Files  128 passed (128)`, `Tests  710 passed (710)` (was 708 at base; +2).
- No Rust touched; no cargo gates run (brief: UI package gates only).

## 4. Deviations

- `onOutputReady` callbacks are called once and cleared when the node is created (not kept as a persistent subscription): the node never changes for the module's life, so a second call cannot happen; a subscriber arriving later is called synchronously. The returned unsubscribe still works for a subscriber that gives up before the context exists.
- `analyserNodeSource` takes `BaseAudioContext` (what `createAnalyserTap` accepts and what `node.context` is), where the brief wrote `AudioContext`.
- The sim tone takes the `speak` hold via `unlock('speak')`/`release('speak')` so the context is running while it plays. Dev-only; no production session rule touched.
- `speakerSource` lives in `routes/talk.$session.tsx` rather than `visual-source.ts`, so `visual-source.ts` (and its test) stays free of a `speech.ts` import (opus-decoder, react-query, the session store).
- One `vi.fn()` call-count assertion in the new speech test was replaced by a plain array: mock call records are cleared between tests in this suite, and the callback fires in the first test that unlocks.

## 5. Open follow-ups

- Read-aloud in chat is now tappable (`speech.outputNode()`), no UI shows it yet.
- A real-device check of the band while a `client`-sink talk speaks on a phone through the relay (headless Chromium only proves the graph).
- `talk-sim.ts` still emits no `TalkLevel` for `listening` (82 follow-up, unchanged).
