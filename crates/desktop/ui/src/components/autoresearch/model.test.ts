import { expect, test } from 'vitest'
import type { ExperimentRow } from '@/rpc/bindings/ExperimentRow'
import { studies } from './fixtures'
import { budgetBars, chartOutcome, clock, filterRuns, fraction, holdoutSummary, keepNeedsPct, layout, matches, metric, outcome, outcomeLabel, pct, phaseChips, phaseText, reviewWord, rowDuration, sigmas, sortStudies, stateLabel, tally, ticks } from './model'

const row = (over: Partial<ExperimentRow>): ExperimentRow => ({
  slug: 's',
  n: 1,
  wave: null,
  slot: null,
  segment: 0,
  state: 'discarded',
  verdict: { kind: 'no_gain' },
  idea: 'x',
  category: 'c',
  metric: 1,
  delta: 0,
  delta_pct: 0,
  z: 0,
  n_samples: 1,
  commit_sha: null,
  deviations: [],
  review: null,
  cost_usd: 0,
  started_at: '2026-10-08T00:00:00Z',
  finished_at: '2026-10-08T00:05:00Z',
  ...over,
})

test('outcome: the ledger state decides, the verdict refines a discard', () => {
  expect(outcome('kept', { kind: 'improved' })).toBe('kept')
  expect(outcome('kept', { kind: 'keep_simpler' })).toBe('kept')
  expect(outcome('rejected_by_review', { kind: 'improved' })).toBe('reverted')
  expect(outcome('crashed', { kind: 'crash' })).toBe('failed')
  expect(outcome('checks_failed', { kind: 'checks_failed' })).toBe('failed')
  expect(outcome('refused', { kind: 'refused', reason: 'r' })).toBe('failed')
  expect(outcome('discarded', { kind: 'guard_failed' })).toBe('failed')
  expect(outcome('discarded', { kind: 'worse' })).toBe('worse')
  expect(outcome('discarded', { kind: 'no_gain' })).toBe('no_gain')
  // The model threw away an improvement it distrusted: still a discard, not a keep.
  expect(outcome('discarded', { kind: 'improved' })).toBe('no_gain')
  expect(outcomeLabel('discarded', { kind: 'improved' })).toBe('discarded')
  expect(outcome('interrupted', null)).toBe('lost')
  expect(outcome('timeout', null)).toBe('lost')
  expect(outcome('measuring', null)).toBe('open')
  expect(outcome(null, null)).toBe('open')
  expect(outcomeLabel('kept', { kind: 'keep_simpler' })).toBe('kept (simpler)')
  expect(outcomeLabel('abandoned', null)).toBe('not logged')
})

test('chartOutcome accepts the ledger state or the verdict kind as the tag; unknown is grey', () => {
  expect(chartOutcome('kept')).toBe('kept')
  expect(chartOutcome('improved')).toBe('kept')
  expect(chartOutcome('rejected_by_review')).toBe('reverted')
  expect(chartOutcome('crashed')).toBe('failed')
  expect(chartOutcome('crash')).toBe('failed')
  expect(chartOutcome('interrupted')).toBe('lost')
  expect(chartOutcome('worse')).toBe('worse')
  expect(chartOutcome('')).toBe('no_gain')
})

test('stateLabel: running names the experiment, paused and finished their reason', () => {
  expect(stateLabel({ state: 'running' }, 35)).toBe('running · experiment 35')
  expect(stateLabel({ state: 'running' }, null)).toBe('running')
  expect(stateLabel({ state: 'paused', reason: { kind: 'crash_streak' } })).toBe('paused · the benchmark keeps failing')
  expect(stateLabel({ state: 'paused', reason: { kind: 'user' } })).toBe('paused · by you')
  expect(stateLabel({ state: 'paused', reason: { kind: 'other', reason: 'disk full' } })).toBe('paused · disk full')
  expect(stateLabel({ state: 'finished' }, null, { kind: 'plateau' })).toBe('finished · plateau')
  expect(stateLabel({ state: 'failed', reason: 'σ too large' })).toBe('failed · σ too large')
  expect(stateLabel({ state: 'preparing' })).toBe('preparing')
})

test('budget bars: fractions clamp, a zero cap is full, text carries units', () => {
  expect(fraction(5, 10)).toBe(0.5)
  expect(fraction(15, 10)).toBe(1)
  expect(fraction(-1, 10)).toBe(0)
  expect(fraction(1, 0)).toBe(1)
  const bars = budgetBars({ usd: 11.4, hours: 4.1, experiments: 34 }, { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 })
  expect(bars.map((b) => b.text)).toEqual(['$11.4 of $20', '4.1 of 10 h', '34 of 120'])
  expect(bars.map((b) => Math.round(b.fraction * 100))).toEqual([57, 41, 28])
})

test('number formatting: real minus sign, sigma suffix, metric precision, clocks, row duration', () => {
  expect(pct(-2.63)).toBe('−2.6 %')
  expect(pct(0.4)).toBe('+0.4 %')
  expect(pct(0)).toBe('0.0 %')
  expect(pct(null)).toBe('—')
  expect(sigmas(3.14)).toBe('3.1σ')
  expect(sigmas(-0.6)).toBe('−0.6σ')
  expect(metric(41.23, 'ms')).toBe('41.2 ms')
  expect(metric(812.4, 'ms')).toBe('812 ms')
  expect(metric(5400, 'ms')).toBe('5,400 ms')
  expect(metric(0.0421, '')).toBe('0.042')
  expect(metric(null, 'ms')).toBe('—')
  expect(clock(6 * 60_000 + 42_000)).toBe('6:42')
  expect(clock(3_725_000)).toBe('1:02:05')
  expect(rowDuration(row({}))).toBe(300_000)
  expect(rowDuration(row({ finished_at: null }), Date.parse('2026-10-08T00:01:00Z'))).toBe(60_000)
})

test('phase chips: editing before the first run, measuring k/N lights the sampling chip, review after', () => {
  const editing = phaseChips({ phase: null, sample: 0, of: 0, elapsed_ms: 0 })
  expect(editing.map((c) => c.status)).toEqual(['done', 'on', 'todo', 'todo', 'todo'])
  const sampling = phaseChips({ phase: 'sampling', sample: 2, of: 3, elapsed_ms: 0 })
  expect(sampling.map((c) => `${c.label}:${c.status}`)).toEqual(['thinking:done', 'editing:done', 'measuring 2/3:on', 'checks:todo', 'review:todo'])
  expect(phaseChips({ phase: 'checks', sample: 3, of: 3, elapsed_ms: 0 })[3]).toEqual({ label: 'checks', status: 'on' })
  expect(phaseChips({ phase: 'review', sample: 3, of: 3, elapsed_ms: 0 })[4]?.status).toBe('on')
  expect(phaseChips({ phase: 'waiting', sample: 1, of: 1, elapsed_ms: 0 })[2]?.status).toBe('on')
  expect(phaseText({ phase: 'sampling', sample: 2, of: 3, elapsed_ms: 0 })).toBe('measuring sample 2 of 3')
  expect(phaseText({ phase: 'waiting', sample: 1, of: 1, elapsed_ms: 0 })).toBe('bench slot')
  expect(phaseText({ phase: null, sample: 0, of: 0, elapsed_ms: 0 })).toBe('editing')
})

test('run filters and tally: kept is the ledger state; flagged is anything a person should look at', () => {
  const rows = [
    row({ n: 1, state: 'kept', verdict: { kind: 'improved' } }),
    row({ n: 2, state: 'rejected_by_review', verdict: { kind: 'improved' }, review: { kind: 'reverted', reason: 'r' } }),
    row({ n: 3, state: 'kept', verdict: { kind: 'improved' }, deviations: ['lib/x.rb'] }),
    row({ n: 4, state: 'discarded', verdict: { kind: 'no_gain' } }),
    row({ n: 5, state: 'refused', verdict: { kind: 'refused', reason: 'evaluator changed' } }),
    row({ n: 6, state: 'discarded', verdict: { kind: 'guard_failed' } }),
    row({ n: 7, state: 'kept', verdict: { kind: 'improved' }, review: { kind: 'flagged', reason: 'hmm' } }),
    row({ n: 8, state: 'crashed', verdict: { kind: 'crash' } }),
  ]
  expect(filterRuns(rows, 'all').map((r) => r.n)).toEqual([1, 2, 3, 4, 5, 6, 7, 8])
  expect(filterRuns(rows, 'kept').map((r) => r.n)).toEqual([1, 3, 7])
  expect(filterRuns(rows, 'flagged').map((r) => r.n)).toEqual([2, 3, 5, 6, 7])
  expect(tally(rows)).toEqual({ kept: 3, reverted: 1, crashed: 1, checks_failed: 0 })
})

test('reviewWord: reverted and flagged both read SUSPICIOUS; error has no verdict; null is unreviewed', () => {
  expect(reviewWord({ kind: 'clean', reason: '' })).toEqual({ word: 'CLEAN', tone: 'clean' })
  expect(reviewWord({ kind: 'reverted', reason: '' }).word).toBe('SUSPICIOUS')
  expect(reviewWord({ kind: 'flagged', reason: '' }).word).toBe('SUSPICIOUS')
  expect(reviewWord({ kind: 'error', reason: '' }).tone).toBe('none')
  expect(reviewWord(null).word).toBe('unreviewed')
})

test('holdout summary means the samples of each checkpoint (R-89) and the keep threshold', () => {
  const d = studies()[0]?.detail
  if (!d) throw new Error('fixture')
  // Three samples per checkpoint in the fixture; the card shows their means, not the first/last raw sample.
  expect(d.series.holdout.filter((h) => h.n === 0)).toHaveLength(3)
  expect(holdoutSummary(d.series)).toEqual({ base: expect.closeTo(61, 5), latest: expect.closeTo(49.7, 5), delta_pct: expect.closeTo(-18.52, 1), n: 31 })
  expect(holdoutSummary({ holdout: [] })).toBeNull()
  // 2σ√2 over the incumbent: 2·0.42·1.414 / 41.2 = 2.9 %; the relative min_effect 0.5 % is below it.
  expect(keepNeedsPct(0.42, 41.2, '0.5%')).toBeCloseTo(2.88, 1)
  // An absolute min_effect of 2 ms over 41.2 ms = 4.9 % wins over the noise term.
  expect(keepNeedsPct(0.42, 41.2, '2')).toBeCloseTo(4.85, 1)
  expect(keepNeedsPct(null, 41.2, '0.5%')).toBeNull()
})

test('rail order and filter: live states first, newest within; slug or goal matches', () => {
  const all = studies().map((s) => s.info)
  const sorted = sortStudies(all)
  expect(sorted[0]?.state.state).toBe('running')
  expect(sorted.filter((s) => s.state.state === 'paused').length).toBe(5)
  expect(sorted.at(-1)?.state.state).toBe('failed')
  expect(sorted.findIndex((s) => s.state.state === 'finished')).toBeGreaterThan(sorted.findLastIndex((s) => s.state.state === 'paused'))
  expect(matches({ slug: 'sort-speed', goal: 'Make parse faster' }, 'PARSE')).toBe(true)
  expect(matches({ slug: 'sort-speed', goal: 'x' }, 'speed')).toBe(true)
  expect(matches({ slug: 'sort-speed', goal: 'x' }, 'alloc')).toBe(false)
  expect(matches({ slug: 'a', goal: 'b' }, '  ')).toBe(true)
})

test('ticks: 1-2-5 ladder within the range', () => {
  expect(ticks(39.5, 54)).toEqual([40, 42, 44, 46, 48, 50, 52, 54])
  expect(ticks(0, 1, 5)).toEqual([0, 0.2, 0.4, 0.6, 0.8, 1])
})

test('layout: the staircase starts at the base, marks follow their tag, crashes have no y, holdout gets its own axis', () => {
  const d = studies()[0]?.detail
  if (!d) throw new Error('fixture')
  const l = layout(d.series, { width: 900, height: 260, unit: 'ms', lastN: 35 })
  expect(l.stair.startsWith(`M${l.x(0).toFixed(1)},${l.y(52.1).toFixed(1)}`)).toBe(true)
  // Lower is better: every later step is drawn lower on screen (larger y) than the base.
  for (const s of d.series.incumbent.slice(1)) expect(l.y(s.metric as number)).toBeGreaterThan(l.y(52.1))
  const crash = l.marks.find((m) => m.n === 5)
  expect(crash?.outcome).toBe('failed')
  expect(crash?.y).toBeNull()
  expect(l.marks.find((m) => m.n === 3)?.outcome).toBe('kept')
  expect(l.marks.find((m) => m.n === 14)?.outcome).toBe('reverted')
  expect(l.marks.find((m) => m.n === 33)?.outcome).toBe('lost')
  expect(l.marks.find((m) => m.n === 35)?.outcome).toBe('open')
  expect(l.marks.find((m) => m.n === 3)?.label).toBe('#3 kept · 50.4 ms')
  // With the run rows the tooltip is the verdict in words (R-97).
  const withRows = layout(d.series, { width: 900, height: 260, unit: 'ms', lastN: 35, rows: [row({ n: 3, state: 'kept', delta_pct: 3.3, z: 4.1, idea: 'memoise the token table' })] })
  expect(withRows.marks.find((m) => m.n === 3)?.label).toBe('#3 kept +3.3 % 4.1σ · 50.4 ms · memoise the token table')
  // One diamond per checkpoint, not per sample.
  expect(l.holdout.length).toBe(4)
  expect(l.yh).not.toBeNull()
  expect(l.right).toBeGreaterThan(16)
  expect(l.band).not.toBeNull()
  // Every mark lies inside the plot area.
  for (const m of l.marks) {
    expect(m.x).toBeGreaterThanOrEqual(l.left)
    expect(m.x).toBeLessThanOrEqual(900 - l.right)
    if (m.y !== null) {
      expect(m.y).toBeGreaterThanOrEqual(l.top)
      expect(m.y).toBeLessThanOrEqual(260 - l.bottom)
    }
  }
  // No holdout, no σ → no right axis, narrow margin, empty path, no band.
  const bare = layout({ experiments: [], incumbent: [], holdout: [], drift: [], baseline_mean: null, sigma: null }, { width: 400, height: 200, unit: '' })
  expect(bare.yh).toBeNull()
  expect(bare.right).toBe(16)
  expect(bare.stair).toBe('')
  expect(bare.band).toBeNull()
})
