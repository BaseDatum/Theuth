import { expect, test } from 'vitest'
import type { AskRequested } from '@/rpc/bindings/AskRequested'
import { createAsks } from './approvals'
import { scope } from './engine-scope'
import { dispatch } from './sessions'

const ask = (id: string, session = 's1'): AskRequested => ({
  type: 'AskRequested',
  id,
  session,
  questions: [{ id: 'q', question: 'which?', options: [] }],
})

test('resolve drops the card by id and a late Requested for it is ignored', () => {
  const asks = createAsks()
  const st = asks.getState()
  st.add(ask('a'))
  st.add(ask('b'))
  st.add(ask('a')) // dedup
  expect(asks.getState().pending.map((p) => p.id)).toEqual(['a', 'b'])
  st.resolve('a')
  expect(asks.getState().pending.map((p) => p.id)).toEqual(['b'])
  st.add(ask('a')) // reordered across a reconnect: stays resolved
  expect(asks.getState().pending.map((p) => p.id)).toEqual(['b'])
  st.resolve('zzz') // unknown id: no-op
  expect(asks.getState().pending.map((p) => p.id)).toEqual(['b'])
})

test('an arrival time is stamped on add, dropped on resolve, kept only for ids a snapshot still reports (114)', () => {
  const asks = createAsks()
  const st = asks.getState()
  st.add(ask('a'))
  st.add(ask('b'))
  const a = asks.getState().at.get('a')
  expect(a).toBeTypeOf('number')
  expect(asks.getState().at.get('b')).toBeGreaterThanOrEqual(a!)
  st.resolve('b')
  expect(asks.getState().at.has('b')).toBe(false)
  st.replace([ask('a'), ask('fresh')])
  expect(asks.getState().at.get('a')).toBe(a)
  expect(asks.getState().at.has('fresh')).toBe(false) // no timestamp in the bindings: the transcript puts it last
})

test('PendingPrompts replaces the pending set with what the engine reports', () => {
  const st = scope('test-asksync')
  dispatch([ask('stale'), { type: 'ApprovalRequested', id: 'ap', session: 's1', tool: 'shell', args: {}, risk: 'mutating' }], st)
  expect(st.asks.getState().pending.map((p) => p.id)).toEqual(['stale'])
  expect(st.approvals.getState().pending.map((p) => p.id)).toEqual(['ap'])
  // Resubscribe: the engine has one other ask and no approvals; a secrets `Changed` is not a prompt.
  dispatch(
    [
      {
        type: 'PendingPrompts',
        approvals: [],
        asks: [ask('live')],
        secrets: [{ type: 'SecretsChanged' }, { type: 'SecretRequested', id: 'sr', session: 's1', name: 'TOKEN', note: '', current: null, title: null, fields: [] }],
      },
    ],
    st,
  )
  expect(st.asks.getState().pending.map((p) => p.id)).toEqual(['live'])
  expect(st.approvals.getState().pending).toEqual([])
  expect(st.secretRequests.getState().pending.map((p) => p.id)).toEqual(['sr'])
  // The snapshot is authoritative: a previously resolved id may be pending again after it.
  dispatch([{ type: 'AskResolved', id: 'live', dismissed: false, timed_out: false }], st)
  expect(st.asks.getState().pending).toEqual([])
  dispatch([{ type: 'PendingPrompts', approvals: [], asks: [ask('live')], secrets: [] }], st)
  expect(st.asks.getState().pending.map((p) => p.id)).toEqual(['live'])
})
