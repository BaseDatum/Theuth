import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { EndlessConfig } from '@/rpc/endless-stubs'
import { tokenEstimate } from '@/lib/endless'
import { Memory } from './Memory'

const config: EndlessConfig = { default_chat: true, default_desk: true, default_meeting: false, view_bytes: 128_000, meeting_view_bytes: 32_000, node_bytes: 512, slack: 0.25, settle_secs: 60, jobs: 8, tries: 5, retry_secs: 10, cap_chars: 30_000, backfill_max_units: 2_000, model: null }

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ endless_settings_get: async () => config, list_models: async () => [], title_model: async () => ({ id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5' }) }),
}))

async function render(): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Memory) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  return paint()
}

test('token estimate: bytes ÷ 3, in thousands', () => {
  expect([tokenEstimate(64_000), tokenEstimate(16_000), tokenEstimate(256_000)]).toEqual(['≈ 21 k tokens', '≈ 5 k tokens', '≈ 85 k tokens'])
})

test('the page reads agent.endless: switches, sliders and the settle count show the stored values; the compactor row offers "Same as the session"', async () => {
  const html = await render()
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="New chats start endless"/)
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Desks start endless"/)
  expect(html).toMatch(/aria-checked="false"[^>]*aria-label="Meetings start endless"/)
  expect(html).toContain('View size · <span class="tabular-nums">128 KB</span>')
  expect(html).toContain('≈ 43 k tokens')
  expect(html).toMatch(/<input type="range" aria-label="View size in KB"[^>]*min="16" max="256" step="8" value="128"/)
  expect(html).toContain('Slack · <span class="tabular-nums">25 %</span>')
  expect(html).toMatch(/<input type="range" aria-label="Slack in percent"[^>]*min="0" max="50" step="2.5" value="25"/)
  expect(html).toMatch(/<input[^>]*type="number"[^>]*min="5" max="180" value="60"/)
  expect(html).toContain('Same as title model · anthropic/claude-haiku-4-5')
  expect(html).not.toContain('Same as the session')
  expect(html).toContain('copy · grounding · status · format — always on')
})
