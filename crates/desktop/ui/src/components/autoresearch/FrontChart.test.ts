import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { FrontChart } from './FrontChart'
import type { KindView } from "@/rpc/bindings/method.autoresearch.output"

const count = (html: string, re: RegExp) => (html.match(re) ?? []).length

const view: Extract<KindView, { kind: 'front' }> = {
  kind: 'front',
  axes: ['time_ms', 'size_kb'],
  members: [1, 2, 4],
  points: [
    { n: 0, sha: 'base0000', scores: { time_ms: 52, size_kb: 79 }, member: false },
    { n: 1, sha: 'keep0001', scores: { time_ms: 6.1, size_kb: 78.6 }, member: true },
    { n: 2, sha: 'keep0002', scores: { time_ms: 20.1, size_kb: 48 }, member: true },
    { n: 3, sha: 'exp:3', scores: { time_ms: 20.1, size_kb: 48 }, member: false },
    { n: 4, sha: 'keep0004', scores: { time_ms: 0.32, size_kb: 266 }, member: true },
    { n: 5, sha: 'exp:5', scores: { time_ms: 30 }, member: false }, // no size: not plottable
  ],
}

test('front chart: a dot per plottable experiment, members outlined and joined by a staircase, labelled for hover and keyboard (159 §6)', () => {
  const html = renderToStaticMarkup(createElement(FrontChart, { view }))
  expect(html).toContain('data-kind="front"')
  expect(count(html, /<circle /g)).toBe(5)
  expect(count(html, /data-member="true"/g)).toBe(3)
  // The staircase runs through the three members in x order: #4 (0.32) → #1 (6.1) → #2 (20.1).
  const stair = html.match(/<path d="([^"]+)"[^>]*data-stair/)
  expect(stair).not.toBeNull()
  expect(stair?.[1]).toMatch(/^M[\d.]+,[\d.]+ H[\d.]+V[\d.]+ H[\d.]+V[\d.]+$/)
  expect(html).toContain('aria-label="#2 · on the front · time_ms 20.1 · size_kb 48.0"')
  expect(html).toContain('aria-label="#0 · time_ms 52.0 · size_kb 79.0"')
  expect(count(html, /tabindex="0" role="button"/g)).toBe(5)
  expect(html).toContain('>time_ms →<')
  expect(html).toContain('>size_kb →<')
})

test('front chart: equal values on an axis draw one tick, not three duplicate keys (R-W1-12)', () => {
  const flat = { ...view, points: view.points.map((p) => ({ ...p, scores: { ...p.scores, size_kb: 48 } })) }
  const html = renderToStaticMarkup(createElement(FrontChart, { view: flat }))
  expect((html.match(/text-anchor="end"/g) ?? []).length).toBe(1)
})

test('front chart: no plottable point paints the empty card, no svg', () => {
  const html = renderToStaticMarkup(createElement(FrontChart, { view: { ...view, points: [], members: [] } }))
  expect(html).toContain('data-kind="front"')
  expect(html).not.toContain('<svg')
})
