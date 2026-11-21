import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Combobox from '@/components/chat/ModelCombobox'
import type { Effort } from '@/components/chat/ModelPicker'
import type * as Rpc from '@/lib/rpc'
import type * as SelectUi from '@/components/ui/select'
import type * as SwitchUi from '@/components/ui/switch'
import type * as Shared from './shared'
import { useConnection } from '@/store/connection'
import { tailPairError } from '@/lib/endless'
import type { EndlessConfig } from "@/rpc/bindings/method.agent.output"
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { tokenEstimate } from '@/lib/endless'
import { bounded } from './shared'
import { Memory } from './Memory'
import { agent } from './categories/agent'

// R1-10: what each picker was given (the closed popover renders no list to read the order from).
const pickers = vi.hoisted(() => [] as { 'aria-label'?: string; effort?: Effort }[])
vi.mock('@/components/chat/ModelCombobox', async (original) => {
  const m = await original<typeof Combobox>()
  return { ...m, ModelCombobox: (p: Parameters<typeof m.ModelCombobox>[0]) => (pickers.push(p), createElement(m.ModelCombobox, p)) }
})

// 201: what each control was given, by its aria-label (selects by `name`), so a test can fire it without a DOM.
type Fire = { onCommit?(v: number): void; onCheckedChange?(v: boolean): void; onValueChange?(v: string): void; action?: { mutate(n: number): void } }
const controls = vi.hoisted(() => ({}) as Record<string, Fire>)
vi.mock('./shared', async (original) => {
  const m = await original<typeof Shared>()
  return {
    ...m,
    Count: (p: Parameters<typeof m.Count>[0]) => ((controls[p['aria-label'] ?? ''] = p), createElement(m.Count, p)),
    Range: (p: Parameters<typeof m.Range>[0]) => ((controls[p['aria-label']] = p), createElement(m.Range, p)),
  }
})
vi.mock('@/components/ui/switch', async (original) => {
  const m = await original<typeof SwitchUi>()
  return { ...m, Switch: (p: Parameters<typeof m.Switch>[0]) => ((controls[p['aria-label'] ?? ''] = p as Fire), createElement(m.Switch, p)) }
})
vi.mock('@/components/ui/select', async (original) => {
  const m = await original<typeof SelectUi>()
  return { ...m, Select: (p: Parameters<typeof m.Select>[0]) => (p.name && (controls[`select:${p.name}`] = p as Fire), createElement(m.Select, p)) }
})
const saved = vi.fn(async (next: EndlessConfig) => next)

const config: EndlessConfig = receiveComponent("method.agent.output.EndlessConfig", { default_chat: true, default_desk: true, default_meeting: false, view_bytes: 128_000, meeting_view_bytes: 32_000, node_bytes: 512, slack: 0.25, settle_secs: 60, jobs: 8, tries: 5, retry_secs: 10, cap_chars: 30_000, backfill_max_units: 2_000, model: null })
let served: EndlessConfig = config
let listed: ModelInfo[] = []

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ endless_settings_get: async () => served, endless_settings_set: saved, list_models: async () => listed, title_model: async () => ({ id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5' }) }),
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

test('the page reads agent.endless: switches and sliders show the stored values; the settle row is gone (201); the compactor row offers "Same as the session"', async () => {
  const html = await render()
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="New chats use fractal context"/)
  expect(html).toMatch(/aria-checked="true"[^>]*aria-label="Desks use fractal context"/)
  expect(html).toMatch(/aria-checked="false"[^>]*aria-label="Meetings use fractal context"/)
  expect(html).toContain('>New chats use fractal context<')
  expect(html).not.toMatch(/endless|autocontext|compressed memory/i)
  // The tab is labelled Context; its id and URL stay `memory` so links keep working.
  expect(agent.tabs.find((t) => t.id === 'memory')?.label).toBe('Context')
  expect(html).toContain('View size · <span class="tabular-nums">128 KB</span>')
  expect(html).toContain('≈ 43 k tokens')
  expect(html).toMatch(/<input type="range" aria-label="View size in KB"[^>]*min="16" max="256" step="8" value="128"/)
  expect(html).toContain('Slack · <span class="tabular-nums">25 %</span>')
  expect(html).toMatch(/<input type="range" aria-label="Slack in percent"[^>]*min="0" max="50" step="2.5" value="25"/)
  expect(html).not.toContain('Settle seconds')
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

test('137F Advanced: the four compactor keys carry the engine ranges; absent received fields use declared serde defaults and stay editable', async () => {
  const field = (html: string, label: string) => html.match(new RegExp(`<input[^>]*aria-label="${label}"[^>]*>`))?.[0] ?? ''
  const absent = await render()
  expect(field(absent, 'Attempt seconds')).toMatch(/min="10" max="3600"[^>]*placeholder="120"/)
  expect(field(absent, 'Local attempt seconds')).toMatch(/min="30" max="7200"[^>]*placeholder="600"/)
  expect(field(absent, 'Local context bytes')).toMatch(/min="4000" max="256000"[^>]*placeholder="24000"/)
  expect(field(absent, 'Local jobs')).toMatch(/min="1" max="16"[^>]*placeholder="1"/)
  for (const [label, value] of [['Attempt seconds', 120], ['Local attempt seconds', 600], ['Local context bytes', 24_000], ['Local jobs', 1]] as const) {
    expect(field(absent, label)).not.toMatch(/ disabled=""/)
    expect(field(absent, label)).toContain(`value="${value}"`)
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
  expect(pickers.some((p) => p['aria-label'] === 'Compactor model')).toBe(true)
})

test('194: the compactor picker carries the picked model’s effort chips at agent.endless.effort and its trigger names the effort; none while unset', async () => {
  const think = { ...lfm, id: 'anthropic/claude-haiku-5-5', name: 'Claude Haiku 5.5', provider: 'anthropic', reasoning: true, efforts: ['low', 'high'], local: false } satisfies ModelInfo
  const compactor = () => pickers.findLast((p) => p['aria-label'] === 'Compactor model')
  await render()
  expect(compactor()?.effort).toBeUndefined()
  served = { ...config, model: think.id, effort: 'high' }
  listed = [think]
  const html = await render()
  served = config
  listed = []
  expect(compactor()?.effort?.levels).toEqual(['low', 'high'])
  expect(compactor()?.effort?.current).toBe('high')
  expect(html).toContain('Claude Haiku 5.5 · high')
})

/** Fires `fire` and returns the config the page wrote. */
async function written(fire: () => void): Promise<EndlessConfig> {
  saved.mockClear()
  fire()
  await vi.waitFor(() => expect(saved).toHaveBeenCalledTimes(1))
  return saved.mock.calls[0]![0]
}

test('201 §12.1: every new key has a control on the page, and each control patches its own field', async () => {
  await render()
  const c = (label: string) => controls[label] ?? expect.fail(`no control "${label}"`)
  const fixed: [keyof EndlessConfig, () => void, unknown][] = [
    ['tail_mode', () => c('select:tail_mode').onValueChange?.('scaled'), 'scaled'],
    ['tail_low_bytes', () => c('Kept after a trim in KB').onCommit?.(32), 32_000],
    ['tail_high_bytes', () => c('Trim at in KB').onCommit?.(96), 96_000],
    ['cold_after_secs', () => c('Trim early when idle in seconds').action?.mutate(600), 600],
    ['merge_at', () => c('Start merging at in percent').onCommit?.(80), 0.8],
    ['profile', () => c('select:profile').onValueChange?.('small'), 'small'],
    ['describe_images', () => c('Describe images').onCheckedChange?.(false), 'off'],
    ['ledger', () => c('Keep a state ledger').onCheckedChange?.(false), false],
    ['ledger_bytes', () => c('Ledger size in KB').action?.mutate(8), 8_000],
    ['ledger_suggestions', () => c('Show summary-model suggestions').onCheckedChange?.(false), false],
    ['prefetch_bytes', () => c('Prefetch size in bytes').action?.mutate(0), 0],
    ['age_by_use', () => c('Keep zoomed lines unmerged').onCheckedChange?.(false), false],
  ]
  for (const [key, fire, value] of fixed) expect((await written(fire))[key], key).toBe(value)
  served = { ...config, tail_mode: 'scaled' }
  await render()
  served = config
  const scaled: [keyof EndlessConfig, () => void, unknown][] = [
    ['tail_low_share', () => c('Kept after a trim in percent').onCommit?.(15), 0.15],
    ['tail_high_share', () => c('Trim at in percent').onCommit?.(40), 0.4],
  ]
  for (const [key, fire, value] of scaled) expect((await written(fire))[key], key).toBe(value)
  const covered = new Set([...fixed, ...scaled].map(([k]) => k))
  for (const key of ['tail_mode', 'tail_low_bytes', 'tail_high_bytes', 'tail_low_share', 'tail_high_share', 'cold_after_secs', 'profile', 'merge_at', 'ledger', 'ledger_bytes', 'ledger_suggestions', 'prefetch_bytes', 'age_by_use', 'describe_images'] as const) expect(covered.has(key), key).toBe(true)
})

test('201: the tail pair is checked in place — a high mark at or below the low one, or out of range, is named and never written', async () => {
  expect(tailPairError('fixed', 24_000, 64_000)).toBeNull()
  expect(tailPairError('fixed', 64_000, 64_000)).toBe('Trim at (64 KB) must be above Kept after a trim (64 KB).')
  expect(tailPairError('fixed', 4_000, 64_000)).toBe('Kept after a trim must be 8 KB–256 KB.')
  expect(tailPairError('scaled', 0.3, 0.2)).toBe('Trim at (20 %) must be above Kept after a trim (30 %).')
  expect(tailPairError('scaled', 0.1, 0.7)).toBe('Trim at must be 5 %–60 %.')
  await render()
  // Two refused commits, then a valid one: only the valid pair reaches the engine.
  const sent = await written(() => {
    controls['Trim at in KB']?.onCommit?.(16)
    controls['Kept after a trim in KB']?.onCommit?.(64)
    controls['Trim at in KB']?.onCommit?.(128)
  })
  expect([sent.tail_low_bytes, sent.tail_high_bytes]).toEqual([24_000, 128_000])
})

test('201: the sizing mode shows its own pair — KB with token estimates when fixed, percent with the resolved KB for the default model when scaled; empty idle field = automatic', async () => {
  const fixed = await render()
  expect(fixed).toMatch(/<input type="range" aria-label="Kept after a trim in KB"[^>]*min="8" max="256" step="4" value="24"/)
  expect(fixed).toMatch(/<input type="range" aria-label="Trim at in KB"[^>]*min="16" max="512" step="8" value="64"/)
  expect(fixed).toContain('≈ 8 k tokens of the newest messages')
  expect(fixed).not.toContain('in percent"')
  expect(fixed).toMatch(/<input[^>]*aria-label="Trim early when idle in seconds"[^>]*placeholder="automatic"/)
  useConnection.setState({ info: { default_model: 'anthropic/big' } as never })
  listed = [{ ...lfm, id: 'anthropic/big', name: 'Big', provider: 'anthropic', context: 200_000, local: false }]
  served = { ...config, tail_mode: 'scaled', tail_low_share: 0.1, tail_high_share: 0.25 }
  const scaled = await render()
  served = config
  listed = []
  useConnection.setState({ info: null })
  expect(scaled).toMatch(/<input type="range" aria-label="Kept after a trim in percent"[^>]*min="2" max="40" step="1" value="10"/)
  expect(scaled).toMatch(/<input type="range" aria-label="Trim at in percent"[^>]*min="5" max="60" step="1" value="25"/)
  expect(scaled).toContain('· ≈ 60 KB')
  expect(scaled).toContain('· ≈ 150 KB')
  expect(scaled).toContain(' on Big')
  expect(scaled).not.toContain('in KB"')
})

test('201: Automatic names what it resolves to for the summary model; the ledger switch gates its suggestions', async () => {
  served = { ...config, model: lfm.id, ledger: false }
  listed = [lfm]
  const html = await render()
  served = config
  listed = []
  expect(html).toContain('Automatic · small, because the model is local')
  expect(html).toMatch(/aria-checked="false"[^>]*disabled=""[^>]*aria-label="Show summary-model suggestions"/)
})
