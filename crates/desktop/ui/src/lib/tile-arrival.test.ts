import '../components/settings/test-dom'
import { afterEach, beforeEach, expect, test } from 'vitest'
import { leaves, tileLeaf, usePanes } from './panes'
import { diffViewOpened, fileViewOpened, noteViewOpened, ptyViewOpened } from './tile-arrival'
import { tileId, useTiles } from './tiles'
import { useWebViews } from './web-views'

const DOC = '01jabcdefghjkmnpqrstvwxyz0'
const pty = { type: 'PtyViewOpened', session: 's1', call_id: 'c1', job: 'j3', name: 'dev', title: 'Dev server', running: true, pty: true, spawned: false }
const note = { type: 'NoteViewOpened', session: 's1', call_id: 'c2', doc: DOC, kind: 'note', title: 'Plan', block: 'b7' }
const FID = '0123456789abcdef'
const file = { type: 'FileViewOpened', session: 's1', call_id: 'c3', id: FID, path: '/repo/src/main.rs', title: 'main.rs', lang: 'rs', line: 10, end_line: null, size: 99, lines: 40 }
const spec = { mode: 'worktree' as const, root: '/repo', a: null, b: null, three_dot: false, paths: [] }
const diff = { type: 'DiffViewOpened', session: 's1', call_id: 'c4', id: 'fedcba9876543210', title: 'working tree', spec, files: 2, added: 5, removed: 1 }

/** `s1`'s chat on screen in a pane: what `tileable` looks for. */
const onScreen = (session: string) => {
  const el = document.body.appendChild(document.createElement('div'))
  el.dataset.pane = session
}

beforeEach(() => {
  usePanes.setState({ root: null, focused: 's1', selected: null, drag: null })
  useTiles.setState({ tiles: {} })
  useWebViews.setState({ views: {} })
})
afterEach(() => {
  document.body.innerHTML = ''
})

const tree = () => leaves(usePanes.getState().root!)

test('a visible chat gets the job terminal tile beside it, keyed by session/job and labelled with the title', () => {
  onScreen('s1')
  ptyViewOpened(pty, true)
  const id = tileId('job-terminal', 's1', 's1/j3')
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(useTiles.getState().tiles[id]).toMatchObject({ kind: 'job-terminal', session: 's1', props: { session: 's1', job: 'j3', label: 'Dev server' } })
})

test('a background job (pty: false) gets its read-only log tile instead, under the same key', () => {
  onScreen('s1')
  ptyViewOpened({ ...pty, job: 'j4', pty: false, title: 'cargo build' }, true)
  const id = tileId('job-log', 's1', 's1/j4')
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(useTiles.getState().tiles[id]).toMatchObject({ kind: 'job-log', session: 's1', props: { session: 's1', job: 'j4', label: 'cargo build' } })
})

test('the same terminal or note again keeps one tile each', () => {
  onScreen('s1')
  ptyViewOpened(pty, true)
  ptyViewOpened({ ...pty, title: 'Dev server (2)' }, true)
  noteViewOpened(note, true)
  noteViewOpened(note, true)
  expect(tree()).toEqual(['s1', tileLeaf(tileId('job-terminal', 's1', 's1/j3')), tileLeaf(tileId('note', 's1', DOC))])
  expect(Object.keys(useTiles.getState().tiles)).toHaveLength(2)
  expect(useTiles.getState().tiles[tileId('job-terminal', 's1', 's1/j3')]?.props).toMatchObject({ label: 'Dev server (2)' })
})

test('a visible chat gets the note tile keyed by the document, carrying its block; a null block is left out', () => {
  onScreen('s1')
  noteViewOpened(note, true)
  const id = tileId('note', 's1', DOC)
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(useTiles.getState().tiles[id]).toMatchObject({ kind: 'note', session: 's1', props: { doc: DOC, title: 'Plan', block: 'b7' } })
  noteViewOpened({ ...note, block: null }, true)
  expect(useTiles.getState().tiles[id]?.props).toEqual({ doc: DOC, title: 'Plan' })
})

test('nothing opens for a chat that is not visible, or visible but not in a tiling pane', () => {
  onScreen('s1')
  ptyViewOpened(pty, false)
  noteViewOpened(note, false)
  document.body.innerHTML = ''
  ptyViewOpened(pty, true)
  noteViewOpened(note, true)
  expect(usePanes.getState().root).toBeNull()
  expect(useTiles.getState().tiles).toEqual({})
})

test('a visible chat gets the file tile keyed by the descriptor id, carrying the line; a null bound is left out', () => {
  onScreen('s1')
  fileViewOpened(file, true)
  const id = tileId('file', 's1', FID)
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(useTiles.getState().tiles[id]).toMatchObject({ kind: 'file', session: 's1' })
  expect(useTiles.getState().tiles[id]?.props).toEqual({ session: 's1', path: '/repo/src/main.rs', id: FID, title: 'main.rs', lang: 'rs', line: 10 })
  fileViewOpened({ ...file, line: 3, end_line: 9 }, true)
  expect(Object.keys(useTiles.getState().tiles)).toEqual([id])
  expect(useTiles.getState().tiles[id]?.props).toMatchObject({ line: 3, end_line: 9 })
})

test('a visible chat gets the diff tile keyed by the descriptor id, carrying the spec it re-requests', () => {
  onScreen('s1')
  diffViewOpened(diff, true)
  diffViewOpened(diff, true)
  const id = tileId('diff', 's1', 'fedcba9876543210')
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(useTiles.getState().tiles[id]).toMatchObject({ kind: 'diff', session: 's1', props: { session: 's1', id: 'fedcba9876543210', title: 'working tree', spec } })
})

test('no file or diff tile opens for a hidden chat or one outside a tiling pane', () => {
  onScreen('s1')
  fileViewOpened(file, false)
  diffViewOpened(diff, false)
  document.body.innerHTML = ''
  fileViewOpened(file, true)
  diffViewOpened(diff, true)
  expect(useTiles.getState().tiles).toEqual({})
})
