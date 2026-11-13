import { receiveComponent } from '@/rpc/bindings/receive'
import { afterEach, expect, test, vi } from 'vitest'
import type { Loaded as LoadedPage } from "@/rpc/bindings/method.agent.output"
import { newDraft } from '@/lib/panes'
import { useSessions } from '@/store/sessions'
import { FocusTracker, Throttle, deriveFocus, typeCounts } from './event-focus'

afterEach(() => vi.useRealTimers())

test('deriveFocus: chat or talk route + pane leaves + popout + desk, drafts out, sorted and unique', () => {
  expect(deriveFocus({ pathname: '/c/b', leaves: ['c', newDraft(), 'a', 'b'] })).toEqual(['a', 'b', 'c'])
  expect(deriveFocus({ pathname: '/talk/t%20x', leaves: [] })).toEqual(['t x'])
  expect(deriveFocus({ pathname: '/c/a/history', leaves: [] })).toEqual([])
  expect(deriveFocus({ pathname: '/', leaves: [], popout: 'p', desk: 'd' })).toEqual(['d', 'p'])
  expect(deriveFocus({ pathname: '/island', leaves: [], popout: null, desk: null })).toEqual([])
})

test('FocusTracker: unchanged sets are not re-sent, a reset (reconnect) re-sends, leavers are named', () => {
  const t = new FocusTracker()
  expect(t.update(['a', 'b'])).toEqual({ send: true, left: [] })
  expect(t.update(['a', 'b'])).toEqual({ send: false, left: [] })
  expect(t.update(['b', 'c'])).toEqual({ send: true, left: ['a'] })
  t.reset()
  expect(t.update(['b', 'c'])).toEqual({ send: true, left: [] })
  // `[]` is a real set (no deltas at all), sent like any other.
  expect(t.update([])).toEqual({ send: true, left: ['b', 'c'] })
})

test('Throttle: a trailing resync coalesces a burst into one run per window; a non-trailing toast drops the rest', () => {
  vi.useFakeTimers()
  const resync = vi.fn()
  const r = new Throttle(resync, 5_000, true)
  r.call()
  r.call()
  r.call()
  expect(resync).toHaveBeenCalledTimes(1)
  vi.advanceTimersByTime(4_999)
  expect(resync).toHaveBeenCalledTimes(1)
  vi.advanceTimersByTime(1)
  expect(resync).toHaveBeenCalledTimes(2)
  // Quiet window: nothing trails.
  vi.advanceTimersByTime(10_000)
  expect(resync).toHaveBeenCalledTimes(2)
  r.call()
  expect(resync).toHaveBeenCalledTimes(3)
  r.call()
  r.cancel()
  vi.advanceTimersByTime(10_000)
  expect(resync).toHaveBeenCalledTimes(3)

  const toast = vi.fn()
  const t = new Throttle(toast, 60_000, false)
  t.call()
  t.call()
  vi.advanceTimersByTime(59_999)
  t.call()
  expect(toast).toHaveBeenCalledTimes(1)
  vi.advanceTimersByTime(1)
  t.call()
  expect(toast).toHaveBeenCalledTimes(2)
})

const page: LoadedPage = receiveComponent("method.agent.output.Loaded", {
  running: false,
  model: 'm',
  effort: null,
  queued: [],
  undone: 0,
  agent: null,
  name: null,
  parent: null,
  skills: [],
  harness_commands: [],
  context_revision: '1',
  context_editable: true,
  context_reason: null,
  context_estimate: null,
  settled: true,
  todos: null,
  communication: null,
  project: null,
  retention: 'durable',
  history: [{ context: null, peer: null, role: 'user', text: 'hi', turn: 1 }],
  first_seq: 1,
  more: false,
})

test('markStale: Lagged / a session leaving focus keep `loaded` and the transcript, set both stale flags, bump the generation', () => {
  const st = useSessions.getState()
  st.loadHistory('focus-a', page, 'replace')
  st.loadHistory('focus-b', page, 'replace')
  st.ensure('focus-never')
  const before = useSessions.getState().sessions['focus-a']!
  st.markStale('focus-a')
  let a = useSessions.getState().sessions['focus-a']!
  expect(a).toMatchObject({ loaded: true, contextStale: true, todosStale: true, historyGeneration: before.historyGeneration + 1 })
  expect(a.entries).toBe(before.entries)
  expect(useSessions.getState().sessions['focus-b']).toMatchObject({ loaded: true, contextStale: false })
  // Every slice (the `Lagged` resync); one never loaded has nothing to go stale.
  st.applyEvents([{ type: 'Lagged', count: 7 }])
  expect(useSessions.getState().lagged).toBe(7)
  st.resetLagged()
  st.markStale()
  a = useSessions.getState().sessions['focus-a']!
  expect(useSessions.getState().lagged).toBe(0)
  expect(a.historyGeneration).toBe(before.historyGeneration + 2)
  expect(useSessions.getState().sessions['focus-b']).toMatchObject({ loaded: true, contextStale: true, todosStale: true })
  expect(useSessions.getState().sessions['focus-never']).toMatchObject({ loaded: false, contextStale: false })
})

test('settled page after a gap: a `running` left true by a lost TurnFinished is corrected, not refused', () => {
  const st = useSessions.getState()
  const id = 'focus-gap'
  st.loadHistory(id, page, 'replace')
  st.applyEvents([
    { type: 'TurnStarted', session: id, turn: 't', text: 'go', model: 'm', turn_seq: 2 },
    { type: 'TextDelta', session: id, turn: 't', text: 'part' },
  ])
  st.markStale(id)
  const s = useSessions.getState().sessions[id]!
  expect(s).toMatchObject({ running: true, contextStale: true })
  const after = receiveComponent('method.agent.output.Loaded', { ...page, context_revision: '2', history: [...page.history, { role: 'user', text: 'go', turn: 2 }, { role: 'assistant', text: 'partial done', turn: 2 }] })
  expect(st.loadHistory(id, after, 'settled', { gen: s.historyGeneration, live: s.live })).toBe(true)
  expect(useSessions.getState().sessions[id]).toMatchObject({ running: false, contextStale: false, loaded: true })
  expect(useSessions.getState().sessions[id]!.entries.map((e) => ('text' in e ? e.text : e.kind))).toEqual(['hi', 'go', 'partial done'])
  // The engine still busy: refused, the stream's text stays.
  st.applyEvents([{ type: 'TurnStarted', session: id, turn: 'u', text: 'more', model: 'm', turn_seq: 3 }])
  st.markStale(id)
  const r = useSessions.getState().sessions[id]!
  expect(st.loadHistory(id, { ...after, settled: false, context_editable: false }, 'settled', { gen: r.historyGeneration, live: r.live })).toBe(false)
  expect(useSessions.getState().sessions[id]).toMatchObject({ running: true, contextStale: true })
})

test('typeCounts: the per-drain histogram', () => {
  expect(typeCounts([{ type: 'TextDelta' }, { type: 'TextDelta' }, { type: 'ToolCallFinished' }])).toEqual({ TextDelta: 2, ToolCallFinished: 1 })
})
