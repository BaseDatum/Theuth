import { afterEach, beforeEach, expect, test } from 'vitest'
import type { LayoutNode, WorkspaceHandle } from '@danfessler/trellis'
import { tiling, unmount, untile } from './panes.testing'
import { chatLeaves, reopenClosed, tileLeaf, usePanes } from './panes'
import { closeSessionTiles, closeTile, openTile, openTileOf, promoteDraft, tileId, tiles } from './tiles'
import { useWebViews, webLeaf } from './web-views'

let ws: WorkspaceHandle
beforeEach(() => {
  useWebViews.setState({ views: {} })
  ws = tiling(['a'])
})
afterEach(() => untile())

const tree = () => usePanes.getState().leaves
const T = (kind: string, session = 'a', key?: string) => tileLeaf(tileId(kind, session, key))
/** The layout as `x(…)` rows, `y(…)` columns and one-view panels. */
const shape = (n: LayoutNode | null | undefined = ws.getDocument().root): string => {
  if (!n) return ''
  if (n.kind === 'panel') return n.views.join('|')
  if (n.kind === 'stage') return shape(n.child)
  return `${n.axis}(${n.children.map((c) => shape(c)).join(', ')})`
}

test('a tile opens right of its chat; opening it again reuses the tile with the new props, never a second one', () => {
  expect(openTile({ kind: 'artifact', host: 'a', props: { callId: 'c1' } })).toBe(tileId('artifact', 'a'))
  expect(shape()).toBe(`x(a, ${T('artifact')})`)
  openTile({ kind: 'artifact', host: 'a', props: { callId: 'c2' } })
  expect(tree()).toEqual(['a', T('artifact')])
  expect(openTileOf('artifact', 'a')?.props).toEqual({ callId: 'c2' })
  // A tile never takes focus: the URL stays on the chat.
  expect(usePanes.getState().focused).toBe('a')
})

test('the same kind under another key is another tile, a tab beside the first; another kind stacks below the last one', () => {
  openTile({ kind: 'skill', host: 'a', props: { name: 'x' }, key: 'x' })
  openTile({ kind: 'skill', host: 'a', props: { name: 'y' }, key: 'y' })
  expect(shape()).toBe(`x(a, ${T('skill', 'a', 'x')}|${T('skill', 'a', 'y')})`)
  openTile({ kind: 'prompt', host: 'a', props: {} })
  expect(shape()).toBe(`x(a, y(${T('skill', 'a', 'x')}|${T('skill', 'a', 'y')}, ${T('prompt')}))`)
})

test('a web frame of the session counts as one of its tiles for placement', () => {
  useWebViews.getState().open({ id: 'w', session: 'a', url: 'https://example.com/', title: 'x' })
  usePanes.getState().place('a', webLeaf('w'), 'right')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  expect(shape()).toBe(`x(a, y(${webLeaf('w')}, ${T('jobs')}))`)
})

test("a trigger inside a tile opens for that tile's chat", () => {
  openTile({ kind: 'jobs', host: 'a', props: {} })
  openTile({ kind: 'job-terminal', host: T('jobs'), props: {}, key: 'j1' })
  expect(openTileOf('job-terminal', 'a', 'j1')?.session).toBe('a')
  expect(tree()).toEqual(['a', T('jobs'), T('job-terminal', 'a', 'j1')])
})

test('nothing opens for a chat with no pane, nor without a workspace', () => {
  usePanes.getState().place('a', 'b', 'right')
  expect(openTile({ kind: 'jobs', host: 'z', props: {} })).toBeNull()
  expect(tree()).toEqual(['a', 'b'])
  untile()
  expect(openTile({ kind: 'jobs', host: 'a', props: {} })).toBeNull()
})

test('closing the last tile leaves the single chat; tiles are never chat leaves and never focused', () => {
  const id = openTile({ kind: 'prompt', host: 'a', props: {} })!
  expect(chatLeaves(tree())).toEqual(['a'])
  usePanes.getState().focus(tileLeaf(id))
  expect(usePanes.getState().focused).toBe('a')
  closeTile(id)
  expect(shape()).toBe('a')
})

test('a tile outlives its chat: closing or replacing the owning chat leaves it open, still holding its session', () => {
  usePanes.getState().place('a', 'b', 'right')
  openTile({ kind: 'prompt', host: 'a', props: {} })
  expect(tree()).toEqual(['a', T('prompt'), 'b'])
  usePanes.getState().focus('a')
  expect(usePanes.getState().close('a')).toBe('b')
  expect(tree()).toEqual([T('prompt'), 'b'])
  usePanes.getState().show('c')
  expect(tree()).toEqual([T('prompt'), 'c'])
  expect(openTileOf('prompt', 'a')).toMatchObject({ kind: 'prompt', session: 'a' })
})

test('a chat picked in the sidebar while a tile is selected splits beside it; the tile stays, the URL never names it', () => {
  usePanes.getState().show('a')
  openTile({ kind: 'artifact', host: 'a', props: { callId: 'c1' } })
  usePanes.getState().select(T('artifact'))
  expect(usePanes.getState().focused).toBe('a')
  usePanes.getState().show('b')
  expect(shape()).toBe(`x(a, b, ${T('artifact')})`)
  expect(tiles().map((t) => t.id)).toEqual([tileId('artifact', 'a')])
  expect(usePanes.getState()).toMatchObject({ focused: 'b', selected: 'b' })
})

test('a sidebar drop on a tile’s centre replaces it; on its edge it splits beside', () => {
  openTile({ kind: 'jobs', host: 'a', props: {} })
  usePanes.getState().place(T('jobs'), 'b', 'center')
  expect(shape()).toBe('x(a, b)')
  openTile({ kind: 'prompt', host: 'b', props: {} })
  usePanes.getState().place(T('prompt', 'b'), 'c', 'bottom')
  expect(shape()).toBe(`x(a, b, y(${T('prompt', 'b')}, c))`)
})

test('a surface opened while a tile (or web view) is selected joins its panel as a tab; nothing closes', () => {
  usePanes.getState().show('a')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  usePanes.getState().select(T('jobs'))
  openTile({ kind: 'prompt', host: 'a', props: {} })
  expect(shape()).toBe(`x(a, ${T('jobs')}|${T('prompt')})`)
  expect(tiles().map((t) => t.id)).toEqual([tileId('jobs', 'a'), tileId('prompt', 'a')])
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: T('prompt') })
  // A web view in the selected pane stays open behind the new tab.
  useWebViews.getState().open({ id: 'w', session: 'a', url: 'https://example.com/', title: 'x' })
  usePanes.getState().place(T('prompt'), webLeaf('w'), 'bottom')
  usePanes.getState().select(webLeaf('w'))
  openTile({ kind: 'skill', host: 'a', props: { name: 's' }, key: 's' })
  expect(shape()).toBe(`x(a, y(${T('jobs')}|${T('prompt')}, ${webLeaf('w')}|${T('skill', 'a', 's')}))`)
  expect(Object.keys(useWebViews.getState().views)).toEqual(['w'])
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: T('skill', 'a', 's') })
})

test('a closed tile, replaced one included, reopens where it was, newest first; a deleted chat’s tiles do not', () => {
  openTile({ kind: 'jobs', host: 'a', props: {} })
  openTile({ kind: 'prompt', host: 'a', props: {} })
  usePanes.getState().select(T('jobs'))
  openTile({ kind: 'skill', host: 'a', props: { name: 's' }, key: 's' })
  expect(shape()).toBe(`x(a, y(${T('jobs')}|${T('skill', 'a', 's')}, ${T('prompt')}))`)
  closeTile(tileId('jobs', 'a'))
  usePanes.getState().place(T('prompt'), 'b', 'center')
  expect(usePanes.getState().closed.map((c) => c.leaf)).toEqual([T('jobs'), T('prompt')])
  expect(reopenClosed()).toBe(T('prompt'))
  expect(reopenClosed()).toBe(T('jobs'))
  // A replaced tile comes back beside the one that took its slot.
  expect(shape()).toBe(`x(a, y(${T('jobs')}|${T('skill', 'a', 's')}, b|${T('prompt')}))`)
  expect(usePanes.getState()).toMatchObject({ closed: [], selected: T('jobs') })
  expect(reopenClosed()).toBeNull()
  closeTile(tileId('jobs', 'a'))
  closeSessionTiles('a')
  expect(usePanes.getState().closed).toEqual([])
})

test('with a chat selected, or from inside the selected tile itself, a surface opens as a new split', () => {
  usePanes.getState().show('a')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  usePanes.getState().select(T('jobs'))
  openTile({ kind: 'job-terminal', host: T('jobs'), props: {}, key: 'j1' })
  expect(tree()).toEqual(['a', T('jobs'), T('job-terminal', 'a', 'j1')])
  usePanes.getState().select('a')
  openTile({ kind: 'prompt', host: 'a', props: {} })
  expect(tree()).toEqual(['a', T('jobs'), T('job-terminal', 'a', 'j1'), T('prompt')])
})

test('a web frame outlives its chat; a chat landing beside frames only goes left of all of them', () => {
  useWebViews.getState().open({ id: 'w', session: 'a', url: 'https://example.com/', title: 'x' })
  useWebViews.getState().open({ id: 'v', session: 'a', url: 'https://example.org/', title: 'y' })
  usePanes.getState().place('a', webLeaf('w'), 'right')
  usePanes.getState().place(webLeaf('w'), webLeaf('v'), 'bottom')
  usePanes.getState().place('a', 'b', 'bottom')
  usePanes.getState().close('a')
  usePanes.getState().close('b')
  expect(tree()).toEqual([webLeaf('w'), webLeaf('v')])
  usePanes.getState().show('c')
  expect(shape()).toBe(`x(c, y(${webLeaf('w')}, ${webLeaf('v')}))`)
})

test('a draft becoming its session retargets every tile it opened to the session, in place; no draft id survives', () => {
  const d = 'draft:x'
  usePanes.getState().place('a', d, 'right')
  openTile({ kind: 'memory', host: d, props: {} })
  openTile({ kind: 'skill', host: d, props: { name: 's' }, key: 's' })
  expect(shape()).toBe(`x(a, ${d}, y(${T('memory', d)}, ${T('skill', d, 's')}))`)
  promoteDraft(d, 's1')
  expect(shape()).toBe(`x(a, s1, y(${T('memory', 's1')}, ${T('skill', 's1', 's')}))`)
  expect(usePanes.getState().focused).toBe('s1')
  expect(tiles().every((t) => t.session === 's1')).toBe(true)
})

test('a draft whose session is already open elsewhere takes it into its own pane; a clashing tile closes', () => {
  const d = 'draft:y'
  usePanes.getState().place('a', 's1', 'right')
  openTile({ kind: 'memory', host: 's1', props: {} })
  usePanes.getState().place('a', d, 'bottom')
  openTile({ kind: 'memory', host: d, props: {} })
  promoteDraft(d, 's1')
  expect(tree().filter((l) => l === 's1')).toHaveLength(1)
  expect(tree()).not.toContain(d)
  expect(tiles().map((t) => t.id)).toEqual([tileId('memory', 's1')])
})

test('the same payload again keeps the view params', () => {
  openTile({ kind: 'media', host: 'a', props: { path: '/x.png', active: true }, key: 'x' })
  const leaf = T('media', 'a', 'x')
  const before = usePanes.getState().views[leaf]
  openTile({ kind: 'media', host: 'a', props: { path: '/x.png', active: true }, key: 'x' })
  expect(usePanes.getState().views[leaf]).toBe(before)
  openTile({ kind: 'media', host: 'a', props: { path: '/x.png', active: false }, key: 'x' })
  expect(usePanes.getState().views[leaf]?.params?.props).toEqual({ path: '/x.png', active: false })
})

test('a deleted chat takes its tiles with it, and only its own, also while the panes are unmounted', () => {
  usePanes.getState().place('a', 'b', 'right')
  openTile({ kind: 'prompt', host: 'a', props: {} })
  openTile({ kind: 'prompt', host: 'b', props: {} })
  openTile({ kind: 'jobs', host: 'b', props: {} })
  closeSessionTiles('a')
  // `b`'s prompt joined `a`'s as a tab (one panel per kind); its jobs went below that panel.
  expect(tree()).toEqual(['a', T('prompt', 'b'), T('jobs', 'b'), 'b'])
  // Another page: no workspace; the persisted layout and the mirror lose the tiles.
  unmount()
  closeSessionTiles('b')
  expect(tree()).toEqual(['a', 'b'])
  ws = tiling([], { persisted: true })
  expect(tree()).toEqual(['a', 'b'])
})
