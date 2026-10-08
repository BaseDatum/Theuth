import { expect, test, vi } from 'vitest'

vi.mock('@/store/sessions', () => ({ on: () => () => undefined }))
vi.mock('@/lib/speech', () => ({
  setTalk: () => undefined,
  stop: () => undefined,
  release: () => Promise.resolve(),
  playedSeq: () => null,
  unlock: () => 'ctx',
}))
vi.mock('@/lib/mic-tap', () => ({}))
vi.mock('@/link/engines', () => ({ LOCAL: 'local', useEngines: { getState: () => ({ active: 'local' }) } }))
vi.mock('sonner', () => ({ toast: { error: () => undefined } }))
const chime = vi.hoisted(() => vi.fn())
vi.mock('@/lib/cue', () => ({ chime }))
vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })

import { useTalkSettings } from './talk-settings'
import { NO_SUBTITLES, type Subtitles, applySubtitles, handle, orbHint, orbState, phaseLine, start } from './talk'

const id = 't'

test('TalkState → orb: Holding keeps the speaking look, Interrupted drops to idle', () => {
  expect(orbState('idle')).toBe('idle')
  expect(orbState('listening')).toBe('idle')
  expect(orbState('thinking')).toBe('thinking')
  expect(orbState('working')).toBe('thinking')
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

test('a barge-in freezes the agent line on the cut sentence until the next turn starts', () => {
  const ev = (state: 'listening' | 'thinking' | 'holding' | 'interrupted' | 'idle') => ({ type: 'TalkState', id, state }) as const
  let s = applySubtitles(NO_SUBTITLES, { type: 'Saying', id, speech: 's1', text: 'Long answer.' })
  s = applySubtitles(s, ev('holding'))
  expect(s.agent).toBe('Long answer.')
  // The speaker stops first (`stopped`, not `done`): the line waits instead of blanking.
  s = applySubtitles(s, { type: 'SpeechFinished', id: 's1', session: 'x', reason: { reason: 'stopped' }, sink: 'device' })
  expect(s.agent).toBe('Long answer.')
  s = applySubtitles(s, { type: 'Interrupted', id, heard_sentences: 1, dropped_sentences: 2 })
  expect(s.agent).toBe('Long answer.')
  expect(s.speech).toBeNull()
  // Frozen: nothing late moves it — a queued sentence starting, the listening state, the user talking.
  s = applySubtitles(s, { type: 'Saying', id, speech: 's2', text: 'Never heard.' })
  s = applySubtitles(s, ev('interrupted'))
  s = applySubtitles(s, ev('listening'))
  s = applySubtitles(s, { type: 'Heard', id, utterance: 1, text: 'wait', final: true })
  s = applySubtitles(s, { type: 'Submitted', id, text: 'wait', interrupted: true })
  expect(s.agent).toBe('Long answer.')
  // It clears with the state: the next turn.
  s = applySubtitles(s, ev('thinking'))
  expect(s.agent).toBe('')
  expect(s.frozen).toBe(false)
  // …and the next turn's sentences show again.
  s = applySubtitles(s, { type: 'Saying', id, speech: 's3', text: 'Sure.' })
  expect(s.agent).toBe('Sure.')
})

test('a cut no Interrupted claims (Esc elsewhere) clears with the next state; streamed text never shows', () => {
  let s = applySubtitles(NO_SUBTITLES, { type: 'Saying', id, speech: 's1', text: 'One.' })
  s = applySubtitles(s, { type: 'SpeechFinished', id: 's1', session: 'x', reason: { reason: 'stopped' }, sink: 'device' })
  expect(s.agent).toBe('One.')
  s = applySubtitles(s, { type: 'TalkState', id, state: 'listening' })
  expect(s.agent).toBe('')
  // Only `Saying` (playback) puts agent text on the line: `Speak`-side events and states do not.
  s = applySubtitles(s, { type: 'TalkState', id, state: 'speaking' })
  expect(s).toEqual({ ...NO_SUBTITLES, said: { s1: 'One.' } })
})

test('phase line above the orb and hint below it follow the state', () => {
  expect(phaseLine('idle', false, false)).toBe('')
  expect(phaseLine('listening', false, false)).toBe('Listening')
  expect(phaseLine('thinking', false, false)).toBe('Thinking…')
  expect(phaseLine('thinking', true, false)).toBe('Working…')
  expect(phaseLine('working', false, false)).toBe('Working…')
  expect(phaseLine('speaking', false, true)).toBe('Speaking')
  expect(phaseLine('speaking', true, true)).toBe('Speaking')
  expect(phaseLine('speaking', true, false)).toBe('Working…')
  expect(phaseLine('holding', false, true)).toBe('Speaking')
  expect(orbHint('idle')).toBe('Tap the orb to speak')
  expect(orbHint('listening')).toBe('Tap the orb to stop listening')
  expect(orbHint('thinking')).toBe('Tap the orb to stop')
  expect(orbHint('working')).toBe('Tap the orb to stop')
  expect(orbHint('speaking')).toBe('Tap the orb to stop')
  expect(orbHint('holding')).toBe('Tap the orb to stop')
})

test('Announced: the harness sentence shows on the agent line until its Saying/SpeechFinished; frozen line holds', () => {
  let s: Subtitles = applySubtitles(NO_SUBTITLES, { type: 'Announced', id, text: 'Waiting for your approval to run shell: git push.' })
  expect(s.agent).toBe('Waiting for your approval to run shell: git push.')
  s = applySubtitles(s, { type: 'Saying', id, speech: 'a1', text: 'Waiting for your approval to run shell: git push.' })
  expect(s.speech).toBe('a1')
  s = applySubtitles(s, { type: 'SpeechFinished', id: 'a1', session: 'x', reason: { reason: 'done' }, sink: 'device' })
  expect(s.agent).toBe('')
  const frozen = applySubtitles({ ...NO_SUBTITLES, agent: 'Cut.', frozen: true }, { type: 'Announced', id, text: 'Waiting.' })
  expect(frozen.agent).toBe('Cut.')
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

test('client sink: once this speaker reports Playing, the agent line follows it, not Saying or SpeechFinished', () => {
  const fin = (id: string) => ({ type: 'SpeechFinished', id, session: 'x', reason: { reason: 'done' }, sink: 'client' }) as const
  // The first sentence: `Saying` (synthesis start) shows it, its first packet confirms.
  let s = applySubtitles(NO_SUBTITLES, { type: 'Saying', id, speech: 's1', text: 'One.' })
  expect(s.agent).toBe('One.')
  s = applySubtitles(s, { type: 'Playing', speech: 's1' })
  expect(s.agent).toBe('One.')
  // Synthesis runs ahead: the second sentence is announced and the first "finished" while the first still plays.
  s = applySubtitles(s, { type: 'Saying', id, speech: 's2', text: 'Two.' })
  s = applySubtitles(s, fin('s1'))
  expect(s.agent).toBe('One.')
  s = applySubtitles(s, { type: 'Playing', speech: 's2' })
  expect(s.agent).toBe('Two.')
  s = applySubtitles(s, fin('s2'))
  expect(s.agent).toBe('Two.')
  s = applySubtitles(s, { type: 'Playing', speech: null })
  expect(s.agent).toBe('')
  // A `Saying` that lands after the first packet of its sentence fills the line in.
  s = applySubtitles(s, { type: 'Playing', speech: 's3' })
  expect(s.agent).toBe('')
  s = applySubtitles(s, { type: 'Saying', id, speech: 's3', text: 'Three.' })
  expect(s.agent).toBe('Three.')
  // A barge-in on this speaker: `Flush` stops it (`Playing null, cut`) before `Interrupted` — the line holds.
  s = applySubtitles(s, { type: 'Playing', speech: null, cut: true })
  expect(s.agent).toBe('Three.')
  s = applySubtitles(s, { type: 'Interrupted', id, heard_sentences: 2, dropped_sentences: 1 })
  expect(s.agent).toBe('Three.')
  expect(s.plays).toBe(true)
  s = applySubtitles(s, { type: 'TalkState', id, state: 'listening' })
  expect(s.agent).toBe('Three.')
  s = applySubtitles(s, { type: 'TalkState', id, state: 'thinking' })
  expect(s.agent).toBe('')
})

test('the turn cue plays when the state leaves listening for thinking after Submitted, for the owner, behind talk.cue', async () => {
  const state = (s: 'listening' | 'thinking' | 'speaking' | 'holding') => handle({ type: 'TalkState', id, state: s })
  await start({ talk_start: () => Promise.resolve('t') } as never, 'sess')
  handle({ type: 'TalkStarted', id, session: 'sess', sink: 'device', aec: 'none', since_ms: 0n })
  handle({ type: 'Heard', id, utterance: 0, text: 'hello', final: true })
  handle({ type: 'Submitted', id, text: '[spoken, 1 s] hello', interrupted: false })
  // Committed, but the turn has not started yet.
  expect(chime).not.toHaveBeenCalled()
  state('thinking')
  expect(chime).toHaveBeenCalledTimes(1)
  expect(chime).toHaveBeenCalledWith('ctx')
  // Nothing else that leaves `listening` chimes: a barge-in resume (Holding → Speaking), a typed turn elsewhere.
  state('speaking')
  state('holding')
  handle({ type: 'Resumed', id, reason: 'backchannel' })
  state('speaking')
  state('listening')
  state('thinking')
  expect(chime).toHaveBeenCalledTimes(1)
  // A barge-in's own utterance, once committed, is a new turn and chimes.
  state('listening')
  handle({ type: 'Submitted', id, text: '[spoken, 1 s, interrupted] wait', interrupted: true })
  state('thinking')
  expect(chime).toHaveBeenCalledTimes(2)
  // The setting off: silent.
  useTalkSettings.getState().setCue(false)
  state('listening')
  handle({ type: 'Submitted', id, text: '[spoken, 1 s] again', interrupted: false })
  state('thinking')
  expect(chime).toHaveBeenCalledTimes(2)
  useTalkSettings.getState().setCue(true)
  // Another window attached to the talk (`mine: false`) stays silent.
  handle({ type: 'TalkFinished', id, reason: { reason: 'stopped' } })
  handle({ type: 'TalkStarted', id, session: 'sess', sink: 'device', aec: 'none', since_ms: 0n })
  handle({ type: 'Submitted', id, text: '[spoken, 1 s] hello', interrupted: false })
  state('thinking')
  expect(chime).toHaveBeenCalledTimes(2)
})
