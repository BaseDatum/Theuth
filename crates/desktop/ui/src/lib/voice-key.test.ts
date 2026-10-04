import { afterEach, expect, test, vi } from 'vitest'
import {
  DONE_MS,
  type Event,
  IDLE,
  SPEAK_WAIT_MS,
  type State,
  composeBody,
  deskCacheKey,
  lastSentence,
  playback,
  reduce,
  sendMethod,
  statusLine,
  turn,
  useVoiceKey,
} from './voice-key'

// No DOM in the test runner: the orchestration's `window.setTimeout` and `isTauri()` (`'…' in window`) need a `window`.
vi.stubGlobal('window', globalThis)

const run = (events: Event[], from: State = IDLE): State => events.reduce(reduce, from)
const samples = [
  { t_ms: 0, x: 10, y: 10, pen: true },
  { t_ms: 16, x: 12, y: 11, pen: true },
]
const sent = (): State => run([{ type: 'start', at: 1 }, { type: 'stop', samples, durationMs: 2000 }, { type: 'sent', desk: 'desk-1' }])

afterEach(() => {
  vi.useRealTimers()
  useVoiceKey.setState(IDLE)
  // `onStart` resets these in the app; the lifetime tests seed the store directly.
  turn(false)
  playback(null)
})

test('point:start opens listening from any phase with the clock and nothing else', () => {
  const s = run([{ type: 'start', at: 1000 }], { ...IDLE, phase: 'done', reply: 'old', desk: 'd' })
  expect(s).toEqual({ ...IDLE, phase: 'listening', startedAt: 1000 })
})

test('point:lock sets the glyph while listening only', () => {
  expect(run([{ type: 'start', at: 1 }, { type: 'lock' }]).locked).toBe(true)
  expect(reduce(IDLE, { type: 'lock' })).toBe(IDLE)
})

test('point:stop moves to sending with the path and the hold, then the final text and the screen lines land', () => {
  const s = run([
    { type: 'start', at: 1 },
    { type: 'stop', samples, durationMs: 4200 },
    { type: 'text', text: 'clone this repo' },
    { type: 'context', lines: ['[screen 1/1 "Chrome"]'], files: ['/a.png'] },
  ])
  expect(s).toMatchObject({ phase: 'sending', samples, durationMs: 4200, text: 'clone this repo', lines: ['[screen 1/1 "Chrome"]'], files: ['/a.png'] })
})

test('an empty transcript is a hint ("Nothing recognized") that clears to idle; the gesture is forgotten', () => {
  const s = run([{ type: 'start', at: 1 }, { type: 'stop', samples, durationMs: 900 }, { type: 'text', text: '  ' }, { type: 'cancel', hint: 'Nothing recognized' }])
  expect(s).toEqual({ ...IDLE, phase: 'hint', hint: 'Nothing recognized' })
  expect(reduce(s, { type: 'idle' })).toBe(IDLE)
})

test('point:cancel with a hint shows it; without one the panel is idle at once', () => {
  const held = run([{ type: 'start', at: 1 }])
  expect(reduce(held, { type: 'cancel', hint: 'Hold to talk · double-tap to lock' })).toMatchObject({ phase: 'hint', hint: 'Hold to talk · double-tap to lock' })
  expect(reduce(held, { type: 'cancel', hint: null })).toBe(IDLE)
})

test('a desk failure while sending is a hint and drops the desk', () => {
  const s = run([{ type: 'start', at: 1 }, { type: 'stop', samples, durationMs: 2000 }, { type: 'cancel', hint: "theuth's desk lives on another engine" }])
  expect(s.phase).toBe('hint')
  expect(s.desk).toBeNull()
})

test('sent → working collects deltas and the approval; playback flips working ↔ speaking; spoken → done; stray events are ignored', () => {
  const w = sent()
  expect(w).toMatchObject({ phase: 'working', desk: 'desk-1', reply: '', spoken: null })
  const withText = run([{ type: 'delta', text: 'Sure, ' }, { type: 'delta', text: 'cloning.' }, { type: 'approval', on: true }], w)
  expect(withText).toMatchObject({ reply: 'Sure, cloning.', approval: true })
  expect(reduce(withText, { type: 'approval', on: false }).approval).toBe(false)
  const speaking = reduce(withText, { type: 'playing', entry: 'e1' })
  expect(speaking).toMatchObject({ phase: 'speaking', spoken: 'e1' })
  // Between blocks the turn is still running: back to working, the spoken entry remembered.
  const between = reduce(speaking, { type: 'silent' })
  expect(between).toMatchObject({ phase: 'working', spoken: 'e1' })
  const done = reduce(between, { type: 'spoken' })
  expect(done.phase).toBe('done')
  // A late block reopens: the engine's `SpeechStarted` → working, playback → speaking.
  expect(reduce(done, { type: 'queued' }).phase).toBe('working')
  expect(reduce(done, { type: 'playing', entry: 'e2' })).toMatchObject({ phase: 'speaking', spoken: 'e2' })
  // `spoken` ends a working reply only; `silent` only leaves speaking; deltas before `sent` have no turn to join.
  expect(reduce(speaking, { type: 'spoken' })).toBe(speaking)
  expect(reduce(w, { type: 'silent' })).toBe(w)
  expect(reduce(IDLE, { type: 'spoken' })).toBe(IDLE)
  expect(reduce(IDLE, { type: 'playing', entry: 'x' })).toBe(IDLE)
  expect(reduce(run([{ type: 'start', at: 1 }]), { type: 'delta', text: 'x' }).reply).toBe('')
})

test("the desk's Error is its own phase with the message; it is ignored outside a reply", () => {
  const s = reduce(reduce(sent(), { type: 'playing', entry: 'e1' }), { type: 'error', message: 'boom' })
  expect(s).toMatchObject({ phase: 'error', error: 'boom' })
  expect(reduce(run([{ type: 'start', at: 1 }]), { type: 'error', message: 'boom' }).phase).toBe('listening')
})

test('the reply is over when the turn has finished AND nothing plays here; the engine finishing synthesis plays no part', () => {
  vi.useFakeTimers()
  useVoiceKey.setState(sent())
  // Turn done first, a block still playing (synthesis finished long ago): nothing happens.
  playback('e1')
  turn(true)
  vi.advanceTimersByTime(SPEAK_WAIT_MS + DONE_MS + 1000)
  expect(useVoiceKey.getState().phase).toBe('speaking')
  // Playback ends: a grace of one `speak` round trip, then done, then concealed after `DONE_MS`.
  playback(null)
  expect(useVoiceKey.getState().phase).toBe('working')
  vi.advanceTimersByTime(SPEAK_WAIT_MS - 1)
  expect(useVoiceKey.getState().phase).toBe('working')
  vi.advanceTimersByTime(1)
  expect(useVoiceKey.getState().phase).toBe('done')
  vi.advanceTimersByTime(DONE_MS)
  expect(useVoiceKey.getState().phase).toBe('idle')
})

test('a block that starts inside the grace keeps the panel; playback ending before the turn does not end the reply', () => {
  vi.useFakeTimers()
  useVoiceKey.setState(sent())
  playback('e1')
  playback(null)
  // Turn still running: no grace armed.
  vi.advanceTimersByTime(SPEAK_WAIT_MS * 3)
  expect(useVoiceKey.getState().phase).toBe('working')
  turn(true)
  vi.advanceTimersByTime(SPEAK_WAIT_MS - 100)
  playback('e2')
  vi.advanceTimersByTime(SPEAK_WAIT_MS * 3)
  expect(useVoiceKey.getState().phase).toBe('speaking')
  // A late block while done (a `SpeechStarted` after the grace) cancels the conceal and reopens speaking.
  playback(null)
  vi.advanceTimersByTime(SPEAK_WAIT_MS)
  expect(useVoiceKey.getState().phase).toBe('done')
  vi.advanceTimersByTime(DONE_MS - 1)
  playback('e3')
  vi.advanceTimersByTime(DONE_MS * 2)
  expect(useVoiceKey.getState().phase).toBe('speaking')
})

test('a turn with nothing to say (voice off, empty block) ends after the grace alone', () => {
  vi.useFakeTimers()
  useVoiceKey.setState(sent())
  turn(true)
  vi.advanceTimersByTime(SPEAK_WAIT_MS)
  expect(useVoiceKey.getState().phase).toBe('done')
  vi.advanceTimersByTime(DONE_MS)
  expect(useVoiceKey.getState().phase).toBe('idle')
})

test('the working status line follows the slice: waiting, reasoning, the running tool, vision, streaming; approval wins', () => {
  expect(statusLine(undefined, undefined, false)).toBe('Thinking…')
  expect(statusLine({ waiting: true, activity: 'tool' }, 'bash', false)).toBe('Thinking…')
  expect(statusLine({ waiting: false, activity: 'reasoning' }, undefined, false)).toBe('Reasoning…')
  expect(statusLine({ waiting: false, activity: 'tool' }, 'bash', false)).toBe('Running bash…')
  expect(statusLine({ waiting: false, activity: 'tool' }, undefined, false)).toBe('Working…')
  expect(statusLine({ waiting: false, activity: 'vision' }, undefined, false)).toBe('Looking at the screen…')
  expect(statusLine({ waiting: false, activity: 'streaming' }, undefined, false)).toBe('Writing…')
  expect(statusLine({ waiting: false, activity: 'streaming' }, undefined, true)).toBe('Needs your approval')
  expect(lastSentence('One. Two! Three?  ')).toBe('Three?')
  expect(lastSentence('')).toBe('')
})

test('placing: entered from idle only, left by Done or a press', () => {
  expect(reduce(IDLE, { type: 'place' })).toEqual({ ...IDLE, phase: 'placing' })
  const placing = reduce(IDLE, { type: 'place' })
  expect(reduce(run([{ type: 'start', at: 1 }]), { type: 'place' }).phase).toBe('listening')
  expect(reduce(placing, { type: 'placed' })).toBe(IDLE)
  expect(reduce(placing, { type: 'start', at: 5 })).toEqual({ ...IDLE, phase: 'listening', startedAt: 5 })
  expect(reduce(sent(), { type: 'placed' }).phase).toBe('working')
})

test('the trail setting defaults to always and persists next to the other voice-key settings', async () => {
  const mem = new Map<string, string>()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, v),
  })
  // Dynamic on purpose: each `import` must run the module's `loadSettings()` against the storage stubbed above.
  vi.resetModules()
  const a = await import('./voice-key')
  expect(a.useVoiceKeySettings.getState().annotate).toBe('always')
  a.useVoiceKeySettings.getState().set({ annotate: 'mouse' })
  expect(JSON.parse(mem.get('voiceKey') ?? '')).toMatchObject({ annotate: 'mouse', enabled: true })
  vi.resetModules()
  const b = await import('./voice-key')
  expect(b.useVoiceKeySettings.getState().annotate).toBe('mouse')
  vi.unstubAllGlobals()
  vi.stubGlobal('window', globalThis)
})

test('composeBody: the voice-note line first, then one [screen …] line per display; none without lines', () => {
  expect(composeBody(' clone this repo ', 4200, [])).toBe('[voice note, 5 s] clone this repo')
  expect(composeBody('clone this repo', 4000, ['[screen 1/2 "A"]', '[screen 2/2 "B"]'])).toBe('[voice note, 4 s] clone this repo\n[screen 1/2 "A"]\n[screen 2/2 "B"]')
  // 45 §4.4: at least one second.
  expect(composeBody('hi', 120, [])).toBe('[voice note, 1 s] hi')
})

test('a running desk is steered, an idle one gets send_input (Composer.tsx:676-686)', () => {
  expect(sendMethod(true)).toBe('steer')
  expect(sendMethod(false)).toBe('send_input')
})

test('the desk cache is per engine', () => {
  expect(deskCacheKey('local')).not.toBe(deskCacheKey('hub-2'))
  expect(deskCacheKey('local')).toBe(deskCacheKey('local', 'theuth'))
})
