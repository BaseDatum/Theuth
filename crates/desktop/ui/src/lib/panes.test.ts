import { afterEach, expect, test, vi } from 'vitest'
import { tiling, TYPES, untile } from './panes.testing'
import {
  draftProject,
  hasPendingDrop,
  INSET_TARGET,
  isDraft,
  newDraft,
  placePayload,
  prepareStored,
  shown,
  STORAGE_KEY,
  tileLeaf,
  usePanes,
  VERSION,
} from './panes'
import { openTile, tileId } from './tiles'
import { useWebViews, webLeaf } from './web-views'
import { useEngines } from '@/link/engines'

afterEach(() => untile())
const leaves = () => usePanes.getState().leaves

test('placing splits the target on the named side and moves an already-open session instead of duplicating it', () => {
  tiling(['a'])
  usePanes.getState().place('a', 'b', 'right')
  expect(leaves()).toEqual(['a', 'b'])
  usePanes.getState().place('a', 'c', 'top')
  expect(leaves()).toEqual(['c', 'a', 'b'])
  // `b` moves from the right of `a` to below `c`: one pane per session.
  usePanes.getState().place('c', 'b', 'bottom')
  expect(leaves()).toEqual(['c', 'b', 'a'])
  expect(usePanes.getState().focused).toBe('b')
  // Dropping on the middle replaces.
  usePanes.getState().place('a', 'd', 'center')
  expect(leaves()).toEqual(['c', 'b', 'd'])
})

test('closing gives the pane up and hands the URL a survivor when the focused chat closed', () => {
  tiling(['a'])
  usePanes.getState().place('a', 'b', 'left')
  usePanes.getState().place('b', 'c', 'bottom')
  expect(usePanes.getState().close('a')).toBeNull() // `c` is focused; nothing to navigate
  expect(leaves()).toEqual(['b', 'c'])
  expect(usePanes.getState().close('c')).toBe('b')
  expect(leaves()).toEqual(['b'])
})

test('a route landing on a session outside the layout shows it in the focused pane', () => {
  tiling(['a'])
  usePanes.getState().place('a', 'b', 'right')
  usePanes.getState().focus('a')
  usePanes.getState().show('z')
  expect(leaves()).toEqual(['z', 'b'])
  usePanes.getState().show('b')
  expect(leaves()).toEqual(['z', 'b'])
  expect(usePanes.getState()).toMatchObject({ focused: 'b', selected: 'b' })
})

test('a route that lands before the workspace mounts is shown once it does', () => {
  untile()
  usePanes.getState().show('r')
  tiling(['a'])
  expect(leaves()).toEqual(['r'])
  expect(usePanes.getState().focused).toBe('r')
})

test('a dropped New chat is a draft leaf until its first send swaps the created session in', () => {
  tiling(['a'])
  const d = newDraft()
  expect(isDraft(d)).toBe(true)
  expect(isDraft('a')).toBe(false)
  usePanes.getState().place('a', d, 'right')
  expect(leaves()).toEqual(['a', d])
  expect(usePanes.getState().focused).toBe(d)
  usePanes.getState().place(d, 's1', 'center')
  expect(leaves()).toEqual(['a', 's1'])
  expect(usePanes.getState().focused).toBe('s1')
})

test('closing hands the URL a real survivor, never a draft', () => {
  tiling(['a'])
  const d = newDraft()
  usePanes.getState().place('a', d, 'center')
  expect(leaves()).toEqual([d])
  usePanes.getState().place(d, 'b', 'left')
  usePanes.getState().place('b', 'c', 'top')
  usePanes.getState().focus('c')
  expect(usePanes.getState().close('c')).toBe('b')
  usePanes.getState().focus('b')
  expect(usePanes.getState().close('b')).toBeNull()
  expect(usePanes.getState().focused).toBe(d)
  expect(leaves()).toEqual([d])
})

test('New chat in a split view replaces the focused pane with a draft; a single pane leaves it to the route', () => {
  tiling(['a'])
  expect(usePanes.getState().fresh()).toBeNull()
  usePanes.getState().place('a', 'b', 'right')
  usePanes.getState().focus('a')
  const d = usePanes.getState().fresh()
  expect(isDraft(d)).toBe(true)
  expect(leaves()).toEqual([d, 'b'])
  expect(usePanes.getState().focused).toBe(d)
  // A focused draft is kept, never stacked.
  expect(usePanes.getState().fresh()).toBe(d)
  // Focused on something no longer shown: the first pane takes it.
  usePanes.getState().focus('gone')
  const e = usePanes.getState().fresh()
  expect(e).toBe(d)
})

test('a draft carries the project preset of "New chat in <project>"', () => {
  const d = newDraft('Q&A / ops')
  expect(draftProject(d)).toBe('Q&A / ops')
  expect(draftProject(newDraft())).toBeUndefined()
  expect(draftProject(newDraft(null))).toBeUndefined()
})

test('a phone shows one leaf: the focused chat pane (a draft included), else the first chat, else the route chat', () => {
  expect(shown([], null, 'a')).toBe('a')
  expect(shown([], 'stale', 'a')).toBe('a')
  const d = newDraft()
  expect(shown(['b', 'c', tileLeaf('x')], 'c', 'a')).toBe('c')
  expect(shown(['b', d], d, 'a')).toBe(d)
  expect(shown(['b', 'c'], 'gone', 'a')).toBe('b')
  expect(shown([tileLeaf('x')], tileLeaf('x'), 'a')).toBe('a')
})

test('focused never holds a tile or web id: picking one only selects it; the URL stays on the chat', () => {
  const navigate = vi.fn()
  const ws = tiling(['a'], { navigate })
  const t = tileLeaf(openTile({ kind: 'jobs', host: 'a', props: {} })!)
  usePanes.getState().focus(t)
  usePanes.getState().focus(webLeaf('w'))
  expect(usePanes.getState().focused).toBe('a')
  // A Trellis focus (its tab clicked) is a user pick: selected follows, focus and the URL do not.
  ws.select(t)
  ws.focus(t)
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: t })
  expect(navigate).not.toHaveBeenCalled()
  usePanes.getState().place('a', 'b', 'right')
  ws.focus('a')
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: 'a' })
  ws.focus('b')
  expect(usePanes.getState()).toMatchObject({ focused: 'b', selected: 'b' })
  expect(navigate).toHaveBeenLastCalledWith('b')
})

test('a pane closed by Trellis (its tab ×) moves the URL off the focused chat and drops a closed web page', async () => {
  const navigate = vi.fn()
  const ws = tiling(['a'], { navigate })
  usePanes.getState().place('a', 'b', 'right')
  await ws.close('b')
  expect(navigate).toHaveBeenCalledWith('a')
  expect(usePanes.getState().focused).toBe('a')
  useWebViews.setState({ views: { w: { id: 'w', session: 'a', url: 'https://x.example/', title: 'x', current: 'https://x.example/', loading: false } } })
  usePanes.getState().place('a', webLeaf('w'), 'right')
  await ws.close(webLeaf('w'))
  expect(useWebViews.getState().views.w).toBeUndefined()
})

test('the layout persists on this machine: a remount restores it, tile params included', () => {
  tiling(['a'])
  usePanes.getState().place('a', 'b', 'right')
  const id = openTile({ kind: 'note', host: 'b', key: 'd1', props: { doc: 'd1' } })!
  untile()
  const stored = JSON.parse(localStorage.getItem(STORAGE_KEY)!)
  expect(stored.version).toBe(VERSION)
  tiling([], { persisted: true })
  expect(leaves()).toEqual(['a', 'b', tileLeaf(id)])
  expect(usePanes.getState().views[tileLeaf(id)]?.params).toMatchObject({ session: 'b', props: { doc: 'd1' } })
})

test('on load, views of deleted sessions, of another engine or of web pages this window lost are dropped', () => {
  tiling(['a'])
  usePanes.getState().place('a', 'gone', 'right')
  openTile({ kind: 'jobs', host: 'gone', props: {} })
  const d = newDraft()
  usePanes.getState().place('a', d, 'bottom')
  openTile({ kind: 'jobs', host: d, props: {} })
  useWebViews.setState({ views: { w: { id: 'w', session: 'a', url: 'https://x.example/', title: 'x', current: 'https://x.example/', loading: false } } })
  usePanes.getState().place('a', webLeaf('w'), 'top')
  untile()
  useWebViews.setState({ views: {} })
  const engine = useEngines.getState().active
  prepareStored({ engine, sessions: new Set(['a']), types: new Set(TYPES) })
  tiling([], { persisted: true })
  // The draft and its tile are kept: a draft has no session to have lost.
  expect(leaves()).toEqual(['a', d, tileLeaf(tileId('jobs', d))])
  untile()
  prepareStored({ engine: `${engine}-other`, sessions: null, types: new Set(TYPES) })
  tiling(['fresh'], { persisted: true })
  expect(leaves()).toEqual(['fresh'])
})

test('a garbled or stale stored layout never crashes: the workspace starts from its default', () => {
  for (const raw of ['{not json', JSON.stringify({ schema: 1, version: VERSION, root: { kind: 'panel', id: 'p', views: 7 }, views: 'x' }), JSON.stringify({ schema: 1, version: VERSION - 1, root: null, views: {} })]) {
    localStorage.setItem(STORAGE_KEY, raw)
    prepareStored({ engine: useEngines.getState().active, sessions: null, types: new Set(TYPES) })
    tiling(['route'], { persisted: true })
    expect(leaves()).toEqual(['route'])
    untile()
  }
})

test('a pane framed by zoom navigation is the one at the window corner (`first`) until the layout is restored', () => {
  const ws = tiling(['a'])
  usePanes.getState().place('a', 'b', 'right')
  expect(usePanes.getState().first).toBe('a')
  expect(ws.navigation.toggle('b')).toBe(true)
  expect(usePanes.getState().first).toBe('b')
  ws.navigation.toggle('b')
  expect(usePanes.getState().first).toBe('a')
})

test('startPayloadDrag + endDrag places standalone view tiles as splits and tabs without a chat owner', () => {
  const ws = tiling(['a'])
  const paneEl = document.createElement('div')
  paneEl.dataset.pane = 'a'
  paneEl.getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 400, right: 400, bottom: 400, x: 0, y: 0, toJSON() {} })
  document.body.appendChild(paneEl)
  const prevFromPoint = document.elementFromPoint
  document.elementFromPoint = () => paneEl
  try {
    // Drop on right edge band (x = 380 -> fx = 0.95 -> 'right')
    usePanes.getState().startPayloadDrag({ kind: 'view', path: '/tasks', title: 'Tasks' }, 'Tasks', 0, 0)
    usePanes.getState().moveDrag(380, 200)
    expect(usePanes.getState().drag?.target?.edge).toBe('right')
    const tasksLeaf = usePanes.getState().endDrag(true)
    expect(tasksLeaf).toBe(tileLeaf(tileId('view', '', '/tasks')))
    expect(leaves()).toEqual(['a', tasksLeaf!])
    expect(ws.view('a')!.panelId).not.toBe(ws.view(tasksLeaf!)!.panelId)
    // Focused stays on the chat leaf; standalone view has empty session owner.
    expect(usePanes.getState().focused).toBe('a')
    expect(usePanes.getState().views[tasksLeaf!]?.params).toMatchObject({ session: '', props: { path: '/tasks', title: 'Tasks' } })

    // Drop a second view onto the center of 'a' (x = 200, y = 200 -> 'tab')
    usePanes.getState().startPayloadDrag({ kind: 'view', path: '/costs', title: 'Costs' }, 'Costs', 0, 0)
    usePanes.getState().moveDrag(200, 200)
    expect(usePanes.getState().drag?.target?.edge).toBe('tab')
    const costsLeaf = usePanes.getState().endDrag(true)
    expect(costsLeaf).toBe(tileLeaf(tileId('view', '', '/costs')))
    expect(ws.view(costsLeaf!)!.panelId).toBe(ws.view('a')!.panelId)
  } finally {
    document.elementFromPoint = prevFromPoint
    paneEl.remove()
  }
})

test('dragging a view while ws is null targets the inset, queues pendingDrop, and flushes on attachWorkspace', () => {
  untile()
  const inset = document.createElement('main')
  inset.setAttribute('data-slot', 'sidebar-inset')
  inset.getBoundingClientRect = () => ({ left: 0, top: 0, width: 800, height: 600, right: 800, bottom: 600, x: 0, y: 0, toJSON() {} })
  document.body.appendChild(inset)
  const prevFromPoint = document.elementFromPoint
  document.elementFromPoint = () => inset
  try {
    usePanes.getState().startPayloadDrag({ kind: 'view', path: '/notebooks', title: 'Notebooks' }, 'Notebooks', 0, 0)
    usePanes.getState().moveDrag(760, 300)
    expect(usePanes.getState().drag?.target).toMatchObject({ session: INSET_TARGET, edge: 'right' })
    const dropped = usePanes.getState().endDrag(true)
    expect(dropped).toBe(tileLeaf(tileId('view', '', '/notebooks')))
    expect(hasPendingDrop()).toBe(true)

    // Mounting the workspace flushes pendingDrop beside the chat.
    const ws = tiling(['a'])
    expect(hasPendingDrop()).toBe(false)
    expect(leaves()).toEqual(['a', dropped!])
    expect(ws.view(dropped!)!.panelId).not.toBe(ws.view('a')!.panelId)
  } finally {
    document.elementFromPoint = prevFromPoint
    inset.remove()
  }
})

test('prepareStored preserves standalone tile:view panes across reloads while pruning dead session panes', () => {
  tiling(['a'])
  usePanes.getState().place('a', 'dead', 'right')
  openTile({ kind: 'jobs', host: 'dead', props: {} })
  placePayload('a', { kind: 'view', path: '/tasks', title: 'Tasks' }, 'right')
  const viewLeaf = tileLeaf(tileId('view', '', '/tasks'))
  untile()

  const engine = useEngines.getState().active
  prepareStored({ engine, sessions: new Set(['a']), types: new Set(TYPES) })
  tiling([], { persisted: true })
  expect(leaves()).toEqual(['a', viewLeaf])
  expect(usePanes.getState().views[viewLeaf]?.params).toMatchObject({ session: '', props: { path: '/tasks', title: 'Tasks' } })
})
