import { expect, test } from 'vitest'
import { leaves, usePanes } from './panes'

const reset = () => usePanes.setState({ root: null, focused: null, drag: null })

test('placing splits the target on the named side and moves an already-open session instead of duplicating it', () => {
  reset()
  usePanes.getState().place('a', 'b', 'right')
  expect(leaves(usePanes.getState().root!)).toEqual(['a', 'b'])
  usePanes.getState().place('a', 'c', 'top')
  expect(leaves(usePanes.getState().root!)).toEqual(['c', 'a', 'b'])
  // `b` moves from the right of `a` to below `c`: one pane per session.
  usePanes.getState().place('c', 'b', 'bottom')
  expect(leaves(usePanes.getState().root!)).toEqual(['c', 'b', 'a'])
  expect(usePanes.getState().focused).toBe('b')
  // Dropping on the middle replaces.
  usePanes.getState().place('a', 'd', 'center')
  expect(leaves(usePanes.getState().root!)).toEqual(['c', 'b', 'd'])
})

test('closing collapses to the sibling; the last split returns to single-chat mode and hands the URL a survivor', () => {
  reset()
  usePanes.getState().place('a', 'b', 'left')
  usePanes.getState().place('b', 'c', 'bottom')
  expect(usePanes.getState().close('a')).toBeNull() // `c` is focused; nothing to navigate
  expect(leaves(usePanes.getState().root!)).toEqual(['b', 'c'])
  expect(usePanes.getState().close('c')).toBe('b')
  expect(usePanes.getState().root).toBeNull()
})

test('a route landing on a session outside the tree shows it in the focused pane', () => {
  reset()
  usePanes.getState().place('a', 'b', 'right')
  usePanes.getState().focus('a')
  usePanes.getState().show('z')
  expect(leaves(usePanes.getState().root!)).toEqual(['z', 'b'])
  usePanes.getState().show('b')
  expect(leaves(usePanes.getState().root!)).toEqual(['z', 'b'])
  expect(usePanes.getState().focused).toBe('b')
})
