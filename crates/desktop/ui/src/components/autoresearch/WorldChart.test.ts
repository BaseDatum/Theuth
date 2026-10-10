import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { ArmWindow } from '@/rpc/bindings/ArmWindow'
import type { KindView } from '@/rpc/bindings/KindView'
import { WorldChart, interval, latestLaunch } from './WorldChart'

type WorldView = Extract<KindView, { kind: 'world' }>

const win = (n: number, arm: string, w: number, units: number, mean: number): ArmWindow => ({
  n,
  arm,
  win: w,
  start_at: `2026-10-${9 + w}T18:00:00Z`,
  end_at: `2026-10-${10 + w}T18:00:00Z`,
  units,
  mean,
  var: mean * (1 - mean),
})

const view: WorldView = {
  kind: 'world',
  windows: [win(2, 'control', 1, 512, 0.041), win(2, 'candidate', 1, 498, 0.058), win(2, 'control', 2, 530, 0.039), win(2, 'candidate', 2, 521, 0.061), win(1, 'control', 1, 500, 0.04)],
  launch: { slug: 'landing', n: 2, state: 'deployed', exposure: 0.1, requested_at: '2026-10-10T17:40:00Z', decided_at: '2026-10-10T17:58:00Z', by: 'user' },
  lambda: [2.6, 148.2],
}

const count = (html: string, re: RegExp) => (html.match(re) ?? []).length

test('world: the latest launch only, both arms per window with intervals, Λ against 1/α, the units table', () => {
  expect(latestLaunch(view.windows).n).toBe(2)
  expect(latestLaunch(view.windows).rows).toHaveLength(4)
  expect(interval(win(1, 'control', 1, 400, 0.04))).toBeCloseTo(1.96 * Math.sqrt((0.04 * 0.96) / 400), 12)
  const html = renderToStaticMarkup(createElement(WorldChart, { view }))
  expect(count(html, /data-arm="control"/g)).toBe(2)
  expect(count(html, /data-arm="candidate"/g)).toBe(2)
  expect(html).not.toContain('data-n="1"')
  expect(html).toContain('data-threshold="100"')
  expect(count(html, /data-lambda="/g)).toBe(2)
  expect(html).toContain('data-launch="deployed"')
  expect(html).toContain('10 % of traffic in two arms (5 % each)')
  expect(count(html, /<tr data-win="/g)).toBe(2)
  expect(html).toContain('148.20')
  // A different α moves the line.
  expect(renderToStaticMarkup(createElement(WorldChart, { view, alpha: 0.05 }))).toContain('data-threshold="20"')
})

test('world: before the first window the card says the launch waits', () => {
  const requested: WorldView = { kind: 'world', windows: [], launch: { ...view.launch!, state: 'requested', decided_at: null, by: null }, lambda: [] }
  const html = renderToStaticMarkup(createElement(WorldChart, { view: requested }))
  expect(html).toContain('launch waits for your press')
  expect(html).toContain('data-launch="requested"')
  expect(renderToStaticMarkup(createElement(WorldChart, { view: { kind: 'world', windows: [], launch: null, lambda: [] } }))).toContain('No window measured yet.')
})
