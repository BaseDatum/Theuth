import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactElement, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import { RunTable } from './RunTable'
import { StudyChart } from './StudyChart'
import { StatePill } from './StudyList'
import { Finished } from './Finished'
import { SidePanel } from './SidePanel'
import { Body as ExperimentBody } from './ExperimentDetail'
import { Bars, Controls, Current, KindPanel } from './StudyView'
import { studies } from './fixtures'
import { budgetBars } from './model'
import type { ExperimentRow } from "@/rpc/bindings/method.autoresearch.output"
import type { ObjectiveDelta } from "@/rpc/bindings/method.autoresearch.output"

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ 'autoresearch.control': async () => ({}) }),
}))
// `Finished` links the report document; a static paint has no router.
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children?: unknown }) => createElement('a', { href: '#' }, children as never),
}))

const all = studies()
const by = (slug: string) => {
  const s = all.find((x) => x.info.slug === slug)
  if (!s) throw new Error(slug)
  return s
}

function paint(el: ReactElement): string {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: el }))
}

const count = (html: string, re: RegExp) => (html.match(re) ?? []).length

test('chart: a mark per experiment coloured by outcome, × for crashes, hollow for interrupted, staircase, band, holdout diamonds', () => {
  const d = by('sort-speed').detail
  const html = paint(createElement(StudyChart, { detail: d, onPick: () => {} }))
  expect(count(html, /data-outcome="kept"/g)).toBe(7)
  expect(count(html, /data-outcome="reverted"/g)).toBe(1)
  expect(count(html, /data-outcome="lost"/g)).toBe(1)
  // 3 crashes drawn as × paths (a <g> each) + 1 checks-failed circle = 4 failed marks.
  expect(count(html, /data-outcome="failed"/g)).toBe(4)
  // The experiment in flight (#35) is a hollow info dot pinned to the top.
  expect(count(html, /data-outcome="open"/g)).toBe(1)
  expect(count(html, /<g [^>]*data-outcome="failed"[^>]*data-n="(5|16|29)"/g)).toBe(3)
  expect(html).toContain('stroke="var(--status-success-fg)" stroke-width="2.2"')
  expect(html).toContain('fill="var(--fg-accent)" fill-opacity="0.1"')
  expect(count(html, /transform="rotate\(45 /g)).toBe(4)
  expect(html).toContain('>holdout<')
  expect(html).toContain('>drift<')
  // Every mark is keyboard reachable and labelled with its verdict line.
  expect(count(html, /tabindex="0" role="button" aria-label="#\d+ /g)).toBe(d.series.experiments.length)
  expect(html).toContain('aria-label="#3 kept · 50.4 ms"')
})

test('chart: a study without holdout or σ draws no right axis and no band', () => {
  const d = by('cold-start').detail
  const html = paint(createElement(StudyChart, { detail: d, onPick: () => {} }))
  expect(html).not.toContain('>holdout<')
  expect(html).not.toContain('fill-opacity="0.1"')
})

test('controls: the buttons each state offers (§8.6)', () => {
  const running = by('sort-speed')
  let html = paint(createElement(Controls, { info: running.info }))
  for (const label of ['Pause', 'Stop after this', 'Stop now — discard the experiment in flight', 'Finalize now', 'New segment']) expect(html).toContain(label)
  expect(html).not.toContain('Resume')
  // Finalize now is disabled while an experiment is in flight (it is Stop-now's job then).
  expect(html).toMatch(/<button[^>]*disabled[^>]*>[^<]*<svg[^]*?Finalize now/)

  const paused = by('parse-alloc')
  html = paint(createElement(Controls, { info: paused.info }))
  for (const label of ['Resume', 'Run one', 'Finalize now', 'New segment']) expect(html).toContain(label)
  expect(html).not.toContain('Pause<')
  expect(html).not.toContain('Stop now')

  // R-85: `stop_now` is refused while preparing, so nothing is offered but the word.
  html = paint(createElement(Controls, { info: by('cold-start').info }))
  expect(html).toContain('preparing')
  expect(html).not.toContain('<button')

  html = paint(createElement(Controls, { info: by('flaky-bench').info }))
  expect(html).toContain('Discard study')
  // R-99: an already-discarded study offers nothing.
  html = paint(createElement(Controls, { info: { ...by('flaky-bench').info, state: { state: 'discarded' } } }))
  expect(html).not.toContain('Discard study')

  html = paint(createElement(Controls, { info: { ...running.info, state: { state: 'finalizing' }, current: null } }))
  expect(html).toContain('finalizing')
  expect(html).not.toContain('<button')
})

test('state pill: words per state, the reason for a pause, the experiment while running, the bare word in the rail', () => {
  expect(paint(createElement(StatePill, { state: by('sort-speed').info.state, currentN: 35 }))).toContain('running · experiment 35')
  expect(paint(createElement(StatePill, { state: by('fixture-guard').info.state }))).toContain('paused · evaluator files changed')
  expect(paint(createElement(StatePill, { state: by('overnight-sim').info.state }))).toContain('paused · budget needs your OK')
  expect(paint(createElement(StatePill, { state: by('lexer-tokens').info.state, stop: by('lexer-tokens').info.stop_reason }))).toContain('finished · plateau')
  expect(paint(createElement(StatePill, { state: by('fixture-guard').info.state, short: true }))).toContain('>paused<')
})

test('current experiment: chips from the progress event, the idea once the row exists, the log tail while measuring', () => {
  const s = by('sort-speed')
  const row = s.rows.find((r) => r.n === 35) ?? null
  let html = paint(createElement(Current, { n: 35, row, progress: { phase: 'sampling', sample: 2, of: 3, elapsed_ms: 402_000 }, tail: 'CASE n4000 time_ms=144.196\nMETRIC time_ms=1948.974', onOpen: () => {} }))
  expect(html).toContain('measuring 2/3')
  expect(html).toContain('Reuse the token buffer')
  expect(html).toContain('6:42')
  expect(html).toContain('METRIC time_ms=1948.974')
  html = paint(createElement(Current, { n: 36, row: null, progress: { phase: null, sample: 0, of: 0, elapsed_ms: 0 }, tail: null, onOpen: () => {} }))
  expect(html).toContain('Experiment 36 · editing')
  expect(html).toContain('the idea is declared at the first measurement')
  expect(html).not.toContain('Sample log tail')
})

test('run table: newest first, verdict badges, deviation marker, σ× and Δ columns', () => {
  const s = by('sort-speed')
  const html = paint(createElement(RunTable, { rows: s.rows, metricName: 'time_ms', direction: 'lower', pending: false, active: 14, onPick: () => {} }))
  const order = [...html.matchAll(/<tr[^>]*><td[^>]*>(\d+)<\/td>/g)].map((m) => Number(m[1]))
  expect(order[0]).toBe(35)
  expect(order.at(-1)).toBe(1)
  expect(html).toContain('>kept<')
  expect(html).toContain('>reverted<')
  expect(html).toContain('>crash<')
  expect(html).toContain('>checks failed<')
  expect(html).toContain('>interrupted<')
  expect(html).toContain('>measuring<')
  expect(html).toContain('· deviation')
  expect(html).toContain('aria-current="true"')
  expect(html).toContain('3.1σ')
  expect(html).toContain('−3.3 %')
  // One Δ convention: a kept lower-is-better row is a negative, success-coloured number with the word on hover.
  expect(html).toMatch(/class="tabular-nums text-success[^"]*" title="better — lower is better">−/)
  expect(count(html, /aria-pressed="true"/g)).toBe(1)
})

test('run table: empty and pending states', () => {
  expect(paint(createElement(RunTable, { rows: [], metricName: 'time_ms', direction: 'lower', pending: false, active: null, onPick: () => {} }))).toContain('No experiments yet.')
  expect(paint(createElement(RunTable, { rows: undefined, metricName: 'time_ms', direction: 'lower', pending: true, active: null, onPick: () => {} }))).not.toContain('No experiments yet.')
})

test('finished: the headline reads the raw Δ with the direction word; the holdout is finalize’s line when the series has only the base (140 §13 Revision 2)', () => {
  const s = by('lexer-tokens')
  const base = { ...s.detail, series: { ...s.detail.series, holdout: s.detail.series.holdout.filter((h) => h.n === 0) }, final_holdout: '61.0 → 49.7 (−18.5 %)' }
  const html = paint(createElement(Finished, { detail: base }))
  expect(html).toMatch(/time_ms [\d.,]+ → [\d.,]+ ms \(−[\d.]+ %, better\) · holdout 61\.0 → 49\.7 \(−18\.5 %\)/)
  const none = paint(createElement(Finished, { detail: { ...base, final_holdout: null } }))
  expect(none).not.toContain('holdout')
  expect(none).not.toContain('→ 61')
})

test('budget bars: the fill turns warning near the cap and danger at it', () => {
  const html = paint(createElement(Bars, { bars: budgetBars({ usd: 19.6, hours: 10, experiments: 3, by_channel: {} }, { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 }) }))
  expect(html).toContain('bg-warning')
  expect(html).toContain('bg-danger')
  expect(html).toContain('bg-fg-accent')
  expect(html).toContain('aria-valuenow="98"')
})

test('kind panel: a `front` view paints the W1 component, not the scalar chart (159 §5.7)', () => {
  const d = by('sort-speed').detail
  const front = { ...d, view: { kind: 'front' as const, axes: ['time_ms', 'size_kb'] as [string, string], points: [], members: [] } }
  const html = paint(createElement(KindPanel, { detail: front, onPick: () => {} }))
  expect(html).toContain('data-kind="front"')
  expect(html).not.toContain('data-outcome=')
})

const objective = (name: string, delta_pct: number, delta: number, call: ObjectiveDelta['call']): ObjectiveDelta => ({ name, role: 'objective', candidate: null, reference: null, delta, delta_pct, z: null, call })
const gate = (objectives: ObjectiveDelta[]): ExperimentRow['gate'] => ({ v: 2, rule: 'dominance', objectives, stat: 3.1, threshold: 2.4, n: 3 })

test('run table: a front study adds one Δ % column per objective with its direction word and call; scalar rows show — (159 W1 H2)', () => {
  const [kept, other, ...rest] = by('sort-speed').rows
  const rows: ExperimentRow[] = [
    receiveComponent("method.autoresearch.output.ExperimentRow", { ...kept!, state: 'kept', gate: gate([objective('time_ms', -12.5, 6.2, 'better'), objective('size_kb', 3.4, -1.7, 'worse')]) }),
    receiveComponent("method.autoresearch.output.ExperimentRow", { ...other!, state: 'discarded', gate: gate([objective('time_ms', 0.2, -0.1, 'same'), objective('size_kb', -8.1, 4.0, 'better')]) }),
    rest[0]!,
  ]
  const html = paint(createElement(RunTable, { rows, metricName: 'time_ms', direction: 'lower', pending: false, active: null, onPick: () => {} }))
  expect(html).toMatch(/<th[^>]*title="lower is better"[^>]*>time_ms Δ %<span[^>]*>lower<\/span><\/th>/)
  expect(html).toMatch(/<th[^>]*title="lower is better"[^>]*>size_kb Δ %<span[^>]*>lower<\/span><\/th>/)
  expect(html.indexOf('time_ms Δ %')).toBeLessThan(html.indexOf('size_kb Δ %'))
  expect(html).toMatch(/text-success[^"]*" title="better — lower is better">−12.5 %<\/span><span class="[^"]*bg-success-bg text-success">better<\/span>/)
  expect(html).toMatch(/text-danger[^"]*" title="worse — lower is better">\+3.4 %<\/span><span class="[^"]*text-danger">worse<\/span>/)
  expect(html).toContain('>same</span>')
  // The scalar row has no objectives: a dash per objective column.
  expect(count(html, /<span class="text-fg-tertiary">—<\/span>/g)).toBe(2)
  // No front rows → no objective columns at all.
  expect(paint(createElement(RunTable, { rows: [rest[0]!], metricName: 'time_ms', direction: 'lower', pending: false, active: null, onPick: () => {} }))).not.toContain('Δ %')
})

test('run table: a waiting row shows its line across the value and Δ cells, badge from outcomeLabel (159 W6)', () => {
  const base = by('sort-speed').rows[0]!
  const rows: ExperimentRow[] = [receiveComponent("method.autoresearch.output.ExperimentRow", { ...base, state: 'waiting', verdict: null, metric: null, delta_pct: null, line: 'waiting · window 2 of at most 14 · closes in 6h' })]
  const html = paint(createElement(RunTable, { rows, metricName: 'conversion', direction: 'higher', pending: false, active: null, onPick: () => {} }))
  expect(html).toMatch(/<td colSpan="2"[^>]*>waiting · window 2 of at most 14 · closes in 6h<\/td>/)
  expect(html).toMatch(/bg-info-bg text-info">waiting for its window</)
})

test('side panel: a literature idea reaches LiteratureIdeas with its citation — link and quote (159 W5 hand-off 1)', () => {
  const d = by('sort-speed').detail
  const ideas = [...d.ideas, { id: 9001, slug: d.info.slug, text: '[paper] Merge natural runs like Timsort', category: 'algorithm', source: 'literature' as const, priority: 1, status: 'open' as const, tried_in: [], at: '2026-10-10T00:00:00Z' }]
  const citations = [{ idea_id: 9001, url: 'https://example.org/timsort', title: 'Timsort', quote: 'runs that are already ordered', verified: true, fetched_at: '2026-10-10T00:00:00Z' }]
  const html = paint(createElement(SidePanel, { detail: { ...d, ideas, citations }, rows: [], onPick: () => {} }))
  expect(html).toContain('href="https://example.org/timsort"')
  expect(html).toContain('paper · Timsort')
  expect(html).toContain('>runs that are already ordered</q>')
})

test('experiment detail: a Hooks block with one preformatted entry per hook output, none when empty (159 W5 hand-off 6)', () => {
  const s = by('sort-speed')
  const d = s.details.get(s.rows[0]!.n)!
  expect(paint(createElement(ExperimentBody, { d: { ...d, hooks: [] }, study: s.detail }))).not.toContain('>Hooks<')
  const html = paint(createElement(ExperimentBody, { d: { ...d, hooks: ['before: warmed cache\nok', 'after: flushed'] }, study: s.detail }))
  expect(html).toContain('data-hooks="2"')
  expect(html).toContain('>Hooks<')
  expect(html).toMatch(/<pre class="[^"]*font-mono[^"]*">before: warmed cache\nok<\/pre>/)
  expect(html).toMatch(/<pre class="[^"]*font-mono[^"]*">after: flushed<\/pre>/)
})
