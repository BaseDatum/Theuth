import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { Heatmap, type HeatmapData, heatmapUnits } from './Heatmap'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))

const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)
const load = (name: string) => {
  const code = readFileSync(new URL(`${name}.json`, RUST), 'utf8')
  const s = parseSpec(code)
  if ('error' in s) throw new Error(s.error)
  return { code, data: Object.values(s.spec.elements).find((e) => e.type === 'Heatmap')!.props as unknown as HeatmapData }
}
const diverging = load('valid-heatmap-diverging')
const sequential = load('valid-heatmap')
const html = (d: HeatmapData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(Heatmap, { props: d, ...extra }))
const cell = (h: string, id: string) => new RegExp(`<td [^>]*data-unit="${id}"[^>]*>`).exec(h)?.[0] ?? ''

test('diverging: negative cells mix chart-3, positive the accent, 0 is transparent, max is 100 %, legend and title', () => {
  const h = html(diverging.data)
  expect(h).toMatchSnapshot()
  expect(h).toContain('>Latency delta vs last week (ms)</div>')
  expect(cell(h, '0:0')).toContain('color-mix(in oklab, var(--chart-3, #3b5b8c) 60%, transparent)')
  expect(cell(h, '0:3')).toContain('color-mix(in oklab, var(--color-fg-accent) 40%, transparent)')
  expect(cell(h, '1:1')).toContain('var(--color-fg-accent) 0%, transparent')
  expect(cell(h, '2:0')).toContain('var(--color-fg-accent) 100%, transparent')
  expect(h.match(/<td title="[^"]+" data-unit="\d+:\d+" role="button" tabindex="0"/g)).toHaveLength(12)
  expect(h).toContain('aria-label="chat · Mon: -12"')
  expect(h).toContain('title="chat · Mon: -12"')
  expect(h).toContain('<span>−20</span><span>0</span><span>+20</span>')
  expect(heatmapUnits(diverging.data)).toHaveLength(12)
  expect(heatmapUnits(diverging.data)[5]).toBe('1:1')
})

test('diverging without max scales to the largest |v|', () => {
  const h = html({ rows: ['r'], cols: ['a', 'b'], cells: [[-8, 4]], palette: 'diverging' })
  expect(cell(h, '0:0')).toContain('var(--chart-3, #3b5b8c) 100%')
  expect(cell(h, '0:1')).toContain('var(--color-fg-accent) 50%')
})

test('sequential keeps its form: accent color-mix at 100 % for the max cell, no legend, no title', () => {
  const h = html(sequential.data)
  expect(cell(h, '2:1')).toContain('color-mix(in oklab, var(--color-fg-accent) 100%, transparent)')
  expect(cell(h, '0:0')).toContain('var(--color-fg-accent) 20%, transparent')
  expect(h).not.toContain('−')
  expect(h).not.toContain('font-medium')
  expect(h.match(/<td /g)).toHaveLength(9)
})

test('highlight paints the active cell with the accent outline; selection dims the rest only once selected', () => {
  const h = html(sequential.data, { highlight: ['1:1'], onSelect: () => {} })
  expect(cell(h, '1:1')).toContain('outline:2px solid var(--fg-accent)')
  expect(cell(h, '0:0')).not.toContain('opacity-50')
  expect(cell(h, '0:0')).not.toContain('outline')
})

test('streaming tolerance: ragged cells render what is present, a missing row is empty, nothing throws', () => {
  const h = html({ rows: ['a', 'b', 'c'], cols: ['x', 'y'], cells: [[1], [2, 3]] })
  expect(h.match(/<td /g)).toHaveLength(3)
  expect(heatmapUnits({ rows: ['a', 'b', 'c'], cols: ['x', 'y'], cells: [[1], [2, 3]] })).toEqual(['0:0', '1:0', '1:1'])
  expect(html({ rows: [], cols: [], cells: [] })).toContain('<table')
})

test('mounts through the fence without the stub marker', () => {
  const h = renderToStaticMarkup(createElement(JsonRenderFence, { code: diverging.code, isIncomplete: false, language: 'json-render' }))
  expect(h).not.toContain('data-stub')
  expect(h).toContain('data-streamdown="json-render"')
  expect(h).toContain('color-mix(in oklab, var(--color-fg-accent)')
  expect(h).toContain('var(--chart-3, #3b5b8c)')
})
