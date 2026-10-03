# 82 — As built: ELECTRIC-WAVE — the audiocn electric waveform for dictation, voice notes and talk

Branch `feat/electric-waveform` off `4d6a6ed6`. Scope: `crates/desktop/ui` only; no Rust. Brief: `/tmp/mc-2026-10-03/omp-electric-wave.md`.

## 1. What changed

### Vendored (shadcn registry `@audiocn`, `https://www.audiocn.dev/r/{name}.json`)

`components.json:23` adds the registry; `bunx shadcn@latest add @audiocn/electric-waveform @audiocn/use-audio-analyser` wrote 18 files, unmodified:

- `src/components/ui/electric-waveform.tsx` (`electric-waveform`)
- `src/lib/audio/{types,decibels,ballistics,zones,taper,bands,bar-levels,wave-line,time,frame-loop,frame-source}.ts` (`core`)
- `src/lib/electric.ts` (`electric`)
- `src/hooks/use-frame-source.ts`, `use-reduced-motion.ts`, `use-visibility.ts`, `use-audio-analyser.ts`, `use-audio-context.tsx`

Source repo `github.com/audiocn/ui`, `license.md`: **MIT** (Copyright 2026 OrcDev; GitHub's detector reports `NOASSERTION` only because the file appends a note about a Warcraft sound sample on the homepage, which is not vendored). The CLI also injected the `core` item's `--meter-*` / `--channel-*` CSS variables into `src/styles.css` (`:root`, `.dark`, `@theme inline`, including self-referencing `--channel-mute: var(--channel-mute)`); nothing in the electric waveform reads them, so that injection was reverted — the only styles.css change is the two `--electric-heat` lines below.

### New

- `src/lib/visual-source.ts` — the two frame sources (§A):
  - `analyserSource(ctx, stream)`: `createAnalyserTap` on `ctx.createMediaStreamSource(stream)` (32 bands, fft 1024, smoothing 0.5); `dispose` drops the analyser and the source node only.
  - `levelSource()`: `createFrameEmitter<VisualFrame>` fed from 20 ms level packets — 32 bands = `L · envelope` (raised cosine over `i/31` centred at 0.3, floor 0.15) × a ±10 % wobble keyed on the sample count; 64-deep history ring; `peakDb = 20·log10(max(L, 1e-4))`; no `timeDomain`. One frame per level, released 20 ms apart by `setTimeout`; `idle()` emits a zero frame; `dispose()` clears the queue.
  - `tapOrLevels(tap, onLevels)`: a site's choice — analyser when the UI holds the stream, else levels from a subscription.
- `src/components/voice/VoiceWave.tsx` — `VoiceWave({ source, state, className, style, label })` renders `<ElectricWaveform mode="wave" arcs sparks loading={state==='thinking'} intensity={recording ? 1.1 : 0.9} className="text-fg-accent h-full w-full">`; `RecordingBox({ recording, source, children })` (the composer's text region, §C); `useVoiceSource(enabled, make)` (build while enabled, dispose on off/unmount).
- Tests: `src/lib/visual-source.test.ts` (6), `src/components/voice/VoiceWave.test.ts` (2).

### Edited

- `src/styles.css:51-52, 57-58` — `--electric-heat: 0%` on `:root`, `70%` under `.dark`. theuth's dark theme is the `.dark` class (`@custom-variant dark (&:is(.dark *))`, `styles.css:7`); the vendored canvas class already keys its `dark:` fallback on the same variant, so the override is explicit documentation plus a guard against the vendored default changing. Verified visually: light = solid ochre line, dark = gold-ochre glow with a white-hot core.
- `src/lib/dictation.ts:94-106, 221` — `tap()` (the remote-engine `Tap` or `null`) and `onLevels(cb)` (raw `DictationLevel` packets of our dictation, fired from `handle`).
- `src/lib/talk.ts:148-160, 215-217, 246` — `currentTap()`, `onLevels(cb)`; `TalkLevel` is now listened to and routed through `handle` to the subscribers (it was not wired before).
- `src/components/chat/Composer.tsx`
  - `:64-66` imports; `:288-295` `noteWave` ref + `noteSource` state + `dropNoteWave()`; `:314-315` `dictSource = useVoiceSource(dictating && dict.phase === 'dictating', () => tapOrLevels(dictation.tap(), dictation.onLevels))`.
  - `:821, 829-840` record(): a dedicated `AudioContext` + `analyserSource(actx, stream)` created before `rec.start()`, disposed (and the context closed) in `onstop`; `:868-869` the recording effect's cleanup also disposes it (unmount mid-recording).
  - `:1080-1112` the text region is wrapped in `<RecordingBox recording source={noteSource}>`: while recording the textarea stays mounted, `invisible` + `aria-hidden`, and the wave covers it (`absolute inset-0`, wrapper `min-h-14`); the toolbar row (MicButton timer, lock/cancel, send) is untouched.
  - `:1167-1171` the dictation stop pill is `h-9 w-32` and holds `<VoiceWave state="listening" source={dictSource} label="Dictation level" />`; click/tooltip unchanged.
- `src/routes/talk.$session.tsx:13, 20, 79-83, 174` — `<VoiceWave>` directly under the orb, width = orb size (`style={{ width: size }}`, min 160 from the orb rule), `h-14` / `h-16` at ≥ 320 px; `mic = useVoiceSource(here, () => tapOrLevels(talk.currentTap(), talk.onLevels))`; state map: `listening|interrupted|holding → listening (mic source)`, `thinking → thinking (pulse, source null)`, `speaking → speaking (source null)`, else `idle`. The orb's centring rule and the `?sim=` path are untouched.
- `docs/design/76-talk.md:902` amendment line.

## 2. Source per site

| Site | Stream in the UI | Source |
|---|---|---|
| Composer dictation pill | remote engine (`dictation.tap()`) | `analyserSource(tap.ctx, tap.stream)` — second `MediaStreamSource` on the tap's stream; worklet untouched |
| Composer dictation pill | local engine | `levelSource()` fed by `dictation.onLevels` (`DictationLevel`, 8 × 20 ms, ~6/s) |
| Composer voice note | recorder stream | `analyserSource(new AudioContext(), stream)`; context closed with the recording |
| Talk band | relay client (`talk.currentTap()`) | `analyserSource(tap.ctx, tap.stream)` |
| Talk band | local engine | `levelSource()` fed by `talk.onLevels` (`TalkLevel.rms`) |
| Talk band, speaking | — | `source=null` (idle hum). `speech.ts` schedules every Opus packet as its own `AudioBufferSourceNode` straight to `ctx.destination` (`speech.ts:172-176`) — no node to tap without changing playback, and the local engine plays natively, so nothing reaches the UI. |

## 3. Gate results

- `bun run check` (tsc -b): clean.
- `CI=1 bun run test`: `Test Files 127 passed (127) · Tests 706 passed (706)`. The first full run (while a Vite dev server and a headless browser were starting on the same machine; 383 s) reported `1 failed | 705 passed` without naming the test in the captured tail; two further runs passed 706/706 (163 s, 58 s). Nothing was loosened.
- No Rust touched; no cargo gates run.
- Smoke: a throwaway Vite page (deleted) rendered the pill, the recording box and a thinking band in light and dark with `levelSource()` driving them — ochre line in light, white-hot core on gold in dark, textarea hidden under the box wave, pulse on the thinking band. The app itself could not be opened without a daemon (the web build gates on pairing), so the three manual checks below remain for a desktop run.

## 4. Deviations

- `LevelMeter.tsx` kept: `src/components/sidebar/SearchDialog.tsx:514` still renders it for the search box's dictation, and SearchDialog is outside this brief's seams. Composer no longer imports it. Follow-up below.
- `talk.ts` had no `TalkLevel` handling at all (the brief assumed it stored them); added the listener + `onLevels` fan-out rather than storing levels in the store (nothing re-renders per packet).
- The brief's `intensity={1.1}` for recording is passed as written; the component clamps it to 1.
- `useAudioAnalyser` (hook) is vendored but unused — the non-hook `createAnalyserTap` from the same file is what `analyserSource` needs. Left as vendored.

## 5. How to see it

1. **Dictate → wave in the pill.** Open a chat, tap the mic once (Ctrl+D). The stop pill (`h-9 w-32`, accent tint) shows the electric line moving with your voice; click it to stop. Local engine: driven by level packets; relay client: by the tap's analyser.
2. **Hold the mic → the box becomes the wave.** Press and hold the mic: the whole text box is replaced by the wave (your draft text is hidden, not lost), the timer counts on the MicButton, swipe up locks, release/Escape/send end it and the textarea comes back exactly as it was.
3. **Open Talk → band under the orb.** `/talk/$session` (or `?sim=thinking` in dev): a band the orb's width sits between the orb and the subtitles — live while you may speak, a pulse while thinking, a quiet hum while the agent speaks. Switch theme: ochre in light, gold with a white-hot core in dark.

## 6. Open follow-ups

- ~~Swap `SearchDialog`'s `LevelMeter` for `VoiceWave` and then delete `LevelMeter.tsx`.~~ Done in 83 (FX-VOICE).
- `talk-sim.ts` emits no `TalkLevel`; a few scripted packets during `listening` would let the band preview a voice without a mic.
- ~~Speaking state could show the agent's own audio if `speech.ts` routed packets through one shared `GainNode`.~~ Done in 83 (FX-VOICE): `speech.outputNode()` + `analyserNodeSource`.
