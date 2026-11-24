import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type SankeyData, sankeyDesc, sankeyFlows, sankeyLayout, SankeySvg } from './Sankey'

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
const fixture = readFileSync(new URL('valid-sankey.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Sankey')!.props as unknown as SankeyData
})()
const svg = (d: SankeyData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(SankeySvg, { data: d, layout: sankeyLayout(d, 560, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))
const n = (d: SankeyData) => (id: string) => sankeyLayout(d, 560, 28).nodes.find((x) => x.id === id)!

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: 5 node + 4 flow units, labels outside the node band, title then desc, state tints', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="[^"]+" role="button" tabindex="0"/g)).toHaveLength(9)
  expect(html).toContain('aria-label="Opus, 16.7 USD"')
  expect(html).toContain('aria-label="Design 123, 12.5 USD, active"')
  expect(html).toContain('data-unit="sonnet→s3" role="button" tabindex="0" aria-label="Sonnet → Docs: 2.4 USD"')
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">Spend by model and session</title><desc id="m-desc">Sankey: Spend by model and session — 4 flows</desc>')
  expect(sankeyDesc({ nodes: [], flows: [{ from: 'a', to: 'b', value: 1 }] })).toBe('Sankey: 1 flow')
  // the flow label sits on a card mask; the node total is mono after the name
  expect(html).toContain('>drafts</text>')
  expect(html).toContain(`fill="${TOKENS.card}"`)
  expect(html).toMatch(/Opus<tspan dx="6" font-family="ui-monospace, monospace" font-size="11"[^>]*>16.7 USD<\/tspan>/)
  // active node: accent stroke 2; every ribbon narrower than 18 px gets a transparent 18 px hit copy
  expect(html).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
  expect(html).not.toContain('marker-end')
  // labels outside: column 0 to the right of x1 + 8, the last column to the left of x0 − 8
  const at = n(data)
  const opus = at('opus')
  const s1 = at('s1')
  const label = (id: string) => Number(new RegExp(`<g data-unit="${id}"[^>]*>.*?<text x="([^"]+)"`).exec(html)?.[1])
  expect(label('opus')).toBeGreaterThanOrEqual(opus.x1 + 8)
  expect(label('s1')).toBeLessThanOrEqual(s1.x0 - 8)
  expect(html).toMatch(/<g data-unit="s1"[^>]*>.*?<text [^>]*text-anchor="end"/)
})

test('highlight paints active; without onSelect the svg is an img; a muted node dims', () => {
  const html = svg(data, { highlight: ['opus→s2'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  const unit = /<g data-unit="opus→s2"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(unit).toContain(`stroke="${TOKENS.accent}" stroke-opacity="0.6"`)
  const muted = svg({ ...data, nodes: data.nodes.map((x) => (x.id === 's3' ? { ...x, state: 'muted' as const } : x)) })
  expect(muted).toMatch(/<g data-unit="s3"[^>]*opacity="0.5"/)
})

test('geometry: link ends lie inside their nodes, widths at a node sum to ≤ its height, stages fix the columns', () => {
  const l = sankeyLayout(data, 560, 28)
  const node = (id: string) => l.nodes.find((x) => x.id === id)!
  for (const k of l.links) {
    const s = node(k.source)
    const t = node(k.target)
    expect(k.y0 - k.width / 2).toBeGreaterThanOrEqual(s.y0 - 0.01)
    expect(k.y0 + k.width / 2).toBeLessThanOrEqual(s.y1 + 0.01)
    expect(k.y1 - k.width / 2).toBeGreaterThanOrEqual(t.y0 - 0.01)
    expect(k.y1 + k.width / 2).toBeLessThanOrEqual(t.y1 + 0.01)
    expect(k.path).toMatch(/^M[\d.]+,[\d.]+C/)
  }
  for (const x of l.nodes) {
    const out = l.links.filter((k) => k.source === x.id).reduce((a, k) => a + k.width, 0)
    const inn = l.links.filter((k) => k.target === x.id).reduce((a, k) => a + k.width, 0)
    expect(Math.max(out, inn)).toBeLessThanOrEqual(x.y1 - x.y0 + 1)
  }
  // authored stages [0,0,1,1,1]
  const x1s = new Set(['s1', 's2', 's3'].map((id) => node(id).x0))
  expect(x1s.size).toBe(1)
  expect(node('s1').x0).toBeGreaterThan(node('opus').x0)
  expect(node('opus').x0).toBe(node('sonnet').x0)
  expect(l.columns).toBe(2)
  expect(l.height).toBe(28 + 24 + 240)
  // no stages: a → b → c lays out by depth, three distinct columns in order
  const chain: SankeyData = { nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }, { id: 'c', label: 'C' }], flows: [{ from: 'a', to: 'b', value: 1 }, { from: 'b', to: 'c', value: 1 }] }
  const c = sankeyLayout(chain)
  const cn = (id: string) => c.nodes.find((x) => x.id === id)!
  expect(cn('a').x0).toBeLessThan(cn('b').x0)
  expect(cn('b').x0).toBeLessThan(cn('c').x0)
  expect(c.nodes.map((x) => x.column)).toEqual([0, 1, 2])
  expect(c.height).toBe(24 + 240)
})

test('streaming tolerance: unknown ends, self flows and cycle-closing flows are dropped, nothing throws', () => {
  const d: SankeyData = {
    nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
    flows: [
      { from: 'a', to: 'ghost', value: 1 },
      { from: 'b', to: 'a', value: 2 },
      { from: 'a', to: 'b', value: 3 },
      { from: 'a', to: 'a', value: 4 },
    ],
  }
  expect(sankeyFlows(d)).toEqual([1])
  const html = svg(d)
  expect(html.match(/data-unit=/g)).toHaveLength(3)
  expect(html).toContain('data-unit="b→a"')
  expect(html).toContain('Sankey: 4 flows</desc>')
  // nodes only (no flows yet) and nothing at all still render a valid svg
  expect(svg({ nodes: [{ id: 'a', label: 'A', stage: 0 }, { id: 'b', label: 'B', stage: 2 }], flows: [] }).match(/data-unit=/g)).toHaveLength(2)
  expect(svg({ nodes: [], flows: [] })).toMatch(/^<svg[^>]*><title/)
  // repeated pairs get a distinct unit id
  const twice = svg({ nodes: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }], flows: [{ from: 'a', to: 'b', value: 1 }, { from: 'a', to: 'b', value: 2 }] })
  expect(twice).toContain('data-unit="a→b"')
  expect(twice).toContain('data-unit="a→b#1"')
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="sankey-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="sankey-[^"]+-title sankey-[^"]+-desc"/)
  expect(html).toContain('Sankey: Spend by model and session — 4 flows</desc>')
})
