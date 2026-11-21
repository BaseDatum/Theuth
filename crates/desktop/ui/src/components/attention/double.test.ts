// 126 §10: the inbox rows of `AttentionKind::Double` — undo during grace only, the screen.
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { DoubleUndo } from '@/components/chat/DoubleChip'
import { doubleIds } from './labels'
import type * as Rpc from '@/lib/rpc'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { doubleStore } from '@/store/double'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ double_take_over: async () => true }),
}))

const item = (over: Partial<AttentionItem> = {}): AttentionItem => ({
  id: 'double:appr-1',
  kind: 'double',
  title: 'double answered Allow for `git push`',
  detail: 'git push origin feat/126-w4',
  at_ms: Date.now(),
  task: null,
  agent: null,
  session: 's1',
  approval: null,
  decision: null,
  packet: null,
  ask: null,
  turn: null,
  ...over,
}) as AttentionItem

test('a double item is keyed by its question id (the `double:` prefix stripped), beside any approval / ask / packet ids', () => {
  expect(doubleIds(item())).toEqual([null, undefined, null, null, 'appr-1'])
  expect(doubleIds(item({ kind: 'approval', id: 'approval:x', approval: 'x' }))).toEqual(['x', undefined, null, null, null])
})

test('undo shows during the undo window only; a digest row (no DoubleAnswered) has no undo', () => {
  doubleStore.setState({ predicted: {}, answered: {}, takenOver: {} })
  const row = () => renderToStaticMarkup(createElement(DoubleUndo, { ids: doubleIds(item()) }))
  expect(row()).toBe('')
  doubleStore.getState().answer({ type: 'DoubleAnswered', question_id: 'appr-1', class: 'c', choice: 'Allow', version: 'v1', undo_until: new Date(Date.now() + 14_000).toISOString() })
  expect(row()).toMatch(/Undo · 1[34] s/)
  doubleStore.getState().answer({ type: 'DoubleAnswered', question_id: 'appr-1', class: 'c', choice: 'Allow', version: 'v1', undo_until: new Date(Date.now() - 1000).toISOString() })
  expect(row()).toBe('')
  doubleStore.getState().answer({ type: 'DoubleAnswered', question_id: 'appr-1', class: 'c', choice: 'Allow', version: 'v1' })
  expect(row()).toBe('')
  // Taken over (undone) once: the button is gone even inside the window.
  doubleStore.getState().answer({ type: 'DoubleAnswered', question_id: 'appr-1', class: 'c', choice: 'Allow', version: 'v1', undo_until: new Date(Date.now() + 14_000).toISOString() })
  doubleStore.getState().takeOver('appr-1')
  expect(row()).toBe('')
  expect(renderToStaticMarkup(createElement(DoubleUndo, { ids: doubleIds(item({ id: 'double:digest:2026-10-07' })) }))).toBe('')
})
