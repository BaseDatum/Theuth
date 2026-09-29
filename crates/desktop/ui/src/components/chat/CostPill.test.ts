import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { CostBucket } from '@/rpc/bindings/CostBucket'
import type { RemoteTokens } from '@/rpc/bindings/RemoteTokens'
import type { SessionCost } from '@/rpc/bindings/SessionCost'
import { Breakdown, HarnessTokens, k } from './CostPill'

const row = (harness: string, model: string | null, turns: number, input: number, output: number): RemoteTokens => ({
  harness,
  model,
  turns,
  tokens: { input, output, cache_read: 1500, cache_write: 0, unknown_calls: 0 },
})

const rows = (h: string, n: number) => Array.from({ length: n }, (_, i) => row(h, `m${i}`, 1, 10, 10))

test('k reads like the TUI: 999, 12.3k, 200k, 1.0M', () => {
  expect([k(999), k(12_345), k(199_999), k(1_000_000)]).toEqual(['999', '12.3k', '200k', '1.0M'])
})

test('Harness tokens: one row per harness model, opencode marked last request only, nothing without rows', () => {
  const html = renderToStaticMarkup(
    createElement(HarnessTokens, {
      rows: [row('omp', 'anthropic/claude-sonnet-4-5', 3, 12_345, 2_000), row('opencode', 'gpt-5', 1, 500, 40), row('omp', null, 1, 1, 1)],
    }),
  )
  expect(html).toContain('Harness tokens')
  expect(html).toMatch(/omp<span[^>]*> · anthropic\/claude-sonnet-4-5<\/span><\/td><td[^>]*>3<\/td><td[^>]*>12\.3k<\/td><td[^>]*>2\.0k<\/td><td[^>]*>1\.5k<\/td><td[^>]*>0<\/td>/)
  expect(html).toContain('opencode<span class="text-fg-tertiary"> · gpt-5</span><span class="ml-1 text-fg-tertiary">(last request only)</span>')
  expect(html.match(/last request only/g)).toHaveLength(1)
  expect(html).not.toContain('Show more')
  expect(renderToStaticMarkup(createElement(HarnessTokens, { rows: [] }))).toBe('')
})

test('Harness tokens: five rows then "Show more (N)"', () => {
  const html = renderToStaticMarkup(createElement(HarnessTokens, { rows: rows('omp', 7) }))
  expect(html.match(/<tr>/g)).toHaveLength(6) // header + 5
  expect(html).toContain('>Show more (2)</button>')
})

const bucket: CostBucket = {
  returned: [],
  estimated_for_unavailable: [],
  calls: 0,
  dispatched: 0,
  unavailable: 0,
  unresolved: 0,
  tokens: { input: 0, output: 0, cache_read: 0, cache_write: 0, unknown_calls: 0 },
}

const cost = (delta_suspected: boolean): SessionCost => ({
  session: 's',
  revision: 1,
  history_coverage: 'complete',
  scope: 'opaque_remote_inclusive',
  direct: bucket,
  descendants: bucket,
  total: bucket,
  children: [],
  remote_meter_gaps: 1,
  delta_suspected,
  remote_tokens: [],
})

test('Breakdown: the per-turn cost line only when delta_suspected (75 §6)', () => {
  const line = 'harness may report per-turn cost — totals may be low'
  expect(renderToStaticMarkup(createElement(Breakdown, { cost: cost(true) }))).toContain(line)
  expect(renderToStaticMarkup(createElement(Breakdown, { cost: cost(false) }))).not.toContain(line)
})
