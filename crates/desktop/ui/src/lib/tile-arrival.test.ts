import '../components/settings/test-dom'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { openWhiteboardTile } from '@/components/tiles/open'
import type * as Open from '@/components/tiles/open'
import { tileLeaf, usePanes } from './panes'
import { tileIds, tileProps, tiling, untile } from './panes.testing'
import { diffViewOpened, fileViewOpened, noteViewOpened, ptyViewOpened, whiteboardViewOpened } from './tile-arrival'
import { tileId, tiles } from './tiles'
import { useWebViews } from './web-views'

// The whiteboard opener alone is stubbed (the Trellis test workspace registers no `tile:whiteboard` view type); every
// other opener stays real. `tileable` stays real too: a chat on screen in a tiling pane.
vi.mock('@/components/tiles/open', async (actual) => ({
  ...(await actual<typeof Open>()),
  openWhiteboardTile: vi.fn((): string | null => 'whiteboard-tile'),
}))

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
  tiling(['s1'])
  useWebViews.setState({ views: {} })
})
afterEach(() => {
  untile()
  document.body.innerHTML = ''
})

const tree = () => usePanes.getState().leaves
const tile = (id: string) => tiles().find((t) => t.id === id)

test('a visible chat gets the job terminal tile beside it, keyed by session/job and labelled with the title', () => {
  onScreen('s1')
  ptyViewOpened(pty, true)
  const id = tileId('job-terminal', 's1', 's1/j3')
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(tile(id)).toMatchObject({ kind: 'job-terminal', session: 's1', props: { session: 's1', job: 'j3', label: 'Dev server' } })
})

test('a background job (pty: false) gets its read-only log tile instead, under the same key', () => {
  onScreen('s1')
  ptyViewOpened({ ...pty, job: 'j4', pty: false, title: 'cargo build' }, true)
  const id = tileId('job-log', 's1', 's1/j4')
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(tile(id)).toMatchObject({ kind: 'job-log', session: 's1', props: { session: 's1', job: 'j4', label: 'cargo build' } })
})

test('the same terminal or note again keeps one tile each', () => {
  onScreen('s1')
  ptyViewOpened(pty, true)
  ptyViewOpened({ ...pty, title: 'Dev server (2)' }, true)
  noteViewOpened(note, true)
  noteViewOpened(note, true)
  expect(tree()).toEqual(['s1', tileLeaf(tileId('job-terminal', 's1', 's1/j3')), tileLeaf(tileId('note', 's1', DOC))])
  expect(tileIds()).toHaveLength(2)
  expect(tileProps(tileId('job-terminal', 's1', 's1/j3'))).toMatchObject({ label: 'Dev server (2)' })
})

test('a visible chat gets the note tile keyed by the document, carrying its block; a null block is left out', () => {
  onScreen('s1')
  noteViewOpened(note, true)
  const id = tileId('note', 's1', DOC)
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(tile(id)).toMatchObject({ kind: 'note', session: 's1', props: { doc: DOC, title: 'Plan', block: 'b7' } })
  noteViewOpened({ ...note, block: null }, true)
  expect(tileProps(id)).toEqual({ doc: DOC, title: 'Plan' })
})

test('nothing opens for a chat that is not visible, or visible but not in a tiling pane', () => {
  onScreen('s1')
  ptyViewOpened(pty, false)
  noteViewOpened(note, false)
  document.body.innerHTML = ''
  ptyViewOpened(pty, true)
  noteViewOpened(note, true)
  expect(tree()).toEqual(['s1'])
})

test('a visible chat gets the file tile keyed by the descriptor id, carrying the line; a null bound is left out', () => {
  onScreen('s1')
  fileViewOpened(file, true)
  const id = tileId('file', 's1', FID)
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(tile(id)).toMatchObject({ kind: 'file', session: 's1' })
  expect(tileProps(id)).toEqual({ session: 's1', path: '/repo/src/main.rs', id: FID, title: 'main.rs', lang: 'rs', line: 10 })
  fileViewOpened({ ...file, line: 3, end_line: 9 }, true)
  expect(tileIds()).toEqual([id])
  expect(tileProps(id)).toMatchObject({ line: 3, end_line: 9 })
})

test('a visible chat gets the diff tile keyed by the descriptor id, carrying the spec it re-requests', () => {
  onScreen('s1')
  diffViewOpened(diff, true)
  diffViewOpened(diff, true)
  const id = tileId('diff', 's1', 'fedcba9876543210')
  expect(tree()).toEqual(['s1', tileLeaf(id)])
  expect(tile(id)).toMatchObject({ kind: 'diff', session: 's1', props: { session: 's1', id: 'fedcba9876543210', title: 'working tree', spec } })
})

test('no file or diff tile opens for a hidden chat or one outside a tiling pane', () => {
  onScreen('s1')
  fileViewOpened(file, false)
  diffViewOpened(diff, false)
  document.body.innerHTML = ''
  fileViewOpened(file, true)
  diffViewOpened(diff, true)
  expect(tileIds()).toEqual([])
})

const WB = '01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b'
const board = { type: 'WhiteboardViewOpened', session: 's1', call_id: 'c5', doc: WB, title: 'Attention inbox' } as const
const opened = vi.mocked(openWhiteboardTile)

test('218: a visible, tileable chat gets the whiteboard tile for the document; nothing navigates', () => {
  opened.mockClear()
  onScreen('s1')
  const go = vi.fn()
  whiteboardViewOpened(board, true, go)
  expect(opened.mock.calls).toEqual([['s1', { doc: WB, title: 'Attention inbox' }]])
  expect(go).not.toHaveBeenCalled()
})

test('218: a hidden chat, one outside a tiling pane, or a tile that cannot open navigates to the whiteboard page', () => {
  opened.mockClear()
  const go = vi.fn()
  onScreen('s1')
  whiteboardViewOpened(board, false, go)
  opened.mockReturnValueOnce(null)
  whiteboardViewOpened(board, true, go)
  document.body.innerHTML = ''
  whiteboardViewOpened({ ...board, session: 's 2' }, true, go)
  expect(opened).toHaveBeenCalledTimes(1)
  expect(go.mock.calls).toEqual([[`/whiteboards/${WB}?session=s1`], [`/whiteboards/${WB}?session=s1`], [`/whiteboards/${WB}?session=s%202`]])
})
