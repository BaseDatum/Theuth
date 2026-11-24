import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { JsonRenderFence } from '../JsonRenderFence'
import { parseSpec } from '../parse'
import { type JourneyData, journeyDesc, journeyLayout, JourneySvg } from './Journey'

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
const fixture = readFileSync(new URL('valid-journey.json', RUST), 'utf8')
const data = (() => {
  const s = parseSpec(fixture)
  if ('error' in s) throw new Error(s.error)
  return Object.values(s.spec.elements).find((e) => e.type === 'Journey')!.props as unknown as JourneyData
})()
const svg = (d: JourneyData, extra: Record<string, unknown> = {}) => renderToStaticMarkup(createElement(JourneySvg, { data: d, layout: journeyLayout(d, 640, d.title ? 28 : 0), tokens: TOKENS, markerId: 'm', ...extra }))

test('snapshot at the fixed layout', () => {
  expect(svg(data)).toMatchSnapshot()
})

test('unit contract: every stage column is a focusable button with data-unit and a transparent hit area; the desc is the Rust first line', () => {
  const html = svg(data, { onSelect: () => {} })
  expect(html.match(/<g data-unit="\d+" role="button" tabindex="0"/g)).toHaveLength(4)
  expect(html).toContain('aria-label="Install, downloads dmg · is it safe?"')
  expect(html).toContain('aria-label="First board, asks how X works · this is a doc"')
  expect(html.match(/fill="transparent"/g)).toHaveLength(4)
  expect(html).toMatch(/^<svg[^>]*role="group"[^>]*aria-labelledby="m-title m-desc"/)
  expect(html).toContain('<title id="m-title">First hour</title><desc id="m-desc">Journey: First hour — 4 stages</desc>')
  expect(journeyDesc({ stages: ['a'], rows: [] })).toBe('Journey: 1 stage')
  expect(html).toContain('>Does</text>')
  expect(html).toContain('>notarisation wait</text>')
  expect(html).toContain(`<polyline points="`)
  expect(html.match(new RegExp(`fill="${TOKENS.danger}"`, 'g'))).toHaveLength(2)
})

test('highlight tints the column; without onSelect the svg is an img', () => {
  const html = svg(data, { highlight: ['2'] })
  expect(html).toMatch(/^<svg[^>]*role="img"/)
  expect(html).toContain(`fill="${TOKENS.accent}" fill-opacity="0.08" stroke="${TOKENS.accent}" stroke-width="2"`)
  expect(svg(data)).not.toContain('fill-opacity="0.08"')
})

test('layout: mood polyline has one point per stage with 0.9 above 0.2; pains sit under their stage', () => {
  const l = journeyLayout(data)
  expect(l.mood!.points).toHaveLength(4)
  expect(l.mood!.points[2]!.y).toBeLessThan(l.mood!.points[0]!.y)
  expect(l.mood!.points[0]!.y).toBeLessThan(l.mood!.mid)
  expect(l.pains.map((p) => p.stage)).toEqual([0, 3])
  expect(l.pains[0]!.x).toBeCloseTo(l.x0 + l.colW * 0.5, 6)
  expect(l.pains[1]!.x).toBeCloseTo(l.x0 + l.colW * 3.5, 6)
  expect(l.pains[0]!.y).toBeGreaterThanOrEqual(l.bottom)
  expect(l.rowsY).toHaveLength(2)
  expect(l.rowsY[1]! - l.rowsY[0]!).toBe(40)
})

test('streaming tolerance: out-of-range pains dropped, mood clamped and cut, short cells leave blanks, nothing thrown', () => {
  const d: JourneyData = { stages: ['a', 'b', 'c'], rows: [{ label: 'r', cells: ['x'] }], mood: [3, -5, 0, 1, 1], pains: [{ stage: 7, text: 'late' }, { stage: 1, text: 'ok' }] }
  const l = journeyLayout(d)
  expect(l.pains.map((p) => p.stage)).toEqual([1])
  expect(l.mood!.points).toHaveLength(3)
  expect(l.mood!.points[0]!.y).toBe(l.mood!.mid - 24)
  expect(l.mood!.points[1]!.y).toBe(l.mood!.mid + 24)
  const html = svg(d)
  expect(html.match(/data-unit=/g)).toHaveLength(3)
  expect(html).not.toContain('>late<')
  expect(svg({ stages: [], rows: [] })).toMatch(/^<svg.*<\/svg>$/)
})

test('mounts through the fence inside a DiagramFrame without the stub marker', () => {
  const html = renderToStaticMarkup(createElement(JsonRenderFence, { code: fixture, isIncomplete: false, language: 'json-render' }))
  expect(html).not.toContain('data-stub')
  expect(html).toContain('data-streamdown="journey-block"')
  expect(html).toMatch(/<svg[^>]*role="img"[^>]*aria-labelledby="journey-[^"]+-title journey-[^"]+-desc"/)
  expect(html).toContain('Journey: First hour — 4 stages</desc>')
})
