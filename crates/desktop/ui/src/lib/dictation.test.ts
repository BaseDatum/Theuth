import { expect, test, vi } from 'vitest'
import type { Api } from '@/rpc/api'
import type { VoiceEvent } from '@/rpc/bindings/VoiceEvent'
import { type Active, type State, anchor, applyEvent, current, finish, splice, start, startOpts } from './dictation'

vi.mock('@/lib/speech', () => ({ release: async () => undefined, hold: async () => undefined }))

const said = (...texts: string[]) => texts.map((text) => ({ text, shaped: text, final: true }))
const textEvent = (e: Partial<Extract<VoiceEvent, { type: 'DictationText' }>>): VoiceEvent => ({
  type: 'DictationText',
  id: 'd1',
  utterance: 0,
  text: '',
  final: false,
  shaped: '',
  command: null,
  actions: [],
  ...e,
})

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
  expect(splice('a', 'b', [...said('one', ''), { text: 'two', shaped: 'two', final: false }])).toEqual({
    value: 'a one two b',
    caret: 9,
  })
})

test('splice renders the shaped text, falling back to the raw text when shaping sent none (153 §3.1)', () => {
  expect(splice('', '', [{ text: 'hi dana comma the deploy is done period', shaped: 'Hi Dana, the deploy is done.', final: true }])).toEqual({
    value: 'Hi Dana, the deploy is done.',
    caret: 28,
  })
  expect(splice('', '', [{ text: 'raw only', shaped: '', final: false }]).value).toBe('raw only')
})

test('splice leaves the value and caret alone when nothing was said', () => {
  expect(splice('hel', 'lo', [])).toEqual({ value: 'hello', caret: 3 })
  expect(splice('hel', 'lo', said('', ''))).toEqual({ value: 'hello', caret: 3 })
})

test('anchor splits at the caret, drops a selection, and defaults to the end', () => {
  expect(anchor('hello world', 5, 5)).toEqual({ before: 'hello', after: ' world' })
  expect(anchor('hello world', 0, 5)).toEqual({ before: '', after: ' world' })
  expect(anchor('hello world', null, null)).toEqual({ before: 'hello world', after: '' })
  // A backwards `end` (never from a browser, but cheap to pin) never duplicates text.
  expect(anchor('abc', 2, 1)).toEqual({ before: 'ab', after: 'c' })
})

const idle: State = { model: { state: 'ready' }, active: null, phase: 'idle', elsewhere: false }
const pending: State = {
  ...idle,
  phase: 'starting',
  active: { id: '', before: 'a', after: 'b', utts: [], levels: [], startedAt: 0, device: null },
}

test('applyEvent follows the §5.2 lifecycle for our id and tracks elsewhere for others', () => {
  // Started beats the RPC reply: the pending dictation adopts the id.
  const started = applyEvent(pending, { type: 'DictationStarted', id: 'd1' })
  expect(started.phase).toBe('dictating')
  expect(started.active?.id).toBe('d1')

  const t0 = applyEvent(started, textEvent({ utterance: 0, text: 'hello', shaped: 'Hello' }))
  // Utterance 2 before 1: the gap is filled with an empty final utterance. `shaped` and `command` are stored.
  const t2 = applyEvent(t0, textEvent({ utterance: 2, text: 'world send it', shaped: 'world', final: true, command: { command: 'send', near_miss: false } }))
  expect(t2.active?.utts).toEqual([
    { text: 'hello', shaped: 'Hello', final: false },
    { text: '', shaped: '', final: true },
    { text: 'world send it', shaped: 'world', final: true, command: { command: 'send', near_miss: false } },
  ])
  // Someone else's text is ignored.
  expect(applyEvent(t2, textEvent({ id: 'other', text: 'x', shaped: 'x', final: true }))).toBe(t2)

  const lv = applyEvent(t2, { type: 'DictationLevel', id: 'd1', levels: Array.from({ length: 30 }, (_, i) => i) })
  expect(lv.active?.levels).toHaveLength(24)
  expect(lv.active?.levels[23]).toBe(29)

  // 153 §8.2: the mic re-armed / lost lands on the active dictation; another id's does not.
  const dev = applyEvent(lv, { type: 'DictationDevice', id: 'd1', device: 'Shure MV7', state: 'changed' })
  expect(dev.active?.device).toEqual({ state: 'changed', name: 'Shure MV7' })
  expect(applyEvent(dev, { type: 'DictationDevice', id: 'other', device: null, state: 'lost' })).toBe(dev)

  const done = applyEvent(dev, { type: 'DictationFinished', id: 'd1', reason: { reason: 'done' } })
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

test('"scratch that" (153 §7.4): an empty final for an EARLIER utterance empties it and the splice re-renders', () => {
  const started = applyEvent(pending, { type: 'DictationStarted', id: 'd1' })
  const foo = applyEvent(started, textEvent({ utterance: 0, text: 'foo', shaped: 'foo', final: true }))
  const scratch = applyEvent(foo, textEvent({ utterance: 1, text: 'scratch that', shaped: '', final: false, command: { command: 'scratch_that' } }))
  const undone = applyEvent(scratch, textEvent({ utterance: 0, text: '', shaped: '', final: true }))
  const bar = applyEvent(undone, textEvent({ utterance: 2, text: 'bar', shaped: 'bar', final: true }))
  expect(bar.active?.utts[0]).toEqual({ text: '', shaped: '', final: true })
  const a = bar.active as Active
  expect(splice(a.before, a.after, a.utts)).toEqual({ value: 'a bar b', caret: 5 })
})

test('finish: "send it" on a closed utterance sends; a partial that only armed it, or Esc, does not', () => {
  const base: Active = { id: 'd1', before: '', after: '', utts: [], levels: [], startedAt: 0, device: null }
  const sent: Active = { ...base, utts: [{ text: 'ship it send it', shaped: 'ship it', final: true, command: { command: 'send', near_miss: false } }] }
  expect(finish(sent, false)).toEqual({ value: 'ship it', caret: 7, id: 'd1', send: true })
  const armed: Active = { ...base, utts: [{ text: 'ship it send', shaped: 'ship it', final: false, command: { command: 'send', near_miss: false } }] }
  expect(finish(armed, false).send).toBe(false)
  // Esc: `before + after` back, no id (nothing to report), never a send.
  expect(finish(sent, true)).toEqual({ value: '', caret: 0, id: '', send: false })
  const scratched: Active = { ...base, utts: [{ text: 'foo', shaped: 'foo', final: true, command: { command: 'scratch_that' } }] }
  expect(finish(scratched, false).send).toBe(false)
})

test('startOpts names the surface, an agent prompt, the slash commands and the press age (153 §14.1)', () => {
  expect(startOpts(['/model', '/title'], 41.6)).toEqual({
    surface: 'composer',
    destination: { kind: 'agent_prompt' },
    app: null,
    commands: ['/model', '/title'],
    press_ago_ms: 42,
  })
  expect(startOpts([], -3).press_ago_ms).toBe(0)
})

test('start passes `opts` as the trailing argument of dictation_start and returns to idle on a refusal', async () => {
  const calls: unknown[][] = []
  const api = {
    dictation_start: async (...args: unknown[]) => {
      calls.push(args)
      throw new Error('already dictating')
    },
    dictation_status: async () => ({ model: { state: 'ready' }, active: null, input_device: null, last: null }),
  } as unknown as Api
  expect(await start(api, 'a', 'b', false, true, ['/model'])).toBe('already dictating')
  expect(calls).toHaveLength(1)
  expect(calls[0]?.[0]).toBeNull()
  expect(calls[0]?.[1]).toMatchObject({ surface: 'composer', destination: { kind: 'agent_prompt' }, commands: ['/model'] })
  expect(current().phase).toBe('idle')
  expect(current().active).toBeNull()
})
