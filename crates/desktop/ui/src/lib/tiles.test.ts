import { beforeEach, expect, test } from 'vitest'
import { chatLeaves, leaves, tileLeaf, usePanes } from './panes'
import { closeSessionTiles, closeTile, openTile, openTileOf, promoteDraft, tileId, useTiles } from './tiles'
import { useWebViews, webLeaf } from './web-views'

beforeEach(() => {
  usePanes.setState({ root: null, focused: 'a', selected: null, drag: null })
  useTiles.setState({ tiles: {} })
  useWebViews.setState({ views: {} })
})

const tree = () => leaves(usePanes.getState().root!)
const T = (kind: string, session = 'a', key?: string) => tileLeaf(tileId(kind, session, key))

test('a tile opens right of its chat; opening it again reuses the tile with the new props, never a second one', () => {
  expect(openTile({ kind: 'artifact', host: 'a', props: { callId: 'c1' } })).toBe(tileId('artifact', 'a'))
  expect(tree()).toEqual(['a', T('artifact')])
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', a: { session: 'a' } })
  openTile({ kind: 'artifact', host: 'a', props: { callId: 'c2' } })
  expect(tree()).toEqual(['a', T('artifact')])
  expect(openTileOf('artifact', 'a')?.props).toEqual({ callId: 'c2' })
  // A tile never takes focus: the URL stays on the chat.
  expect(usePanes.getState().focused).toBe('a')
})

test('the same kind under another key is another tile; a session with tiles stacks the next below its last one', () => {
  openTile({ kind: 'skill', host: 'a', props: { name: 'x' }, key: 'x' })
  openTile({ kind: 'skill', host: 'a', props: { name: 'y' }, key: 'y' })
  expect(tree()).toEqual(['a', T('skill', 'a', 'x'), T('skill', 'a', 'y')])
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', b: { dir: 'col', a: { session: T('skill', 'a', 'x') } } })
})

test('a web frame of the session counts as one of its tiles for placement', () => {
  useWebViews.getState().open({ id: 'w', session: 'a', url: 'https://example.com/', title: 'x' })
  usePanes.getState().place('a', webLeaf('w'), 'right')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  expect(tree()).toEqual(['a', webLeaf('w'), T('jobs')])
})

test('a trigger inside a tile opens for that tile\'s chat', () => {
  openTile({ kind: 'jobs', host: 'a', props: {} })
  openTile({ kind: 'job-terminal', host: T('jobs'), props: {}, key: 'j1' })
  expect(useTiles.getState().tiles[tileId('job-terminal', 'a', 'j1')]?.session).toBe('a')
  expect(tree()).toEqual(['a', T('jobs'), T('job-terminal', 'a', 'j1')])
})

test('nothing opens for a chat with no pane in a split tree', () => {
  usePanes.getState().place('a', 'b', 'right')
  expect(openTile({ kind: 'jobs', host: 'z', props: {} })).toBeNull()
  expect(useTiles.getState().tiles).toEqual({})
  expect(tree()).toEqual(['a', 'b'])
})

test('closing the last tile returns to the single chat; tiles are never chat leaves and never focused', () => {
  const id = openTile({ kind: 'prompt', host: 'a', props: {} })!
  expect(chatLeaves(usePanes.getState().root!)).toEqual(['a'])
  usePanes.getState().focus(tileLeaf(id))
  expect(usePanes.getState().focused).toBe('a')
  closeTile(id)
  expect(usePanes.getState().root).toBeNull()
  expect(useTiles.getState().tiles).toEqual({})
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
  expect(useTiles.getState().tiles[tileId('prompt', 'a')]).toMatchObject({ kind: 'prompt', session: 'a' })
})

test('a selected tile is taken over by the chat picked in the sidebar: same pane, its payload gone; the URL never names it', () => {
  usePanes.getState().show('a')
  openTile({ kind: 'artifact', host: 'a', props: { callId: 'c1' } })
  usePanes.getState().resize([], 0.3)
  usePanes.getState().select(T('artifact'))
  expect(usePanes.getState().focused).toBe('a')
  usePanes.getState().show('b')
  expect(usePanes.getState().root).toEqual({ dir: 'row', ratio: 0.3, a: { session: 'a' }, b: { session: 'b' } })
  expect(useTiles.getState().tiles).toEqual({})
  expect(usePanes.getState()).toMatchObject({ focused: 'b', selected: 'b' })
})

test('a surface opened while a tile (or web view) is selected takes over its pane in place and disposes it', () => {
  usePanes.getState().show('a')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  usePanes.getState().resize([], 0.3)
  usePanes.getState().select(T('jobs'))
  openTile({ kind: 'prompt', host: 'a', props: {} })
  expect(usePanes.getState().root).toEqual({ dir: 'row', ratio: 0.3, a: { session: 'a' }, b: { session: T('prompt') } })
  expect(Object.keys(useTiles.getState().tiles)).toEqual([tileId('prompt', 'a')])
  expect(usePanes.getState()).toMatchObject({ focused: 'a', selected: T('prompt') })
  // A web view in the selected pane is closed for good when a tile takes it over.
  useWebViews.getState().open({ id: 'w', session: 'a', url: 'https://example.com/', title: 'x' })
  usePanes.getState().place(T('prompt'), webLeaf('w'), 'bottom')
  usePanes.getState().select(webLeaf('w'))
  openTile({ kind: 'skill', host: 'a', props: { name: 's' }, key: 's' })
  expect(tree()).toEqual(['a', T('prompt'), T('skill', 'a', 's')])
  expect(useWebViews.getState().views).toEqual({})
  expect(usePanes.getState().focused).toBe('a')
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

test('a web frame outlives its chat; a chat landing beside frames only goes left of the whole tree', () => {
  useWebViews.getState().open({ id: 'w', session: 'a', url: 'https://example.com/', title: 'x' })
  usePanes.getState().place('a', webLeaf('w'), 'right')
  usePanes.getState().place(webLeaf('w'), webLeaf('v'), 'bottom')
  usePanes.getState().place('a', 'b', 'bottom')
  usePanes.getState().close('a')
  usePanes.getState().close('b')
  expect(tree()).toEqual([webLeaf('w'), webLeaf('v')])
  usePanes.getState().show('c')
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', a: { session: 'c' }, b: { dir: 'col', a: { session: webLeaf('w') }, b: { session: webLeaf('v') } } })
})

test('a draft becoming its session retargets every tile it opened to the session, in place; no draft id survives', () => {
  const d = 'draft:x'
  usePanes.getState().place('a', d, 'right')
  openTile({ kind: 'memory', host: d, props: {} })
  openTile({ kind: 'skill', host: d, props: { name: 's' }, key: 's' })
  expect(tree()).toEqual(['a', d, T('memory', d), T('skill', d, 's')])
  promoteDraft(d, 's1')
  expect(tree()).toEqual(['a', 's1', T('memory', 's1'), T('skill', 's1', 's')])
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', a: { session: 'a' }, b: { dir: 'row', a: { session: 's1' }, b: { dir: 'col', a: { session: T('memory', 's1') } } } })
  expect(usePanes.getState().focused).toBe('s1')
  expect(Object.keys(useTiles.getState().tiles).sort()).toEqual([tileId('memory', 's1'), tileId('skill', 's1', 's')])
  expect(Object.values(useTiles.getState().tiles).every((t) => t.session === 's1')).toBe(true)
})

test('the same payload again keeps the stored object', () => {
  openTile({ kind: 'media', host: 'a', props: { path: '/x.png', active: true }, key: 'x' })
  const before = useTiles.getState().tiles[tileId('media', 'a', 'x')]
  openTile({ kind: 'media', host: 'a', props: { path: '/x.png', active: true }, key: 'x' })
  expect(useTiles.getState().tiles[tileId('media', 'a', 'x')]).toBe(before)
  openTile({ kind: 'media', host: 'a', props: { path: '/x.png', active: false }, key: 'x' })
  expect(useTiles.getState().tiles[tileId('media', 'a', 'x')]).not.toBe(before)
})

test('a deleted chat takes its tiles with it, and only its own', () => {
  usePanes.getState().place('a', 'b', 'right')
  openTile({ kind: 'prompt', host: 'a', props: {} })
  openTile({ kind: 'prompt', host: 'b', props: {} })
  closeSessionTiles('a')
  expect(tree()).toEqual(['a', 'b', T('prompt', 'b')])
  expect(Object.keys(useTiles.getState().tiles)).toEqual([tileId('prompt', 'b')])
})

test('moving a leaf: each edge of another leaf, a centre drop swaps, a drop on itself does nothing; focus stays', () => {
  usePanes.getState().place('a', 'b', 'right')
  const t = T('jobs')
  openTile({ kind: 'jobs', host: 'a', props: {} })
  usePanes.getState().focus('a')
  const { move } = usePanes.getState()
  expect(tree()).toEqual(['a', t, 'b'])
  move(t, 'b', 'right')
  expect(tree()).toEqual(['a', 'b', t])
  move(t, 'a', 'left')
  expect(tree()).toEqual([t, 'a', 'b'])
  move(t, 'b', 'top')
  expect(tree()).toEqual(['a', t, 'b'])
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', b: { dir: 'col', a: { session: t }, b: { session: 'b' } } })
  move(t, 'a', 'bottom')
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', a: { dir: 'col', a: { session: 'a' }, b: { session: t } }, b: { session: 'b' } })
  const before = usePanes.getState().root
  move('b', 'a', 'center')
  expect(tree()).toEqual(['b', t, 'a'])
  // The swap keeps the shape: only the two leaves trade places.
  expect(usePanes.getState().root).toMatchObject({ dir: 'row', a: { dir: 'col' } })
  move('a', 'a', 'center')
  move('a', 'a', 'left')
  expect(tree()).toEqual(['b', t, 'a'])
  expect(before).not.toBe(usePanes.getState().root)
  expect(usePanes.getState().focused).toBe('a')
})
