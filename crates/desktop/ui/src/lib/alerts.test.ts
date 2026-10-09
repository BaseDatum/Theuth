// 155 §7.6 / §11: one notification per `alert.id`, none for the session in view, the event path for items the
// engine does not decide.
import { afterEach, expect, test, vi } from 'vitest'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import { AlertNotifier } from './alerts'

afterEach(() => vi.useRealTimers())

const item = (id: string, over: Partial<AttentionItem> = {}): AttentionItem =>
  ({ id, kind: 'ask', title: id, detail: null, at_ms: 1, task: null, agent: null, session: 's1', approval: null, decision: null, packet: null, ask: null, turn: null, priority: null, rank: 5, alert: null, ...over }) as AttentionItem
const alert = (id: string, why = 'why') => ({ id, class: 'needs_you' as const, why, at_ms: 1 })

test('one notification per alert.id across refetches; a new episode (new id) notifies again; the body is `why`', () => {
  const notify = vi.fn()
  const n = new AlertNotifier(notify, () => false)
  n.list([item('ask:a', { title: 'omp asks: which?', alert: alert('ask:a@1', 'a session asked you a question') }), item('show:c')])
  n.list([item('ask:a', { alert: alert('ask:a@1') })])
  expect(notify.mock.calls).toEqual([['omp asks: which?', 'a session asked you a question']])
  n.list([item('ask:a', { alert: alert('ask:a@9') })])
  expect(notify).toHaveBeenCalledTimes(2)
})

test('none for the session in view: the alert is marked seen and not posted; a sessionless item always posts', () => {
  const notify = vi.fn()
  const n = new AlertNotifier(notify, (s) => s === 's1')
  n.list([item('ask:a', { alert: alert('ask:a@1') }), item('hire:amy', { session: null, alert: alert('hire:amy@1') })])
  expect(notify.mock.calls).toEqual([['hire:amy', 'why']])
  n.list([item('ask:a', { alert: alert('ask:a@1') })])
  expect(notify).toHaveBeenCalledTimes(1)
})

test('event path: posts after the grace when the list never carries the item with an `alert` key (old engine); dropped when it does', () => {
  vi.useFakeTimers()
  const notify = vi.fn()
  const n = new AlertNotifier(notify, () => false, 1_000)
  n.legacy('approval:1', 'Approval: shell', 'rm x')
  n.legacy('finished:s1:', 'Turn finished', 'title')
  // An old engine's list: no `alert` key on the approval; no finished item at all.
  const old = item('approval:1', { kind: 'approval' })
  delete (old as Partial<AttentionItem>).alert
  n.list([old])
  vi.advanceTimersByTime(999)
  expect(notify).not.toHaveBeenCalled()
  vi.advanceTimersByTime(1)
  expect(notify.mock.calls).toEqual([
    ['Approval: shell', 'rm x'],
    ['Turn finished', 'title'],
  ])

  // A new engine: the item shows up with `alert: null` (focused, or inside the delay) → the engine decides, no legacy post.
  n.legacy('approval:2', 'Approval: shell', 'rm y')
  n.list([item('approval:2', { kind: 'approval', alert: null })])
  vi.advanceTimersByTime(5_000)
  expect(notify).toHaveBeenCalledTimes(2)

  // `dispose` drops what is still held.
  n.legacy('approval:3', 'Approval: shell', 'rm z')
  n.dispose()
  vi.advanceTimersByTime(5_000)
  expect(notify).toHaveBeenCalledTimes(2)
})
