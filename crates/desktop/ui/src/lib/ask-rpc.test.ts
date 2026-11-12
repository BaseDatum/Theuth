import { expect, test, vi } from 'vitest'
import type { AskEvent } from "@/rpc/bindings/event.agent_2enotebook_2dask.output"
import { EMPTY_ASK, applyAskEvent, askEventOf, hitHref } from './ask-rpc'

vi.mock('@/store/sessions', () => ({ on: () => () => {} }))

test('askEventOf takes the NotebookAsk frame of its session only (contract §2 wire shape)', () => {
  const plan: AskEvent = { type: 'plan', branches: [{ n: 1, notebooks: ['a'], query: 'q', instructions: 'i' }] }
  expect(askEventOf({ type: 'NotebookAsk', session: 's1', event: plan }, 's1')).toEqual(plan)
  expect(askEventOf({ type: 'NotebookAsk', session: 's2', event: plan }, 's1')).toBeNull()
  expect(askEventOf({ type: 'Changed', source: 'x', status: 'completed' }, 's1')).toBeNull()
  expect(askEventOf({ type: 'NotebookAsk', session: 's1', event: 'plan' }, 's1')).toBeNull()
  expect(askEventOf(null, 's1')).toBeNull()
})

test('applyAskEvent folds plan → branch hits → partials (replacing) → answer → done', () => {
  let s = EMPTY_ASK
  s = applyAskEvent(s, { type: 'plan', branches: [{ n: 1, notebooks: ['a'], query: 'q1', instructions: '' }, { n: 2, notebooks: ['b'], query: 'q2', instructions: '' }] })
  s = applyAskEvent(s, { type: 'branch', n: 2, hits: [{ id: 'd1', block: 'b1', title: 'T', score: 0.5 }] })
  s = applyAskEvent(s, { type: 'partial', n: 2, answer: 'so far' })
  s = applyAskEvent(s, { type: 'partial', n: 2, answer: 'so far, and more' })
  expect(s.plan?.map((b) => b.n)).toEqual([1, 2])
  expect(s.hits[2]?.[0]?.id).toBe('d1')
  expect(s.hits[1]).toBeUndefined()
  expect(s.partial[2]).toBe('so far, and more')
  expect(s.answer).toBeNull()
  s = applyAskEvent(s, { type: 'answer', markdown: '**done** [1](theuth://sources/d1#b1)' })
  s = applyAskEvent(s, { type: 'done', ok: false, error: 'branch 1 failed twice' })
  expect(s.answer).toContain('theuth://sources/d1#b1')
  expect(s.done).toEqual({ ok: false, error: 'branch 1 failed twice' })
})

test('hitHref is the §6.1 chip grammar; no block → the document', () => {
  expect(hitHref({ id: 'd1', block: 'b1', title: '', score: 0 }, 'sources')).toBe('theuth://sources/d1#b1')
  expect(hitHref({ id: 'n1', block: null, title: '', score: 0 }, 'notes')).toBe('theuth://notes/n1')
})
