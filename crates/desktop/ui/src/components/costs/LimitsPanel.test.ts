import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { BudgetLimit, ProviderLimits, UsageLimits } from '@/rpc/bindings/method.agent.output'
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

function render(limits: UsageLimits): string {
  const qc = new QueryClient()
  qc.setQueryData(LIMITS_KEY, limits)
  return renderToStaticMarkup(
    createElement(QueryClientProvider, { client: qc }, createElement(TooltipProvider, null, createElement(LimitsPanel, { now: NOW }))),
  )
}

const provider = (id: string, label: string, extra: Partial<ProviderLimits>): ProviderLimits => ({
  id, provider: id, label, auth: 'api_key', account: null, plan: null, status: 'ok', message: null, source: 'response headers', observed_at_ms: NOW - 60_000,
  windows: [], rates: [], balance: null, ...extra,
})
const STALE = 'last known values from before the engine restarted'

// The screen this fixes: real limits stay; header-only providers that never reported a value, an unknown schema and
// dreaming's run budgets were empty noise.
test('only providers with measured limits get a card; routine budgets are not caps', () => {
  const budgets: BudgetLimit[] = [
    { id: 'routine:dreaming', label: 'dreaming', period: 'night', cap_usd: 100, spent_usd: 0, resets_at_ms: null },
    { id: 'agent:atlas', label: 'atlas', period: 'month', cap_usd: 40, spent_usd: 10, resets_at_ms: NOW + 86_400_000 },
  ]
  const html = render({
    generated_at_ms: NOW,
    budgets,
    providers: [
      provider('anthropic:oauth', 'Claude · subscription', { auth: 'oauth', source: 'usage endpoint', windows: [{ id: 'seven_day', label: 'Weekly', used_fraction: 0.4, resets_at_ms: NOW + 3_600_000, window_secs: 604_800 }] }),
      provider('xai:api_key', 'xAI', { status: 'stale', message: 'no fresh data since 14:00', rates: [{ id: 'requests', label: 'Requests', scope: null, limit: 100, remaining: 60, resets_at_ms: null }] }),
      provider('lm-studio', 'lm-studio · gateway', { auth: 'gateway', status: 'stale', message: STALE }),
      provider('opencode-zen', 'OpenCode Zen', { status: 'stale', message: STALE, observed_at_ms: NOW - 15 * 86_400_000 }),
      provider('mystery', 'Mystery', { source: 'none', status: 'unavailable', balance: { label: 'Credits', currency: 'USD', remaining: null, limit: null, used: null } }),
    ],
  })
  expect(html).toContain('Claude · subscription')
  expect(html).toContain('xAI')
  expect(html).toContain('stale')
  expect(html).toContain('60 / 100 free')
  expect(html).toContain('no fresh data since 14:00')
  for (const gone of ['lm-studio', 'OpenCode Zen', 'Mystery', STALE, 'dreaming']) expect(html).not.toContain(gone)
  expect(html).toContain('theuth budgets')
  expect(html).toContain('atlas')
  expect(html).not.toContain('No usage limits reported')
})

test('nothing measured and only routine budgets: one sentence, no cards', () => {
  const html = render({
    generated_at_ms: NOW,
    budgets: [{ id: 'routine:dreaming', label: 'dreaming', period: 'night', cap_usd: 1, spent_usd: 0, resets_at_ms: null }],
    providers: [provider('nvidia', 'nvidia · gateway', { auth: 'gateway', status: 'stale', message: STALE }), provider('groq', 'Groq', { status: 'not_configured', source: 'none', observed_at_ms: null })],
  })
  expect(html).toContain('No usage limits reported by your providers.')
  for (const gone of ['theuth budgets', 'nvidia', 'Groq', 'dreaming']) expect(html).not.toContain(gone)
})
