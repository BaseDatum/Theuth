import { afterEach, expect, test, vi } from 'vitest'
import { chatLeaves, closeWeb, leaves, placeWeb, shown, usePanes } from './panes'
import { httpUrl, isLoopback, isWeb, useWebViews, webId, webLeaf } from './web-views'

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
