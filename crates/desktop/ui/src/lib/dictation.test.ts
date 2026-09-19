import { expect, test } from 'vitest'
import { type State, applyEvent, splice } from './dictation'

const said = (...texts: string[]) => texts.map((text) => ({ text, final: true }))

test('splice inserts the region at the caret with one separating space inside it (54 §5.1)', () => {
  expect(splice('', '', said('hello'))).toEqual({ value: 'hello', caret: 5 })
  expect(splice('hi', '', said('there'))).toEqual({ value: 'hi there', caret: 8 })
  expect(splice('', 'world', said('hello'))).toEqual({ value: 'hello world', caret: 5 })
  // `before` already ends in a space: no double space.
  expect(splice('hi ', 'x', said('there'))).toEqual({ value: 'hi there x', caret: 8 })
  // `after` starts with punctuation: no space before it.
  expect(splice('so', '. Then', said('yes'))).toEqual({ value: 'so yes. Then', caret: 6 })
  // Mid-word caret: the user put it there; never merge into their word. Caret after `there`, before the trail space.
  expect(splice('hel', 'lo', said('there'))).toEqual({ value: 'hel there lo', caret: 9 })
  // Utterances join with one space; empty ones drop; open ones count too.
  expect(splice('a', 'b', [...said('one', ''), { text: 'two', final: false }])).toEqual({
    value: 'a one two b',
    caret: 9,
  })
})

test('splice leaves the value and caret alone when nothing was said', () => {
  expect(splice('hel', 'lo', [])).toEqual({ value: 'hello', caret: 3 })
  expect(splice('hel', 'lo', said('', ''))).toEqual({ value: 'hello', caret: 3 })
})

const idle: State = { model: { state: 'ready' }, active: null, phase: 'idle', elsewhere: false }
const pending: State = {
  ...idle,
  phase: 'starting',
  active: { id: '', before: 'a', after: 'b', utts: [], levels: [], startedAt: 0 },
}

test('applyEvent follows the §5.2 lifecycle for our id and tracks elsewhere for others', () => {
  // Started beats the RPC reply: the pending dictation adopts the id.
  const started = applyEvent(pending, { type: 'DictationStarted', id: 'd1' })
  expect(started.phase).toBe('dictating')
  expect(started.active?.id).toBe('d1')

  const t0 = applyEvent(started, { type: 'DictationText', id: 'd1', utterance: 0, text: 'hello', final: false })
  // Utterance 2 before 1: the gap is filled with an empty final utterance.
  const t2 = applyEvent(t0, { type: 'DictationText', id: 'd1', utterance: 2, text: 'world', final: true })
  expect(t2.active?.utts).toEqual([
    { text: 'hello', final: false },
    { text: '', final: true },
    { text: 'world', final: true },
  ])
  // Someone else's text is ignored.
  expect(applyEvent(t2, { type: 'DictationText', id: 'other', utterance: 0, text: 'x', final: true })).toBe(t2)

  const lv = applyEvent(t2, { type: 'DictationLevel', id: 'd1', levels: Array.from({ length: 30 }, (_, i) => i) })
  expect(lv.active?.levels).toHaveLength(24)
  expect(lv.active?.levels[23]).toBe(29)

  const done = applyEvent(lv, { type: 'DictationFinished', id: 'd1', reason: { reason: 'done' } })
  expect(done.active).toBeNull()
  expect(done.phase).toBe('idle')

  // Another frontend's dictation while we are idle.
  const theirs = applyEvent(idle, { type: 'DictationStarted', id: 'd9' })
  expect(theirs.elsewhere).toBe(true)
  expect(theirs.active).toBeNull()
  expect(applyEvent(theirs, { type: 'DictationFinished', id: 'd9', reason: { reason: 'stopped' } }).elsewhere).toBe(
    false,
  )
  // Model state flows through untouched.
  expect(applyEvent(idle, { type: 'DictationModel', state: { state: 'loading' } }).model).toEqual({ state: 'loading' })
})
