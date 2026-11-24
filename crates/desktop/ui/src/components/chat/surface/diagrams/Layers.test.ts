import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type LayersData, layersDesc, layersLayout, LayersSvg } from './Layers'

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
const fixture = readFileSync(new URL('valid-layers.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Layers')!.props as unknown as LayersData
})()
const svg = (d: LayersData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(LayersSvg, { data: d, layout: layersLayout(d, 560, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: every band is a focusable button with data-unit, items as chips, note right-aligned; the desc is the Rust first line', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="\d+" role="button" tabindex="0"/g)).toHaveLength(4)
  expect(html).toContain('aria-label="Surfaces, desktop, TUI, Slack, Signal"')
  expect(html).toContain('aria-label="Agent, loop, tools, skills, active"')
  expect(html).toContain('aria-description="crates/agent/src/lib.rs"')
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">theuth stack</title><desc id="m-desc">Layers: theuth stack — 4 layers</desc>')
  expect(layersDesc({ layers: [{ label: 'x' }] })).toBe('Layers: 1 layer')
  expect(html).toContain('text-anchor="end"')
  expect(html).toContain('>one catalog</text>')
  expect(html.match(/fill-opacity="0.6"/g)).toHaveLength(12)
  // the authored `active` state paints accent 2 px
  expect(html).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
})

test('highlight paints active; without onSelect the svg is an img', () => {
  const html = svg(data, { highlight: ['3'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  const unit = /<g data-unit="3"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(unit).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
})

test('layout: bottom-up puts layer 0 under layer 3; heights 48 with items, 36 without', () => {
  const down = layersLayout(data)
  const up = layersLayout({ ...data, direction: 'bottom-up' })
  expect(down.bands[0]!.y).toBeLessThan(down.bands[3]!.y)
  expect(up.bands[0]!.y).toBeGreaterThan(up.bands[3]!.y)
  expect(up.height).toBe(down.height)
  const mixed = layersLayout({ layers: [{ label: 'a', items: ['x'] }, { label: 'b' }] })
  expect(mixed.bands.map((b) => b.h)).toEqual([48, 36])
})

test('overflow: twelve long items end in a +n chip and no chip passes the note', () => {
  const items = Array.from({ length: 12 }, (_, i) => `a-rather-long-item-${i}`)
  const l = layersLayout({ layers: [{ label: 'Layer', items, note: 'a note' }] })
  const b = l.bands[0]!
  const last = b.chips[b.chips.length - 1]!
  expect(last.text).toMatch(/^\+\d+$/)
  expect(b.chips.length - 1 + Number(last.text.slice(1))).toBe(12)
  for (const c of b.chips) expect(c.x + c.w).toBeLessThanOrEqual(b.noteStart)
  expect(b.chips[0]!.text).toBe('a-rather-long…')
})

test('streaming tolerance: a layout with more bands than data renders without throwing; zero layers is a valid svg', () => {
  const html = renderToStaticMarkup(createElement(LayersSvg, { data: { layers: [{ label: 'a' }] }, layout: layersLayout({ layers: [{ label: 'a' }, { label: 'b' }, { label: 'c' }] }), tokens: TOKENS, markerId: 'm' }))
  expect(html.match(/data-unit=/g)).toHaveLength(1)
  expect(svg({ layers: [] })).toMatch(/^<svg.*<\/svg>$/)
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="layers-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="layers-[^"]+-title layers-[^"]+-desc"/)
  expect(html).toContain('Layers: theuth stack — 4 layers</desc>')
})
