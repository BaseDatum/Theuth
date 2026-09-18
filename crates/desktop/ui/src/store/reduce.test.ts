import { expect, test } from 'vitest'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import type { Usage } from '@/rpc/bindings/Usage'
import type { TodoSnapshot } from '@/rpc/bindings/TodoSnapshot'
import type { WireEvent } from '@/rpc/events'
import type { LoadedPage } from '@/rpc/extra'
import { adopt, apply, empty, fromHistory, route, type Ctx } from './reduce'
import { type Freshness, type SessionSlice, freshness, useSessions } from './sessions'

const S = 'sess'
const T = 'turn'
const usage: Usage = { prompt: 10, completion: 5, cached: 0 }
const ctx = (): Ctx => ({ sent: new Set() })
/** A settled, editable `Loaded` at revision `rev` without paging fields. */
const loaded = (rev = '1'): Omit<LoadedPage, 'history' | 'first_seq' | 'more'> => ({
  running: false,
  model: 'm',
  effort: null,
  queued: [],
  undone: 0,
  agent: null,
  name: null,
  parent: null,
  skills: [],
  context_revision: rev,
  context_editable: true,
  context_reason: null,
  context_estimate: null,
  settled: true,
  todos: null,
})
const msg = (seq: number): ContextEntry => ({ target: { kind: 'message', seq }, active: true, removed: false, remove_with: [], attachments: [] })
/** A native plan at `version` whose items are `texts`, all pending. */
const plan = (version: number, ...texts: string[]): TodoSnapshot => ({
  version,
  source: 'native',
  items: texts.map((text) => ({ text, status: 'pending', reason: null })),
})
const todoTexts = (s: SessionSlice | undefined) => s?.todos?.items.map((t) => t.text)

function run(events: WireEvent[], c = ctx()) {
  return events.reduce((s, e) => apply(s, e, c), empty(S))
}

test('deltas append to one open assistant entry; a tool event closes it', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 },
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

test('tool-only turns omit whitespace assistant rows', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 },
    { type: 'TextDelta', session: S, turn: T, text: '\n' },
    { type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'read', args: { path: 'a' } },
  ])
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'tool'])
  expect(fromHistory({ role: 'assistant', text: ' \n', turn: 1 })).toEqual([])
})

test('stored todo continuation renders as a harness nudge with literal task text, not a user or tool', () => {
  const body = 'Unfinished work:\n- Check &lt;button&gt; &amp; &amp;lt;literal&amp;gt;\nContinue working.'
  const [entry] = fromHistory({ role: 'user', text: `<todo_continue>\n${body}\n</todo_continue>`, turn: null, context: msg(3) })
  expect(entry).toMatchObject({
    kind: 'nudge',
    text: 'Unfinished work:\n- Check <button> & &lt;literal&gt;\nContinue working.',
    context: msg(3),
  })
  expect(entry).not.toHaveProperty('turn')
})

test('pending tool is replaced by its start (same id), finished by call id, abandoned ones pruned', () => {
  let s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
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

test('ToolCallProgress names the file being written; ToolCallStarted clears it; ArtifactShown attaches to the tool entry', () => {
  let s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'c1', name: 'show' },
    { type: 'ToolCallProgress', session: S, turn: T, call_id: 'c1', args: '{"name": "report.pdf", "content": "# Quar' },
  ])
  expect(s.progress.get('c1')).toMatchObject({ name: 'report.pdf', writing: true })
  s = apply(s, { type: 'ToolCallProgress', session: S, turn: T, call_id: 'c1', args: '{"path": "/tmp/x/notes.md", "ti' }, ctx())
  expect(s.progress.get('c1')).toMatchObject({ name: 'notes.md', writing: false })
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'show', args: { name: 'report.pdf' } }, ctx())
  expect(s.progress.has('c1')).toBe(false)
  const artifact = {
    id: 'abc',
    session: S,
    call_id: 'c1',
    name: 'report.pdf',
    title: 'Quarterly',
    kind: 'pdf' as const,
    mime: 'application/pdf',
    bytes: 84_000,
    version: 1,
    path: '/d/artifacts/abc.pdf',
    source: null,
    preview: null,
    pages: 3,
    origin: null,
    created_at: 0,
  }
  s = apply(s, { type: 'ArtifactShown', session: S, call_id: 'c1', artifact }, ctx())
  const tool = s.entries.find((e) => e.kind === 'tool')
  expect(tool && s.artifacts.get(tool.callId)).toEqual(artifact)
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
    { type: 'TurnStarted', session: S, turn: 'T2', text: 'steer', model: 'm', turn_seq: 4 },
  ])
  expect(s.queued).toEqual([])
  expect(s.entries).toHaveLength(1)
  expect(s.entries[0]).toMatchObject({ kind: 'user', text: 'steer', injected: true, turn: 4 })
  expect(s.lastInjected).toBeUndefined()
})

test('TurnStarted echo: skipped for our own send (once) and harness wrappers, skill shows a chip', () => {
  const c = ctx()
  c.sent.add('mine')
  let s = apply(empty(S), { type: 'TurnStarted', session: S, turn: T, text: 'mine', model: 'm', turn_seq: 1 }, c)
  expect(s.entries).toEqual([])
  expect(c.sent.size).toBe(0)
  s = apply(s, { type: 'TurnStarted', session: S, turn: T, text: '<message from="p">\nhi\n</message>', model: 'm', turn_seq: 2 }, c)
  expect(s.entries).toEqual([])
  s = apply(s, { type: 'TurnStarted', session: S, turn: T, text: '<skill name="k">\nb\n</skill>\n\nwords', model: 'm', turn_seq: 3 }, c)
  expect(s.entries[0]).toMatchObject({ kind: 'user', skill: 'k', text: 'words', turn: 3 })
})

test('Compacted: clear wipes, other styles add a divider after the closed tail', () => {
  const streaming = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const div = apply(streaming, { type: 'Compacted', session: S, turn: T, epoch: 2, before: 1, after: 400, folded: 3, style: 'summary' }, ctx())
  expect(div.entries.map((e) => e.kind)).toEqual(['assistant', 'compacted'])
  expect(div.entries[0]).toMatchObject({ open: false })
  expect(div.entries[1]).toMatchObject({ epoch: 2, style: 'summary' })
  expect(div.context).toBe(400)
  const cleared = apply(streaming, { type: 'Compacted', session: S, turn: null, epoch: 3, before: 1, after: 50, folded: 3, style: 'clear' }, ctx())
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

test('ScrapeProgress attaches inflight URLs to the crawl card by call id, dropped on finish', () => {
  let s = run([{ type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'crawl', args: { url: 'https://a.example' } }])
  s = apply(
    s,
    {
      type: 'ScrapeProgress',
      session: S,
      call_id: 'c1',
      inflight: ['https://a.example/1', 'https://b.example/2'],
      done: ['https://a.example/'],
      limit: 50,
    },
    ctx(),
  )
  expect(s.scrape.get('c1')).toEqual({
    inflight: ['https://a.example/1', 'https://b.example/2'],
    done: ['https://a.example/'],
    limit: 50,
  })
  s = apply(s, { type: 'ToolCallFinished', session: S, turn: T, call_id: 'c1', output: 'ok', is_error: false, files: [] }, ctx())
  expect(s.scrape.has('c1')).toBe(false)
})

test('fromHistory maps roles; a tool result tuple becomes a result', () => {
  expect(fromHistory({ role: 'tool', call_id: 'c.1', name: 'read', args: { p: 1 }, result: ['out', true], files: ['/s.png'] })[0]).toMatchObject({
    kind: 'tool',
    callId: 'c.1',
    child: true,
    pending: false,
    result: { output: 'out', isError: true, files: ['/s.png'] },
  })
  expect(fromHistory({ role: 'tool', call_id: 'c', name: 'read', args: null, result: null, files: [] })[0]).not.toHaveProperty('result')
  expect(fromHistory({ role: 'assistant', text: 'a', turn: 3 })[0]).toMatchObject({ kind: 'assistant', open: false, turn: 3 })
  expect(fromHistory({ role: 'compacted', epoch: 1, summary: 's', style: 'cut' })[0]).toMatchObject({ kind: 'compacted', summary: 's' })
  expect(fromHistory({ role: 'user', text: '<context_summary>x</context_summary>', turn: null })).toEqual([])
  expect(fromHistory({ role: 'user', text: '<message from="p">\nhi\n</message>\n<message from="q">\nyo\n</message>', turn: 9 })).toMatchObject([
    { kind: 'peer', from: 'p', text: 'hi' },
    { kind: 'peer', from: 'q', text: 'yo' },
  ])
})

test('fromHistory carries the turn on openers; a steer has none', () => {
  expect(fromHistory({ role: 'user', text: 'ask', turn: 3 })[0]).toMatchObject({ kind: 'user', turn: 3 })
  expect(fromHistory({ role: 'user', text: '<skill name="k">\nb\n</skill>\n\nwords', turn: 3 })[0]).toMatchObject({ kind: 'user', skill: 'k', turn: 3 })
  const steer = fromHistory({ role: 'user', text: 'steer', turn: null })[0]
  expect(steer?.kind === 'user' ? steer.turn : 'not a user row').toBeUndefined()
})

test('store: Lagged counts, history replace then prepend keeps newer entries and sets paging', () => {
  const st = useSessions.getState()
  st.applyEvents([{ type: 'Lagged', count: 3 }, { type: 'TextDelta', session: S, turn: T, text: 'live' }])
  expect(useSessions.getState().lagged).toBe(3)
  const page = loaded()
  st.loadHistory(S, { ...page, history: [{ role: 'user', text: 'two', turn: 7 }], first_seq: 7, more: true }, 'replace')
  let s = useSessions.getState().sessions[S]
  expect(s?.entries.map((e) => e.kind)).toEqual(['user'])
  expect(s).toMatchObject({ loaded: true, firstSeq: 7, more: true, model: 'm' })
  st.loadHistory(S, { ...page, history: [{ role: 'user', text: 'one', turn: 2 }], first_seq: 2, more: false }, 'prepend')
  s = useSessions.getState().sessions[S]
  expect(s?.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['one', 'two'])
  expect(s).toMatchObject({ firstSeq: 2, more: false })
  st.markSent(S, 'mine', ['/a.png'])
  st.applyEvents([{ type: 'TurnStarted', session: S, turn: T, text: 'mine', model: 'm', turn_seq: 9 }])
  s = useSessions.getState().sessions[S]
  expect(s?.entries.at(-1)).toMatchObject({ kind: 'user', text: 'mine', files: ['/a.png'] })
  expect(s?.entries).toHaveLength(3)
})

test('store: TurnStarted stamps the optimistic send and its reply with the turn; truncate drops from that turn', () => {
  const st = useSessions.getState()
  const page = { ...loaded(), first_seq: 1, more: false }
  st.loadHistory(
    S,
    {
      ...page,
      history: [
        { role: 'user', text: 'one', turn: 1 },
        { role: 'assistant', text: 'A', turn: 1 },
      ],
    },
    'replace',
  )
  st.markSent(S, 'two')
  expect(useSessions.getState().sessions[S]?.entries.at(-1)).not.toHaveProperty('turn')
  st.applyEvents([
    { type: 'TurnStarted', session: S, turn: T, text: 'two', model: 'm', turn_seq: 7 },
    { type: 'TextDelta', session: S, turn: T, text: 'B' },
  ])
  let s = useSessions.getState().sessions[S]
  expect(s?.turnSeq).toBe(7)
  expect(s?.entries.map((e) => (e.kind === 'user' || e.kind === 'assistant' ? [e.kind, e.turn] : e.kind))).toEqual([
    ['user', 1],
    ['assistant', 1],
    ['user', 7],
    ['assistant', 7],
  ])
  st.truncate(S, 99)
  expect(useSessions.getState().sessions[S]).toBe(s)
  st.truncate(S, 7)
  s = useSessions.getState().sessions[S]
  expect(s?.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['one', 'assistant'])
})

test('ContextChanged marks the slice stale and moves the generation; the installed revision is a no-op unless stale', () => {
  const base: SessionSlice = { ...empty(S), contextRevision: '3', contextEditable: true, loaded: true }
  expect(apply(base, { type: 'ContextChanged', session: S, revision: '3', reason: 'settled' }, ctx())).toBe(base)
  const s = apply(base, { type: 'ContextChanged', session: S, revision: '4', reason: 'context_edit' }, ctx())
  expect(s).toMatchObject({ contextRevision: '4', contextStale: true, contextEditable: false, contextEstimated: true, historyGeneration: 1 })
  // Stale at the same revision (a refused reload): the echo still triggers the retry.
  expect(apply(s, { type: 'ContextChanged', session: S, revision: '4', reason: 'settled' }, ctx()).historyGeneration).toBe(2)
  // A turn's settled notice does not turn a real usage figure into an estimate.
  expect(apply({ ...base, context: 100 }, { type: 'ContextChanged', session: S, revision: '5', reason: 'settled' }, ctx()).contextEstimated).toBe(false)
})

test('turn activity bumps live; bookkeeping (jobs, queue, title) does not; a turn disables editing until the settled reload', () => {
  const s0 = { ...empty(S), contextEditable: true }
  const s1 = apply(s0, { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 }, ctx())
  expect(s1).toMatchObject({ live: 1, contextEditable: false })
  const s2 = apply(s1, { type: 'TextDelta', session: S, turn: T, text: 'x' }, ctx())
  expect(s2.live).toBe(2)
  const s3 = apply(s2, { type: 'InputQueued', session: S, id: 'q', text: 'later' }, ctx())
  const s4 = apply(s3, { type: 'TitleChanged', session: S, title: 't' }, ctx())
  const s5 = apply(s4, { type: 'JobOutput', session: S, id: 'j', chunk: 'out' }, ctx())
  expect(s5.live).toBe(2)
  expect(apply(s5, { type: 'MessageDelivered', session: S, from: 'p', text: 'm' }, ctx()).live).toBe(3)
  const done = apply(s5, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 42 }, ctx())
  expect(done).toMatchObject({ context: 42, contextEstimated: false, running: false })
})

test('fromHistory carries the row context onto every card it becomes', () => {
  const context = msg(7)
  const cards = fromHistory({
    role: 'user',
    text: '<message from="p">\nhi\n</message>\n<message from="q">\nyo\n</message>',
    turn: null,
    context,
  })
  expect(cards).toHaveLength(2)
  for (const c of cards) expect(c.context).toBe(context)
  expect(fromHistory({ role: 'tool', call_id: 'c', name: 'read', args: null, result: null, files: [], context })[0]?.context).toBe(context)
  expect(fromHistory({ role: 'compacted', epoch: 1, summary: 's', style: 'cut', context })[0]?.context).toBe(context)
  expect(fromHistory({ role: 'assistant', text: 'a', turn: 3 })[0]?.context).toBeUndefined()
})

test('adopt keeps the ids (and job tails) of cards a reloaded snapshot renders again, pairing duplicates in order', () => {
  const old = [
    ...fromHistory({ role: 'user', text: 'same', turn: 1 }),
    ...fromHistory({ role: 'assistant', text: 'A', turn: 1 }),
    ...fromHistory({ role: 'user', text: 'same', turn: 3 }),
    { ...fromHistory({ role: 'tool', call_id: 'c1', name: 'shell', args: null, result: null, files: [] })[0]!, job: { id: 'j', command: 'ls', lines: ['x'] } },
  ]
  const fresh = [
    ...fromHistory({ role: 'user', text: 'same', turn: 1, context: msg(1) }),
    ...fromHistory({ role: 'user', text: 'same', turn: 3, context: msg(3) }),
    ...fromHistory({ role: 'tool', call_id: 'c1', name: 'shell', args: null, result: ['ok', false], files: [], context: msg(4) }),
    ...fromHistory({ role: 'assistant', text: 'new', turn: 3 }),
  ]
  const out = adopt(old, fresh)
  expect(out.map((e) => e.id)).toEqual([old[0]!.id, old[2]!.id, old[3]!.id, fresh[3]!.id])
  expect(out[0]?.context).toEqual(msg(1))
  expect(out[2]).toMatchObject({ kind: 'tool', job: { id: 'j' }, result: { output: 'ok' } })
})

test('store: a page requested before the generation moved is dropped; one requested after lands', () => {
  const id = 'paged'
  const st = useSessions.getState()
  st.loadHistory(id, { ...loaded('1'), history: [{ role: 'user', text: 'two', turn: 7 }], first_seq: 7, more: true }, 'replace')
  const before = freshness(useSessions.getState().sessions[id]!)
  st.applyEvents([{ type: 'ContextChanged', session: id, revision: '2', reason: 'context_edit' }])
  expect(st.loadHistory(id, { ...loaded('1'), history: [{ role: 'user', text: 'removed', turn: 2 }], first_seq: 2, more: false }, 'prepend', before)).toBe(false)
  const s = useSessions.getState().sessions[id]!
  expect(s.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['two'])
  expect(s).toMatchObject({ contextStale: true, contextEditable: false, firstSeq: 7 })
  expect(st.loadHistory(id, { ...loaded('2'), history: [{ role: 'user', text: 'one', turn: 2 }], first_seq: 2, more: false }, 'prepend', freshness(s))).toBe(true)
  expect(useSessions.getState().sessions[id]!.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['one', 'two'])
})

test('store: a settled snapshot is refused when a stream raced it (deltas kept) or the actor was busy, and installed when nothing did', () => {
  const id = 'settled'
  const st = useSessions.getState()
  st.loadHistory(id, { ...loaded('1'), history: [{ role: 'user', text: 'one', turn: 1 }], first_seq: 1, more: false }, 'replace')
  st.applyEvents([{ type: 'ContextChanged', session: id, revision: '2', reason: 'context_edit' }])
  const fresh: Freshness = freshness(useSessions.getState().sessions[id]!)
  const snapshot = { ...loaded('2'), history: [{ role: 'user' as const, text: 'one', turn: 1, context: msg(1) }], first_seq: 1, more: false }
  // Actor busy: `settled: false` is not a settled success.
  expect(st.loadHistory(id, { ...snapshot, settled: false, context_editable: false, context_reason: 'busy' }, 'settled', fresh)).toBe(false)
  // A turn raced the read: the stream wins, the snapshot is dropped, the slice stays stale.
  st.applyEvents([
    { type: 'TurnStarted', session: id, turn: T, text: 'two', model: 'm', turn_seq: 5 },
    { type: 'TextDelta', session: id, turn: T, text: 'B' },
  ])
  expect(st.loadHistory(id, snapshot, 'settled', fresh)).toBe(false)
  let s = useSessions.getState().sessions[id]!
  expect(s.entries.map((e) => ('text' in e ? e.text : e.kind))).toEqual(['one', 'two', 'B'])
  expect(s).toMatchObject({ contextStale: true, running: true })
  st.applyEvents([
    { type: 'TurnFinished', session: id, turn: T, stop_reason: 'stop', usage, context: 50 },
    { type: 'ContextChanged', session: id, revision: '3', reason: 'settled' },
  ])
  s = useSessions.getState().sessions[id]!
  const oldIds = s.entries.map((e) => e.id)
  const after = {
    ...loaded('3'),
    context_estimate: 40,
    history: [
      { role: 'user' as const, text: 'one', turn: 1, context: msg(1) },
      { role: 'user' as const, text: 'two', turn: 5, context: msg(5) },
      { role: 'assistant' as const, text: 'B', turn: 5, context: msg(6) },
    ],
    first_seq: 1,
    more: false,
  }
  expect(st.loadHistory(id, after, 'settled', freshness(s))).toBe(true)
  s = useSessions.getState().sessions[id]!
  expect(s).toMatchObject({ contextStale: false, contextEditable: true, contextRevision: '3', running: false })
  // The snapshot's cards wear the ids already on screen; live cards now carry their stored identity.
  expect(s.entries.map((e) => e.id)).toEqual(oldIds)
  expect(s.entries.map((e) => e.context?.target)).toEqual([{ kind: 'message', seq: 1 }, { kind: 'message', seq: 5 }, { kind: 'message', seq: 6 }])
  // Real usage from the turn outlives the reconcile; an edit's estimate replaces it.
  expect(s).toMatchObject({ context: 50, contextEstimated: false })
  st.applyEvents([{ type: 'ContextChanged', session: id, revision: '4', reason: 'context_edit' }])
  s = useSessions.getState().sessions[id]!
  expect(st.loadHistory(id, { ...after, context_revision: '4', context_estimate: 12 }, 'settled', freshness(s))).toBe(true)
  expect(useSessions.getState().sessions[id]).toMatchObject({ context: 12, contextEstimated: true })
})

test('store: unload invalidates loaded slices for the reconnect/harness-switch reload; a superseded full page is dropped', () => {
  const id = 'unloaded'
  const st = useSessions.getState()
  st.loadHistory(id, { ...loaded('1'), history: [], first_seq: null, more: false }, 'replace')
  const stale = freshness(useSessions.getState().sessions[id]!)
  st.unload()
  expect(useSessions.getState().sessions[id]).toMatchObject({ loaded: false, contextStale: true })
  // The reload after `unload` lands (loaded was false), leaving the slice fresh.
  expect(st.loadHistory(id, { ...loaded('1'), history: [], first_seq: null, more: false }, 'replace', freshness(useSessions.getState().sessions[id]!))).toBe(true)
  expect(useSessions.getState().sessions[id]).toMatchObject({ loaded: true, contextStale: false })
  // A full page requested before all that has nothing to add.
  expect(st.loadHistory(id, { ...loaded('0'), history: [{ role: 'user', text: 'old', turn: 1 }], first_seq: 1, more: false }, 'replace', stale)).toBe(false)
  expect(useSessions.getState().sessions[id]?.entries).toEqual([])
})

test('TodoChanged: a higher version replaces the list (an undo may select an older, shorter one), equal is idempotent, older is ignored', () => {
  const base = empty(S)
  const v2 = apply(base, { type: 'TodoChanged', session: S, snapshot: plan(2, 'a', 'b') }, ctx())
  expect(todoTexts(v2)).toEqual(['a', 'b'])
  // Same version again: the very same slice, nothing re-renders.
  expect(apply(v2, { type: 'TodoChanged', session: S, snapshot: plan(2, 'a', 'b') }, ctx())).toBe(v2)
  // A late lower version (an out-of-order frame) never rolls the plan back.
  expect(apply(v2, { type: 'TodoChanged', session: S, snapshot: plan(1, 'a') }, ctx())).toBe(v2)
  // Undo: the projection version moves forward while the selected list is the older, shorter one.
  const v3 = apply(v2, { type: 'TodoChanged', session: S, snapshot: plan(3, 'a') }, ctx())
  expect(todoTexts(v3)).toEqual(['a'])
  expect(v3.todos?.version).toBe(3)
  // Plan bookkeeping is not turn activity: a settled snapshot requested meanwhile still lands.
  expect(v3.live).toBe(0)
})

test('store: a slow load never overwrites a newer TodoChanged; an authoritative equal snapshot repairs stale; null keeps the list', () => {
  const id = 'todos-race'
  const st = useSessions.getState()
  // Subscribe-then-snapshot: the event lands first, the snapshot read before it lands later.
  st.applyEvents([{ type: 'TodoChanged', session: id, snapshot: plan(5, 'new') }])
  st.loadHistory(id, { ...loaded(), todos: plan(4, 'old'), history: [], first_seq: null, more: false }, 'replace')
  let s = useSessions.getState().sessions[id]
  expect(todoTexts(s)).toEqual(['new'])
  expect(s).toMatchObject({ loaded: true, todosStale: false })
  // Reconnect: the list is kept and labelled stale; the reload's read (issued after the gap) at the same version clears it.
  st.unload()
  s = useSessions.getState().sessions[id]
  expect(s).toMatchObject({ todosStale: true })
  expect(todoTexts(s)).toEqual(['new'])
  expect(st.loadHistory(id, { ...loaded(), todos: plan(5, 'new'), history: [], first_seq: null, more: false }, 'replace', freshness(s!))).toBe(true)
  expect(useSessions.getState().sessions[id]).toMatchObject({ todosStale: false })
  // A failed/absent read (`null`) is not an empty plan.
  st.markTodosStale(id)
  st.setTodos(id, null)
  s = useSessions.getState().sessions[id]
  expect(todoTexts(s)).toEqual(['new'])
  expect(s?.todosStale).toBe(true)
  // An authoritative `get_todos` at the same version confirms it; an empty snapshot at a higher version is a real clear.
  st.setTodos(id, plan(5, 'new'))
  expect(useSessions.getState().sessions[id]?.todosStale).toBe(false)
  st.setTodos(id, plan(6))
  expect(useSessions.getState().sessions[id]?.todos).toEqual({ version: 6, source: 'native', items: [] })
})

test('store: a refused page still carries a newer plan but not proof of freshness; paging never rolls the plan back', () => {
  const id = 'todos-paged'
  const st = useSessions.getState()
  st.loadHistory(id, { ...loaded('1'), todos: plan(1, 'a'), history: [{ role: 'user', text: 'two', turn: 7 }], first_seq: 7, more: true }, 'replace')
  const before = freshness(useSessions.getState().sessions[id]!)
  st.unload()
  // Read before the gap: refused as a page (moved), yet its higher version is newer than what we show; stale stays.
  expect(st.loadHistory(id, { ...loaded('1'), todos: plan(2, 'a', 'b'), history: [{ role: 'user', text: 'one', turn: 2 }], first_seq: 2, more: false }, 'prepend', before)).toBe(false)
  let s = useSessions.getState().sessions[id]!
  expect(s.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['two'])
  expect(todoTexts(s)).toEqual(['a', 'b'])
  expect(s.todosStale).toBe(true)
  // The reload after the gap at the same version confirms it.
  st.loadHistory(id, { ...loaded('1'), todos: plan(2, 'a', 'b'), history: [{ role: 'user', text: 'two', turn: 7 }], first_seq: 7, more: true }, 'replace', freshness(s))
  s = useSessions.getState().sessions[id]!
  expect(s.todosStale).toBe(false)
  // Older page landing with the plan as it was at its read time: rows prepend, the plan stays current.
  expect(st.loadHistory(id, { ...loaded('1'), todos: plan(1, 'a'), history: [{ role: 'user', text: 'one', turn: 2 }], first_seq: 2, more: false }, 'prepend', freshness(s))).toBe(true)
  s = useSessions.getState().sessions[id]!
  expect(s.entries.map((e) => (e.kind === 'user' ? e.text : e.kind))).toEqual(['one', 'two'])
  expect(todoTexts(s)).toEqual(['a', 'b'])
})

test('store: expansion survives snapshots, events and the reconnect reload; plans are per session', () => {
  const a = 'todos-a'
  const b = 'todos-b'
  const st = useSessions.getState()
  st.applyEvents([{ type: 'TodoChanged', session: a, snapshot: plan(1, 'a1') }])
  st.setTodosExpanded(a, true)
  st.applyEvents([
    { type: 'TodoChanged', session: a, snapshot: plan(2, 'a1', 'a2') },
    { type: 'TodoChanged', session: b, snapshot: plan(9, 'b1') },
  ])
  st.unload()
  st.loadHistory(a, { ...loaded(), todos: plan(2, 'a1', 'a2'), history: [], first_seq: null, more: false }, 'replace', freshness(useSessions.getState().sessions[a]!))
  const sa = useSessions.getState().sessions[a]
  const sb = useSessions.getState().sessions[b]
  expect(sa).toMatchObject({ todosExpanded: true, todosStale: false })
  expect(todoTexts(sa)).toEqual(['a1', 'a2'])
  // The sibling's higher version is its own; it stays collapsed and stale until its own refresh.
  expect(todoTexts(sb)).toEqual(['b1'])
  expect(sb).toMatchObject({ todosExpanded: false, todosStale: true })
})
