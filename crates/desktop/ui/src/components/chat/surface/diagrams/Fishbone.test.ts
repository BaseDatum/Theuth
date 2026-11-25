import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type FishboneData, fishboneDesc, fishboneLayout, FishboneSvg } from './Fishbone'

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
  accent: '#1d5f82',
  success: '#265c47',
  danger: '#8f2f1f',
  font: 'sans-serif',
  series: ['#1d5f82', '#265c47', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'],
}
const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)
const fixture = readFileSync(new URL('valid-fishbone.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Fishbone')!.props as unknown as FishboneData
})()
const svg = (d: FishboneData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(FishboneSvg, { data: d, layout: fishboneLayout(d, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))

/** Does the segment a→b pass through the box (sampled at 1/128 steps)? */
function crosses(a: { x: number; y: number }, b: { x: number; y: number }, r: { x: number; y: number; w: number; h: number }): boolean {
  for (let i = 0; i <= 128; i++) {
    const t = i / 128
    const x = a.x + (b.x - a.x) * t
    const y = a.y + (b.y - a.y) * t
    if (x > r.x && x < r.x + r.w && y > r.y && y < r.y + r.h) return true
  }
  return false
}

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: four causes and the effect are focusable buttons with data-unit and 18 px hit strokes; the desc is the Rust first line', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="\d+" role="button" tabindex="0"/g)).toHaveLength(4)
  expect(html.match(/<g data-unit="effect" role="button" tabindex="0"/g)).toHaveLength(1)
  expect(html).toContain('aria-label="Model, long context, cold cache"')
  expect(html).toContain('aria-label="Slow replies"')
  expect(html.match(/stroke="transparent" stroke-width="18"/g)).toHaveLength(4)
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">Why are replies slow?</title><desc id="m-desc">Fishbone: Why are replies slow? — 4 causes</desc>')
  expect(fishboneDesc({ effect: 'e', causes: [{ category: 'c', items: [] }] })).toBe('Fishbone: 1 cause')
  expect(html).toContain('>57-row table</text>')
  expect(html.match(/marker-end="url\(#m\)"/g)).toHaveLength(1)
})

test('highlight paints the bone and the effect accent; without onSelect the svg is an img', () => {
  const html = svg(data, { highlight: ['1', 'effect'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  const bone = /<g data-unit="1"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(bone).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
  const effect = /<g data-unit="effect"[^>]*>(.*?)<\/g>/.exec(html)?.[1] ?? ''
  expect(effect).toContain(`stroke="${TOKENS.accent}" stroke-width="2"`)
})

test('geometry: bones alternate sides, lean away from the head, never cross another label mask or the effect box; ≤ 6 ticks', () => {
  const many: FishboneData = { effect: 'A rather long effect label here', causes: Array.from({ length: 8 }, (_, i) => ({ category: `Category ${i}`, items: Array.from({ length: i + 1 }, (_, k) => `item ${k} of cause ${i}`) })) }
  for (const d of [data, many]) {
    const l = fishboneLayout(d)
    l.bones.forEach((b, i) => {
      expect(b.side).toBe(i % 2 === 0 ? 1 : -1)
      expect(Math.sign(l.spineY - b.end.y)).toBe(b.side)
      expect(b.end.x).toBeLessThan(b.foot.x)
      expect(b.foot.x).toBeLessThan(l.effect.x)
      expect(crosses(b.foot, b.end, l.effect), `bone ${i} vs effect`).toBe(false)
      l.bones.forEach((o, j) => {
        if (i !== j) expect(crosses(b.foot, b.end, o.label), `bone ${i} vs label ${j}`).toBe(false)
      })
      expect(b.ticks.length).toBeLessThanOrEqual(6)
      expect(b.label.x).toBeGreaterThanOrEqual(12)
      for (const t of b.ticks) expect(t.to.x - 4 - t.text.length * 6.6).toBeGreaterThanOrEqual(11)
    })
    expect(l.bones[0]!.foot.x).toBeGreaterThan(l.bones[2]?.foot.x ?? -Infinity)
    expect(l.effect.x + l.effect.w).toBeLessThanOrEqual(l.width)
  }
  expect(fishboneLayout(many).bones[7]!.ticks).toHaveLength(6)
  expect(svg(many).match(/<title>item \d of cause 7<\/title>/g)).toHaveLength(6)
})

test('streaming tolerance: a layout with more bones than data renders without throwing; zero causes is a valid svg', () => {
  const html = renderToStaticMarkup(createElement(FishboneSvg, { data: { effect: 'e', causes: [{ category: 'a', items: [] }] }, layout: fishboneLayout({ effect: 'e', causes: [{ category: 'a', items: [] }, { category: 'b', items: ['x'] }] }), tokens: TOKENS, markerId: 'm' }))
  expect(html.match(/data-unit=/g)).toHaveLength(2)
  expect(svg({ effect: 'e', causes: [] })).toMatch(/^<svg.*data-unit="effect".*<\/svg>$/)
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="fishbone-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="fishbone-[^"]+-title fishbone-[^"]+-desc"/)
  expect(html).toContain('Fishbone: Why are replies slow? — 4 causes</desc>')
})
