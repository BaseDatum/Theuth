import { expect, test } from 'vitest'
import { draftProject, isDraft, leaves, newDraft, usePanes } from './panes'

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

test('a dropped New chat is a draft leaf until its first send swaps the created session in', () => {
  reset()
  const d = newDraft()
  expect(isDraft(d)).toBe(true)
  expect(isDraft('a')).toBe(false)
  usePanes.getState().place('a', d, 'right')
  expect(leaves(usePanes.getState().root!)).toEqual(['a', d])
  expect(usePanes.getState().focused).toBe(d)
  usePanes.getState().place(d, 's1', 'center')
  expect(leaves(usePanes.getState().root!)).toEqual(['a', 's1'])
  expect(usePanes.getState().focused).toBe('s1')
})

test('a lone draft is not collapsed to the route chat; closing hands the URL a real survivor, never a draft', () => {
  reset()
  const d = newDraft()
  // Centre drop on the single chat replaces it with the draft: the tree must survive, or the chat would come back.
  usePanes.getState().place('a', d, 'center')
  expect(leaves(usePanes.getState().root!)).toEqual([d])
  usePanes.getState().place(d, 'b', 'left')
  usePanes.getState().place('b', 'c', 'top')
  usePanes.getState().focus('c')
  expect(usePanes.getState().close('c')).toBe('b')
  usePanes.getState().focus('b')
  expect(usePanes.getState().close('b')).toBeNull()
  expect(usePanes.getState().focused).toBe(d)
  expect(leaves(usePanes.getState().root!)).toEqual([d])
})

test('New chat in a split view replaces the focused pane with a draft; a single pane leaves the tree to the route', () => {
  reset()
  expect(usePanes.getState().fresh()).toBeNull()
  expect(usePanes.getState().root).toBeNull()
  usePanes.getState().place('a', 'b', 'right')
  usePanes.getState().focus('a')
  const d = usePanes.getState().fresh()
  expect(isDraft(d)).toBe(true)
  expect(leaves(usePanes.getState().root!)).toEqual([d, 'b'])
  expect(usePanes.getState().focused).toBe(d)
  // Focused on something no longer shown: the first pane takes it.
  usePanes.getState().focus('gone')
  const e = usePanes.getState().fresh()
  expect(leaves(usePanes.getState().root!)).toEqual([e, 'b'])
})

test('New chat on a focused draft keeps it instead of stacking a second one', () => {
  reset()
  usePanes.getState().place('a', 'b', 'right')
  const d = usePanes.getState().fresh()
  expect(usePanes.getState().fresh()).toBe(d)
  expect(leaves(usePanes.getState().root!)).toEqual(['a', d])
})

test('a draft carries the project preset of "New chat in <project>"', () => {
  reset()
  usePanes.getState().place('a', 'b', 'right')
  const d = usePanes.getState().fresh('Q&A / ops')!
  expect(isDraft(d)).toBe(true)
  expect(draftProject(d)).toBe('Q&A / ops')
  expect(draftProject(newDraft())).toBeUndefined()
  expect(draftProject(newDraft(null))).toBeUndefined()
})
