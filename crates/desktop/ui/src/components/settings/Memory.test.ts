import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Combobox from '@/components/chat/ModelCombobox'
import type * as Rpc from '@/lib/rpc'
import type { EndlessConfig } from '@/rpc/bindings/EndlessConfig'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { tokenEstimate } from '@/lib/endless'
import { bounded } from './shared'
import { Memory } from './Memory'

// R1-10: what each picker was given (the closed popover renders no list to read the order from).
const pickers = vi.hoisted(() => [] as { 'aria-label'?: string; localFirst?: boolean }[])
vi.mock('@/components/chat/ModelCombobox', async (original) => {
  const m = await original<typeof Combobox>()
  return { ...m, ModelCombobox: (p: Parameters<typeof m.ModelCombobox>[0]) => (pickers.push(p), createElement(m.ModelCombobox, p)) }
})

const config: EndlessConfig = { default_chat: true, default_desk: true, default_meeting: false, view_bytes: 128_000, meeting_view_bytes: 32_000, node_bytes: 512, slack: 0.25, settle_secs: 60, jobs: 8, tries: 5, retry_secs: 10, cap_chars: 30_000, backfill_max_units: 2_000, model: null }
let served: EndlessConfig = config
let listed: ModelInfo[] = []

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ endless_settings_get: async () => served, list_models: async () => listed, title_model: async () => ({ id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5' }) }),
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

test('137F: the compactor model applies live — next summary call, old calls cancelled, no restart — and local models run one at a time', async () => {
  const html = await render()
  expect(html).toContain('A change applies to the next summary call, and calls still running on the previous model are cancelled; no restart.')
  expect(html).toContain('Local models summarise one call at a time with a longer timeout')
  expect(html).not.toMatch(/after (a )?restart/i)
  expect(html).toMatch(/<button[^>]*id="compactor-model"[^>]*aria-label="Compactor model"/)
})

test('137F Advanced: the four compactor keys carry the engine ranges; absent (older engine) they stay editable with the default as placeholder', async () => {
  const field = (html: string, label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? ''
  const absent = await render()
  expect(field(absent, 'Attempt seconds')).toMatch(/min="10" max="3600"[^>]*placeholder="120"/)
  expect(field(absent, 'Local attempt seconds')).toMatch(/min="30" max="7200"[^>]*placeholder="600"/)
  expect(field(absent, 'Local context bytes')).toMatch(/min="4000" max="256000"[^>]*placeholder="24000"/)
  expect(field(absent, 'Local jobs')).toMatch(/min="1" max="16"[^>]*placeholder="1"/)
  for (const label of ['Attempt seconds', 'Local attempt seconds', 'Local context bytes', 'Local jobs']) {
    expect(field(absent, label)).not.toMatch(/ disabled=""/)
    expect(field(absent, label)).not.toMatch(/ value="/)
  }
  served = { ...config, attempt_secs: 90, local_attempt_secs: 1200, local_ctx_bytes: 32_000, local_jobs: 2 } as EndlessConfig
  const present = await render()
  served = config
  expect(field(present, 'Attempt seconds')).toContain('value="90"')
  expect(field(present, 'Local attempt seconds')).toContain('value="1200"')
  expect(field(present, 'Local context bytes')).toContain('value="32000"')
  expect(field(present, 'Local jobs')).toContain('value="2"')
})

test('137G Advanced: request size and speed & spend rows carry the engine ranges and defaults; the USD fields take decimals, an empty field writes nothing', async () => {
  const field = (html: string, label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label.replace(/[()]/g, '\\$&')}"[^>]*>`))?.[0] ?? ''
  const html = await render()
  expect(html).toContain('>Request size</h4>')
  expect(html).toContain('>Speed &amp; spend</h4>')
  expect(field(html, 'Unsummarized lines in full')).toMatch(/min="4000" max="128000"[^>]*placeholder="32000"/)
  expect(field(html, 'Summary context')).toMatch(/min="8000" max="256000"[^>]*placeholder="48000"/)
  expect(field(html, 'Lines per summary call')).toMatch(/min="1" max="64"[^>]*placeholder="16"/)
  expect(field(html, 'Summaries at once (cloud)')).toMatch(/min="1" max="32"[^>]*value="8"/)
  expect(field(html, 'Spend per call (USD)')).toMatch(/min="0.001" max="10" step="0.001"[^>]*placeholder="0.1"/)
  expect(field(html, 'Daily spend per chat (USD)')).toMatch(/min="0" max="1000" step="0.01"[^>]*placeholder="5"/)
  expect(html.indexOf('Request size')).toBeLessThan(html.indexOf('Speed &amp; spend'))
  expect(bounded('0.25', 0, 1000, 0.01)).toBe(0.25)
  expect(bounded('0.25', 0, 1000)).toBe(0)
  // An emptied day cap must not write 0 (= no cap).
  expect(bounded('', 0, 1000, 0.01)).toBeUndefined()
  expect(bounded('  ', 0, 1000)).toBeUndefined()
})

const lfm = { id: 'mlx/LFM3-1.7B-MLX-bf16', name: 'LFM3-1.7B-MLX-bf16', provider: 'mlx', context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: true } satisfies ModelInfo

test('177 U5: Test summary call sits under the compactor model and tests what a summary would use — the compactor model, else the title model', async () => {
  const html = await render()
  expect(html).toContain('Test summary call')
  expect(html).toContain('Sends one summary of a fixed sample to anthropic/claude-haiku-4-5')
  expect(html.indexOf('aria-label="Compactor model"')).toBeLessThan(html.indexOf('Test summary call'))
  expect(html).not.toMatch(/<button[^>]*disabled=""[^>]*>Test summary call/)
  served = { ...config, model: lfm.id }
  listed = [lfm]
  const local = await render()
  served = config
  listed = []
  expect(local).toContain(`Sends one summary of a fixed sample to ${lfm.id}`)
  // 177 U4: the picker's trigger tags a local model.
  expect(local).toMatch(/LFM3-1.7B-MLX-bf16<\/span><span[^>]*>local<\/span>/)
  // 177 U4: the compactor picker lists local models first.
  expect(pickers.filter((p) => p['aria-label'] === 'Compactor model').every((p) => p.localFirst === true)).toBe(true)
  expect(pickers.some((p) => p['aria-label'] === 'Compactor model')).toBe(true)
})
