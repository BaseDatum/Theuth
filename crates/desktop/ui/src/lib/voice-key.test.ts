import { expect, test } from 'vitest'
import { type Event, IDLE, type State, composeBody, deskCacheKey, reduce, sendMethod } from './voice-key'

const run = (events: Event[], from: State = IDLE): State => events.reduce(reduce, from)
const samples = [
  { t_ms: 0, x: 10, y: 10 },
  { t_ms: 16, x: 12, y: 11 },
]

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

test('sent → replying collects deltas, approval and errors; spoken → done; a late utterance reopens replying; deltas elsewhere are ignored', () => {
  const sent = run([{ type: 'start', at: 1 }, { type: 'stop', samples, durationMs: 2000 }, { type: 'sent', desk: 'desk-1' }])
  expect(sent).toMatchObject({ phase: 'replying', desk: 'desk-1', reply: '' })
  const replied = run([{ type: 'delta', text: 'Sure, ' }, { type: 'delta', text: 'cloning.' }, { type: 'approval' }, { type: 'error', message: 'boom' }], sent)
  expect(replied).toMatchObject({ reply: 'Sure, cloning.', approval: true, error: 'boom' })
  const done = reduce(replied, { type: 'spoken' })
  expect(done.phase).toBe('done')
  expect(reduce(done, { type: 'speaking' }).phase).toBe('replying')
  // `spoken` only ends a reply; a delta before `sent` has no turn to join.
  expect(reduce(IDLE, { type: 'spoken' })).toBe(IDLE)
  expect(reduce(run([{ type: 'start', at: 1 }]), { type: 'delta', text: 'x' }).reply).toBe('')
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
