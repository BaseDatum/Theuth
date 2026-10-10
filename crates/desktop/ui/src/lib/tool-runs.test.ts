// fix/thinking-open-while-streaming: a thinking block's open state is display state keyed by entry id, and opening one
// while it streams pins it so `foldRuns` cannot fold it into a tool run.
import { beforeEach, expect, test } from 'vitest'
import { useToolRuns } from './tool-runs'

const reset = () => useToolRuns.setState({ thinking: {}, pinned: new Set() })
beforeEach(reset)

test('opening a streaming block pins it; closing keeps the pin; reopening a pinned block keeps the same set', () => {
  const st = useToolRuns.getState()
  st.toggleThinking('r1', true)
  expect(useToolRuns.getState().thinking.r1).toBe(true)
  expect(useToolRuns.getState().pinned.has('r1')).toBe(true)
  const pinned = useToolRuns.getState().pinned
  st.toggleThinking('r1', true)
  expect(useToolRuns.getState().thinking.r1).toBe(false)
  expect(useToolRuns.getState().pinned).toBe(pinned)
  st.toggleThinking('r1', false)
  expect(useToolRuns.getState().thinking.r1).toBe(true)
  expect(useToolRuns.getState().pinned).toBe(pinned)
})

test('opening a finished block never pins, and the set identity is unchanged so the rows memo does not refold', () => {
  const before = useToolRuns.getState().pinned
  useToolRuns.getState().toggleThinking('r2', false)
  expect(useToolRuns.getState().thinking.r2).toBe(true)
  expect(useToolRuns.getState().pinned).toBe(before)
  expect(before.has('r2')).toBe(false)
})

test('toggling is per entry id', () => {
  const st = useToolRuns.getState()
  st.toggleThinking('a', true)
  st.toggleThinking('b', false)
  st.toggleThinking('a', true)
  expect(useToolRuns.getState().thinking).toEqual({ a: false, b: true })
  expect([...useToolRuns.getState().pinned]).toEqual(['a'])
})
