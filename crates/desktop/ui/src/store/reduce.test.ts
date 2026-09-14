import { expect, test } from 'vitest'
import type { Usage } from '@/rpc/bindings/Usage'
import type { WireEvent } from '@/rpc/events'
import { apply, empty, fromHistory, route, type Ctx } from './reduce'
import { type SessionSlice, useSessions } from './sessions'

const S = 'sess'
const T = 'turn'
const usage: Usage = { prompt: 10, completion: 5, cached: 0 }
const ctx = (): Ctx => ({ sent: new Set() })

function run(events: WireEvent[], c = ctx()) {
  return events.reduce((s, e) => apply(s, e, c), empty(S))
}

test('deltas append to one open assistant entry; a tool event closes it', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm' },
    { type: 'TextDelta', session: S, turn: T, text: 'Hel' },
    { type: 'TextDelta', session: S, turn: T, text: 'lo' },
    { type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'shell', args: { command: 'ls' } },
    { type: 'TextDelta', session: S, turn: T, text: 'after' },
  ])
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'assistant', 'tool', 'assistant'])
  expect(s.entries[1]).toMatchObject({ kind: 'assistant', text: 'Hello', open: false })
  expect(s.entries[3]).toMatchObject({ kind: 'assistant', text: 'after', open: true })
  expect(s.activity).toBe('streaming')
  expect(s.waiting).toBe(false)
})

test('pending tool is replaced by its start (same id), finished by call id, abandoned ones pruned', () => {
  let s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm' },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'c1', name: 'read' },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'c2.1', name: 'shell' },
  ])
  const pendingId = s.entries[1]?.id
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'read', args: { path: 'a' } }, ctx())
  expect(s.entries[1]).toMatchObject({ id: pendingId, kind: 'tool', pending: false, args: { path: 'a' }, child: false })
  s = apply(s, { type: 'ToolCallFinished', session: S, turn: T, call_id: 'c1', output: 'ok', is_error: false, files: ['/p.png'] }, ctx())
  expect(s.entries[1]).toMatchObject({ result: { output: 'ok', isError: false, files: ['/p.png'] } })
  expect(s.entries[2]).toMatchObject({ pending: true, child: true })
  s = apply(s, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 15 }, ctx())
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'tool'])
  expect(s).toMatchObject({ running: false, activity: 'idle', usage, context: 15 })
})

test('InputQueued dedupes by id and never touches the streaming tail', () => {
  const base = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const q = apply(base, { type: 'InputQueued', session: S, id: 'i1', text: 'steer' }, ctx())
  expect(q.queued).toEqual([{ id: 'i1', text: 'steer' }])
  expect(q.entries[0]).toMatchObject({ open: true })
  expect(apply(q, { type: 'InputQueued', session: S, id: 'i1', text: 'steer' }, ctx())).toBe(q)
  expect(apply(q, { type: 'InputWithdrawn', session: S, id: 'nope' }, ctx())).toBe(q)
  expect(apply(q, { type: 'InputWithdrawn', session: S, id: 'i1' }, ctx()).queued).toEqual([])
})

test('InputInjected moves the steer into the transcript and suppresses its echo', () => {
  const s = run([
    { type: 'InputQueued', session: S, id: 'i1', text: 'steer' },
    { type: 'InputInjected', session: S, id: 'i1', turn: 'T2' },
    { type: 'TurnStarted', session: S, turn: 'T2', text: 'steer', model: 'm' },
  ])
  expect(s.queued).toEqual([])
  expect(s.entries).toHaveLength(1)
  expect(s.entries[0]).toMatchObject({ kind: 'user', text: 'steer', injected: true })
  expect(s.lastInjected).toBeUndefined()
})

test('TurnStarted echo: skipped for our own send (once) and harness wrappers, skill shows a chip', () => {
  const c = ctx()
  c.sent.add('mine')
  let s = apply(empty(S), { type: 'TurnStarted', session: S, turn: T, text: 'mine', model: 'm' }, c)
  expect(s.entries).toEqual([])
  expect(c.sent.size).toBe(0)
  s = apply(s, { type: 'TurnStarted', session: S, turn: T, text: '<message from="p">\nhi\n</message>', model: 'm' }, c)
  expect(s.entries).toEqual([])
  s = apply(s, { type: 'TurnStarted', session: S, turn: T, text: '<skill name="k">\nb\n</skill>\n\nwords', model: 'm' }, c)
  expect(s.entries[0]).toMatchObject({ kind: 'user', skill: 'k', text: 'words' })
})

test('Compacted: clear wipes, other styles add a divider after the closed tail', () => {
  const streaming = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const div = apply(streaming, { type: 'Compacted', session: S, turn: T, epoch: 2, before: 1, folded: 3, style: 'summary' }, ctx())
  expect(div.entries.map((e) => e.kind)).toEqual(['assistant', 'compacted'])
  expect(div.entries[0]).toMatchObject({ open: false })
  expect(div.entries[1]).toMatchObject({ epoch: 2, style: 'summary' })
  const cleared = apply(streaming, { type: 'Compacted', session: S, turn: null, epoch: 3, before: 1, folded: 3, style: 'clear' }, ctx())
  expect(cleared.entries).toHaveLength(1)
  expect(cleared.entries[0]).toMatchObject({ kind: 'info', text: 'context cleared (epoch 3)' })
})

test('MessageDelivered interjects above a streaming tail without closing it', () => {
  const streaming = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const s = apply(streaming, { type: 'MessageDelivered', session: S, from: 'Scout', text: 'done' }, ctx())
  expect(s.entries.map((e) => e.kind)).toEqual(['peer', 'assistant'])
  expect(s.entries[1]).toMatchObject({ open: true })
})

test('SubagentSpawned creates the child, adds a row to the parent and its ancestors, notes the spawn', () => {
  const draft: Record<string, SessionSlice> = { [S]: empty(S) }
  expect(route(draft, { type: 'SubagentSpawned', parent: S, child: 'kid', name: 'Scout' }, ctx())).toBe(true)
  expect(draft.kid).toMatchObject({ name: 'Scout', parent: S })
  expect(draft[S]?.children.kid).toMatchObject({ name: 'Scout', depth: 1, state: { state: 'running' } })
  expect(draft[S]?.entries[0]).toMatchObject({ kind: 'info', text: '↳ spawned Scout' })
  route(draft, { type: 'SubagentSpawned', parent: 'kid', child: 'grandkid', name: 'Deep' }, ctx())
  expect(draft[S]?.children.grandkid?.depth).toBe(2)
  expect(draft.kid?.children.grandkid?.depth).toBe(1)
  route(draft, { type: 'TurnFinished', session: 'kid', turn: T, stop_reason: 'cancelled', usage, context: 1 }, ctx())
  expect(draft[S]?.children.kid?.state).toEqual({ state: 'cancelled' })
  expect(draft[S]?.entries.at(-1)).toMatchObject({ kind: 'info', text: '↳ Scout cancelled' })
})

test('route leaves untouched slices alone and ignores foreign events', () => {
  const other = empty('other')
  const draft = { [S]: empty(S), other }
  expect(route(draft, { type: 'Connected', connections: 1 }, ctx())).toBe(false)
  expect(route(draft, { type: 'TextDelta', session: S, turn: T, text: 'a' }, ctx())).toBe(true)
  expect(draft.other).toBe(other)
  expect(Object.keys(draft)).toEqual([S, 'other'])
})

test('job tail attaches to the tool card by call id, follows output, freezes or drops on finish', () => {
  let s = run([{ type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'shell', args: { command: 'make' } }])
  s = apply(s, { type: 'JobStarted', session: S, id: 'j1', command: 'make', call_id: 'c1', log: '/l' }, ctx())
  s = apply(s, { type: 'JobOutput', session: S, id: 'j1', chunk: '10%\r55%\ndone' }, ctx())
  expect(s.entries[0]).toMatchObject({ job: { id: 'j1', lines: ['55%', 'done'] } })
  const inline = apply(s, { type: 'JobFinished', session: S, id: 'j1', command: 'make', exit: { exited: 0 }, secs: 2n, output: '', inline: true, pty: false }, ctx())
  expect(inline.entries[0]).not.toHaveProperty('job')
  const late = apply(s, { type: 'JobFinished', session: S, id: 'j1', command: 'make', exit: 'timeout', secs: 9n, output: '', inline: false, pty: false }, ctx())
  expect(late.entries[0]).toMatchObject({ job: { exit: { exit: 'timeout', secs: 9 } } })
  expect(apply(s, { type: 'JobOutput', session: S, id: 'j9', chunk: 'x' }, ctx())).toBe(s)
})

test('fromHistory maps roles; a tool result tuple becomes a result', () => {
  expect(fromHistory({ role: 'tool', call_id: 'c.1', name: 'read', args: { p: 1 }, result: ['out', true] })[0]).toMatchObject({
    kind: 'tool',
    callId: 'c.1',
    child: true,
    pending: false,
    result: { output: 'out', isError: true, files: [] },
  })
  expect(fromHistory({ role: 'tool', call_id: 'c', name: 'read', args: null, result: null })[0]).not.toHaveProperty('result')
  expect(fromHistory({ role: 'assistant', text: 'a' })[0]).toMatchObject({ kind: 'assistant', open: false })
  expect(fromHistory({ role: 'compacted', epoch: 1, summary: 's', style: 'cut' })[0]).toMatchObject({ kind: 'compacted', summary: 's' })
  expect(fromHistory({ role: 'user', text: '<context_summary>x</context_summary>' })).toEqual([])
  expect(fromHistory({ role: 'user', text: '<message from="p">\nhi\n</message>\n<message from="q">\nyo\n</message>' })).toMatchObject([
    { kind: 'peer', from: 'p', text: 'hi' },
    { kind: 'peer', from: 'q', text: 'yo' },
  ])
})

test('store: Lagged counts, history replace then prepend keeps newer entries and sets paging', () => {
  const st = useSessions.getState()
  st.applyEvents([{ type: 'Lagged', count: 3 }, { type: 'TextDelta', session: S, turn: T, text: 'live' }])
  expect(useSessions.getState().lagged).toBe(3)
  const page = { running: false, model: 'm', effort: null, queued: [], undone: 0, agent: null, name: null, parent: null, skills: [] }
  st.loadHistory(S, { ...page, history: [{ role: 'user', text: 'two' }], first_seq: 7, more: true }, 'replace')
  let s = useSessions.getState().sessions[S]
  expect(s?.entries.map((e) => e.kind)).toEqual(['user'])
  expect(s).toMatchObject({ loaded: true, firstSeq: 7, more: true, model: 'm' })
  st.loadHistory(S, { ...page, history: [{ role: 'user', text: 'one' }], first_seq: 2, more: false }, 'prepend')
  s = useSessions.getState().sessions[S]
  expect(s?.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['one', 'two'])
  expect(s).toMatchObject({ firstSeq: 2, more: false })
  st.markSent(S, 'mine', ['/a.png'])
  st.applyEvents([{ type: 'TurnStarted', session: S, turn: T, text: 'mine', model: 'm' }])
  s = useSessions.getState().sessions[S]
  expect(s?.entries.at(-1)).toMatchObject({ kind: 'user', text: 'mine', files: ['/a.png'] })
  expect(s?.entries).toHaveLength(3)
})
