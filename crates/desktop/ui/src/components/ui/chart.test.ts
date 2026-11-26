import '../settings/test-dom'
import type * as Recharts from 'recharts'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { expect, test, vi } from 'vitest'
import { ChartContainer } from './chart'

const seen = vi.hoisted(() => [] as { debounce?: number; initialDimension?: object }[])
vi.mock('recharts', async (original) => ({
  ...(await original<typeof Recharts>()),
  ResponsiveContainer: (props: { debounce?: number; initialDimension?: object }) => (seen.push(props), null),
}))

test('a chart redraws for a resize burst once it settles (100 ms by default), its first size still at once', async () => {
  const root = createRoot(document.body.appendChild(document.createElement('div')))
  await act(async () => root.render(h(ChartContainer, { config: {}, children: h('div') })))
  expect(seen.at(-1)).toMatchObject({ debounce: 100, initialDimension: { width: expect.any(Number), height: expect.any(Number) } })
  await act(async () => root.render(h(ChartContainer, { config: {}, debounce: 0, children: h('div') })))
  expect(seen.at(-1)!.debounce).toBe(0)
  await act(async () => root.unmount())
})
