import { afterEach, beforeEach, expect, test } from 'vitest'
import type { LayoutNode, PanelNode, WorkspaceHandle } from '@danfessler/trellis'
import { mounts, tiling, unmount, untile } from './panes.testing'
import { picked, placeWeb, tileLeaf, usePanes } from './panes'
import { closeTile, openTile, tileId } from './tiles'
import { useWebViews, webLeaf } from './web-views'

let ws: WorkspaceHandle
beforeEach(() => {
  useWebViews.setState({ views: {} })
  mounts.clear()
  ws = tiling(['a'])
})
afterEach(() => untile())

const T = (kind: string, key?: string, session = 'a') => tileLeaf(tileId(kind, session, key))
const file = (key: string, session = 'a') => openTile({ kind: 'file', host: session, key, props: { path: `/${key}` } })
const web = (id: string, session = 'a') => {
  useWebViews.getState().open({ id, session, url: `https://${id}.example/`, title: id })
  return placeWeb(session, id)
}
/** The layout as `x(…)` rows, `y(…)` columns and panels (`a|b`: tabs, `*` marks the active one when there are several). */
const shape = (n: LayoutNode | null | undefined = ws.getDocument().root): string => {
  if (!n) return ''
  if (n.kind === 'panel') return n.views.length < 2 ? n.views.join('') : n.views.map((v) => (v === n.selected ? `${v}*` : v)).join('|')
  if (n.kind === 'stage') return shape(n.child)
  return `${n.axis}(${n.children.map((c) => shape(c)).join(', ')})`
}
const panelOf = (leaf: string): PanelNode => {
  const find = (n: LayoutNode | null | undefined): PanelNode | null =>
    !n ? null : n.kind === 'panel' ? (n.views.includes(leaf) ? n : null) : n.kind === 'stage' ? find(n.child) : n.children.map(find).find((p) => p) ?? null
  return find(ws.getDocument().root)!
}

test("a second show_web lands as the active tab of the first one's panel", () => {
  web('w1')
  web('w2')
  expect(shape()).toBe(`x(a, ${webLeaf('w1')}|${webLeaf('w2')}*)`)
  // Focus stays on the chat: a frame never takes the URL.
  expect(usePanes.getState().focused).toBe('a')
})

test('different kinds never group: each opens by the split rules', () => {
  web('w1')
  file('f1')
  openTile({ kind: 'job-terminal', host: 'a', key: 'j1', props: {} })
  expect(shape()).toBe(`x(a, y(${webLeaf('w1')}, ${T('file', 'f1')}, ${T('job-terminal', 'j1')}))`)
})

test('same-key reuse beats grouping: the open tab comes to the front, no second view', () => {
  file('f1', 'a')
  file('f2', 'a')
  expect(shape()).toBe(`x(a, ${T('file', 'f1')}|${T('file', 'f2')}*)`)
  file('f1', 'a')
  expect(shape()).toBe(`x(a, ${T('file', 'f1')}*|${T('file', 'f2')})`)
  web('w1')
  web('w2')
  web('w1')
  expect(panelOf(webLeaf('w1')).selected).toBe(webLeaf('w1'))
  expect(panelOf(webLeaf('w1')).views).toEqual([webLeaf('w1'), webLeaf('w2')])
})

test('grouping beats joining the selected tile’s panel', () => {
  file('f1')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  usePanes.getState().select(T('jobs'))
  file('f2')
  expect(shape()).toBe(`x(a, y(${T('file', 'f1')}|${T('file', 'f2')}*, ${T('jobs')}))`)
  expect(usePanes.getState().selected).toBe(T('jobs'))
  // A kind not laid out yet joins the selected tile's panel as its active tab; the selected tile stays open.
  openTile({ kind: 'prompt', host: 'a', props: {} })
  expect(shape()).toBe(`x(a, y(${T('file', 'f1')}|${T('file', 'f2')}*, ${T('jobs')}|${T('prompt')}*))`)
})

test('a selection in the joined panel moves to the new tab, the one showing there now', () => {
  file('f1')
  usePanes.getState().select(T('file', 'f1'))
  file('f2')
  expect(usePanes.getState()).toMatchObject({ selected: T('file', 'f2'), focused: 'a' })
})

test('a kind split across panels joins the one used last: a tab dragged out, then a pick', () => {
  file('f1')
  file('f2')
  // The user drags `f2`'s tab out below its panel (a Trellis move, not the facade's).
  ws.dock(T('file', 'f2'), { beside: panelOf(T('file', 'f1')).id, edge: 'bottom' })
  expect(shape()).toBe(`x(a, y(${T('file', 'f1')}, ${T('file', 'f2')}))`)
  file('f3')
  expect(shape()).toBe(`x(a, y(${T('file', 'f1')}, ${T('file', 'f2')}|${T('file', 'f3')}*))`)
  picked(T('file', 'f1'))
  file('f4')
  expect(shape()).toBe(`x(a, y(${T('file', 'f1')}|${T('file', 'f4')}*, ${T('file', 'f2')}|${T('file', 'f3')}*))`)
  // Reusing a tab counts as use too.
  file('f3')
  file('f5')
  expect(panelOf(T('file', 'f5')).views).toEqual([T('file', 'f2'), T('file', 'f3'), T('file', 'f5')])
})

test('another chat’s surface of the same kind joins the panel too', () => {
  usePanes.getState().place('a', 'b', 'right')
  file('f1', 'a')
  file('f2', 'b')
  expect(panelOf(T('file', 'f2', 'b')).views).toEqual([T('file', 'f1'), T('file', 'f2', 'b')])
})

test('closing one tab leaves the others in their panel', () => {
  web('w1')
  web('w2')
  web('w3')
  usePanes.getState().close(webLeaf('w2'))
  expect(shape()).toBe(`x(a, ${webLeaf('w1')}|${webLeaf('w3')}*)`)
  expect(Object.keys(useWebViews.getState().views).sort()).toEqual(['w1', 'w3'])
  file('f1')
  file('f2')
  closeTile(tileId('file', 'a', 'f2'))
  expect(shape()).toBe(`x(a, y(${webLeaf('w1')}|${webLeaf('w3')}*, ${T('file', 'f1')}))`)
})

test('no view remounts while grouping, revealing, dragging a tab out or closing a sibling', () => {
  web('w1')
  file('f1')
  file('f2')
  web('w2')
  file('f1')
  ws.dock(T('file', 'f2'), { beside: panelOf(T('file', 'f1')).id, edge: 'right' })
  file('f3')
  closeTile(tileId('file', 'a', 'f1'))
  for (const leaf of usePanes.getState().leaves) expect([leaf, mounts.get(leaf)]).toEqual([leaf, 1])
})

test('grouped panels round-trip through the saved layout', () => {
  web('w1')
  web('w2')
  file('f1')
  file('f2')
  file('f1')
  const before = shape()
  unmount()
  ws = tiling([], { persisted: true })
  expect(shape()).toBe(before)
  expect(shape()).toBe(`x(a, y(${webLeaf('w1')}|${webLeaf('w2')}*, ${T('file', 'f1')}*|${T('file', 'f2')}))`)
})
