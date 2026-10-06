# 117 – Read-aloud on iOS: the last two words came out on the next mic press (speech.ts tail)

Status: as-built · 2026-10-06 · amends 43 §7 / the 2026-09-24 fix 2b8e9a64. Branch `feat/117-speech-tail` in `~/Develop/theuth-117`, base `9b82a6d0` (main). `crates/desktop/ui/src/lib/speech.ts` + `speech.test.ts` only; no Rust, no Composer/talk/dictation change, no bindings change.

Commits (oldest first):
- `2319f030` fix(speech): hold the session TAIL_MS past the last onended; cut a frozen tail on unlock (117)
- `7289f4ca` docs(design): 117 speech-tail (this note)
- `76359a00` fix(speech): our own resume in flight is not a frozen context (117 D2 R1)
- docs(design): 117 speech-tail R1 (this note)

## The complaint (Sky, 2026-10-06 17:08, verbatim)

"Have a small audio issue whenever I send a voice note from a remote device like an iPad or an iPhone. When the model replies, it starts reading it out loud, which is ideal, what it's supposed to do. But then the audio stops playing before like the last two words. So then whenever anything else starts using the microphone again, those last two words play. Like it got paused or something and it's waiting for me to recreate the audio context. It seems to only happen on longer replies, but we definitely need to figure out what's going on there and fix it."

## Diagnosis (file:line on 9b82a6d0)

`speech.ts` is the one audio-session owner of the UI: `unlock(use)` L80–95 (`cut()` only when nothing is held, `ctx.resume()`), `release(use)` L102–109 (last holder out: `cut()` then `ctx.suspend()`, type → `auto`), `cut()` L112–126, `push()` L184–220 with `src.onended` L209–218 → `drain()` L223–228 → `clear(speech)` L231–235 → `stop()` L238–244 → `release('speak')`, `onFinished()` L382–395 (`SpeechFinished { done }` sets `done` and drains). The 2026-09-24 fix made `speak` release on the LAST `onended` and stop every source before a suspend; the symptom survived it. Two routes remain, both iOS-only in practice:

- **Route A — suspending at `onended` strands the rendered tail.** `onended` fires when a source has finished *rendering*. The rendered audio is still in the output path (WebKit's render-ahead ring buffer to the GPU process, the device output latency, more over Bluetooth). `drain → clear → stop → release('speak') → ctx.suspend()` ran synchronously on that last `onended`, so the last tens to hundreds of milliseconds froze instead of playing. WebKit plays the stranded frames when the context next resumes — `unlock('record' | 'dictate' | 'talk')` on the next mic press: exactly "the last two words play when something uses the microphone again". Between the blocks of a multi-block reply the same suspend/resume churn happened.
- **Route B — a context suspended from outside with `speak` held.** iOS can suspend or interrupt the context without us (`ctx.state` → `'interrupted'` (WebKit) or `'suspended'`: route change, interruption, backgrounding). The remaining sources freeze in the graph, their `onended` never fires, `active` stays non-empty, `done` is set but `drain()` never releases, `uses` keeps `speak`. The next `unlock(use)` saw `uses.size === 1`, skipped `cut()` (L82) and `ctx.resume()` (L93) played the frozen tail. Nothing in the file looked at `ctx.state`.

Which route fires on Sky's device is not known (no device here); both are real code paths and both are closed.

## What changed

**D1 — tail grace.** `TAIL_MS = 400` (exported; render-ahead + device output latency, what `onended` does not account for). `drain()` now calls `end(speech)` instead of `clear()`: the badge drops at once when it is this speech (so `idle()` resolves and a queued block proceeds immediately) and, when `id === speech || id === null`, the decoder is freed, `id = null`, and `armTail()` schedules `release('speak')` after `TAIL_MS + ctx.outputLatency * 1000` ms (`outputLatency` read as optional: absent on older WebKits and the test fake). No `cut()` on this path (`active` is empty; `heard()` already reported the natural end; `next` carries over for talk's gapless `startQueued`). `unlock('speak')` inside the grace clears the timer and keeps the hold, so the context never suspends between blocks that follow within 400 ms. `release('speak')` (hence `stop()`, the `hidden` handler, talk-sim) clears the timer and releases at once as before. `onFinished()` with a reason other than `done` keeps the immediate `clear(speech) → stop()` path.

**D2 — unlocking a non-running context cuts what is frozen in it.** `unlock(use)` cuts when nothing is held (as before) OR when `ctx.state !== 'running'`: whatever is still scheduled in a suspended or interrupted context plays on `resume()`, and the use that unlocks it is starting something new. When that cut hits an utterance in progress (`id !== null` or `active` non-empty) and `use !== 'speak'`, `abandon()` gives it up: `setSpeaking(null)`, decoder freed, `id = null`, tail timer cleared, `uses.delete('speak')` before `uses.add(use)` — no suspend, and `session()` computes the mic type. For `use === 'speak'` (`start`, `startQueued`, `prime`) the cut is enough: `start()`/`begin()` take over `id`, `speak()` sets the new badge. No `statechange` listener and no self-resume: if WebKit resumes an interrupted context on its own (foregrounded again) the rest of the reply plays and drains normally; only a user-driven unlock cuts the tail.

**D3 — tests** (`speech.test.ts`, 12 → 16). `FakeContext` gains writable `state` (`'running'`; `suspend()`/`resume()` flip it) and `outputLatency = 0`; fake timers in `beforeEach`, real ones in `afterEach`; `window` stubbed to `globalThis` for `window.setTimeout`; `react`'s `useSyncExternalStore` mocked to return the snapshot so `useSpeaking()` asserts the badge without a render. The two 2026-09-24 tests that asserted a suspend on the last `onended` now assert *no* suspend there, `playback` still set, badge null, and exactly one suspend + `auto` after `TAIL_MS`. New: next block inside the grace (never a suspend; one after the second block's grace); `stop()` inside the grace (one suspend, none from the timer); Route B with `unlock('record')` (both frozen sources stopped, output node unchanged, badge null, `play-and-record`, calls `stop, stop, resume`; `release('record')` → one suspend, `auto`); Route B with `start('s2')` (old sources stopped, the new packet's source not).

**Revision 1 (review of 7289f4ca) — D2: our own resume in flight is not a frozen context.** `ctx.state` lags our `ctx.resume()` by the session's activation time (iOS: tens to hundreds of ms); a second `unlock` inside that window — `lib/mic-tap.ts::openTap` unlocks before and again after `await getUserMedia` — read `suspended`, took the frozen branch and would have cut sources scheduled since the first unlock (and sent a spurious `onHeard(null, true)`). `resuming` now tracks the pending `resume()` promise (cleared when it settles, and after a `suspend` in `release`); `suspended` while it is in flight is not frozen, `interrupted` still is (WebKit's resume stays pending until the interruption ends). Test: the fake `resume()` can be deferred; a double unlock during activation stops nothing, holds both uses, and the context ends `running`.

## Device test (Sky, iPad/iPhone after a rebuild)

1. Voice note → a long reply: the speech plays to the last word; nothing is left to replay.
2. Press the mic afterwards: no old audio comes out first.
3. The Now Playing island drops within about a second of the last word.
4. A reply with several blocks (tool calls between prose): no suspend/resume gap or replay between blocks.
5. Lock the screen mid-reply, unlock: if iOS resumes, the rest plays; pressing the mic instead plays nothing old.

## As built

- `speech.ts` (513 lines): `TAIL_MS` L68, `tailTimer` L69, `resuming` L71, `unlock` L95–125 (frozen cut + `abandon`, resume tracking), `release` L132–140 (clears the tail and the resume window), `drain` L255–260, `end` L266–275, `armTail` L278–286, `clearTail` L288–291, `abandon` L294–301, `clear` L304–308 (non-`done` reasons only), `stop` L311–317. Header comment and the `active`/`done` comments mention the grace and the non-running cut.
- Gates in `crates/desktop/ui` on the committed tree, first at `7289f4ca`: `bun run check` → **exit 0**; `CI=1 bun run test` → **145 files, 843 passed | 1 skipped (844)** in 248 s; `bunx vitest run src/lib/speech.test.ts` → **16 passed (16)**. Again at `76359a00` (R1): `bun run check` → **exit 0**; `CI=1 bun run test` → **145 files, 844 passed | 1 skipped (845)** in 72 s, no flake, no re-run needed; `bunx vitest run src/lib/speech.test.ts` → **17 passed (17)**.
- Deviation from the brief: the tail timer is a `number` from `window.setTimeout` (the file's sibling `voice-key.ts` convention; the runner has no DOM, so the test stubs `window = globalThis`) rather than a bare `setTimeout` handle. `TAIL_MS` is exported so the tests advance by the real constant.
- Environment note, not a code change: the worktree's pre-copied gitignored `src/rpc/bindings` (464 files) was stale — `bun run check` gave 119 errors on the untouched base, one of them in `speech.ts` (`held` not narrowed because `SpeechAudio.id` typed `any`). Replacing it with the 482-file set from `~/Develop/theuth-bindings` (`b5aaf52d`, read only) gives 0 errors; the same as design 115 found.
- Not verified here: any device behaviour (no iOS build in this wave); the 400 ms figure is the brief's, with `outputLatency` added on top where WebKit reports it.
