import { expect, test, vi } from 'vitest'

vi.mock('@/store/sessions', () => ({ on: () => () => undefined }))
vi.mock('@/lib/speech', () => ({ setTalk: () => undefined, stop: () => undefined, release: () => Promise.resolve(), playedSeq: () => null }))
vi.mock('@/lib/mic-tap', () => ({}))
vi.mock('@/link/engines', () => ({ LOCAL: 'local', useEngines: { getState: () => ({ active: 'local' }) } }))
vi.mock('sonner', () => ({ toast: { error: () => undefined } }))

import { NO_SUBTITLES, type Subtitles, applySubtitles, orbState } from './talk'

const id = 't'

test('TalkState → orb: Holding keeps the speaking look, Interrupted drops to idle', () => {
  expect(orbState('idle')).toBe('idle')
  expect(orbState('listening')).toBe('idle')
  expect(orbState('thinking')).toBe('thinking')
  expect(orbState('speaking')).toBe('speaking')
  expect(orbState('holding')).toBe('speaking')
  expect(orbState('interrupted')).toBe('idle')
})

test('agent line: Saying until its own SpeechFinished; another utterance finishing leaves it', () => {
  let s: Subtitles = applySubtitles(NO_SUBTITLES, { type: 'Saying', id, speech: 's1', text: 'Hello there.' })
  expect(s.agent).toBe('Hello there.')
  s = applySubtitles(s, { type: 'SpeechFinished', id: 's0', session: 'x', reason: { reason: 'done' }, sink: 'device' })
  expect(s.agent).toBe('Hello there.')
  s = applySubtitles(s, { type: 'SpeechFinished', id: 's1', session: 'x', reason: { reason: 'done' }, sink: 'device' })
  expect(s.agent).toBe('')
})

test('agent line clears on Interrupted', () => {
  const s = applySubtitles(applySubtitles(NO_SUBTITLES, { type: 'Saying', id, speech: 's1', text: 'Long answer.' }), {
    type: 'Interrupted',
    id,
    heard_sentences: 1,
    dropped_sentences: 2,
  })
  expect(s.agent).toBe('')
  expect(s.speech).toBeNull()
})

test('user line: partials append, a shorter partial never shrinks it, the final replaces, Submitted clears', () => {
  let s = applySubtitles(NO_SUBTITLES, { type: 'Heard', id, utterance: 0, text: 'so what', final: false })
  s = applySubtitles(s, { type: 'Heard', id, utterance: 0, text: 'so what would you', final: false })
  expect(s.user).toBe('so what would you')
  // A late, shorter partial (reordered window) must not flicker the line back.
  s = applySubtitles(s, { type: 'Heard', id, utterance: 0, text: 'so what', final: false })
  expect(s.user).toBe('so what would you')
  // A revision that is not a prefix is what the recognizer now believes.
  s = applySubtitles(s, { type: 'Heard', id, utterance: 0, text: 'So, what would', final: false })
  expect(s.user).toBe('So, what would')
  s = applySubtitles(s, { type: 'Heard', id, utterance: 0, text: 'So, what would you say?', final: true })
  s = applySubtitles(s, { type: 'Heard', id, utterance: 1, text: 'And why', final: false })
  expect(s.user).toBe('So, what would you say? And why')
  s = applySubtitles(s, { type: 'Submitted', id, text: '[spoken, 3 s] …', interrupted: false })
  expect(s.user).toBe('')
  expect(s.utts).toEqual([])
})
