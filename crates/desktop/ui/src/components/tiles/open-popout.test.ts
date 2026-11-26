import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Popout from '@/lib/popout'
import { tileLeaf, usePanes } from '@/lib/panes'
import { tiling, untile } from '@/lib/panes.testing'
import { tileId } from '@/lib/tiles'
import { useNotices } from '@/store/notices'
import { openRoute, openTalkView, openViewFromCard } from './open'

// A popped window (`lib/popout.ts` `POPOUT`) is read once at load; here it is switched per test.
const win = vi.hoisted(() => ({ popped: false, opened: [] as unknown[] }))
vi.mock('@/lib/popout', async (original) => ({
  ...(await original<typeof Popout>()),
  isPopout: () => win.popped,
  openPopout: async (t: unknown) => {
    win.opened.push(t)
  },
}))

const phone = (matches: boolean) => {
  window.matchMedia = () => ({ matches, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} }) as unknown as MediaQueryList
}
const errors = () => useNotices.getState().history.filter((n) => n.kind === 'error')

beforeEach(() => {
  win.popped = false
  win.opened = []
  phone(false)
})
afterEach(() => {
  phone(false)
  untile()
})

test("a popped whiteboard's Open in pane opens the view in a window of its own and never navigates the whiteboard away", () => {
  win.popped = true
  untile()
  const navigate = vi.fn()
  expect(openViewFromCard({ path: '/tasks/graph?project=p', title: 'Graph' }, { navigate })).toBeNull()
  expect(navigate).not.toHaveBeenCalled()
  expect(win.opened).toEqual([{ kind: 'page', path: '/tasks/graph?project=p' }])
})

test('a narrow popped window is no phone: it still opens a window, not the route in place', () => {
  win.popped = true
  untile()
  phone(true)
  const navigate = vi.fn()
  openViewFromCard({ path: '/tasks' }, { navigate })
  expect(navigate).not.toHaveBeenCalled()
  expect(win.opened).toEqual([{ kind: 'page', path: '/tasks' }])
})

test('in the main window with a workspace Open in pane splits beside the chat', () => {
  tiling(['a'])
  const navigate = vi.fn()
  const leaf = openViewFromCard({ path: '/tasks/graph' }, { navigate })
  expect(leaf).toBe(tileLeaf(tileId('view', '', '/tasks/graph')))
  expect(usePanes.getState().leaves).toEqual(['a', leaf])
  expect(navigate).not.toHaveBeenCalled()
  expect(win.opened).toEqual([])
})

test('on a phone Open in pane navigates to the route', () => {
  phone(true)
  const navigate = vi.fn()
  expect(openViewFromCard({ path: 'tasks' }, { navigate })).toBeNull()
  expect(navigate).toHaveBeenCalledWith('/tasks')
  expect(win.opened).toEqual([])
})

test('a route that does not resolve opens nothing and lands nowhere: the status bar says why', () => {
  phone(true)
  const navigate = vi.fn()
  const before = errors().length
  expect(openViewFromCard({ path: '/no-such-page' }, { navigate })).toBeNull()
  expect(navigate).not.toHaveBeenCalled()
  expect(errors().length).toBe(before + 1)
})

test("a popped chat's Talk opens the talk window instead of navigating the chat away", () => {
  win.popped = true
  untile()
  const navigate = vi.fn()
  expect(openTalkView('s 1', null, navigate)).toBeNull()
  expect(navigate).not.toHaveBeenCalled()
  expect(win.opened).toEqual([{ kind: 'talk', session: 's 1' }])
})

test('leaving a card for a chat opens its window from a popped window, navigates in the main one', () => {
  const navigate = vi.fn()
  win.popped = true
  openRoute('/c/s%201', navigate)
  openRoute('/new', navigate)
  expect(win.opened).toEqual([{ kind: 'chat', session: 's 1' }, { kind: 'page', path: '/new' }])
  expect(navigate).not.toHaveBeenCalled()
  win.popped = false
  openRoute('/c/s%201', navigate)
  expect(navigate).toHaveBeenCalledWith('/c/s%201')
})
