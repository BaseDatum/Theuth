import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { LayoutNode, WorkspaceHandle } from '@danfessler/trellis'
import { mounts, tiling, untile } from './panes.testing'
import { placeWeb, tileLeaf, usePanes } from './panes'
import { openTile, tileId } from './tiles'
import { useWebViews, webLeaf } from './web-views'

let ws: WorkspaceHandle
const navigate = vi.fn()
beforeEach(() => {
  useWebViews.setState({ views: {} })
  mounts.clear()
  navigate.mockClear()
  ws = tiling(['a', 'b'], { navigate })
})
afterEach(() => {
  untile()
  Reflect.deleteProperty(document, 'elementFromPoint')
})

/** The layout as `x(…)` rows, `y(…)` columns and panels (`a|b`: tabs, `*` marks the active one when there are several). */
const shape = (n: LayoutNode | null | undefined = ws.getDocument().root): string => {
  if (!n) return ''
  if (n.kind === 'panel') return n.views.length < 2 ? n.views.join('') : n.views.map((v) => (v === n.selected ? `${v}*` : v)).join('|')
  if (n.kind === 'stage') return shape(n.child)
  return `${n.axis}(${n.children.map((c) => shape(c)).join(', ')})`
}
const panel = (leaf: string) => ws.view(leaf)!.panelId
const tab = (leaf: string) => [...ws.element.querySelectorAll<HTMLElement>('[data-trellis-part="tab"]')].find((t) => t.dataset.view === leaf)!
const click = (el: Element) => el.dispatchEvent(new MouseEvent('click', { bubbles: true, button: 0 }))
const state = () => usePanes.getState()
const web = (id: string, session = 'a') => {
  useWebViews.getState().open({ id, session, url: `https://${id}.example/`, title: id })
  return placeWeb(session, id)
}

/** A sidebar row dragged over `el` (a pane, 400 × 400 at the origin) to (`x`, `y`) and dropped there. */
function sidebarDrop(session: string, el: Element, x: number, y: number) {
  el.getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 400, right: 400, bottom: 400, x: 0, y: 0, toJSON() {} })
  Object.assign(document, { elementFromPoint: () => el })
  state().startDrag(session, session, 0, 0)
  state().moveDrag(x, y)
  const target = state().drag?.target ?? null
  state().endDrag(true)
  return target
}
const paneEl = (leaf: string) => {
  const el = document.createElement('div')
  el.dataset.pane = leaf
  return document.body.appendChild(el)
}

test('a chat dropped on the middle of another chat’s panel (a Trellis tab or header drag) becomes its tab, nothing remounts', () => {
  // Trellis commits a centre or tab-bar drop as a dock into the panel.
  ws.dock('b', { into: panel('a') })
  expect(shape()).toBe('a|b*')
  expect(state().leaves).toEqual(['a', 'b'])
  expect(Object.fromEntries(mounts)).toEqual({ a: 1, b: 1 })
})

test('a chat joins a tile’s panel: chats and tiles share tabs', () => {
  const t = tileLeaf(openTile({ kind: 'jobs', host: 'a', props: {} })!)
  state().place(t, 'c', 'tab')
  expect(shape()).toBe(`x(a, ${t}|c*, b)`)
  expect(state()).toMatchObject({ focused: 'c', selected: 'c' })
  // A dragged chat too.
  ws.dock('a', { into: panel(t) })
  expect(shape()).toBe(`x(${t}|c|a*, b)`)
  expect(Object.fromEntries(mounts)).toEqual({ a: 1, b: 1, c: 1, [t]: 1 })
})

test('a sidebar row dropped on a pane’s middle adds the chat as the active tab of its panel; an edge band still splits', () => {
  const target = sidebarDrop('c', paneEl('a'), 200, 200)
  expect(target?.edge).toBe('tab')
  expect(shape()).toBe('x(a|c*, b)')
  expect(state()).toMatchObject({ focused: 'c', selected: 'c' })
  expect(sidebarDrop('d', paneEl('b'), 390, 200)?.edge).toBe('right')
  expect(shape()).toBe('x(a|c*, b, d)')
  expect(sidebarDrop('e', paneEl('b'), 200, 5)?.edge).toBe('top')
  expect(shape()).toBe('x(a|c*, y(e, b), d)')
})

test('a sidebar row over a panel’s tab bar joins that panel; an open chat moves there instead of duplicating', () => {
  ws.dock('b', { into: panel('a') })
  state().place('a', 'c', 'right')
  const bar = ws.element.querySelector(`[data-trellis-part="tabbar"][data-panel="${panel('a')}"]`)!
  expect(sidebarDrop('c', bar, 10, 10)?.edge).toBe('tab')
  expect(shape()).toBe('a|b|c*')
  expect(Object.fromEntries(mounts)).toEqual({ a: 1, b: 1, c: 1 })
})

test('activating a chat tab (click, keyboard) focuses it and moves the URL; closing the active tab focuses the one shown next', async () => {
  ws.dock('b', { into: panel('a') })
  state().place('a', 'c', 'right')
  navigate.mockClear()
  click(tab('a'))
  expect(state()).toMatchObject({ focused: 'a', selected: 'a' })
  expect(navigate).toHaveBeenLastCalledWith('a')
  ws.run('tab.next')
  expect(shape()).toBe('x(a|b*, c)')
  expect(state()).toMatchObject({ focused: 'b', selected: 'b' })
  expect(navigate).toHaveBeenLastCalledWith('b')
  // The tab's ×: its sibling is on screen now, so it is focused, not the first chat elsewhere.
  click(tab('b').querySelector('[data-trellis-part="tab-close"]')!)
  await Promise.resolve()
  expect(shape()).toBe('x(a, c)')
  expect(state()).toMatchObject({ focused: 'a', selected: 'a' })
  expect(navigate).toHaveBeenLastCalledWith('a')
  // The facade's close (a header ×) does the same.
  ws.dock('c', { into: panel('a') })
  state().focus('c')
  expect(state().close('c')).toBe('a')
})

test('a route to a chat behind another tab brings its tab to the front', () => {
  ws.dock('b', { into: panel('a') })
  expect(shape()).toBe('a|b*')
  state().show('a')
  expect(shape()).toBe('a*|b')
  expect(state()).toMatchObject({ focused: 'a', selected: 'a' })
})

test('an inactive chat tab stays mounted: switching tabs never remounts a chat', () => {
  ws.dock('b', { into: panel('a') })
  for (let i = 0; i < 3; i++) {
    click(tab('a'))
    click(tab('b'))
  }
  expect(ws.view('a')!.visible).toBe(false)
  expect(Object.fromEntries(mounts)).toEqual({ a: 1, b: 1 })
})

test('panels holding chat tabs round-trip through the saved layout', () => {
  const t = tileLeaf(openTile({ kind: 'note', host: 'b', key: 'n', props: { doc: 'n' } })!)
  ws.dock('b', { into: panel('a') })
  state().place(t, 'c', 'tab')
  const before = shape()
  untile()
  ws = tiling([], { persisted: true })
  expect(shape()).toBe(before)
  expect(state().views[t]?.params).toMatchObject({ session: 'b', props: { doc: 'n' } })
})

test('agent surfaces group by kind only: a chats-only panel never takes a show_web; a chat + web panel does', () => {
  ws.dock('b', { into: panel('a') })
  web('w1')
  expect(shape()).toBe(`x(a|b*, ${webLeaf('w1')})`)
  // The user puts the page into the chats' panel: the next page joins it there, its kind's panel.
  ws.dock(webLeaf('w1'), { into: panel('a') })
  web('w2')
  expect(shape()).toBe(`a|b|${webLeaf('w1')}|${webLeaf('w2')}*`)
  // A tile of another kind still opens by the split rules (below the frames of its chat), never as a tab there.
  openTile({ kind: 'file', host: 'a', key: 'f', props: { path: '/f' } })
  expect(shape()).toBe(`y(a|b|${webLeaf('w1')}|${webLeaf('w2')}*, ${tileLeaf(tileId('file', 'a', 'f'))})`)
  expect(state().focused).toBe('a')
})
