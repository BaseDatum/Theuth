// fix/thinking-open-while-streaming: a thinking block's open state is display state keyed by entry id, and opening one
// while it streams pins it so `foldRuns` cannot fold it into a tool run.
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test } from 'vitest'
import { disclosureKey, useDisclosure, useToolRuns } from './tool-runs'

const reset = () => useToolRuns.setState({ thinking: {}, pinned: new Set(), disclosed: {} })
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

// 171F B13: disclosures keep their open state in the store, keyed by `disclosureKey`.
test('disclosureKey joins the defined parts with | and skips null/undefined', () => {
  expect(disclosureKey('s1', undefined, 3, 'x')).toBe('s1|3|x')
  expect(disclosureKey(null, 'a', 0, '')).toBe('a|0|')
})

function Probe({ k, initial }: { k: string; initial?: boolean }) {
  const [open] = useDisclosure(k, initial)
  return h('i', null, open ? 'open' : 'shut')
}

test('a preset store renders open on the first static render; initial applies only while the key is absent', () => {
  useToolRuns.setState({ disclosed: { k: true, off: false } })
  expect(renderToStaticMarkup(h(Probe, { k: 'k' }))).toBe('<i>open</i>')
  expect(renderToStaticMarkup(h(Probe, { k: 'missing' }))).toBe('<i>shut</i>')
  expect(renderToStaticMarkup(h(Probe, { k: 'missing', initial: true }))).toBe('<i>open</i>')
  expect(renderToStaticMarkup(h(Probe, { k: 'off', initial: true }))).toBe('<i>shut</i>')
})

test('setDisclosed round-trips per key; the hook setter resolves a function against the current value', () => {
  const st = useToolRuns.getState()
  st.setDisclosed('a', true)
  st.setDisclosed('b', false)
  st.setDisclosed('a', false)
  expect(useToolRuns.getState().disclosed).toEqual({ a: false, b: false })
  let set: ((next: boolean | ((open: boolean) => boolean)) => void) | undefined
  renderToStaticMarkup(h(() => ((set = useDisclosure('c', true)[1]), null)))
  set?.((open) => !open)
  expect(useToolRuns.getState().disclosed.c).toBe(false)
  set?.(true)
  expect(useToolRuns.getState().disclosed.c).toBe(true)
})
