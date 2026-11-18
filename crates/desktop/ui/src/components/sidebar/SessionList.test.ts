import { receiveComponent } from '@/rpc/bindings/receive'
import { expect, test } from 'vitest'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { createSessions, type SessionSlice } from '@/store/sessions'
import { empty } from '@/store/reduce'
import { activeRoots, rowActivity, runningSubagents } from './SessionItem'
import { buildRows, listOffset, newChatLabel, type Row, watchListOffset } from './SessionList'

const s = (id: string, project: string | null, updated_at: string, pinned = false, archived = false): SessionSummary => (receiveComponent("method.agent.output.SessionSummary", {
  id,
  created_at: updated_at,
  updated_at,
  title: id,
  tags: [],
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned,
  archived,
  project,
  notebook: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  memory: 'epochs',
  here: true,
}))

const keys = (rows: Row[]) => rows.map((r) => r.key)

test('pinned lead above every project; projects in first-seen order of the newest ordering, Ungrouped last, headers only when a project exists', () => {
  const sessions = [
    s('a', null, '2026-01-03T00:00:00Z'),
    s('b', 'beta', '2026-01-02T00:00:00Z'),
    s('c', 'alpha', '2026-01-01T00:00:00Z', true),
    s('d', 'beta', '2026-01-04T00:00:00Z'),
  ]
  const rows = buildRows(sessions, [])
  expect(keys(rows).slice(0, 2)).toEqual(['h:pinned', 'c'])
  expect(rows.flatMap((r) => ('project' in r ? [r.project] : []))).toEqual(['beta', null])
  expect(keys(rows).filter((k) => !k.startsWith('h:'))).toEqual(['c', 'p:beta', 'd', 'b', 'p:', 'a'])
  expect(keys(buildRows(sessions, [], new Set(['beta'])))).not.toContain('d')
  expect(keys(buildRows([sessions[0]!], [])).some((k) => k.startsWith('p:'))).toBe(false)
  expect(keys(buildRows([sessions[2]!], []))).toEqual(['h:pinned', 'c'])
})

test('a desk session never appears in the chat list — pinned, archived, in a project or ungrouped — and neither does its tree, even with a subagent open', () => {
  const desk = (id: string, project: string | null, pinned: boolean, archived = false) => ({ ...s(id, project, '2026-01-05T00:00:00Z', pinned, archived), desk: 'theuth' })
  const sessions = [desk('d1', null, true), desk('d2', 'beta', false), desk('d3', null, false, true), s('a', 'beta', '2026-01-01T00:00:00Z')]
  const rows = buildRows(sessions, ['d1'], new Set(), true)
  expect(keys(rows).filter((k) => !k.startsWith('h:'))).toEqual(['p:beta', 'a'])
  expect(keys(buildRows(sessions.slice(0, 3), [], new Set(), true))).toEqual([])
  const child = { ...s('d1c', null, '2026-01-06T00:00:00Z'), parent: 'd1' }
  const grandchild = { ...s('d1cc', null, '2026-01-06T00:00:00Z'), parent: 'd1c' }
  expect(keys(buildRows([...sessions, child, grandchild], ['d1cc'], new Set(), true)).filter((k) => !k.startsWith('h:'))).toEqual(['p:beta', 'a'])
})

test('an autoresearch study session (`autoresearch-<slug>`) and its wave children never appear in the chat list, even when open (140 §13 Revision 2)', () => {
  const study = { ...s('st', null, '2026-01-05T00:00:00Z'), title: 'autoresearch-sort-a3' }
  const child = { ...s('stc', null, '2026-01-06T00:00:00Z'), parent: 'st' }
  const sessions = [study, child, s('a', null, '2026-01-01T00:00:00Z'), { ...s('b', null, '2026-01-02T00:00:00Z'), title: 'autoresearch plans' }]
  const rows = buildRows(sessions, ['stc'], new Set(), true)
  expect(keys(rows).filter((k) => !k.startsWith('h:'))).toEqual(['b', 'a'])
})

test('archived roots leave the groups for one collapsed Archived header at the bottom; open, they list newest first with their open tree; no orphans', () => {
  const sessions = [
    s('a', null, '2026-01-03T00:00:00Z'),
    s('b', 'beta', '2026-01-02T00:00:00Z', true, true),
    s('c', 'alpha', '2026-01-04T00:00:00Z', false, true),
    { ...s('c1', null, '2026-01-05T00:00:00Z', false, true), parent: 'c' },
    s('d', 'beta', '2026-01-01T00:00:00Z'),
  ]
  const closed = keys(buildRows(sessions, []))
  expect(closed).toEqual(['p:beta', 'h:beta:Older', 'd', 'p:', 'h::Older', 'a', 'p:archived'])
  expect(buildRows(sessions, []).at(-1)).toEqual({ key: 'p:archived', archived: 2, collapsed: true })
  expect(keys(buildRows(sessions, ['c1'], new Set(), true))).toEqual([
    'p:beta', 'h:beta:Older', 'd', 'p:', 'h::Older', 'a', 'p:archived', 'c', 't:c', 'b',
  ])
  expect(keys(buildRows([sessions[0]!], [], new Set(), true))).not.toContain('p:archived')
})

test('project header menu label names the project; the null bucket is the plain new chat', () => {
  expect(newChatLabel('alpha')).toBe('New chat in “alpha”')
  expect(newChatLabel(null)).toBe('New chat (no project)')
})

test('running subagents: a live slice counts, so does the list snapshot for a child this client has no slice for; idle ones do not', () => {
  const d = (id: string, running: boolean) => ({ ...s(id, null, '2026-01-01T00:00:00Z'), parent: 'a', running })
  const slice = (running: boolean) => ({ running }) as SessionSlice
  const subs = [d('live', false), d('cold', true), d('idle', false), d('none', false)]
  expect(runningSubagents(subs, { live: slice(true), idle: slice(false) })).toBe(2)
  expect(runningSubagents(subs, { live: slice(false), idle: slice(false) })).toBe(1)
  expect(runningSubagents([], {})).toBe(0)
})

// 141 §9.4: a watch-only child has no turn of its own — the harness's state is its live bit, live from the slice, else the snapshot.
test('running subagents: a watch-only child counts while the harness says running or waiting on you, not when idle or finished', () => {
  const meta = (state: 'running' | 'requires_action' | 'idle' | 'completed') => receiveComponent('method.agent.output.ObservedMeta', { source: 'opencode_ext', external: 'x', state, can_cancel: false, cost_rule: 'unreported', detail: null })
  const d = (id: string, state: 'running' | 'requires_action' | 'idle' | 'completed') => ({ ...s(id, null, '2026-01-01T00:00:00Z'), parent: 'a', running: false, kind: 'observed' as const, observed: meta(state) })
  const subs = [d('run', 'running'), d('ask', 'requires_action'), d('idle', 'idle'), d('done', 'completed')]
  expect(runningSubagents(subs, {})).toBe(2)
  // The slice's live state outranks the snapshot: `run` finished since the list was read; `idle` was resumed.
  const slice = (state: 'running' | 'completed') => ({ running: false, observed: meta(state) }) as SessionSlice
  expect(runningSubagents(subs, { run: slice('completed'), idle: slice('running') })).toBe(2)
})

test('the virtualizer’s scrollMargin is the height of the content above the list, in scroll coordinates, and follows it as it grows', () => {
  // A scroller at viewport y=100 holding a menu of `above` px, then the list.
  let above = 200
  let scrollTop = 0
  const box = (top: () => number) => ({ getBoundingClientRect: () => ({ top: top() }) })
  const menu = box(() => 100 - scrollTop) as unknown as Element
  const list = box(() => 100 + above - scrollTop) as unknown as Element & { getBoundingClientRect(): { top: number } }
  const scroller = { ...box(() => 100), get scrollTop() { return scrollTop }, children: [menu, list] }
  // Scrolled or not, the offset is where the list starts in the scroller's own coordinates.
  expect(listOffset(scroller, list)).toBe(200)
  scrollTop = 450
  expect(listOffset(scroller, list)).toBe(200)
  // The observer watches only what is above the list — the list's own height changes with every row measured and
  // must not feed back — and reports again when that content grows (agents resolving into Desks).
  const observed: Element[] = []
  let fire = () => {}
  let disconnected = false
  class RO {
    constructor(cb: () => void) { fire = cb }
    observe(el: Element) { observed.push(el) }
    unobserve() {}
    disconnect() { disconnected = true }
  }
  const seen: number[] = []
  const stop = watchListOffset(scroller, list, (o) => seen.push(o), RO as unknown as typeof ResizeObserver)
  expect(observed).toEqual([menu])
  expect(seen).toEqual([200])
  above = 520
  fire()
  expect(seen).toEqual([200, 520])
  stop()
  expect(disconnected).toBe(true)
})

test('183 W-7: a row owned elsewhere shows its running glyph from list_sessions.running; a local row from its own activity', () => {
  // Local (`remoteRunning` undefined): the slice's activity as is.
  expect(rowActivity('streaming')).toBe('streaming')
  expect(rowActivity('idle')).toBe('idle')
  // Remote: only `running` can light the glyph; the slice's activity (fed while focused) refines which one.
  expect(rowActivity('idle', true)).toBe('thinking')
  expect(rowActivity('tool', true)).toBe('tool')
  expect(rowActivity('tool', false)).toBe('idle')
  expect(rowActivity('idle', false)).toBe('idle')
})

// ACTIVE-TOP: a root with anything going on leaves its usual place (pinned, a collapsed project, Archived) for the `Active` header on top.
test('active roots lead under one Active header, out of pinned, collapsed projects and Archived, never twice; idle again they return', () => {
  const sessions = [
    s('a', null, '2026-01-03T00:00:00Z'),
    s('pin', 'beta', '2026-01-02T00:00:00Z', true),
    s('col', 'beta', '2026-01-05T00:00:00Z'),
    s('arc', 'alpha', '2026-01-04T00:00:00Z', false, true),
    s('d', 'alpha', '2026-01-01T00:00:00Z'),
  ]
  const active = new Set(['pin', 'col', 'arc'])
  const rows = keys(buildRows(sessions, ['col'], new Set(['beta']), false, active))
  expect(rows.slice(0, 5)).toEqual(['h:active', 'col', 't:col', 'arc', 'pin'])
  expect(rows.filter((k) => ['pin', 'col', 'arc'].includes(k))).toHaveLength(3)
  expect(rows).not.toContain('h:pinned')
  expect(rows).not.toContain('p:archived')
  expect(keys(buildRows(sessions, [], new Set(), false, new Set()))).toEqual(keys(buildRows(sessions, [])))
  expect(keys(buildRows(sessions, [], new Set(), false, new Set(['a']))).slice(0, 3)).toEqual(['h:active', 'a', 'h:pinned'])
})

test('desks, advisors and study sessions never enter Active, even when their ids are in the set', () => {
  const sessions = [
    { ...s('desk', null, '2026-01-05T00:00:00Z'), desk: 'theuth' },
    { ...s('adv', null, '2026-01-05T00:00:00Z'), kind: 'advisor' as const },
    { ...s('st', null, '2026-01-05T00:00:00Z'), title: 'autoresearch-x' },
    s('a', null, '2026-01-01T00:00:00Z'),
  ]
  expect(keys(buildRows(sessions, [], new Set(), true, new Set(['desk', 'adv', 'st']))).filter((k) => !k.startsWith('h:'))).toEqual(['a'])
})

test('activeRoots: own turn (here or remote, slice or snapshot), a job, an open terminal (quiet too), a reminder, a prompt, or any of those on a descendant', () => {
  const root = (id: string, p: Partial<SessionSummary> = {}) => ({ ...s(id, null, '2026-01-01T00:00:00Z'), ...p })
  const kid = (id: string, parent: string, p: Partial<SessionSummary> = {}) => ({ ...root(id, p), parent })
  const slice = (id: string, p: Partial<SessionSlice>) => ({ ...empty(id), ...p })
  const roots = ['idle', 'turn', 'cold', 'remote', 'stale', 'job', 'pty', 'quiet', 'rem', 'ask', 'sub', 'subjob', 'subquiet', 'deep', 'subask'].map((id) =>
    root(id, { running: id === 'cold' || id === 'remote', here: id !== 'remote' && id !== 'stale' }),
  )
  const subs = new Map([
    ['sub', [kid('sub1', 'sub', { running: true })]],
    ['subjob', [kid('sj1', 'subjob')]],
    ['subquiet', [kid('sq1', 'subquiet')]],
    ['deep', [kid('d1', 'deep'), kid('d2', 'd1')]],
    ['subask', [kid('sa1', 'subask')]],
    ['idle', [kid('i1', 'idle')]],
  ])
  const slices = {
    turn: slice('turn', { running: true, activity: 'tool' }),
    // A remote row's slice activity alone does not light it: `list_sessions.running` gates.
    stale: slice('stale', { activity: 'streaming' }),
    job: slice('job', { jobs: new Set(['j1']) }),
    pty: slice('pty', { ptys: new Set(['j2']), terminals: new Set(['j2']) }),
    // An open terminal whose screen stopped changing (`terminals` without `ptys`) still counts.
    quiet: slice('quiet', { terminals: new Set(['j4']) }),
    rem: slice('rem', { reminders: new Set(['r1']) }),
    sj1: slice('sj1', { jobs: new Set(['j3']) }),
    sq1: slice('sq1', { terminals: new Set(['j5']) }),
    d2: slice('d2', { reminders: new Set(['r2']) }),
    i1: slice('i1', {}),
  }
  expect(activeRoots(roots, subs, slices, new Set(['ask', 'sa1']))).toEqual(['turn', 'cold', 'remote', 'job', 'pty', 'quiet', 'rem', 'ask', 'sub', 'subjob', 'subquiet', 'deep', 'subask'])
  expect(activeRoots(roots, subs, {}, new Set())).toEqual(['cold', 'remote', 'sub'])
})

test('activeRoots over the store: a snapshot-seeded quiet PTY keeps its root (or its parent) Active until it exits', () => {
  const store = createSessions()
  const root = (id: string) => s(id, null, '2026-01-01T00:00:00Z')
  const roots = [root('own'), root('parent')]
  const subs = new Map([['parent', [{ ...root('kid'), parent: 'parent' }]]])
  const active = () => activeRoots(roots, subs, store.getState().sessions, new Set())
  // After a reload nothing streamed yet: `list_live_counts` lists every running PTY, quiet ones included.
  store.getState().seedLive([{ session: 'own', jobs: [], ptys: ['p1'], reminders: [] }, { session: 'kid', jobs: [], ptys: ['p2'], reminders: [] }])
  expect(active()).toEqual(['own', 'parent'])
  expect(store.getState().sessions.own!.ptys.size).toBe(0)
  const finished = (session: string, id: string) => ({ type: 'JobFinished', session, id, command: 'top', kind: 'pty', exit: 'killed', secs: 1, output: '', inline: false, pty: true }) as const
  store.getState().applyEvents([finished('own', 'p1')])
  expect(active()).toEqual(['parent'])
  store.getState().applyEvents([finished('kid', 'p2')])
  expect(active()).toEqual([])
})
