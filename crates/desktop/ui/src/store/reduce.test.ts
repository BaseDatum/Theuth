import { expect, test } from 'vitest'
import type { ContextEntry } from '@/rpc/bindings/ContextEntry'
import type { Usage } from '@/rpc/bindings/Usage'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { TodoSnapshot } from '@/rpc/bindings/TodoSnapshot'
import type { WireEvent } from '@/rpc/events'
import type { LoadedPage, VisionNote, VisionSource } from '@/rpc/extra'
import { adopt, apply, empty, fromHistory, resumeFromLoaded, route, settleCompacting, type Ctx } from './reduce'
import { type Freshness, type SessionSlice, browserBusy, freshness, shownActivity, useSessions } from './sessions'
import type { CodeToolRan } from '@/lib/codemap/types'
import codemapEvents from '@/components/codemap/__fixtures__/events.json'
import { runningSubagents } from '@/components/sidebar/SessionItem'

const S = 'sess'
const T = 'turn'
const usage: Usage = { prompt: 10, completion: 5, cached: 0 }
const ctx = (now = 0): Ctx => ({ sent: new Set(), now })
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
  harness_commands: [],
  context_revision: rev,
  context_editable: true,
  context_reason: null,
  context_estimate: null,
  settled: true,
  todos: null,
  communication: null,
  project: null,
  retention: 'durable',
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

test('reasoning deltas stream into one entry that closes when the answer starts', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 },
    { type: 'ReasoningDelta', session: S, turn: T, text: 'Let me ' },
    { type: 'ReasoningDelta', session: S, turn: T, text: 'think.' },
  ])
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'reasoning'])
  expect(s.entries[1]).toMatchObject({ kind: 'reasoning', text: 'Let me think.', done: false })
  expect(s.activity).toBe('reasoning')
  const t = apply(s, { type: 'TextDelta', session: S, turn: T, text: 'Red.' }, ctx())
  expect(t.entries.map((e) => e.kind)).toEqual(['user', 'reasoning', 'assistant'])
  expect(t.entries[1]).toMatchObject({ kind: 'reasoning', done: true })
  // A second thinking block (after a tool call) is its own entry, not appended to the closed one.
  const u = apply(t, { type: 'ReasoningDelta', session: S, turn: T, text: 'More.' }, ctx())
  expect(u.entries.map((e) => e.kind)).toEqual(['user', 'reasoning', 'assistant', 'reasoning'])
  expect(u.entries[3]).toMatchObject({ kind: 'reasoning', text: 'More.', done: false })
  // A reload rebuilds the same collapsed block from the stored row.
  expect(fromHistory({ role: 'reasoning', text: 'Let me think.', turn: 1 })).toMatchObject([{ kind: 'reasoning', text: 'Let me think.', done: true }])
})

test('ReasoningDone closes the block with the engine time; without it the client clock fills in (91)', () => {
  const open = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 },
    { type: 'ReasoningDelta', session: S, turn: T, text: 'Let me ' },
    { type: 'ReasoningDelta', session: S, turn: T, text: 'think.' },
  ], ctx(1000))
  expect(open.entries[1]).toMatchObject({ kind: 'reasoning', done: false, startedAt: 1000 })
  // The engine's measurement wins and survives the text that follows.
  const done = apply(open, { type: 'ReasoningDone', session: S, turn: T, ms: 9000 }, ctx(10_400))
  expect(done.entries[1]).toMatchObject({ kind: 'reasoning', done: true, ms: 9000 })
  expect(done.entries[1]).not.toHaveProperty('startedAt')
  const text = apply(done, { type: 'TextDelta', session: S, turn: T, text: 'Red.' }, ctx(20_000))
  expect(text.entries[1]).toMatchObject({ kind: 'reasoning', done: true, ms: 9000 })
  expect(text.entries.map((e) => e.kind)).toEqual(['user', 'reasoning', 'assistant'])
  // Fallback: an older engine / a harness never sends ReasoningDone.
  const fell = apply(open, { type: 'TextDelta', session: S, turn: T, text: 'Red.' }, ctx(10_400))
  expect(fell.entries[1]).toMatchObject({ kind: 'reasoning', done: true, ms: 9400 })
  // Stored rows carry the engine's time; rows from before 91 have none.
  expect(fromHistory({ role: 'reasoning', text: 't', turn: 1, thinking_ms: 9000 })).toMatchObject([{ kind: 'reasoning', done: true, ms: 9000 }])
  expect(fromHistory({ role: 'reasoning', text: 't', turn: 1 })[0]).not.toHaveProperty('ms', expect.anything())
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

test('stored todo continuation renders as an engine nudge with literal task text, not a user or tool', () => {
  const body = 'Unfinished work:\n- Check &lt;button&gt; &amp; &amp;lt;literal&amp;gt;\nContinue working.'
  const [entry] = fromHistory({ role: 'user', text: `<todo_continue>\n${body}\n</todo_continue>`, turn: null, context: msg(3) })
  expect(entry).toMatchObject({
    kind: 'engine',
    notice: 'todo_continue',
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

test('browserBusy: a browser_* call without its result during a running turn, not other tools, not after the turn', () => {
  let s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'c1', name: 'read' },
  ])
  expect(browserBusy(s)).toBe(false)
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'c2', name: 'browser_run', args: {} }, ctx())
  expect(browserBusy(s)).toBe(true)
  s = apply(s, { type: 'ToolCallFinished', session: S, turn: T, call_id: 'c2', output: 'ok', is_error: false, files: [] }, ctx())
  expect(browserBusy(s)).toBe(false)
  s = apply(s, { type: 'ToolCallPending', session: S, turn: T, call_id: 'c3', name: 'browser_click' }, ctx())
  expect(browserBusy(s)).toBe(true)
  s = apply(s, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 15 }, ctx())
  expect(browserBusy(s)).toBe(false)
  expect(browserBusy(undefined)).toBe(false)
})

test('detached harvest: Harvesting after TurnFinished shows only while idle, a new turn takes over, Harvested clears it', () => {
  let s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 15 },
    { type: 'Harvesting', session: S },
  ])
  expect(s).toMatchObject({ running: false, activity: 'idle', harvesting: true })
  expect(shownActivity(s.activity, s.harvesting)).toBe('harvesting')
  s = apply(s, { type: 'TurnStarted', session: S, turn: 'T2', text: 'y', model: 'm', turn_seq: 3 }, ctx())
  expect(s).toMatchObject({ running: true, activity: 'thinking', harvesting: true })
  expect(shownActivity(s.activity, s.harvesting)).toBe('thinking')
  s = apply(s, { type: 'Harvested', session: S, added: 1, updated: 0 }, ctx())
  expect(s).toMatchObject({ running: true, activity: 'thinking', harvesting: false })
  expect(s.entries.at(-1)).toMatchObject({ kind: 'info', text: 'knowledge +1 ~0' })
  s = apply(s, { type: 'TurnFinished', session: S, turn: 'T2', stop_reason: 'stop', usage, context: 15 }, ctx())
  expect(shownActivity(s.activity, s.harvesting)).toBe('idle')
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

const APP_ARTIFACT = {
  id: 'app1',
  session: S,
  call_id: 'a1',
  name: 'drill',
  title: 'DR drill',
  kind: 'app' as const,
  mime: 'text/x-openui',
  bytes: 120,
  version: 1,
  path: '/d/artifacts/app1.openui',
  source: '/d/artifacts/meta.app.json',
  preview: null,
  pages: null,
  origin: null,
  created_at: 0,
}

test('an app call keeps a draft: envelope-repaired partial source, kept across an unparsable chunk, promoted away by ArtifactShown', () => {
  let s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'a1', name: 'app' },
    { type: 'ToolCallProgress', session: S, turn: T, call_id: 'a1', args: '{"name": "drill", "source": "root = Stack([title])\\ntitle = TextContent(\\"## DR' },
  ])
  // The envelope is repaired; the source inside is exactly the bytes so far, including its unterminated string.
  expect(s.appDrafts.get('a1')).toMatchObject({ name: 'drill', edit: false, status: 'streaming', source: 'root = Stack([title])\ntitle = TextContent("## DR' })
  const before = s.appDrafts.get('a1')
  // A chunk cut inside a key does not repair to an object: the previous source stays.
  s = apply(s, { type: 'ToolCallProgress', session: S, turn: T, call_id: 'a1', args: '{"na' }, ctx())
  expect(s.appDrafts.get('a1')?.source).toBe(before?.source)
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'a1', name: 'app', args: { name: 'drill', source: 'root = Stack([])', edit: false } }, ctx())
  expect(s.appDrafts.get('a1')).toMatchObject({ status: 'validating', source: 'root = Stack([])' })
  s = apply(s, { type: 'ArtifactShown', session: S, call_id: 'a1', artifact: APP_ARTIFACT }, ctx())
  expect(s.appDrafts.has('a1')).toBe(false)
  expect(s.artifacts.get('a1')).toEqual(APP_ARTIFACT)
})

test('a non-app call streaming arguments never becomes a draft', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'c1', name: 'show' },
    { type: 'ToolCallProgress', session: S, turn: T, call_id: 'c1', args: '{"name": "root = x", "source": "y"}' },
  ])
  expect(s.appDrafts.size).toBe(0)
})

test('an edit draft carries its base; a failed publish keeps the draft readable with the error, a success drops it', () => {
  const start: WireEvent[] = [
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'a2', name: 'app' },
    { type: 'ToolCallProgress', session: S, turn: T, call_id: 'a2', args: '{"name": "drill", "edit": true, "base_call_id": "a1", "source": "title = TextContent(\\"## v2\\")\\n' },
  ]
  let s = run(start)
  expect(s.appDrafts.get('a2')).toMatchObject({ edit: true, base: 'a1', source: 'title = TextContent("## v2")\n' })
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'a2', name: 'app', args: { name: 'drill', edit: true, base_call_id: 'a1', source: 'title = TextContent("## v2")\n' } }, ctx())
  s = apply(s, { type: 'ToolCallFinished', session: S, turn: T, call_id: 'a2', output: 'error: unknown component Foo', is_error: true, files: [] }, ctx())
  expect(s.appDrafts.get('a2')).toMatchObject({ status: 'failed', error: 'error: unknown component Foo', base: 'a1' })
  // The failed draft is not an artifact and survives the turn's end for reading.
  s = apply(s, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 1 }, ctx())
  expect(s.appDrafts.get('a2')?.status).toBe('failed')

  let ok = run(start)
  ok = apply(ok, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'a2', name: 'app', args: { name: 'drill', edit: true, base_call_id: 'a1', source: 'x = 1' } }, ctx())
  ok = apply(ok, { type: 'ToolCallFinished', session: S, turn: T, call_id: 'a2', output: 'published drill v2', is_error: false, files: [] }, ctx())
  expect(ok.appDrafts.has('a2')).toBe(false)
})

test('a draft abandoned mid-stream is cancelled at TurnFinished, not dropped; its pending card goes', () => {
  const s = run([
    { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 },
    { type: 'ToolCallPending', session: S, turn: T, call_id: 'a3', name: 'app' },
    { type: 'ToolCallProgress', session: S, turn: T, call_id: 'a3', args: '{"name": "drill", "source": "root = Sta' },
    { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 1 },
  ])
  expect(s.appDrafts.get('a3')).toMatchObject({ status: 'cancelled', source: 'root = Sta' })
  expect(s.entries.some((e) => e.kind === 'tool')).toBe(false)
  expect(s.progress.size).toBe(0)
})

test('InputQueued dedupes by id and never touches the streaming tail', () => {
  const base = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const q = apply(base, { type: 'InputQueued', session: S, id: 'i1', text: 'steer', defer: false }, ctx())
  expect(q.queued).toEqual([{ id: 'i1', text: 'steer', defer: false }])
  expect(q.entries[0]).toMatchObject({ open: true })
  expect(apply(q, { type: 'InputQueued', session: S, id: 'i1', text: 'steer', defer: false }, ctx())).toBe(q)
  expect(apply(q, { type: 'InputWithdrawn', session: S, id: 'nope' }, ctx())).toBe(q)
  expect(apply(q, { type: 'InputWithdrawn', session: S, id: 'i1' }, ctx()).queued).toEqual([])
})

test('InputInjected moves the steer into the transcript and suppresses its echo', () => {
  const s = run([
    { type: 'InputQueued', session: S, id: 'i1', text: 'steer', defer: false },
    { type: 'InputInjected', session: S, id: 'i1', turn: 'T2' },
    { type: 'TurnStarted', session: S, turn: 'T2', text: 'steer', model: 'm', turn_seq: 4 },
  ])
  expect(s.queued).toEqual([])
  expect(s.entries).toHaveLength(1)
  expect(s.entries[0]).toMatchObject({ kind: 'user', text: 'steer', injected: true, turn: 4 })
  expect(s.lastInjected).toBeUndefined()
})

test('TurnStarted echo: skipped for our own send (once) and engine wrappers, skill shows a chip', () => {
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

// REGEN-STAMP (74 §3): a regenerated turn's `TurnStarted` replays the stored opener; whatever `<time …/>` line the
// engine's row carries is never the bubble's text, on the live echo, a steer injection or the history reload — and
// wrapped openers still classify behind it.
test('a leading <time …/> line never renders as user text; the row still classifies and keeps its time', () => {
  const stamp = '<time now="2026-10-03 00:05 +01:00 (Sat)"/>'
  const live = run([{ type: 'TurnStarted', session: S, turn: T, text: `${stamp}\nhello`, model: 'm', turn_seq: 1 }])
  expect(live.entries).toHaveLength(1)
  expect(live.entries[0]).toMatchObject({ kind: 'user', text: 'hello', turn: 1 })
  // An own send is matched by its bare text even when the echo arrives stamped.
  const c = ctx()
  c.sent.add('hello')
  expect(apply(empty(S), { type: 'TurnStarted', session: S, turn: T, text: `${stamp}\nhello`, model: 'm', turn_seq: 1 }, c).entries).toEqual([])
  // A stamped wrapper is still the wrapper: no `you` row.
  const peer = run([{ type: 'TurnStarted', session: S, turn: T, text: `${stamp}\n<message from="p">\nhi\n</message>`, model: 'm', turn_seq: 2 }])
  expect(peer.entries).toEqual([])
  const steer = run([
    { type: 'InputQueued', session: S, id: 'i1', text: `${stamp}\nlater`, defer: false },
    { type: 'InputInjected', session: S, id: 'i1', turn: 'T2' },
  ])
  expect(steer.entries[0]).toMatchObject({ kind: 'user', text: 'later', injected: true })
  // History: the stamp's instant is the hover time for a row without `messages.at`; the row's own `at` wins.
  const stampAt = Date.parse('2026-10-03T00:05+01:00')
  expect(fromHistory({ role: 'user', text: `${stamp}\nhello`, turn: 3 })[0]).toMatchObject({ kind: 'user', text: 'hello', turn: 3, at: stampAt })
  const rowAt = '2026-10-03T01:00:00Z'
  expect(fromHistory({ role: 'user', text: `${stamp}\nhello`, turn: 3, context: { ...msg(3), at: rowAt } })[0]).toMatchObject({ text: 'hello', at: Date.parse(rowAt) })
  expect(fromHistory({ role: 'user', text: 'plain', turn: 3 })[0]).not.toHaveProperty('at')
})

test('Compacted: clear wipes, other styles add a divider after the closed tail', () => {
  const streaming = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const div = apply(streaming, { type: 'Compacted', session: S, turn: T, epoch: 2, before: 1, after: 400, folded: 3, style: 'summary' }, ctx())
  expect(div.entries.map((e) => e.kind)).toEqual(['assistant', 'compacted'])
  expect(div.entries[0]).toMatchObject({ open: false })
  expect(div.entries[1]).toMatchObject({ epoch: 2, style: 'summary' })
  // 70 COMPACT-SIZE: `after` is the engine's estimate → `≈` until the provider's next number.
  expect([div.context, div.contextEstimated]).toEqual([400, true])
  const billed = apply(div, { type: 'ContextUsed', session: S, turn: T, context: 410 }, ctx())
  expect([billed.context, billed.contextEstimated]).toEqual([410, false])
  const cleared = apply(streaming, { type: 'Compacted', session: S, turn: null, epoch: 3, before: 1, after: 50, folded: 3, style: 'clear' }, ctx())
  expect(cleared.entries).toHaveLength(1)
  expect(cleared.entries[0]).toMatchObject({ kind: 'info', text: 'context cleared (epoch 3)' })
})

test('Compacting is idempotent; settleCompacting undoes it only while still compacting (87 COMPACT-NONBLOCK)', () => {
  const compacting: WireEvent = { type: 'Compacting', session: S, turn: null }
  const once = run([compacting])
  expect(once.activity).toBe('compacting')
  expect(apply(once, compacting, ctx())).toBe(once)
  expect(settleCompacting(once).activity).toBe('idle')
  const done = apply(once, { type: 'Compacted', session: S, turn: null, epoch: 2, before: 1, after: 50, folded: 3, style: 'cut' }, ctx())
  expect(done.activity).toBe('idle')
  expect(done.entries.at(-1)).toMatchObject({ kind: 'compacted', epoch: 2, style: 'cut' })
  expect(settleCompacting(done)).toBe(done)
})

test('store: settleCompacting flips compacting back to idle and is a no-op otherwise', () => {
  const id = 'settle'
  const st = useSessions.getState()
  st.applyEvents([{ type: 'Compacting', session: id, turn: null }])
  expect(useSessions.getState().sessions[id]?.activity).toBe('compacting')
  st.settleCompacting(id)
  const idle = useSessions.getState().sessions[id]
  expect(idle?.activity).toBe('idle')
  st.settleCompacting(id)
  expect(useSessions.getState().sessions[id]).toBe(idle)
})

test('MessageDelivered interjects above a streaming tail without closing it', () => {
  const streaming = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }])
  const s = apply(streaming, { type: 'MessageDelivered', session: S, from: 'Scout', text: 'done' }, ctx())
  expect(s.entries.map((e) => e.kind)).toEqual(['peer', 'assistant'])
  expect(s.entries[1]).toMatchObject({ open: true })
})

test('PeerDelivered renders once from its origin; the wrapper echo and the replayed row are the same card', () => {
  const origin = { id: 'm1', from: 'session:abc', reply_to: null, scope: null }
  const wrapper = '<message from="forged" id="m1">\nhi\n</message>'
  const live = run([
    { type: 'PeerDelivered', session: S, origin, text: 'hi', seq: 7, turn: T },
    { type: 'PeerDelivered', session: S, origin, text: 'hi', seq: 7, turn: T },
    { type: 'TurnStarted', session: S, turn: T, text: wrapper, model: 'm', turn_seq: 7 },
  ])
  expect(live.entries).toHaveLength(1)
  expect(live.entries[0]).toMatchObject({ kind: 'peer', from: 'session:abc', text: 'hi', origin, seq: 7 })
  const replayed = fromHistory({ role: 'user', text: wrapper, turn: 7, context: msg(7), peer: origin })
  expect(replayed).toHaveLength(1)
  expect(replayed[0]).toMatchObject({ kind: 'peer', from: 'session:abc', text: 'hi', origin, context: msg(7) })
  // The settled reload keeps the live card's id (matched by exchange id, not by text or label).
  expect(adopt(live.entries, replayed)[0]?.id).toBe(live.entries[0]?.id)
  // A legacy wrapper without provenance is a label only.
  expect(fromHistory({ role: 'user', text: wrapper, turn: 7 })[0]).not.toHaveProperty('origin')
})

test('SubagentSpawned creates the child, adds a row to the parent and its ancestors, notes the spawn', () => {
  const draft: Record<string, SessionSlice> = { [S]: empty(S) }
  expect(route(draft, { type: 'SubagentSpawned', parent: S, child: 'kid', name: 'Scout', cap: 'read_only' }, ctx())).toBe(true)
  expect(draft.kid).toMatchObject({ name: 'Scout', parent: S })
  expect(draft[S]?.children.kid).toMatchObject({ name: 'Scout', depth: 1, state: { state: 'running' }, cap: 'read_only' })
  expect(draft[S]?.entries[0]).toMatchObject({ kind: 'info', text: '↳ spawned Scout' })
  route(draft, { type: 'SubagentSpawned', parent: 'kid', child: 'grandkid', name: 'Deep' }, ctx())
  expect(draft[S]?.children.grandkid?.depth).toBe(2)
  expect(draft.kid?.children.grandkid?.depth).toBe(1)
  route(draft, { type: 'TurnFinished', session: 'kid', turn: T, stop_reason: 'cancelled', usage, context: 1 }, ctx())
  expect(draft[S]?.children.kid?.state).toEqual({ state: 'cancelled' })
  expect(draft[S]?.entries.at(-1)).toMatchObject({ kind: 'info', text: '↳ Scout cancelled' })
})

test('93: a child\'s streaming events beat its row on every ancestor, throttled to 250 ms; the end of a turn and the child itself do not', () => {
  const draft: Record<string, SessionSlice> = { [S]: empty(S) }
  route(draft, { type: 'SubagentSpawned', parent: S, child: 'kid', name: 'Scout' }, ctx())
  route(draft, { type: 'SubagentSpawned', parent: 'kid', child: 'grandkid', name: 'Deep' }, ctx())
  expect(draft[S]?.children.kid).not.toHaveProperty('beat')
  const kid = draft.kid
  expect(route(draft, { type: 'TextDelta', session: 'kid', turn: T, text: 'a' }, ctx(1000))).toBe(true)
  expect(draft[S]?.children.kid?.beat).toBe(1000)
  // The child's own slice took the delta but no beat: beats live on the ancestors' rows only.
  expect(draft.kid).not.toBe(kid)
  expect(draft.kid).not.toHaveProperty('beat')
  // Throttle: 100 ms later is too soon, 300 ms later is a new beat.
  const parent = draft[S]
  route(draft, { type: 'TextDelta', session: 'kid', turn: T, text: 'b' }, ctx(1100))
  expect(draft[S]?.children.kid?.beat).toBe(1000)
  expect(draft[S]).toBe(parent)
  route(draft, { type: 'TextDelta', session: 'kid', turn: T, text: 'c' }, ctx(1300))
  expect(draft[S]?.children.kid?.beat).toBe(1300)
  // A grandchild's delta beats its row on both ancestors; the direct child's row is left alone.
  route(draft, { type: 'ReasoningDelta', session: 'grandkid', turn: T, text: 'x' }, ctx(2000))
  expect(draft[S]?.children.grandkid?.beat).toBe(2000)
  expect(draft.kid?.children.grandkid?.beat).toBe(2000)
  expect(draft[S]?.children.kid?.beat).toBe(1300)
  // The end of the turn patches the state but is no beat.
  route(draft, { type: 'TurnFinished', session: 'kid', turn: T, stop_reason: 'stop', usage, context: 1 }, ctx(5000))
  expect(draft[S]?.children.kid).toMatchObject({ state: { state: 'idle' }, beat: 1300 })
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
  s = apply(s, { type: 'JobStarted', session: S, id: 'j1', command: 'make', call_id: 'c1', log: '/l', name: null, pty: false }, ctx())
  s = apply(s, { type: 'JobOutput', session: S, id: 'j1', chunk: '10%\r55%\ndone' }, ctx())
  expect(s.entries[0]).toMatchObject({ job: { id: 'j1', lines: ['55%', 'done'] } })
  const inline = apply(s, { type: 'JobFinished', session: S, id: 'j1', command: 'make', exit: { exited: 0 }, secs: 2n, output: '', inline: true, pty: false }, ctx())
  expect(inline.entries[0]).not.toHaveProperty('job')
  const late = apply(s, { type: 'JobFinished', session: S, id: 'j1', command: 'make', exit: 'timeout', secs: 9n, output: '', inline: false, pty: false }, ctx())
  expect(late.entries[0]).toMatchObject({ job: { exit: { exit: 'timeout', secs: 9 } } })
  expect(apply(s, { type: 'JobOutput', session: S, id: 'j9', chunk: 'x' }, ctx())).toBe(s)
  // Only a PTY job lights the header's terminals icon, like the globe follows `browser_*`.
  expect(s.terminal).toBe(false)
  expect(apply(s, { type: 'JobStarted', session: S, id: 'j2', command: 'top', call_id: 'c9', log: '/l', name: null, pty: true }, ctx()).terminal).toBe(true)
})

test('live PTY set follows screen changes: idle or exit drops a job, a new screen re-adds it', () => {
  let s = run([{ type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'shell', args: { command: 'top' } }])
  expect(s.ptys.size).toBe(0)
  s = apply(s, { type: 'JobStarted', session: S, id: 'j1', command: 'top', call_id: 'c1', log: '/l', name: null, pty: true }, ctx())
  expect([...s.ptys]).toEqual(['j1'])
  const idle = { type: 'JobIdle', session: S, id: 'j1', command: 'top', name: null, quiet_secs: 60n, secs: 90n, nudge: 1, user_typed_secs: null, output: '' } as const
  s = apply(s, idle, ctx())
  expect(s.ptys.size).toBe(0)
  expect(apply(s, idle, ctx())).toBe(s)
  s = apply(s, { type: 'JobScreen', session: S, id: 'j1', rows: ['$ '], cursor: [0, 2] }, ctx())
  expect([...s.ptys]).toEqual(['j1'])
  s = apply(s, { type: 'JobFinished', session: S, id: 'j1', command: 'top', exit: { exited: 0 }, secs: 99n, output: '', inline: false, pty: true }, ctx())
  expect(s.ptys.size).toBe(0)
  // A plain job is a background job, not a terminal, until it finishes (inline or not).
  s = apply(s, { type: 'JobStarted', session: S, id: 'j2', command: 'make', call_id: 'c1', log: '/l', name: null, pty: false }, ctx())
  expect(s.ptys.size).toBe(0)
  expect([...s.jobs]).toEqual(['j2'])
  s = apply(s, { type: 'JobFinished', session: S, id: 'j2', command: 'make', exit: { exited: 0 }, secs: 1n, output: '', inline: true, pty: false }, ctx())
  expect(s.jobs.size).toBe(0)
})

test('LIVE-GLYPHS: reminders follow ReminderSet/Fired/Cancelled; an unknown id is a no-op returning the same slice', () => {
  let s = empty(S)
  expect(s.reminders.size).toBe(0)
  const set = (id: string) => ({ type: 'ReminderSet', session: S, id, message: 'm', set_at: 1n, fires_at: 2n, seconds: 1n }) as const
  s = apply(s, set('r1'), ctx())
  s = apply(s, set('r2'), ctx())
  expect([...s.reminders]).toEqual(['r1', 'r2'])
  expect(apply(s, set('r1'), ctx())).toBe(s)
  s = apply(s, { type: 'ReminderFired', session: S, id: 'r1' }, ctx())
  expect([...s.reminders]).toEqual(['r2'])
  s = apply(s, { type: 'ReminderCancelled', session: S, id: 'r2' }, ctx())
  expect(s.reminders.size).toBe(0)
  expect(apply(s, { type: 'ReminderFired', session: S, id: 'r9' }, ctx())).toBe(s)
  expect(apply(s, { type: 'ReminderCancelled', session: S, id: 'r9' }, ctx())).toBe(s)
  // No transcript row, no activity change: the fired card comes from the `<reminder>` input.
  expect(s.entries).toEqual([])
  expect(s.activity).toBe('idle')
})

test('LIVE-GLYPHS: seedLive installs the snapshot for a session with no slice and clears stale sets of sessions not in it', () => {
  const st = useSessions.getState()
  const stale = 'seed-stale'
  const fresh = 'seed-fresh'
  st.applyEvents([
    { type: 'JobStarted', session: stale, id: 'j1', command: 'make', call_id: 'c1', log: '/l', name: null, pty: false },
    { type: 'ReminderSet', session: stale, id: 'r1', message: 'm', set_at: 1n, fires_at: 2n, seconds: 1n },
  ])
  const before = useSessions.getState().sessions[stale]!
  expect([...before.jobs]).toEqual(['j1'])
  expect(useSessions.getState().sessions[fresh]).toBeUndefined()
  st.seedLive([{ session: fresh, jobs: ['j7'], ptys: ['j8'], reminders: ['r3'] }])
  const seeded = useSessions.getState().sessions[fresh]!
  expect([...seeded.jobs]).toEqual(['j7'])
  expect([...seeded.ptys]).toEqual(['j8'])
  expect([...seeded.reminders]).toEqual(['r3'])
  expect(seeded).toMatchObject({ loaded: false, entries: [] })
  const cleared = useSessions.getState().sessions[stale]!
  expect(cleared.jobs.size + cleared.ptys.size + cleared.reminders.size).toBe(0)
  expect(cleared.entries).toBe(before.entries)
  // A slice with nothing live and no row is left untouched.
  st.seedLive([])
  expect(useSessions.getState().sessions[stale]).toBe(cleared)
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

const src = (seq: number, label: string): VisionSource => ({
  target: { kind: 'binary', owner: { kind: 'message', seq }, part: 0, image: null },
  fingerprint: `fp${seq}`,
  label,
})

test('VisionProgress: questioning/inspecting set the status line; the terminal stage clears it and yields one vision card', () => {
  const sources = [src(3, 'a.png'), src(3, 'b.png')]
  const started = run([{ type: 'TurnStarted', session: S, turn: T, text: 'look', turn_seq: 3, model: 'm' }])
  const asking = apply(started, { type: 'VisionProgress', session: S, turn: T, stage: 'questioning', model: 'main/m', sources, seq: null }, ctx())
  expect(asking).toMatchObject({ activity: 'vision', visionStatus: 'asking main/m what to look for…' })
  const looking = apply(asking, { type: 'VisionProgress', session: S, turn: T, stage: 'inspecting', model: 'v/eyes', sources, seq: null }, ctx())
  expect(looking.visionStatus).toBe('v/eyes inspecting 2 images…')
  const done = apply(looking, { type: 'VisionProgress', session: S, turn: T, stage: 'completed', model: 'v/eyes', sources, seq: 4 }, ctx())
  expect(done).toMatchObject({ activity: 'thinking', visionStatus: undefined })
  expect(done.entries.map((e) => e.kind)).toEqual(['user', 'vision'])
  expect(done.entries[1]).toMatchObject({ kind: 'vision', model: 'v/eyes', sources, seq: 4, text: '' })
  expect(done.entries[1]).not.toHaveProperty('failed')
  const failed = apply(looking, { type: 'VisionProgress', session: S, turn: T, stage: 'failed', model: 'v/eyes', sources: [sources[0]!], seq: 5 }, ctx())
  expect(failed.entries[1]).toMatchObject({ kind: 'vision', failed: true, seq: 5 })
  expect(failed.activity).toBe('thinking')
})

test('fromHistory: a row with a vision note is a vision card carrying the note, never a user bubble; the live card adopts its id', () => {
  const note: VisionNote = {
    version: 1,
    question_model: 'main/m',
    vision_model: 'v/eyes',
    observations: [
      { source: src(3, 'a.png'), status: 'answered' },
      { source: src(3, 'b.png'), status: 'failed' },
    ],
  }
  const text = '<vision_observation>\nSources: …\n</vision_observation>'
  const cards = fromHistory({ role: 'user', text, turn: null, context: msg(4), vision: note })
  expect(cards).toHaveLength(1)
  expect(cards[0]).toMatchObject({ kind: 'vision', model: 'v/eyes', note, text, seq: 4, sources: note.observations.map((o) => o.source), context: msg(4) })
  // The same note on `context.vision` (the row's stored metadata) is enough.
  expect(fromHistory({ role: 'user', text, turn: null, context: { ...msg(4), vision: note } })[0]?.kind).toBe('vision')
  const live = run([{ type: 'VisionProgress', session: S, turn: T, stage: 'completed', model: 'v/eyes', sources: [], seq: 4 }])
  expect(adopt(live.entries, cards)[0]?.id).toBe(live.entries[0]?.id)
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

test('store: unmarkSent after a failed send drops the optimistic row and its echo skip; sent history rows stay', () => {
  const st = useSessions.getState()
  st.loadHistory(S, { ...loaded(), history: [{ role: 'user', text: 'same', turn: 1 }], first_seq: 1, more: false }, 'replace')
  st.markSent(S, 'same')
  expect(useSessions.getState().sessions[S]?.entries).toHaveLength(2)
  st.unmarkSent(S, 'same')
  const s = useSessions.getState().sessions[S]
  expect(s?.entries.map((e) => (e.kind === 'user' ? e.turn : e.kind))).toEqual([1])
  // The retry lands as a fresh echo, not a skipped one.
  st.applyEvents([{ type: 'TurnStarted', session: S, turn: T, text: 'same', model: 'm', turn_seq: 9 }])
  expect(useSessions.getState().sessions[S]?.entries.filter((e) => e.kind === 'user')).toHaveLength(2)
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

test('harness model (06 §8.2): the event sets it beside the harness name without touching the transcript; a load carries it', () => {
  const H = 'harness'
  let s = run([
    { type: 'HarnessModelChanged', session: H, model: 'anthropic/claude-opus-4-8' },
    { type: 'TurnStarted', session: H, turn: T, text: 'go', model: 'omp', turn_seq: 1 },
    { type: 'TextDelta', session: H, turn: T, text: 'Hel' },
  ])
  expect(s).toMatchObject({ model: 'omp', harnessModel: 'anthropic/claude-opus-4-8' })
  const same = apply(s, { type: 'HarnessModelChanged', session: H, model: 'anthropic/claude-opus-4-8' }, ctx())
  expect(same).toBe(s)
  const live = s.live
  s = apply(s, { type: 'HarnessModelChanged', session: H, model: 'openai/gpt-5.4' }, ctx())
  expect(s.harnessModel).toBe('openai/gpt-5.4')
  expect(s.live).toBe(live)
  // The streaming tail stays open: the next delta still appends to it.
  s = apply(s, { type: 'TextDelta', session: H, turn: T, text: 'lo' }, ctx())
  expect(s.entries.map((e) => (e.kind === 'assistant' ? e.text : e.kind))).toEqual(['user', 'Hello'])

  const st = useSessions.getState()
  st.loadHistory(H, { ...loaded(), agent: 'omp', harness_model: 'anthropic/claude-opus-4-8', history: [], first_seq: null, more: false }, 'replace')
  expect(useSessions.getState().sessions[H]).toMatchObject({ agent: 'omp', harnessModel: 'anthropic/claude-opus-4-8' })
  // A reload without the field (older server) keeps what the event said.
  st.applyEvents([{ type: 'HarnessModelChanged', session: H, model: 'openai/gpt-5.4' }])
  st.loadHistory(H, { ...loaded(), agent: 'omp', history: [], first_seq: null, more: false }, 'replace')
  expect(useSessions.getState().sessions[H]?.harnessModel).toBe('openai/gpt-5.4')
})

test('harness commands (75 §3): the manifest lands beside the harness model without touching the transcript; a load carries it and the window size', () => {
  const H = 'harness'
  const cmds = [
    { name: 'skill:ponytail', description: 'lazy code' },
    { name: '/ponytail', description: 'switch level', hint: 'lite|full|ultra' },
  ]
  let s = run([
    { type: 'TurnStarted', session: H, turn: T, text: 'go', model: 'omp', turn_seq: 1 },
    { type: 'TextDelta', session: H, turn: T, text: 'Hel' },
  ])
  const live = s.live
  s = apply(s, { type: 'HarnessCommandsChanged', session: H, commands: cmds }, ctx())
  expect(s.harnessCommands).toEqual(cmds)
  expect(s.live).toBe(live)
  s = apply(s, { type: 'TextDelta', session: H, turn: T, text: 'lo' }, ctx())
  expect(s.entries.map((e) => (e.kind === 'assistant' ? e.text : e.kind))).toEqual(['user', 'Hello'])

  // `size` is the harness's window; a native round (no size) keeps the last one.
  s = apply(s, { type: 'ContextUsed', session: H, turn: T, context: 1200, size: 200_000 }, ctx())
  expect(s).toMatchObject({ context: 1200, contextSize: 200_000 })
  s = apply(s, { type: 'ContextUsed', session: H, turn: T, context: 1300 }, ctx())
  expect(s).toMatchObject({ context: 1300, contextSize: 200_000 })

  const st = useSessions.getState()
  st.loadHistory(H, { ...loaded(), agent: 'omp', harness_commands: cmds, context_size: 128_000, history: [], first_seq: null, more: false }, 'replace')
  expect(useSessions.getState().sessions[H]).toMatchObject({ harnessCommands: cmds, contextSize: 128_000 })
  // A reload without the size (older server) keeps what the event said.
  st.loadHistory(H, { ...loaded(), agent: 'omp', harness_commands: [], history: [], first_seq: null, more: false }, 'replace')
  expect(useSessions.getState().sessions[H]).toMatchObject({ harnessCommands: [], contextSize: 128_000 })
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
  const s3 = apply(s2, { type: 'InputQueued', session: S, id: 'q', text: 'later', defer: true }, ctx())
  const s4 = apply(s3, { type: 'TitleChanged', session: S, title: 't', tags: ['rust'] }, ctx())
  const s5 = apply(s4, { type: 'JobOutput', session: S, id: 'j', chunk: 'out' }, ctx())
  expect(s5.live).toBe(2)
  expect(apply(s5, { type: 'MessageDelivered', session: S, from: 'p', text: 'm' }, ctx()).live).toBe(3)
  const done = apply(s5, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 42 }, ctx())
  expect(done).toMatchObject({ context: 42, contextEstimated: false, running: false })
})

test('ContextUsed moves the gauge mid-turn without ending the turn or bumping live; the turn end still lands', () => {
  const s1 = apply(empty(S), { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 }, ctx())
  const s2 = apply({ ...s1, contextEstimated: true }, { type: 'ContextUsed', session: S, turn: T, context: 1200 }, ctx())
  expect(s2).toMatchObject({ context: 1200, contextEstimated: false, running: true, live: s1.live })
  const s3 = apply(s2, { type: 'ContextUsed', session: S, turn: T, context: 3400 }, ctx())
  expect(s3.context).toBe(3400)
  expect(apply(s3, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 3400 }, ctx())).toMatchObject({ context: 3400, running: false })
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

test('store: an access error mid-turn outlives the turn end and the settled reload as one row', () => {
  const id = 'sso'
  const st = useSessions.getState()
  // A fresh session as the composer seeds it (`FRESH`).
  st.loadHistory(id, { ...loaded('0'), history: [], first_seq: null, more: false }, 'replace')
  st.markSent(id, 'hello')
  const message = 'provider: SSO session expired; run `aws sso login`'
  st.applyEvents([
    { type: 'TurnStarted', session: id, turn: T, text: 'hello', model: 'm', turn_seq: 1 },
    { type: 'Error', session: id, turn: T, message },
    { type: 'TurnFinished', session: id, turn: T, stop_reason: 'error', usage, context: 3 },
    { type: 'ContextChanged', session: id, revision: '1', reason: 'settled' },
  ])
  let s = useSessions.getState().sessions[id]!
  expect(s.entries.map((e) => ('text' in e ? [e.kind, e.text] : e.kind))).toEqual([['user', 'hello'], ['error', message]])
  expect(s).toMatchObject({ running: false, waiting: false, activity: 'idle', contextStale: true })
  const errorId = s.entries[1]!.id
  // The authoritative page carries the stored row: same card, same id, still one error.
  const page = {
    ...loaded('1'),
    history: [
      { role: 'user' as const, text: 'hello', turn: 1, context: msg(1) },
      { role: 'error' as const, text: message },
    ],
    first_seq: 1,
    more: false,
  }
  expect(st.loadHistory(id, page, 'settled', freshness(s))).toBe(true)
  s = useSessions.getState().sessions[id]!
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'error'])
  expect(s.entries[1]).toMatchObject({ id: errorId, kind: 'error', text: message })
  expect(s).toMatchObject({ running: false, activity: 'idle', contextStale: false })
  // Reopened later: the stored row renders as the same error line, never as assistant text.
  expect(fromHistory({ role: 'error', text: message })).toMatchObject([{ kind: 'error', text: message }])
})

test('store: unload invalidates loaded slices for the reconnect/engine-switch reload; a superseded full page is dropped', () => {
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

test('store: todosShown is per session, hidden by default, and a repeated value is a no-op', () => {
  const a = 'shown-a'
  const b = 'shown-b'
  expect(empty(a).todosShown).toBe(false)
  const st = useSessions.getState()
  st.applyEvents([
    { type: 'TodoChanged', session: a, snapshot: plan(1, 'a1') },
    { type: 'TodoChanged', session: b, snapshot: plan(1, 'b1') },
  ])
  st.setTodosShown(a, true)
  const after = useSessions.getState()
  expect(after.sessions[a]).toMatchObject({ todosShown: true, todosExpanded: false })
  expect(after.sessions[b]).toMatchObject({ todosShown: false })
  st.setTodosShown(a, true)
  expect(useSessions.getState()).toBe(after)
})

test('AppCallChanged keeps the newest status per statement of each record, bounded to the newest records, without touching the tail', () => {
  const call = (record: string, statement: string, status: 'running' | 'succeeded', ms: number): WireEvent => ({
    type: 'AppCallChanged',
    session: S,
    record_call_id: record,
    request_id: `${record}-${statement}-${status}`,
    statement_id: statement,
    tool: 'argo_sync',
    risk: 'mutating',
    trigger: 'press',
    status,
    ms,
  })
  let s = run([{ type: 'TextDelta', session: S, turn: T, text: 'a' }, call('r1', 'sync', 'running', 0), call('r1', 'sync', 'succeeded', 340), call('r1', 'other', 'succeeded', 5)])
  expect(s.entries).toMatchObject([{ kind: 'assistant', open: true }])
  expect(s.appCalls.get('r1')?.get('sync')).toMatchObject({ status: 'succeeded', ms: 340 })
  expect(s.appCalls.get('r1')?.size).toBe(2)
  for (let i = 0; i < 32; i++) s = apply(s, call(`x${i}`, 'q', 'succeeded', i), ctx())
  expect(s.appCalls.size).toBe(32)
  expect(s.appCalls.has('r1')).toBe(false)
  expect(s.appCalls.has('x31')).toBe(true)
})

const codeEvents = (codemapEvents as { type: string }[]).filter((e): e is CodeToolRan => e.type === 'CodeToolRan')
const ROOT = '/Users/sky/Develop/theuth'

test('CodeToolRan: roots ∪= root, last is the newest call, trail newest-first with a target per tool, capped at 20', () => {
  let s = run(codeEvents)
  expect(s.codemap.roots).toEqual([ROOT])
  expect(s.codemap.last).toMatchObject({ root: ROOT, tool: 'code_impact', callId: 'toolu_01C', seq: 42 })
  expect(s.codemap.trail.map((t) => [t.tool, t.callId, t.target])).toEqual([
    ['code_impact', 'toolu_01C', '9532e7ff..176bf52'],
    ['code_refs', 'toolu_01B', 'crates/tools/src/registry.rs#L1366'],
    ['code_map', 'toolu_01A', 'crates/tools'],
  ])
  for (let i = 0; i < 22; i++) s = apply(s, { ...codeEvents[0]!, call_id: `x${i}`, seq: 100 + i }, ctx())
  expect(s.codemap.trail).toHaveLength(20)
  expect(s.codemap.trail[0]).toMatchObject({ callId: 'x21', seq: 121 })
  expect(s.codemap.roots).toEqual([ROOT])
})

test('ToolCallStarted edit/write_file appends the root-relative path under its root; a path outside every root with no root is ignored', () => {
  let s = run([codeEvents[0]!])
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'e1', name: 'edit', args: { path: `${ROOT}/crates/tools/src/registry.rs`, old: 'a', new: 'b' } }, ctx())
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'e2', name: 'write_file', args: { path: 'crates/desktop/ui/src/x.ts', content: '' } }, ctx())
  s = apply(s, { type: 'ToolCallStarted', session: S, turn: T, call_id: 'e3', name: 'read', args: { path: 'crates/other.rs' } }, ctx())
  expect(s.codemap.edited).toEqual({ [ROOT]: ['crates/tools/src/registry.rs', 'crates/desktop/ui/src/x.ts'] })
  const none = run([{ type: 'ToolCallStarted', session: S, turn: T, call_id: 'e4', name: 'write_file', args: { path: '/elsewhere/z.rs', content: '' } }])
  expect(none.codemap.edited).toEqual({})
})

test('109 §8.2: the banner follows TurnResumed → ResumeReport, the next turn clears it; a lost job keeps its exit', () => {
  const at = '2026-10-06T12:05:00Z'
  let s = run([{ type: 'TurnResumed', session: S, turn: T, kind: 'interrupted', interrupted_at: at, resumed_at: at, partial_dropped: true, cause: 'crash' }])
  expect(s.resume).toEqual({ turn: T, kind: 'interrupted', at: Date.parse(at), resumed: true, report: undefined, here: true, cause: 'crash' })
  s = apply(s, { type: 'ResumeReport', session: S, turn: T, replayed: [['c1', 'read_file'], ['c2', 'grep']], interrupted: [['c3', 'git push']], lost_jobs: ['j1'], children_restored: 0, children_handed_off: 0 }, ctx())
  expect(s.resume?.report).toEqual({ replayed: 2, skipped: ['git push'], lostJobs: 1 })
  // The resumed turn's own events keep the banner; a different turn drops it.
  expect(apply(s, { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 }, ctx()).resume).toBe(s.resume)
  expect(apply(s, { type: 'TurnStarted', session: S, turn: 'next', text: 'x', model: 'm', turn_seq: 2 }, ctx()).resume).toBeUndefined()
  const suspended = apply(s, { type: 'TurnSuspended', session: S, turn: 'u', reason: 'update' }, ctx())
  expect(suspended.resume).toMatchObject({ turn: 'u', kind: 'suspended', resumed: false })

  let j = run([{ type: 'ToolCallStarted', session: S, turn: T, call_id: 'c1', name: 'shell', args: { command: 'make' } }])
  j = apply(j, { type: 'JobStarted', session: S, id: 'j1', command: 'make', call_id: 'c1', log: '/l', name: null, pty: false }, ctx())
  j = apply(j, { type: 'JobFinished', session: S, id: 'j1', command: 'make', exit: 'lost', secs: 4n, output: '', inline: false, pty: false }, ctx())
  expect(j.entries[0]).toMatchObject({ job: { exit: { exit: 'lost', secs: 4 } } })
  expect(j.jobs.size).toBe(0)
})

test('139 §9.3: the banner carries owner/here/cause/from — from Loaded.resume, TurnResumed, TurnSuspended{release}, ResumePending, SessionOwnerChanged', () => {
  const at = '2026-10-06T12:05:00Z'
  // A replica's row: who runs it; a pre-139 row omits `here` and reads as owned here.
  const theirs = { turn: T, kind: 'interrupted' as const, interrupted_at: at, resumed: false, owner: 'A', here: false, cause: 'crash' as const }
  expect(resumeFromLoaded(undefined, theirs)).toEqual({ turn: T, kind: 'interrupted', at: Date.parse(at), resumed: false, owner: 'A', here: false, cause: 'crash' })
  expect(resumeFromLoaded(undefined, { turn: T, kind: 'interrupted', interrupted_at: at, resumed: false } as never)).toMatchObject({ here: true })
  // The same turn, loaded again after ownership moved: the row's owner/here win over what we had.
  const held = { turn: T, kind: 'interrupted' as const, at: Date.parse(at), resumed: false, here: true }
  expect(resumeFromLoaded(held, theirs)).toEqual({ ...held, owner: 'A', here: false })
  expect(resumeFromLoaded(held, { ...theirs, owner: undefined, here: true })).toBe(held)

  // TurnResumed on the owner: cause + from.
  const resumed = run([{ type: 'TurnResumed', session: S, turn: T, kind: 'suspended', interrupted_at: at, resumed_at: at, partial_dropped: false, cause: 'transfer', from: 'A' }])
  expect(resumed.resume).toMatchObject({ kind: 'suspended', resumed: true, here: true, cause: 'transfer', from: 'A' })
  // Relayed to a replica's clients (the follower re-publishes the owner's events): the view stays "not here".
  let replica: SessionSlice = { ...empty(S), resume: resumeFromLoaded(undefined, theirs) }
  replica = apply(replica, { type: 'TurnResumed', session: S, turn: T, kind: 'interrupted', interrupted_at: at, resumed_at: at, partial_dropped: false, cause: 'crash' }, ctx())
  expect(replica.resume).toMatchObject({ resumed: true, here: false, owner: 'A' })
  // The old owner's side of a transfer: TurnSuspended{release} → cause transfer; B's row lands → not here, owner B.
  let a = run([{ type: 'TurnSuspended', session: S, turn: T, reason: 'release' }])
  expect(a.resume).toMatchObject({ kind: 'suspended', resumed: false, here: true, cause: 'transfer' })
  a = apply(a, { type: 'SessionOwnerChanged', session: S, owner: 'B', epoch: 2, here: false }, ctx())
  expect(a.resume).toMatchObject({ here: false, owner: 'B', cause: 'transfer' })
  // Taking it back: here again, no foreign owner.
  expect(apply(a, { type: 'SessionOwnerChanged', session: S, owner: 'me', epoch: 3, here: true }, ctx()).resume).toMatchObject({ here: true, owner: undefined })
  expect(apply(empty(S), { type: 'SessionOwnerChanged', session: S, owner: 'B', epoch: 2, here: false }, ctx()).resume).toBeUndefined()
  expect(apply(run([{ type: 'TurnSuspended', session: S, turn: T, reason: 'update' }]), { type: 'TurnStarted', session: S, turn: 'n', text: 'x', model: 'm', turn_seq: 1 }, ctx()).resume).toBeUndefined()
  expect(run([{ type: 'TurnSuspended', session: S, turn: T, reason: 'update' }]).resume?.cause).toBeUndefined()
  // A system takeover (D5): the turn waits for the user's Resume.
  const pending = run([{ type: 'ResumePending', session: S, turn: T, from: 'A' }])
  expect(pending.resume).toMatchObject({ kind: 'interrupted', resumed: false, here: true, cause: 'takeover', from: 'A' })
  const after = apply(pending, { type: 'TurnResumed', session: S, turn: T, kind: 'interrupted', interrupted_at: at, resumed_at: at, partial_dropped: false, cause: 'takeover', from: 'A' }, ctx())
  expect(after.resume).toMatchObject({ resumed: true, cause: 'takeover', from: 'A', here: true })
})

test('109 LIVE: a child\u2019s TurnResumed shows it running (own slice and the parent\u2019s row, no chat line) until TurnFinished', () => {
  const at = '2026-10-06T12:05:00Z'
  const draft: Record<string, SessionSlice> = { [S]: empty(S) }
  route(draft, { type: 'SubagentSpawned', parent: S, child: 'kid', name: 'Scout' }, ctx())
  route(draft, { type: 'TurnFinished', session: 'kid', turn: 'old', stop_reason: 'stop', usage, context: 1 }, ctx())
  expect(draft[S]?.children.kid).toMatchObject({ state: { state: 'idle' }, started_at: null })
  const lines = draft[S]?.entries.length
  expect(route(draft, { type: 'TurnResumed', session: 'kid', turn: T, kind: 'interrupted', interrupted_at: at, resumed_at: at, partial_dropped: false, cause: 'crash' }, ctx())).toBe(true)
  expect(draft[S]?.children.kid?.state).toEqual({ state: 'running' })
  expect(draft[S]?.children.kid?.started_at).toEqual(expect.any(String))
  expect(draft[S]?.entries.length).toBe(lines)
  expect(draft.kid).toMatchObject({ running: true, waiting: true, activity: 'thinking' })
  const kid = { id: 'kid', running: false } as SessionSummary
  expect(runningSubagents([kid], draft)).toBe(1)
  route(draft, { type: 'TurnFinished', session: 'kid', turn: T, stop_reason: 'stop', usage, context: 1 }, ctx())
  expect(draft[S]?.children.kid).toMatchObject({ state: { state: 'idle' }, started_at: null })
  expect(draft.kid?.running).toBe(false)
  expect(runningSubagents([kid], draft)).toBe(0)
})

test('109 §8.2: the interrupted partial loads greyed and the regenerated reply\u2019s first delta replaces it', () => {
  const st = useSessions.getState()
  const at = '2026-10-06T12:05:00Z'
  st.loadHistory(
    S,
    {
      ...loaded(),
      history: [
        { role: 'user', text: 'hi', turn: 1, context: msg(1) },
        { role: 'interrupted', text: 'half a re', at },
      ],
      first_seq: 1,
      more: false,
      resume: { turn: T, kind: 'interrupted', interrupted_at: at, resumed: false, here: true },
    },
    'replace',
  )
  let s = useSessions.getState().sessions[S] as SessionSlice
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'interrupted'])
  expect(s.resume).toEqual({ turn: T, kind: 'interrupted', at: Date.parse(at), resumed: false, here: true })
  s = apply(s, { type: 'TurnResumed', session: S, turn: T, kind: 'interrupted', interrupted_at: at, resumed_at: at, partial_dropped: true, cause: 'crash' }, ctx())
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'interrupted'])
  s = apply(s, { type: 'TextDelta', session: S, turn: T, text: 'half a reply' }, ctx())
  expect(s.entries.map((e) => e.kind)).toEqual(['user', 'assistant'])
  // A live state already seen for the same turn outranks the row; another open turn copies the row's flag; no row keeps what we have.
  const row = { turn: T, kind: 'interrupted' as const, interrupted_at: at, resumed: false, here: true }
  expect(resumeFromLoaded(s.resume, row)).toBe(s.resume)
  expect(resumeFromLoaded(s.resume, { ...row, turn: 'other' })).toMatchObject({ turn: 'other', resumed: false })
  expect(resumeFromLoaded(s.resume, { ...row, turn: 'other', resumed: true })).toMatchObject({ turn: 'other', resumed: true })
  expect(resumeFromLoaded(s.resume, undefined)).toBe(s.resume)
})

test('109 BANNER: a banner still "resuming…" flips to resumed when the row says so or the open turn is gone', () => {
  const at = '2026-10-06T12:05:00Z'
  const waiting = { turn: T, kind: 'interrupted' as const, at: Date.parse(at), resumed: false, here: true }
  const row = { turn: T, kind: 'interrupted' as const, interrupted_at: at, resumed: false, here: true }
  // Ask mode, still waiting: nothing changes.
  expect(resumeFromLoaded(waiting, row)).toBe(waiting)
  // The engine relaunched it before this client subscribed (the live TurnResumed was missed).
  expect(resumeFromLoaded(waiting, { ...row, resumed: true })).toEqual({ ...waiting, resumed: true })
  // The turn finished: no open turn in the row, so it ran.
  expect(resumeFromLoaded(waiting, undefined)).toEqual({ ...waiting, resumed: true })
  expect(resumeFromLoaded({ ...waiting, kind: 'suspended' }, undefined)).toEqual({ ...waiting, kind: 'suspended', resumed: true })
  // Already resumed (with its report): kept as is until the next TurnStarted clears it.
  const done = { ...waiting, resumed: true, report: { replayed: 1, skipped: [], lostJobs: 0 } }
  expect(resumeFromLoaded(done, undefined)).toBe(done)
  expect(resumeFromLoaded(done, row)).toBe(done)
  expect(resumeFromLoaded(undefined, undefined)).toBeUndefined()
})

test('109 REPORT: a Loaded.resume row with a report fills the banner counts; a live report for the turn wins', () => {
  const at = '2026-10-06T12:05:00Z'
  const report = { replayed: [['c1', 'read_file'], ['c2', 'grep']] as [string, string][], interrupted: [['c3', 'git push']] as [string, string][], lost_jobs: ['j1'], children_restored: 0, children_handed_off: 0 }
  const row = { turn: T, kind: 'interrupted' as const, interrupted_at: at, resumed: true, report, here: true }
  const counts = { replayed: 2, skipped: ['git push'], lostJobs: 1 }
  // A desktop that connected after the engine resumed: no banner yet, the row brings everything.
  expect(resumeFromLoaded(undefined, row)).toEqual({ turn: T, kind: 'interrupted', at: Date.parse(at), resumed: true, report: counts, here: true })
  // Still "resuming…" for the same turn: flips and takes the persisted counts.
  const waiting = { turn: T, kind: 'interrupted' as const, at: Date.parse(at), resumed: false, here: true }
  expect(resumeFromLoaded(waiting, row)).toEqual({ ...waiting, resumed: true, report: counts })
  // A live report already held for the turn outranks the row's.
  const live = { ...waiting, resumed: true, report: { replayed: 9, skipped: [], lostJobs: 0 } }
  expect(resumeFromLoaded(live, row)).toBe(live)
  // A row without a report keeps the live one.
  expect(resumeFromLoaded(live, { ...row, report: undefined })).toBe(live)
  // Another turn's row replaces banner and counts.
  expect(resumeFromLoaded(live, { ...row, turn: 'other' })).toEqual({ turn: 'other', kind: 'interrupted', at: Date.parse(at), resumed: true, report: counts, here: true })
  // The next turn still clears it.
  const s = { ...empty(S), resume: resumeFromLoaded(undefined, row) }
  expect(apply(s, { type: 'TurnStarted', session: S, turn: T, text: 'x', model: 'm', turn_seq: 1 }, ctx()).resume).toBe(s.resume)
  expect(apply(s, { type: 'TurnStarted', session: S, turn: 'next', text: 'x', model: 'm', turn_seq: 2 }, ctx()).resume).toBeUndefined()
})
