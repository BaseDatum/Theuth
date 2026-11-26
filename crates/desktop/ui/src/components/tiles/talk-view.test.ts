import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { tileLeaf, usePanes } from '@/lib/panes'
import { tileProps, tiling, untile } from '@/lib/panes.testing'
import { tileId } from '@/lib/tiles'
import { openTalkView } from './open'

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
