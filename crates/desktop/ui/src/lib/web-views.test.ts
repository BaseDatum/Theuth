import { afterEach, expect, test, vi } from 'vitest'
import { chatLeaves, closeWeb, leaves, placeWeb, shown, usePanes } from './panes'
import {
  STATE_EVERY_MS,
  WEB_TILE_MIN_WIDTH,
  type WebViewStateReport,
  collapseSidebarFor,
  httpUrl,
  isLoopback,
  isWeb,
  placeTile,
  stateForwarder,
  useWebViews,
  webId,
  webLeaf,
} from './web-views'

const native = vi.hoisted(() => ({ close: vi.fn(async () => {}) }))
vi.mock('@/lib/web-view-native', async (original) => ({ ...(await original<object>()), close: native.close }))

const reset = () => {
  usePanes.setState({ root: null, focused: null, drag: null })
  useWebViews.setState({ views: {} })
  native.close.mockClear()
}
afterEach(reset)

const ID = '0123456789abcdef'
const W = webLeaf(ID)

test('a web leaf is `web:<id>`; the same id opened again navigates the frame instead of adding one (205 D4)', () => {
  reset()
  expect(isWeb(W)).toBe(true)
  expect(isWeb('s1')).toBe(false)
  expect(webId(W)).toBe(ID)
  const { open, patch } = useWebViews.getState()
  open({ id: ID, session: 's1', url: 'http://localhost:3000/', title: 'dev' })
  patch(ID, { current: 'http://localhost:3000/login', popped: true })
  open({ id: ID, session: 's1', url: 'http://localhost:3000/docs', title: 'docs' })
  const views = useWebViews.getState().views
  expect(Object.keys(views)).toEqual([ID])
  expect(views[ID]).toMatchObject({ session: 's1', url: 'http://localhost:3000/docs', current: 'http://localhost:3000/docs', title: 'docs', popped: true })
})

test('closing a frame forgets it and destroys its native web view; an unknown id is no call', () => {
  reset()
  useWebViews.getState().open({ id: ID, session: 's1', url: 'https://example.com/', title: 'x' })
  useWebViews.getState().close(ID)
  expect(useWebViews.getState().views).toEqual({})
  expect(native.close).toHaveBeenCalledWith(ID)
  useWebViews.getState().close(ID)
  expect(native.close).toHaveBeenCalledTimes(1)
})

test('a web tile opens right of its session without taking focus; the session-id consumers never see it', () => {
  reset()
  // The single chat on screen counts as the session's pane.
  expect(placeWeb('s1', ID)).toBe(true)
  const { root, focused } = usePanes.getState()
  expect(leaves(root!)).toEqual(['s1', W])
  expect(focused).toBe('s1')
  expect(chatLeaves(root!)).toEqual(['s1'])
  // A phone shows the chat, not the frame.
  expect(shown(root, focused, 's1')).toBe('s1')
  // Placing it again leaves it where it is; a session with no pane here gets none.
  expect(placeWeb('s1', ID)).toBe(true)
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', W])
  expect(placeWeb('s9', 'fedcba9876543210')).toBe(false)
  // Focus never lands on the frame.
  usePanes.getState().focus(W)
  expect(usePanes.getState().focused).toBe('s1')
})

test('closing the chat beside a web tile keeps the tile as a tree (no route names it) and the URL where it was', () => {
  reset()
  placeWeb('s1', ID)
  expect(usePanes.getState().close('s1')).toBeNull()
  expect(usePanes.getState().root).toEqual({ session: W })
  expect(usePanes.getState().focused).toBeNull()
  // A chat routed to next goes beside the frame, not over it.
  usePanes.getState().show('s2')
  expect(leaves(usePanes.getState().root!)).toEqual(['s2', W])
  expect(usePanes.getState().focused).toBe('s2')
  // Closing the frame instead leaves the single chat.
  closeWeb(ID)
  expect(usePanes.getState().root).toBeNull()
})

test('New chat with a chat and a web tile is single-pane mode: the tile is no chat pane to turn into a draft', () => {
  reset()
  placeWeb('s1', ID)
  expect(usePanes.getState().fresh()).toBeNull()
  usePanes.getState().place('s1', 's2', 'bottom')
  const draft = usePanes.getState().fresh()
  expect(draft).not.toBeNull()
  expect(leaves(usePanes.getState().root!)).toEqual(['s1', draft, W])
})

test('the address field and every external link take http(s) only: a bare host gets http://, any other scheme is refused', () => {
  expect(httpUrl(' localhost:3000/x ')).toBe('http://localhost:3000/x')
  expect(httpUrl('example.com')).toBe('http://example.com')
  expect(httpUrl('https://example.com')).toBe('https://example.com')
  for (const bad of ['javascript:alert(1)', 'file:///etc/passwd', 'data:text/html,x', 'theuth-page://x', 'mailto:a@b.c', 'http://', ''])
    expect(httpUrl(bad)).toBeNull()
  expect(isLoopback('http://localhost:5173/')).toBe(true)
  expect(isLoopback('http://app.localhost/')).toBe(true)
  expect(isLoopback('http://127.0.0.1:8080/')).toBe(true)
  expect(isLoopback('http://[::1]:3000/')).toBe(true)
  expect(isLoopback('http://192.168.1.4/')).toBe(false)
  expect(isLoopback('https://example.com/')).toBe(false)
})

test('tile state goes to the engine only on change, at most once per view per STATE_EVERY_MS, the last of a burst included (209 D9)', () => {
  vi.useFakeTimers()
  const sent: WebViewStateReport[] = []
  let up = true
  const f = stateForwarder((r) => up && sent.push(r) > 0)
  const st = (view: string, url: string, loading: boolean): WebViewStateReport => ({ session: 's1', view, url, title: 't', loading })
  try {
    f.push(st('web:a', 'https://a.example/', true))
    expect(sent).toHaveLength(1)
    // A burst inside the window: only its last state goes, when the window ends.
    f.push(st('web:a', 'https://a.example/x', true))
    f.push(st('web:a', 'https://a.example/x', false))
    // Another view has its own window.
    f.push(st('web:b', 'https://b.example/', false))
    expect(sent.map((r) => r.view)).toEqual(['web:a', 'web:b'])
    vi.advanceTimersByTime(STATE_EVERY_MS - 1)
    expect(sent).toHaveLength(2)
    vi.advanceTimersByTime(1)
    expect(sent.at(-1)).toEqual(st('web:a', 'https://a.example/x', false))
    // The same state again is no call, even after the window.
    vi.advanceTimersByTime(STATE_EVERY_MS)
    f.push(st('web:a', 'https://a.example/x', false))
    expect(sent).toHaveLength(3)
    // Not delivered (no engine): the next change goes at once.
    up = false
    f.push(st('web:b', 'https://b.example/y', true))
    up = true
    f.push(st('web:b', 'https://b.example/y', false))
    expect(sent.at(-1)).toEqual(st('web:b', 'https://b.example/y', false))
    // A closed view's waiting state is dropped.
    f.push(st('web:b', 'https://b.example/z', true))
    f.drop('web:b')
    vi.advanceTimersByTime(STATE_EVERY_MS)
    expect(sent).toHaveLength(4)
  } finally {
    f.stop()
    vi.useRealTimers()
  }
})

test('209 D11: an iPad tile placed narrower than WEB_TILE_MIN_WIDTH with the sidebar open inline collapses it; nothing else does', () => {
  const ipad = { width: WEB_TILE_MIN_WIDTH - 1, sidebarOpen: true, phone: false, ios: true }
  expect(collapseSidebarFor(ipad)).toBe(true)
  expect(collapseSidebarFor({ ...ipad, width: 300 })).toBe(true)
  expect(collapseSidebarFor({ ...ipad, width: WEB_TILE_MIN_WIDTH })).toBe(false)
  // Already collapsed, a phone (one pane, the sidebar a sheet), the desktop, or a tile not laid out yet.
  expect(collapseSidebarFor({ ...ipad, sidebarOpen: false })).toBe(false)
  expect(collapseSidebarFor({ ...ipad, phone: true })).toBe(false)
  expect(collapseSidebarFor({ ...ipad, ios: false })).toBe(false)
  expect(collapseSidebarFor({ ...ipad, width: 0 })).toBe(false)
})

test('209 D11: a frame decides on its first measured width, once; a width of 0 decides nothing; closing it forgets the decision', () => {
  const ipad = { width: 300, sidebarOpen: true, phone: false, ios: true }
  // Not laid out yet: no decision, so the real width still counts.
  expect(placeTile('p1', { ...ipad, width: 0 })).toBe(false)
  expect(placeTile('p1', ipad)).toBe(true)
  // Decided: a remount (the user reopened the sidebar) does not fold it again.
  expect(placeTile('p1', ipad)).toBe(false)
  // A wide first placement decides too (nothing to fold), and later narrowing does not fold.
  expect(placeTile('p2', { ...ipad, width: WEB_TILE_MIN_WIDTH + 100 })).toBe(false)
  expect(placeTile('p2', ipad)).toBe(false)
  useWebViews.getState().open({ id: 'p1', session: 's', url: 'https://a.example/', title: 'A' })
  useWebViews.getState().close('p1')
  expect(placeTile('p1', ipad)).toBe(true)
  useWebViews.getState().close('p1')
  useWebViews.getState().close('p2')
})
