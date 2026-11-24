import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { squarify, type TreemapData, treemapDesc, treemapIds, treemapLayout, TreemapSvg } from './Treemap'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({ diagramTokens: () => TOKENS }))

const TOKENS: DiagramTokens = {
  background: '#fff',
  foreground: '#111',
  primary: '#111',
  muted: '#ddd',
  mutedForeground: '#666',
  border: '#ccc',
  card: '#fafafa',
  accent: '#b4531e',
  success: '#2f5d4a',
  danger: '#9b2d1f',
  font: 'sans-serif',
  series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}
const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)
const fixture = readFileSync(new URL('valid-treemap.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Treemap')!.props as unknown as TreemapData
})()
const svg = (d: TreemapData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(TreemapSvg, { data: d, layout: treemapLayout(d, 560, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))

test('squarify: areas proportional, the union covers the box, no overlaps, aspect ratios bounded, input order kept', () => {
  const values = [6, 6, 4, 3, 2, 2, 1]
  const rects = squarify(values, 0, 0, 6, 4)
  expect(rects).toHaveLength(7)
  const total = values.reduce((a, b) => a + b, 0)
  let area = 0
  rects.forEach((r, i) => {
    expect(r.w * r.h).toBeCloseTo((24 * values[i]!) / total, 9)
    expect(r.x).toBeGreaterThanOrEqual(-1e-9)
    expect(r.y).toBeGreaterThanOrEqual(-1e-9)
    expect(r.x + r.w).toBeLessThanOrEqual(6 + 1e-9)
    expect(r.y + r.h).toBeLessThanOrEqual(4 + 1e-9)
    expect(Math.max(r.w / r.h, r.h / r.w)).toBeLessThanOrEqual(4)
    area += r.w * r.h
    for (let j = 0; j < i; j++) {
      const o = rects[j]!
      const overlap = Math.max(0, Math.min(r.x + r.w, o.x + o.w) - Math.max(r.x, o.x)) * Math.max(0, Math.min(r.y + r.h, o.y + o.h) - Math.max(r.y, o.y))
      expect(overlap, `${i} vs ${j}`).toBeLessThan(1e-9)
    }
  })
  expect(area).toBeCloseTo(24, 9)
  // the two 6s come first in input order, and the largest cells are placed first (left column)
  expect(rects[0]!.x).toBe(0)
  expect(rects[0]!.w).toBeCloseTo(rects[1]!.w, 9)
  expect(squarify([], 0, 0, 10, 10)).toEqual([])
  expect(squarify([0, 5], 0, 0, 10, 10)[0]).toEqual({ x: 0, y: 0, w: 0, h: 0 })
})

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: every cell is a focusable button with data-unit, group headers, tints and the Rust desc', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="[^"]+" role="button" tabindex="0"/g)).toHaveLength(6)
  expect(html).toContain('aria-label="agent, 36200, active"')
  expect(html).toContain('aria-label="apps, 5600"')
  expect(html).toContain('aria-description="packages/apps/src/library.ts"')
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">Lines by crate</title><desc id="m-desc">Treemap: Lines by crate — 6 items</desc>')
  expect(treemapDesc({ items: [{ label: 'x', value: 1 }] })).toBe('Treemap: 1 item')
  expect(html).toContain('>crates</text>')
  expect(html).toContain('>desktop</text>')
  expect(html).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
  expect(html).toContain('>36200</text>')
  expect(treemapIds([{ label: 'a', value: 1 }, { label: 'a', value: 1 }, { label: 'b', value: 1 }])).toEqual(['a', 'a#1', 'b'])
})

test('small cells keep their unit and an 18 px hit area but draw no text', () => {
  const d: TreemapData = { items: [{ label: 'big', value: 1000 }, { label: 'tiny', value: 1 }] }
  const html = svg(d)
  const tiny = /<g data-unit="tiny"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(tiny).not.toBe('')
  expect(tiny).not.toContain('<text')
  expect(tiny).toMatch(/<rect [^>]*width="18"[^>]*fill="transparent"/)
  const big = /<g data-unit="big"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(big).toContain('>big</text>')
  expect(big).not.toContain('fill="transparent"')
  // no groups → no header strip
  expect(html).not.toContain(`stroke="${TOKENS.border}"`)
})

test('highlight paints active; without onSelect the svg is an img', () => {
  const html = svg(data, { highlight: ['ui'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  const unit = /<g data-unit="ui"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(unit).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
})

test('layout: groups tile the map by total, cells stay inside their group under the 20 px header', () => {
  const l = treemapLayout(data, 560, 28)
  expect(l.width).toBe(584)
  expect(l.height).toBe(28 + 336 + 24)
  expect(l.groups.map((g) => g.label)).toEqual(['crates', 'desktop', 'packages'])
  l.cells.forEach((c, i) => {
    const g = l.groups[c.group]!
    expect(c.y, `cell ${i}`).toBeGreaterThanOrEqual(g.y + 20)
    expect(c.x).toBeGreaterThanOrEqual(g.x)
    expect(c.x + c.w).toBeLessThanOrEqual(g.x + g.w + 1e-9)
    expect(c.y + c.h).toBeLessThanOrEqual(g.y + g.h + 1e-9)
  })
})

test('streaming tolerance: a layout from fewer items than the data renders without throwing; 0 items is a valid svg', () => {
  const partial: TreemapData = { items: data.items.slice(0, 2) }
  const html = renderToStaticMarkup(createElement(TreemapSvg, { data, layout: treemapLayout(partial), tokens: TOKENS, markerId: 'm' }))
  expect(html.match(/data-unit=/g)).toHaveLength(2)
  const empty = svg({ items: [] })
  expect(empty).toContain('<desc id="m-desc">Treemap: 0 items</desc>')
  expect(empty).not.toContain('data-unit=')
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="treemap-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="treemap-[^"]+-title treemap-[^"]+-desc"/)
  expect(html).toContain('Treemap: Lines by crate — 6 items</desc>')
})
