import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import { TooltipProvider } from '@/components/ui/tooltip'
import { costsFixture, limitsFixture } from '@/dev/costs-fixture'
import { CUBE_KEY, LIMITS_KEY } from '@/lib/costs/queries'
import type { CostsSearch } from '@/lib/costs/search'
import { CostsPage } from './CostsPage'

vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  useNavigate: () => () => undefined,
}))
// Partial: the Subscription value card's imports (deep links) read `queryKeys` at module load.
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/components/ui/sidebar', () => ({ useSidebar: () => ({ open: true, isMobile: false, toggleSidebar: () => undefined }) }))
vi.mock('@/components/sidebar/SearchTrigger', () => ({ SearchTrigger: () => null }))
vi.mock('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))

const NOW = Date.parse('2026-11-02T14:25:00Z')

// The page's ranges (30D, Today) read the clock: pin it to the fixtures' NOW, or the window drifts off their rows.
beforeEach(() => {
  vi.useFakeTimers({ now: NOW, toFake: ['Date'] })
})
afterEach(() => {
  vi.useRealTimers()
})

function render(s: CostsSearch, seed = true): string {
  const qc = new QueryClient()
  if (seed) {
    qc.setQueryData(CUBE_KEY, costsFixture(NOW))
    qc.setQueryData(LIMITS_KEY, limitsFixture(NOW))
  }
  const tree: ReactNode = createElement(QueryClientProvider, { client: qc }, createElement(TooltipProvider, null, createElement(CostsPage, { search: s, visible: true })))
  return renderToStaticMarkup(tree)
}

test('the page renders every section from the sim fixture synchronously — no spinner, skeleton or loading text, tokens only', () => {
  const html = render({ sim: 'costs' })
  for (const t of ['Costs', 'Spend · 30D', 'Today', 'Calls', 'Tokens', 'Basis', 'Spend over time', 'Usage limits', 'By model · 30D', 'By owner · 30D', 'By provider · 30D', 'By kind · 30D', 'By project · 30D', 'When · hour × weekday', 'Sessions · 30D', 'Ledger']) expect(html).toContain(t)
  for (const p of ['Today', '24H', '7D', '30D', 'MTD', '90D', 'All', 'Custom']) expect(html).toContain(`>${p}<`)
  for (const c of ['Provider', 'Model', 'Owner', 'Kind', 'Via', 'Basis', 'Project', 'Session']) expect(html).toMatch(new RegExp(`${c} <b[^>]*>(all|any)</b>`))
  // Limits: a gauge chart per window; providers with nothing measured (not configured / unavailable / error) are not shown.
  expect(html).toContain('Claude subscription')
  expect(html).toContain('limited')
  expect(html).not.toMatch(/Groq/)
  expect(html).not.toMatch(/Amazon Bedrock/)
  expect(html).toContain('recharts-radial-bar')
  // Charts are Recharts surfaces inside the shadcn container; the deleted root keeps its books.
  expect((html.match(/data-slot="chart"/g) ?? []).length).toBeGreaterThanOrEqual(10)
  expect(html).toContain('Deleted session · ')
  expect(html).toContain('Cost records start')
  expect(html).not.toMatch(/Loading|animate-spin|data-slot="skeleton"/)
  expect(html).not.toMatch(/(fill|stroke|color|background)[=:]\s*"?#[0-9a-f]{3,8}/i)
})

test('granularity guard: Hour is disabled for 30D, Day for Today; group-by and metric come from the URL', () => {
  const month = render({ sim: 'costs' })
  expect(month).toMatch(/aria-disabled="true"[^>]*>Hour</)
  expect(month).toMatch(/aria-checked="true"[^>]*>Day</)
  const today = render({ sim: 'costs', range: 'today', by: 'owner', metric: 'calls' })
  expect(today).toMatch(/aria-disabled="true"[^>]*>Day</)
  expect(today).toMatch(/aria-checked="true"[^>]*>Hour</)
  expect(today).toMatch(/aria-checked="true"[^>]*>Owner</)
  expect(today).toContain('Calls over time')
})

test('an empty ledger says so once and still shows the limits panel', () => {
  const html = render({}, false)
  expect(html).toContain('No model calls recorded yet.')
  expect(html).toContain('Usage limits')
  expect(html).not.toContain('Spend over time')
})
