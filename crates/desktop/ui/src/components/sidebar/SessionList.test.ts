import { expect, test } from 'vitest'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionSlice } from '@/store/sessions'
import { runningSubagents } from './SessionItem'
import { buildRows, listOffset, newChatLabel, type Row, watchListOffset } from './SessionList'

const s = (id: string, project: string | null, updated_at: string, pinned = false, archived = false): SessionSummary => ({
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
})

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
  const meta = (state: 'running' | 'requires_action' | 'idle' | 'completed') => ({ source: 'opencode_ext' as const, external: 'x', state, can_cancel: false, cost_rule: 'unreported' as const, detail: null })
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
