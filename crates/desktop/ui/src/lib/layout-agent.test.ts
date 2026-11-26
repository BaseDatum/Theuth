import { afterEach, expect, test, vi } from 'vitest'
import type { SplitNode } from '@danfessler/trellis'
import { openViewFromCard } from '@/components/tiles/open'
import { installLayoutAgent, LAYOUT_REQUESTED_TOPIC, LAYOUT_RESOLVED_TOPIC } from './layout-agent'
import { applyLayoutOp, hasPendingDrop, serializeLayout, tileLeaf, usePanes } from './panes'
import { tiling, untile } from './panes.testing'
import { tileId } from './tiles'
import { availableViews, validateViewPath, viewTitle } from './view-title'
import { useConnection } from '@/store/connection'
import { dispatch } from '@/store/sessions'

afterEach(() => {
  untile()
  useConnection.setState({ api: null })
})

test('viewTitle derives clean human titles from route paths and respects explicit overrides', () => {
  expect(viewTitle('/tasks')).toBe('Tasks')
  expect(viewTitle('/tasks/t-1')).toBe('Tasks')
  expect(viewTitle('/notebooks')).toBe('Notebooks')
  expect(viewTitle('/notes')).toBe('Notes')
  expect(viewTitle('/agents')).toBe('Agents')
  expect(viewTitle('/routines')).toBe('Routines')
  expect(viewTitle('/meetings')).toBe('Meetings')
  expect(viewTitle('/meetings/new')).toBe('Meeting')
  expect(viewTitle('/knowledge')).toBe('Knowledge')
  expect(viewTitle('/skills')).toBe('Skills')
  expect(viewTitle('/artifacts')).toBe('Artifacts')
  expect(viewTitle('/whiteboards/wb-1')).toBe('Whiteboards')
  expect(viewTitle('/autoresearch')).toBe('Autoresearch')
  expect(viewTitle('/attention')).toBe('Inbox')
  expect(viewTitle('/costs')).toBe('Costs')
  expect(viewTitle('/settings/appearance')).toBe('Settings')
  expect(viewTitle('/secrets')).toBe('Secrets')
  expect(viewTitle('/whiteboards/wb-1', 'Q4 Roadmap')).toBe('Q4 Roadmap')
})

test('serializeLayout reports workspace state, panes, panel membership, and layout tree', () => {
  tiling(['a'])
  const opened = applyLayoutOp({ op: 'open_view', path: '/tasks' }, 'a')
  expect(opened.kind).toBe('done')

  const snap = serializeLayout('/c/a')
  expect(snap.active_route).toBe('/c/a')
  expect(snap.workspace_mounted).toBe(true)
  expect(snap.focused).toBe('a')
  expect(snap.tree).not.toBeNull()
  const tasksLeaf = tileLeaf(tileId('view', '', '/tasks'))
  expect(snap.panes).toEqual([
    expect.objectContaining({ id: 'a', kind: 'chat', session: 'a', active_in_panel: true, tab_index: 0 }),
    expect.objectContaining({
      id: tasksLeaf,
      kind: 'tile:view',
      title: 'Tasks',
      path: '/tasks',
      session: '',
      active_in_panel: true,
      tab_index: 0,
    }),
  ])
})

test('applyLayoutOp: open_view creates, reuses, selects without stealing focus, and rejects invalid paths', () => {
  const ws = tiling(['a'])
  const bad = applyLayoutOp({ op: 'open_view', path: 'https://evil.example' }, 'a')
  expect(bad.kind).toBe('failed')
  const traversal = applyLayoutOp({ op: 'open_view', path: '/tasks/../secrets' }, 'a')
  expect(traversal.kind).toBe('failed')

  const r1 = applyLayoutOp({ op: 'open_view', path: '/tasks', focus: true }, 'a')
  const tasksLeaf = tileLeaf(tileId('view', '', '/tasks'))
  expect(r1).toEqual({ kind: 'done', result: { pane: tasksLeaf, path: '/tasks', title: 'Tasks', reused: false } })
  // Standing invariant: focused always stays a chat leaf; frames are selected-only.
  expect(usePanes.getState().focused).toBe('a')
  expect(usePanes.getState().selected).toBe(tasksLeaf)

  const r2 = applyLayoutOp({ op: 'open_view', path: '/tasks', title: 'Sprint Tasks' }, 'a')
  expect(r2).toEqual({ kind: 'done', result: { pane: tasksLeaf, path: '/tasks', title: 'Sprint Tasks', reused: true } })
  expect(ws.getDocument().views[tasksLeaf]?.params).toMatchObject({ props: { path: '/tasks', title: 'Sprint Tasks' } })
})

test('applyLayoutOp: close closes panes and refuses closing the last remaining pane', () => {
  const navigate = vi.fn()
  tiling(['a'])
  expect(applyLayoutOp({ op: 'close', pane: 'a' }, 'a', navigate)).toEqual({
    kind: 'failed',
    reason: 'cannot close the last remaining pane',
  })

  applyLayoutOp({ op: 'open_view', path: '/costs' }, 'a', navigate)
  const costsLeaf = tileLeaf(tileId('view', '', '/costs'))
  expect(applyLayoutOp({ op: 'close', pane: costsLeaf }, 'a', navigate)).toEqual({
    kind: 'done',
    result: { closed: costsLeaf },
  })
  expect(usePanes.getState().leaves).toEqual(['a'])
  expect(applyLayoutOp({ op: 'close', pane: 'missing' }, 'a', navigate).kind).toBe('failed')
})

test('applyLayoutOp: focus navigates chats and selects frames without moving chat focus', () => {
  const navigate = vi.fn()
  tiling(['a', 'b'])
  applyLayoutOp({ op: 'open_view', path: '/notes' }, 'a', navigate)
  const notesLeaf = tileLeaf(tileId('view', '', '/notes'))

  expect(applyLayoutOp({ op: 'focus', pane: notesLeaf }, 'a', navigate)).toEqual({
    kind: 'done',
    result: { focused: notesLeaf },
  })
  expect(usePanes.getState().focused).toBe('a')
  expect(usePanes.getState().selected).toBe(notesLeaf)
  expect(navigate).not.toHaveBeenCalled()

  expect(applyLayoutOp({ op: 'focus', pane: 'b' }, 'a', navigate)).toEqual({
    kind: 'done',
    result: { focused: 'b' },
  })
  expect(usePanes.getState().focused).toBe('b')
  expect(usePanes.getState().selected).toBe('b')
  expect(navigate).toHaveBeenCalledWith('/c/b')
})

test('applyLayoutOp: move splits or tabs tiles, but refuses tabbing two chat leaves together', () => {
  const ws = tiling(['a', 'b'])
  // Standing invariant: chat tabs are user-only.
  expect(applyLayoutOp({ op: 'move', pane: 'b', target: 'a', edge: 'tab' }, 'a')).toEqual({
    kind: 'failed',
    reason: 'chat tabs are user-only; split instead of tabbing chats together',
  })

  // Splitting chats is allowed.
  expect(applyLayoutOp({ op: 'move', pane: 'b', target: 'a', edge: 'bottom' }, 'a')).toEqual({
    kind: 'done',
    result: { moved: 'b', target: 'a', edge: 'bottom' },
  })

  // Tabbing a view tile onto a chat (or another tile) is allowed.
  applyLayoutOp({ op: 'open_view', path: '/tasks' }, 'a')
  const tasksLeaf = tileLeaf(tileId('view', '', '/tasks'))
  expect(applyLayoutOp({ op: 'move', pane: tasksLeaf, target: 'a', edge: 'tab' }, 'a')).toEqual({
    kind: 'done',
    result: { moved: tasksLeaf, target: 'a', edge: 'tab' },
  })
  expect(ws.view(tasksLeaf)!.panelId).toBe(ws.view('a')!.panelId)
})

test('applyLayoutOp: resize updates split weights and rejects invalid shares or unsplit layouts', () => {
  const ws = tiling(['a'])
  expect(applyLayoutOp({ op: 'resize', pane: 'a', share: 0.4 }, 'a').kind).toBe('failed')

  applyLayoutOp({ op: 'open_view', path: '/tasks' }, 'a')
  const tasksLeaf = tileLeaf(tileId('view', '', '/tasks'))

  expect(applyLayoutOp({ op: 'resize', pane: tasksLeaf, share: 0 }, 'a').kind).toBe('failed')
  expect(applyLayoutOp({ op: 'resize', pane: tasksLeaf, share: 1 }, 'a').kind).toBe('failed')

  const res = applyLayoutOp({ op: 'resize', pane: tasksLeaf, share: 0.35 }, 'a')
  expect(res).toEqual({ kind: 'done', result: { resized: tasksLeaf, share: 0.35 } })
  const root = ws.getDocument().root as SplitNode
  expect(root.kind).toBe('split')
  expect(root.weights[0]).toBeCloseTo(0.65, 5)
  expect(root.weights[1]).toBeCloseTo(0.35, 5)
})

test('applyLayoutOp: open_view when workspace is unmounted queues pendingDrop and navigates to session', () => {
  untile()
  const navigate = vi.fn()
  const res = applyLayoutOp({ op: 'open_view', path: '/costs', placement: 'right' }, 's1', navigate)
  const costsLeaf = tileLeaf(tileId('view', '', '/costs'))
  expect(res).toEqual({ kind: 'done', result: { pane: costsLeaf, path: '/costs', title: 'Costs', reused: false } })
  expect(navigate).toHaveBeenCalledWith('/c/s1')
  expect(hasPendingDrop()).toBe(true)

  // Mounting workspace flushes the queued open_view.
  tiling(['s1'])
  expect(hasPendingDrop()).toBe(false)
  expect(usePanes.getState().leaves).toEqual(['s1', costsLeaf])
})

test('installLayoutAgent subscribes to layout events, replies via answer_layout_command, and deduplicates until resolved', () => {
  tiling(['a'])
  const answer = vi.fn(async () => true)
  useConnection.setState({ api: { answer_layout_command: answer } as unknown as ReturnType<typeof useConnection.getState>['api'] })
  const navigate = vi.fn()
  const off = installLayoutAgent(navigate, () => '/c/a')
  try {
    const req = {
      key: LAYOUT_REQUESTED_TOPIC,
      event: { type: 'LayoutCommandRequested', id: 'call-1', session: 'a', op: { op: 'list' } },
    }
    dispatch([req as never])
    expect(answer).toHaveBeenCalledTimes(1)
    expect(answer).toHaveBeenLastCalledWith(
      'call-1',
      expect.objectContaining({ kind: 'done', result: expect.objectContaining({ active_route: '/c/a', focused: 'a' }) }),
      'a',
    )

    // Duplicate delivery while unresolved is ignored.
    dispatch([req as never])
    expect(answer).toHaveBeenCalledTimes(1)

    // Once resolved, the id is cleared from the handled set.
    dispatch([{ key: LAYOUT_RESOLVED_TOPIC, event: { type: 'LayoutCommandResolved', id: 'call-1' } } as never])
    dispatch([req as never])
    expect(answer).toHaveBeenCalledTimes(2)
  } finally {
    off()
  }
})

test('availableViews lists built-in views with detail_templates and serializeLayout exposes available_views', () => {
  const views = availableViews()
  const paths = views.map((v) => v.path)
  for (const p of [
    '/attention',
    '/agents',
    '/tasks',
    '/tasks/graph',
    '/routines',
    '/autoresearch',
    '/artifacts',
    '/knowledge',
    '/skills',
    '/notes',
    '/whiteboards',
    '/notebooks',
    '/meetings',
    '/meetings/new',
    '/secrets',
    '/costs',
    '/settings/appearance',
  ]) {
    expect(paths).toContain(p)
  }
  const settings = views.find((v) => v.path === '/settings/appearance')
  expect(settings?.detail_template).toBe('/settings/<tab>')
  const tasks = views.find((v) => v.path === '/tasks')
  expect(tasks?.detail_template).toBe('/tasks/<id>')
  const knowledge = views.find((v) => v.path === '/knowledge')
  expect(knowledge?.detail_template).toBe('/knowledge/<concept>')

  tiling(['a'])
  const snap = serializeLayout('/c/a')
  expect(snap.available_views.length).toBeGreaterThanOrEqual(17)
  expect(snap.available_views.some((v) => v.path === '/tasks')).toBe(true)
})

test('validateViewPath accepts known top-level and parameterized routes and rejects unknown paths with a hint', () => {
  for (const valid of [
    '/tasks',
    '/tasks/t-123',
    '/tasks/graph',
    '/notebooks',
    '/notebooks/nb-1',
    '/notes/n-1',
    '/agents/ops',
    '/agents/ops/memory',
    '/agents/new',
    '/routines/nightly',
    '/routines/nightly/runs',
    '/routines/nightly/graph',
    '/routines/new',
    '/meetings/m-1',
    '/meetings/new',
    '/meetings/briefs',
    '/autoresearch/study-1',
    '/autoresearch/new',
    '/whiteboards/wb-1',
    '/knowledge/facts/my-fact',
    '/artifacts/call-1',
    '/attention/att-1',
    '/skills/pdf',
    '/sources/src-1',
    '/settings/appearance',
    '/costs?range=7d',
  ]) {
    expect(validateViewPath(valid)).toEqual({ ok: true, normalized: valid })
  }

  expect(validateViewPath('/tasks/')).toEqual({ ok: true, normalized: '/tasks' })

  for (const invalid of ['/does-not-exist', '/tasks/a/b/c', '/c/session-1', 'tasks', '/tasks/../secrets', 'https://example.com']) {
    const res = validateViewPath(invalid)
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.reason).toContain(`unknown view path '${invalid}'`)
      expect(res.reason).toContain('pane_list()')
      expect(res.reason).toContain('available_views')
    }
  }
})

test('applyLayoutOp: open_view rejects unknown view paths so agents never open a 404 pane', () => {
  tiling(['a'])
  const res = applyLayoutOp({ op: 'open_view', path: '/does-not-exist' }, 'a')
  expect(res.kind).toBe('failed')
  if (res.kind === 'failed') {
    expect(res.reason).toContain("unknown view path '/does-not-exist'")
    expect(res.reason).toContain('available_views')
  }
  expect(usePanes.getState().leaves).toEqual(['a'])
})

test('openViewFromCard: a new view joins the views panel, an open one is revealed; nothing closes, a stale replace target is ignored', () => {
  tiling(['a'])
  const tasks = openViewFromCard({ path: '/tasks' }, { session: 'a' })
  expect(tasks).toBe(tileLeaf(tileId('view', '', '/tasks')))
  usePanes.getState().select(tasks!)
  const costs = openViewFromCard({ path: '/costs' }, { session: 'a' })
  expect(usePanes.getState().leaves).toEqual(['a', tasks, costs])
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: costs })
  expect(openViewFromCard({ path: '/tasks' }, { placement: 'replace', target: 'gone' })).toBe(tasks)
  expect(usePanes.getState().leaves).toEqual(['a', tasks, costs])
  expect(usePanes.getState().selected).toBe(tasks)
})
