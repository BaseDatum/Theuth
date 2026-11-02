import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { limitsFixture } from '@/dev/costs-fixture'
import { LIMITS_KEY } from '@/lib/costs/queries'
import { LimitsPanel } from './LimitsPanel'

vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))

const NOW = Date.parse('2026-11-02T14:25:00Z')

// 160-Z live proof: the page clock ticks per minute while the snapshot refreshes every 15 s, so a snapshot newer than
// the page's `now` read "generated in 45 s". A snapshot is never from the future.
test('a snapshot newer than the page clock reads "just now", not "in 45 s"', () => {
  const qc = new QueryClient()
  qc.setQueryData(LIMITS_KEY, { ...limitsFixture(NOW), generated_at_ms: NOW + 45_000 })
  const html = renderToStaticMarkup(
    createElement(QueryClientProvider, { client: qc }, createElement(TooltipProvider, null, createElement(LimitsPanel, { now: NOW }))),
  )
  expect(html).toContain('generated just now')
  expect(html).not.toMatch(/generated in \d/)
})
