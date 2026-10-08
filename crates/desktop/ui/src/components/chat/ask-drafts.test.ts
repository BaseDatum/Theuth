// 138 ASK-OWN: a typed answer survives changing the pick and is sent only while it is the pick.
import { beforeEach, expect, test } from 'vitest'
import type { DoublePredicted } from '@/rpc/bindings/DoublePredicted'
import type { Question } from '@/rpc/bindings/Question'
import { type AskDraft, allAnswered, askDrafts, draftsFor, ownPick, picked, seedDraft, toAnswer, typed, withDoublePick } from './ask-drafts'

const q1: Question = { id: 'q1', question: 'Which?', options: [{ label: 'A' }, { label: 'B' }], recommended: 1 }
const q2: Question = { id: 'q2', question: 'Free?', options: [] }
const qs = [q1, q2]

/** Focus on the box / its ○: the pick after `ownPick`, through `picked` when it changed. */
const focused = (d: AskDraft, i: number): AskDraft => {
  const p = ownPick(d.picks[i] ?? null, d.own[i] ?? '')
  return p === (d.picks[i] ?? null) ? d : picked(d, i, p)
}

const predicted = (over: Partial<DoublePredicted> = {}): DoublePredicted => ({
  type: 'DoublePredicted',
  question_id: 'ask-1',
  kind: 'ask',
  class: 'ask',
  choice: 'A',
  p: 0.9,
  probs: [],
  rung: 1,
  version: 'baseline',
  explain: { neighbours: [], parts: { oracle: 0.5, memory: 0.4, prior: 0.1 }, promote_hint: '' },
  ...over,
})

beforeEach(() => askDrafts.setState({ asks: {} }))

test('seed: the recommended option pre-picked, every box empty', () => {
  expect(seedDraft(qs)).toEqual({ picks: [{ kind: 'choice', index: 1 }, null], own: ['', ''] })
  expect(seedDraft(qs).picks).toEqual(draftsFor(qs))
})

test('type → pick an option: the words stay in the box, the option is the pick', () => {
  const d = picked(typed(seedDraft(qs), 0, 'my own'), 0, { kind: 'choice', index: 0 })
  expect(d.own[0]).toBe('my own')
  expect(d.picks[0]).toEqual({ kind: 'choice', index: 0 })
})

test('focusing the box with kept words makes them the pick again; a blank box changes nothing', () => {
  const kept = picked(typed(seedDraft(qs), 0, 'my own'), 0, { kind: 'choice', index: 0 })
  expect(focused(kept, 0)).toEqual({ picks: [{ kind: 'text', text: 'my own' }, null], own: ['my own', ''] })
  const blank = seedDraft(qs)
  expect(focused(blank, 0)).toBe(blank)
  const spaces = typed(seedDraft(qs), 0, '   ')
  expect(ownPick({ kind: 'choice', index: 0 }, '   ')).toEqual({ kind: 'choice', index: 0 })
  expect(focused(picked(spaces, 0, { kind: 'skipped' }), 0).picks[0]).toEqual({ kind: 'skipped' })
  // Already the pick: the same object back.
  const t = typed(seedDraft(qs), 0, 'x')
  expect(focused(t, 0)).toBe(t)
})

test('Skip and un-skip keep the words; typing after a choice selects the text', () => {
  const t = typed(seedDraft(qs), 0, 'words')
  const s = picked(t, 0, { kind: 'skipped' })
  expect(s).toEqual({ picks: [{ kind: 'skipped' }, null], own: ['words', ''] })
  const u = picked(s, 0, null)
  expect(u).toEqual({ picks: [null, null], own: ['words', ''] })
  const c = picked(u, 0, { kind: 'choice', index: 1 })
  expect(typed(c, 0, 'words!').picks[0]).toEqual({ kind: 'text', text: 'words!' })
})

test('emptying the box keeps a blank text pick, so the ask is not answered', () => {
  const d = typed(typed(seedDraft([q2]), 0, 'abc'), 0, '')
  expect(d).toEqual({ picks: [{ kind: 'text', text: '' }], own: [''] })
  expect(allAnswered(d.picks)).toBe(false)
  expect(allAnswered(typed(seedDraft([q2]), 0, 'abc').picks)).toBe(true)
})

test('two questions are independent', () => {
  const d = picked(typed(typed(seedDraft(qs), 0, 'one'), 1, 'two'), 0, { kind: 'choice', index: 0 })
  expect(d).toEqual({ picks: [{ kind: 'choice', index: 0 }, { kind: 'text', text: 'two' }], own: ['one', 'two'] })
})

test('Send maps the pick only: an option with words kept → the option; text → the trimmed words', () => {
  const d = picked(typed(typed(seedDraft(qs), 0, 'kept words'), 1, '  free  '), 0, { kind: 'choice', index: 0 })
  expect(d.picks.map(toAnswer)).toEqual([{ kind: 'choice', index: 0 }, { kind: 'text', text: 'free' }])
  expect(picked(d, 1, { kind: 'skipped' }).picks.map(toAnswer)).toEqual([{ kind: 'choice', index: 0 }, { kind: 'skipped' }])
  expect(toAnswer({ kind: 'text', text: '  ' })).toEqual({ kind: 'skipped' })
  expect(toAnswer(null)).toEqual({ kind: 'skipped' })
})

test('store: seeded from the questions on first write, keyed per ask, one entry for both surfaces, dropped on resolve', () => {
  const s = askDrafts.getState()
  s.update('ask-1', qs, (d) => typed(d, 0, 'hello')) // the chat card
  s.update('ask-2', [q2], (d) => d) // an unchanged update writes nothing
  expect(Object.keys(askDrafts.getState().asks)).toEqual(['ask-1'])
  expect(askDrafts.getState().asks['ask-1']).toEqual({ picks: [{ kind: 'text', text: 'hello' }, null], own: ['hello', ''] })
  s.update('ask-1', qs, (d) => picked(d, 0, { kind: 'choice', index: 0 })) // the Attention screen, same entry
  expect(askDrafts.getState().asks['ask-1']).toEqual({ picks: [{ kind: 'choice', index: 0 }, null], own: ['hello', ''] })
  s.update('ask-2', [q2], (d) => typed(d, 0, 'other'))
  expect(askDrafts.getState().asks['ask-2']).toEqual({ picks: [{ kind: 'text', text: 'other' }], own: ['other'] })
  const before = askDrafts.getState()
  s.drop('ask-1')
  expect(Object.keys(askDrafts.getState().asks)).toEqual(['ask-2'])
  s.drop('ask-1') // already gone: no state change
  expect(askDrafts.getState()).toBe(askDrafts.getState())
  expect(askDrafts.getState()).not.toBe(before)
})

test("the double's pre-pick through the store: an untouched pick moves, a touched one stays, the words are untouched", () => {
  const prepick = (id: string, p: DoublePredicted | undefined) =>
    askDrafts.getState().update(id, [q1], (d) => {
      const picks = withDoublePick(d.picks, [q1], p)
      return picks === d.picks ? d : { ...d, picks }
    })
  prepick('ask-1', undefined) // nothing to apply: no entry written
  expect(askDrafts.getState().asks['ask-1']).toBeUndefined()
  prepick('ask-1', predicted({ choice: 'A' })) // the model recommended B; the owner has not touched it
  expect(askDrafts.getState().asks['ask-1']).toEqual({ picks: [{ kind: 'choice', index: 0 }], own: [''] })
  askDrafts.getState().update('ask-2', [q1], (d) => picked(typed(d, 0, 'own'), 0, { kind: 'choice', index: 1 }))
  const touched = askDrafts.getState().asks['ask-2']
  prepick('ask-2', predicted({ choice: 'A' }))
  // The owner typed and went back to the recommended option: still "untouched" by the pick rule, so the double moves it, the words stay.
  expect(askDrafts.getState().asks['ask-2']).toEqual({ picks: [{ kind: 'choice', index: 0 }], own: ['own'] })
  askDrafts.getState().update('ask-3', [q1], (d) => typed(d, 0, 'own'))
  const text = askDrafts.getState().asks['ask-3']
  prepick('ask-3', predicted({ choice: 'A' }))
  expect(askDrafts.getState().asks['ask-3']).toBe(text)
  expect(touched).not.toBe(askDrafts.getState().asks['ask-2'])
})
