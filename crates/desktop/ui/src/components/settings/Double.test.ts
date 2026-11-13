import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { DoubleClass } from "@/rpc/bindings/method.dreaming.output"
import type { DoubleClassDetail } from "@/rpc/bindings/method.dreaming.output"
import type { DoubleStatus } from "@/rpc/bindings/method.dreaming.output"
import type { Kpis } from "@/rpc/bindings/method.dreaming.output"
import { Double, chartProps, forgetClass, mergeCurves, runLine, setCap, setOptIn } from './Double'

const zero: Kpis = { n: 0, accuracy: 0, brier: 0, ece: 0, coverage_2pct: 0, asks_avoided: 0, minutes_saved: 0, overrides_per_100: 0, undo_rate_per_100: 0 }
/** What C's stub answers before W1/W2: `enabled: false`, no version, no run, empty numbers, no classes. */
const STUB: DoubleStatus = { enabled: false, paused: false, venue: 'auto', approvals_off_sessions: 0, kpis: zero, baseline: zero }
let status: DoubleStatus = STUB
let classes: DoubleClass[] = []
/** The double_* reads the tab made. */
const asked: string[] = []
const ask = <T>(name: string, value: T): T => (asked.push(name), value)
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    double_status: async () => ask('double_status', status),
    double_classes: async () => ask('double_classes', classes),
    double_class: async (): Promise<DoubleClassDetail> => ({ class: classes[0] as DoubleClass, curve: [], examples: [], promote_hint: '' }),
    double_settings_get: async () => ask('double_settings_get', {
      enabled: true,
      capture: { approvals: true, asks: true, attention: true, packets: true, drafts: true },
      ladder: { grace_secs: 30, caps: { read_only: 2, mutating: 1, destructive: 1 }, mutating_opt_in: false },
      training: { venue: 'auto', window: '02:00-06:00', idle_min: 15, run_cap_min: 90, triggers: { labels: 200, days: 7 }, share_data: true, tiers: { a: { enabled: true }, b: { enabled: true, rank: 8, batch: 8, epochs: 3, lr: 0.0002 }, c: { enabled: false } } },
      retention_days: 365,
      model: { checkpoint: 'default', adapter_rank: 8 },
    }),
    mesh_status: async () => ({ self_id: 'me', self_name: 'me', mesh_name: null, peers: [], lease: { holder: null, reason: '', since_ms: 0 }, traffic: [], replica: null }),
  }),
}))

/** The tab against an engine that runs the double (`running`) or not; `config_flag_get` answers before the first paint. */
async function render(running = true): Promise<string> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(['config-flag', 'double.enabled'], { key: 'double.enabled', enabled: running, running, restart_required: false })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Double) as ReactNode }))
  for (let pass = 0; pass < 3; pass++) {
    paint()
    for (const query of qc.getQueryCache().findAll()) {
      if ('enabled' in query.options && query.options.enabled === false) continue
      await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
    }
  }
  return paint()
}

test('the empty stub state renders every section without crashing', async () => {
  status = STUB
  classes = []
  const html = await render()
  for (const s of ['Status', 'Improvement', 'Classes', 'Training', 'Training log']) expect(html).toContain(s)
  expect(html).toContain('baseline')
  expect(html).toContain('No run yet.')
  expect(html).toContain('No labelled weeks yet')
  expect(html).toContain('No classes yet')
  expect(html).toContain('No training runs yet.')
  expect(html).toContain('Rebuild (tier C)')
  expect(html).toContain('02:00-06:00')
})

test('while the engine runs with the double off the tab holds only its switch and asks for nothing else (188 F1)', async () => {
  asked.length = 0
  const html = await render(false)
  expect(html).toContain('Off: nothing is predicted or learned.')
  expect(html).toContain('While the double runs, this tab also holds')
  for (const s of ['Improvement', 'Classes', 'Training log', 'aria-label="Pause learning"']) expect(html).not.toContain(s)
  expect(asked).toEqual([])
  await render(true)
  expect(asked).toContain('double_status')
})

test('a class row shows n, rung glyph, accuracy, ECE, cap and the last decision', async () => {
  const k: Kpis = { ...zero, n: 24, accuracy: 0.96, ece: 0.03 }
  classes = [{ key: 'approval:shell:mutating:git push:root', kind: 'approval', n: 24, rung: 1, cap: 1, cap_reason: 'mutating — opt in to allow L2', mutating_opt_in: false, median_answer_ms: 4000, kpis: k, baseline: zero, last_decision_at: '2026-10-06T09:00:00Z' }]
  status = { ...STUB, enabled: true, active_version: 'a-2026-10-06', last_run: { id: 'run-1', tier: 'a', venue: 'local', state: 'promoted', started_at: '2026-10-06T02:00:00Z', ended_at: '2026-10-06T02:00:40Z', result: 'promoted' } }
  const html = await render()
  expect(html).toContain('approval:shell:mutating:git push:root')
  expect(html).toContain('◐ L1')
  expect(html).toContain('96 %')
  expect(html).toContain('0.030')
  expect(html).toContain('a-2026-10-06')
  expect(html).toContain('tier A · local')
})

test('row actions hit the right methods with the class key: cap → double_set_cap(key, rung, opt_in); opt in keeps the cap; forget → class target', async () => {
  const calls: unknown[][] = []
  const api = {
    double_set_cap: async (...a: unknown[]) => (calls.push(['set_cap', ...a]), classes[0] as DoubleClass),
    double_forget: async (...a: unknown[]) => (calls.push(['forget', ...a]), 24),
  }
  const c = { key: 'k', cap: 1, mutating_opt_in: false } as DoubleClass
  await setCap(api, c, 2)
  await setOptIn(api, c, true)
  await forgetClass(api, 'k')
  expect(calls).toEqual([
    ['set_cap', 'k', 2, false],
    ['set_cap', 'k', 1, true],
    ['forget', { target: 'class', key: 'k' }],
  ])
})

test('the overall curve is the n-weighted merge of the per-class curves, weeks ascending; chart series in percent', () => {
  const pt = (week: string, n: number, acc: number, cov: number, nb: number, accB: number): DoubleClassDetail['curve'][number] => ({
    week,
    current: { ...zero, n, accuracy: acc, coverage_2pct: cov },
    baseline: { ...zero, n: nb, accuracy: accB, coverage_2pct: 0 },
  })
  const d = (curve: DoubleClassDetail['curve']): DoubleClassDetail => ({ class: {} as DoubleClass, curve, examples: [], promote_hint: '' })
  const merged = mergeCurves([d([pt('2026-10-05', 10, 1.0, 0.5, 10, 0.6), pt('2026-09-28', 5, 0.8, 0.2, 0, 0)]), d([pt('2026-10-05', 30, 0.9, 0.9, 30, 0.7)])])
  expect(merged.map((m) => m.week)).toEqual(['2026-09-28', '2026-10-05'])
  expect(merged[1]?.acc).toBeCloseTo((10 * 1.0 + 30 * 0.9) / 40)
  expect(merged[1]?.cov).toBeCloseTo((10 * 0.5 + 30 * 0.9) / 40)
  expect(merged[1]?.accBase).toBeCloseTo((10 * 0.6 + 30 * 0.7) / 40)
  expect(merged[0]?.accBase).toBe(0)
  const props = chartProps(merged)
  expect(props.labels).toEqual(['28 Sep', '5 Oct'])
  expect(props.series.map((s) => s.name)).toEqual(['accuracy', 'accuracy · baseline', 'coverage @ 2 %', 'coverage · baseline'])
  expect(props.series[0]?.values).toEqual([80, 92.5])
  expect(runLine({ id: 'r', tier: 'b', venue: 'node:gpu-1', state: 'rejected', started_at: '2026-10-06T02:00:00Z', ended_at: '2026-10-06T02:30:00Z', result: 'rejected: golden' })).toMatch(/^tier B · node:gpu-1 · .* · 30 min · rejected: golden$/)
})
