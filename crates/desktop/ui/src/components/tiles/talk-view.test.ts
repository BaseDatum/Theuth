import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { closeLeaves, onPaneClosed, tileLeaf, usePanes } from '@/lib/panes'
import { tileProps, tiling, unmount, untile } from '@/lib/panes.testing'
import { closeAllTiles, tileId } from '@/lib/tiles'
import { openTalkView, talkPaneToKeep } from './open'

const talkLeaf = (session: string) => tileLeaf(tileId('view', '', `/talk/${session}`))
const tree = () => usePanes.getState().leaves
const phone = (matches: boolean) => {
  window.matchMedia = () => ({ matches, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} }) as unknown as MediaQueryList
}

beforeEach(() => {
  phone(false)
  tiling(['a'])
})
afterEach(() => {
  phone(false)
  untile()
})

test("the composer's Talk opens the talk view in a pane titled Talk right of the chat, selected, the chat kept", () => {
  const navigate = vi.fn()
  expect(openTalkView('a', 'a', navigate)).toBe(talkLeaf('a'))
  expect(tree()).toEqual(['a', talkLeaf('a')])
  expect(tileProps(tileId('view', '', '/talk/a'))).toEqual({ path: '/talk/a', title: 'Talk' })
  expect(usePanes.getState().selected).toBe(talkLeaf('a'))
  expect(usePanes.getState().focused).toBe('a')
  expect(navigate).not.toHaveBeenCalled()
})

test('Talk again while its pane is open focuses that pane instead of opening another', () => {
  const navigate = vi.fn()
  openTalkView('a', 'a', navigate)
  usePanes.getState().select('a')
  expect(openTalkView('a', 'a', navigate)).toBe(talkLeaf('a'))
  expect(tree()).toEqual(['a', talkLeaf('a')])
  expect(usePanes.getState().selected).toBe(talkLeaf('a'))
  expect(navigate).not.toHaveBeenCalled()
})

test('where nothing tiles (a phone, no pane host, no workspace) Talk navigates to the full talk view', () => {
  const navigate = vi.fn()
  phone(true)
  expect(openTalkView('a', 'a', navigate)).toBeNull()
  phone(false)
  expect(openTalkView('a', null, navigate)).toBeNull()
  expect(tree()).toEqual(['a'])
  untile()
  expect(openTalkView('a', 'a', navigate)).toBeNull()
  expect(navigate.mock.calls).toEqual([['a'], ['a'], ['a']])
})

test("closing the talk pane (its ×, a layout close) fires the talk view's close hook; another pane closing does not", () => {
  const leaf = openTalkView('a', 'a', vi.fn())!
  const ended = vi.fn()
  onPaneClosed(usePanes.getState().ws!, leaf, ended)
  usePanes.getState().close('a')
  expect(ended).not.toHaveBeenCalled()
  closeLeaves([leaf])
  expect(ended).toHaveBeenCalledTimes(1)
  expect(tree()).not.toContain(leaf)
})

test('the workspace unmounting (another page) does not count as the talk pane closing', () => {
  const leaf = openTalkView('a', 'a', vi.fn())!
  const ended = vi.fn()
  onPaneClosed(usePanes.getState().ws!, leaf, ended)
  unmount()
  expect(ended).not.toHaveBeenCalled()
})

test("crossing into the phone layout closes the talk pane without ending the talk, and names its session for the full view", () => {
  const leaf = openTalkView('a', 'a', vi.fn())!
  const ended = vi.fn()
  const off = onPaneClosed(usePanes.getState().ws!, leaf, ended)
  expect(talkPaneToKeep(null)).toBeNull()
  expect(talkPaneToKeep('b')).toBeNull()
  expect(talkPaneToKeep('a')).toBe('a')
  closeAllTiles()
  expect(tree()).toEqual(['a'])
  expect(ended).not.toHaveBeenCalled()
  // The next deliberate close is seen again: the layout flag does not stick.
  off()
  const again = openTalkView('a', 'a', vi.fn())!
  onPaneClosed(usePanes.getState().ws!, again, ended)
  closeLeaves([again])
  expect(ended).toHaveBeenCalledTimes(1)
})
