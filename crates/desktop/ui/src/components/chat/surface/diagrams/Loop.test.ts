import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type LoopData, loopDesc, loopLayout, LoopSvg } from './Loop'

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
const fixture = readFileSync(new URL('valid-loop.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Loop')!.props as unknown as LoopData
})()
const svg = (d: LoopData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(LoopSvg, { data: d, layout: loopLayout(d.steps.length, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: every step is a focusable button with data-unit, tooltip text and an ≥ 18 px hit area; the desc is the Rust first line', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="\d+" role="button" tabindex="0"/g)).toHaveLength(5)
  expect(html).toContain('aria-label="1. Measure, 20 prompts"')
  expect(html).toContain('aria-label="3. Build, active"')
  expect(html).toContain('aria-description="crates/surface/src/prompt.rs"')
  expect(html.match(/fill="transparent"/g)).toHaveLength(5)
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">Build loop</title><desc id="m-desc">Loop: Build loop — 5 steps</desc>')
  expect(loopDesc({ steps: [{ label: 'x' }] })).toBe('Loop: 1 step')
  expect(html).toContain('>Learning</text>')
  // state tints: active accent, muted at half opacity
  expect(html).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
  expect(html).toContain('opacity="0.5"')
  expect(html.match(/marker-end="url\(#m\)"/g)).toHaveLength(5)
})

test('highlight paints active; without onSelect the svg is an img', () => {
  const html = svg(data, { highlight: ['1'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  const unit = /<g data-unit="1"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(unit).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
})

test('geometry: every arc stays clear of the boxes it does not join and of the hub', () => {
  for (const n of [2, 3, 5, 8]) {
    const l = loopLayout(n)
    for (const arc of l.arcs) {
      for (let i = 0; i < n; i++) {
        if (i === arc.from || i === arc.to) continue
        const b = l.steps[i]!
        expect(Math.abs(arc.mid.x - b.x) > b.w / 2 || Math.abs(arc.mid.y - b.y) > b.h / 2, `n=${n} arc ${arc.from} vs box ${i}`).toBe(true)
      }
      // the arc midpoint is on the circle, the hub text sits at the centre
      expect(Math.hypot(arc.mid.x - l.cx, arc.mid.y - l.cy)).toBeCloseTo(l.r, 6)
    }
    expect(l.steps[0]!.y).toBeLessThan(l.cy)
    expect(Math.abs(l.steps[0]!.x - l.cx)).toBeLessThan(1e-6)
  }
})

test('streaming tolerance: a layout with more steps than data renders without throwing', () => {
  const html = renderToStaticMarkup(createElement(LoopSvg, { data: { steps: [{ label: 'a' }] }, layout: loopLayout(3), tokens: TOKENS, markerId: 'm' }))
  expect(html.match(/data-unit=/g)).toHaveLength(1)
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="loop-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="loop-[^"]+-title loop-[^"]+-desc"/)
  expect(html).toContain('Loop: Build loop — 5 steps</desc>')
})
