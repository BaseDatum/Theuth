import '../settings/test-dom'
import type * as ReactDom from 'react-dom'
import { act, createElement as h, useRef } from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { useTier } from './Header'

vi.mock('react-dom', async (original) => {
  const actual = await original<typeof ReactDom>()
  return { ...actual, flushSync: vi.fn(actual.flushSync) }
})

const proto = HTMLElement.prototype as { clientWidth?: number; offsetWidth?: number }
const offset = Object.getOwnPropertyDescriptor(proto, 'offsetWidth')
const ResizeObserverBefore = globalThis.ResizeObserver
afterEach(() => {
  // Ours are own properties of `HTMLElement.prototype`: `clientWidth` is jsdom's on `Element.prototype` underneath.
  delete proto.clientWidth
  if (offset) Object.defineProperty(proto, 'offsetWidth', offset)
  else delete proto.offsetWidth
  globalThis.ResizeObserver = ResizeObserverBefore
  document.body.innerHTML = ''
})

test('a pane resize that keeps the header tier renders nothing; only a tier change is flushed', async () => {
  const size = { pane: 800, bar: 500 }
  Object.defineProperty(proto, 'clientWidth', { configurable: true, get: () => size.pane })
  Object.defineProperty(proto, 'offsetWidth', { configurable: true, get: () => size.bar })
  let resized = () => {}
  globalThis.ResizeObserver = class {
    constructor(cb: () => void) {
      resized = cb
    }
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
  let tier = -1
  let renders = 0
  const Bar = () => {
    renders++
    const header = useRef<HTMLDivElement>(null)
    const row = useRef<HTMLDivElement>(null)
    tier = useTier(header, row)
    return h('div', { ref: header }, h('div', { ref: row }))
  }
  const root = createRoot(document.body.appendChild(document.createElement('div')))
  await act(async () => root.render(h(Bar)))
  expect(tier).toBe(0)
  const before = renders
  for (const pane of [790, 700, 600]) {
    size.pane = pane
    await act(async () => resized())
  }
  expect(flushSync).not.toHaveBeenCalled()
  expect(renders).toBe(before)
  size.pane = 400
  await act(async () => resized())
  expect(flushSync).toHaveBeenCalledTimes(1)
  expect(tier).toBeGreaterThan(0)
  await act(async () => root.unmount())
})
