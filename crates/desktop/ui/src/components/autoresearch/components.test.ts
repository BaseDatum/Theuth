import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactElement, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { RunTable } from './RunTable'
import { StudyChart } from './StudyChart'
import { StatePill } from './StudyList'
import { Bars, Controls, Current } from './StudyView'
import { studies } from './fixtures'
import { budgetBars } from './model'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ 'autoresearch.control': async () => ({}) }),
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
  for (const label of ['Pause', 'Stop after this', 'Stop now — discard the experiment in flight', 'Finalize now']) expect(html).toContain(label)
  expect(html).not.toContain('Resume')
  // Finalize now is disabled while an experiment is in flight (it is Stop-now's job then).
  expect(html).toMatch(/<button[^>]*disabled[^>]*>[^<]*<svg[^]*?Finalize now/)

  const paused = by('parse-alloc')
  html = paint(createElement(Controls, { info: paused.info }))
  for (const label of ['Resume', 'Run one', 'Finalize now']) expect(html).toContain(label)
  expect(html).not.toContain('Pause<')
  expect(html).not.toContain('Stop now')

  html = paint(createElement(Controls, { info: by('cold-start').info }))
  expect(html).toContain('Stop now')
  expect(html).not.toContain('Pause')

  html = paint(createElement(Controls, { info: by('flaky-bench').info }))
  expect(html).toContain('Discard study')

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

test('current experiment: chips from the progress event, the idea once the row exists', () => {
  const s = by('sort-speed')
  const row = s.rows.find((r) => r.n === 35) ?? null
  let html = paint(createElement(Current, { n: 35, row, progress: { phase: 'sampling', sample: 2, of: 3, elapsed_ms: 402_000 }, onOpen: () => {} }))
  expect(html).toContain('measuring 2/3')
  expect(html).toContain('Reuse the token buffer')
  expect(html).toContain('6:42')
  html = paint(createElement(Current, { n: 36, row: null, progress: { phase: null, sample: 0, of: 0, elapsed_ms: 0 }, onOpen: () => {} }))
  expect(html).toContain('Experiment 36 · editing')
  expect(html).toContain('the idea is declared at the first measurement')
})

test('run table: newest first, verdict badges, deviation marker, σ× and Δ columns', () => {
  const s = by('sort-speed')
  const html = paint(createElement(RunTable, { rows: s.rows, metricName: 'time_ms', pending: false, active: 14, onPick: () => {} }))
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
  expect(count(html, /aria-pressed="true"/g)).toBe(1)
})

test('run table: empty and pending states', () => {
  expect(paint(createElement(RunTable, { rows: [], metricName: 'time_ms', pending: false, active: null, onPick: () => {} }))).toContain('No experiments yet.')
  expect(paint(createElement(RunTable, { rows: undefined, metricName: 'time_ms', pending: true, active: null, onPick: () => {} }))).not.toContain('No experiments yet.')
})

test('budget bars: the fill turns warning near the cap and danger at it', () => {
  const html = paint(createElement(Bars, { bars: budgetBars({ usd: 19.6, hours: 10, experiments: 3 }, { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 }) }))
  expect(html).toContain('bg-warning')
  expect(html).toContain('bg-danger')
  expect(html).toContain('bg-fg-accent')
  expect(html).toContain('aria-valuenow="98"')
})
