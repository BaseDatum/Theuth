import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { DoublePredicted } from '@/rpc/bindings/DoublePredicted'
import { doubleStore } from '@/store/double'
import { recommendedLabel, withDoublePick } from './AskCard'
import { DoubleChip, chipText, countdownText, disagrees, preselect, rungGlyph, secondsLeft, takeOver } from './DoubleChip'

const calls = vi.hoisted(() => ({ takeOver: [] as string[], takeOverReply: true }))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    double_take_over: async (id: string) => {
      calls.takeOver.push(id)
      return calls.takeOverReply
    },
    double_forget: async () => 1,
  }),
}))

const predicted = (over: Partial<DoublePredicted> = {}): DoublePredicted => ({
  type: 'DoublePredicted',
  question_id: 'q1',
  kind: 'approval',
  class: 'approval:shell:mutating:git push:root',
  choice: 'Allow',
  p: 0.91,
  probs: [
    ['Allow', 0.91],
    ['Deny', 0.07],
    ['none', 0.02],
  ],
  rung: 0,
  version: 'baseline',
  explain: { neighbours: [], parts: { oracle: 0.5, memory: 0.4, prior: 0.1 }, promote_hint: '12 more correct in a row' },
  ...over,
})

test('rung glyphs and chip text (§10): ○ L0 · ◐ L1 · ● L2 · ●● L3; "double · Allow · 91 %"', () => {
  expect([0, 1, 2, 3].map(rungGlyph)).toEqual(['○', '◐', '●', '●●'])
  expect(chipText(predicted())).toBe('double · Allow · 91 %')
})

test('countdown text at 30 / 10 / 0 s from `will_act_at` (Revision 1: 30 s grace)', () => {
  const now = Date.UTC(2026, 9, 7, 12, 0, 0)
  const at = (secs: number) => new Date(now + secs * 1000).toISOString()
  expect(secondsLeft(at(30), now)).toBe(30)
  expect(countdownText('Allow', 30)).toBe('double answers Allow in 30 s — tap to take over')
  expect(countdownText('Allow', secondsLeft(at(10), now) as number)).toBe('double answers Allow in 10 s — tap to take over')
  expect(secondsLeft(at(-3), now)).toBe(0)
  expect(countdownText('Allow', 0)).toBe('double answers Allow now…')
  expect(secondsLeft(undefined, now)).toBeNull()
  expect(secondsLeft('not a date', now)).toBeNull()
})

test('L1 pre-selection comes from DoublePredicted, never at L0, never for `none`; labels match case-insensitively', () => {
  expect(preselect(predicted({ rung: 0 }), ['Allow', 'Deny'])).toBeNull()
  expect(preselect(predicted({ rung: 1 }), ['Allow', 'Deny'])).toBe(0)
  expect(preselect(predicted({ rung: 1, choice: 'deny' }), ['Allow', 'Deny'])).toBe(1)
  expect(preselect(predicted({ rung: 1, index: 1 }), ['Start now', 'Discard'])).toBe(1)
  expect(preselect(predicted({ rung: 2, choice: 'none' }), ['Allow', 'Deny'])).toBeNull()
  expect(preselect(predicted({ rung: 1, choice: 'Later' }), ['Allow', 'Deny'])).toBeNull()
  expect(preselect(undefined, ['Allow'])).toBeNull()
})

test('disagreement: the model recommends one option, the double (L1+) another → both show', () => {
  expect(disagrees(predicted({ rung: 1 }), 'Deny')).toBe(true)
  expect(disagrees(predicted({ rung: 1 }), 'Allow')).toBe(false)
  expect(disagrees(predicted({ rung: 0 }), 'Deny')).toBe(false)
  expect(disagrees(predicted({ rung: 1 }), undefined)).toBe(false)
  const q = { id: 'q', question: '?', options: [{ label: 'A' }, { label: 'B' }], recommended: 1 }
  expect(recommendedLabel([q])).toBe('B')
  // The double pre-selects A while the model's recommended (B) stays as rendered.
  expect(withDoublePick([{ kind: 'choice', index: 1 }], [q], predicted({ rung: 1, choice: 'A' }))).toEqual([{ kind: 'choice', index: 0 }])
  // The owner already changed the draft: left alone.
  expect(withDoublePick([{ kind: 'text', text: 'own' }], [q], predicted({ rung: 1, choice: 'A' }))).toEqual([{ kind: 'text', text: 'own' }])
  // Two questions: the ask hash keys a set, no per-question index — nothing pre-selected.
  expect(withDoublePick([null, null], [q, { ...q, id: 'r' }], predicted({ rung: 1, choice: 'A' }))).toEqual([null, null])
})

test('the chip renders the rung glyph, the text, the disagreement badge; the L2 countdown replaces the chip text', () => {
  doubleStore.setState({ predicted: {}, answered: {}, takenOver: {} })
  expect(renderToStaticMarkup(createElement(DoubleChip, { ids: ['q1'] }))).toBe('')
  doubleStore.getState().predict(predicted({ rung: 1 }))
  const l1 = renderToStaticMarkup(createElement(DoubleChip, { ids: ['nope', 'q1'], recommended: 'Deny' }))
  expect(l1).toContain('◐')
  expect(l1).toContain('double · Allow · 91 %')
  expect(l1).toContain('model recommends Deny')
  doubleStore.getState().predict(predicted({ rung: 2, will_act_at: new Date(Date.now() + 28_000).toISOString() }))
  const l2 = renderToStaticMarkup(createElement(DoubleChip, { ids: ['q1'] }))
  expect(l2).toContain('●')
  expect(l2).toMatch(/double answers Allow in 2[78] s — tap to take over/)
  expect(l2).not.toContain('double · Allow · 91 %')
  doubleStore.getState().answer({ type: 'DoubleAnswered', question_id: 'q1', class: 'c', choice: 'Allow', version: 'v1', undo_until: new Date(Date.now() + 20_000).toISOString() })
  const done = renderToStaticMarkup(createElement(DoubleChip, { ids: ['q1'] }))
  expect(done).toContain('answered by double · Allow')
  expect(done).toMatch(/undo · (19|20) s/)
})

test('take over calls double_take_over(question_id); true marks the card taken, false only toasts', async () => {
  doubleStore.setState({ predicted: {}, answered: {}, takenOver: {} })
  calls.takeOver.length = 0
  calls.takeOverReply = true
  const api = { double_take_over: async (id: string) => (calls.takeOver.push(id), calls.takeOverReply) }
  expect(await takeOver(api, 'q1')).toBe(true)
  expect(calls.takeOver).toEqual(['q1'])
  expect(doubleStore.getState().takenOver.q1).toBe(true)
  calls.takeOverReply = false
  expect(await takeOver(api, 'q2')).toBe(false)
  expect(doubleStore.getState().takenOver.q2).toBeUndefined()
})
