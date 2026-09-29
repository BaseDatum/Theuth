import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { RemoteTokens } from '@/rpc/bindings/RemoteTokens'
import { HarnessTokens, k } from './CostPill'

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
