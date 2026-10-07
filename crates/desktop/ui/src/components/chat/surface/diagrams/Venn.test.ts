import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type VennData, vennDesc, vennLayout, vennRegion, VennSvg } from './Venn'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
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
const fixture = readFileSync(new URL('valid-venn.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Venn')!.props as unknown as VennData
})()
const svg = (d: VennData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(VennSvg, { data: d, layout: vennLayout(d.sets.length >= 3 ? 3 : 2, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: 3 set units + 3 overlap units, labels, the Rust desc', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="[^"]+" role="button" tabindex="0"/g)).toHaveLength(6)
  expect(html).toContain('<g data-unit="tests" role="button" tabindex="0" aria-label="Has tests, 41"')
  expect(html).toContain('<g data-unit="tests∩docs∩review" role="button" tabindex="0" aria-label="Release-ready"')
  expect(html).toContain('<g data-unit="tests∩review" role="button" tabindex="0" aria-label="Ship candidates"')
  expect(html.match(/<circle [^>]*r="18" fill="transparent"/g)).toHaveLength(3)
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">Release gates</title><desc id="m-desc">Venn: Release gates — 3 sets</desc>')
  expect(vennDesc({ sets: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }] })).toBe('Venn: 2 sets')
  expect(html).toContain('>41</text>')
  expect(html).toContain('>Documented + test…</text>')
})

test('highlight paints the set circle active; without onSelect the svg is an img', () => {
  const html = svg(data, { highlight: ['docs'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  const unit = /<g data-unit="docs"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(unit).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
})

test('geometry: region points lie in exactly their sets; every set label is outside its own circle', () => {
  const inside = (p: { x: number; y: number }, c: { x: number; y: number }, r: number) => Math.hypot(p.x - c.x, p.y - c.y) < r
  for (const n of [2, 3] as const) {
    const l = vennLayout(n)
    const ids = ['a', 'b', 'c'].slice(0, n)
    for (const lens of l.lens) {
      l.circles.forEach((c, i) => expect(inside(lens.at, c, l.r), `n=${n} lens ${lens.sets} vs circle ${i}`).toBe(lens.sets.includes(i)))
      expect(vennRegion([ids[lens.sets[1]]!, ids[lens.sets[0]]!], l, ids)).toEqual(lens.at)
    }
    l.labels.forEach((lb, i) => expect(inside(lb.at, l.circles[i]!, l.r), `n=${n} label ${i}`).toBe(false))
    expect(l.circles.length).toBe(n)
    expect(l.lens.length).toBe(n === 2 ? 1 : 3)
  }
  const l3 = vennLayout(3)
  for (const c of l3.circles) expect(inside(l3.center, c, l3.r)).toBe(true)
  expect(vennRegion(['a', 'b', 'c'], l3, ['a', 'b', 'c'])).toEqual(l3.center)
  expect(vennRegion(['a', 'b', 'c'], vennLayout(2), ['a', 'b'])).toBeNull()
  expect(vennRegion(['a', 'a'], l3, ['a', 'b', 'c'])).toBeNull()
  expect(vennRegion(['a', 'zzz'], l3, ['a', 'b', 'c'])).toBeNull()
})

test('streaming tolerance: dangling overlaps are dropped, 0–1 sets render an empty svg, a 4th set is not drawn', () => {
  const dangling: VennData = { sets: data.sets, overlaps: [{ sets: ['tests', 'nope'], label: 'x' }, ...(data.overlaps ?? [])] }
  const html = svg(dangling)
  expect(html).not.toContain('tests∩nope')
  expect(html.match(/data-unit=/g)).toHaveLength(6)
  const one = svg({ sets: [{ id: 'a', label: 'A' }], overlaps: [{ sets: ['a', 'a'], label: 'x' }] })
  expect(one).toContain('<desc id="m-desc">Venn: 1 set</desc>')
  expect(one).not.toContain('data-unit=')
  expect(svg({ sets: [] })).not.toContain('<circle')
  const four = svg({ sets: [...data.sets, { id: 'd', label: 'D' }] })
  expect(four.match(/<circle [^>]*r="96"/g)).toHaveLength(3)
  expect(four).toContain('Venn: 4 sets</desc>')
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="venn-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="venn-[^"]+-title venn-[^"]+-desc"/)
  expect(html).toContain('Venn: Release gates — 3 sets</desc>')
})
