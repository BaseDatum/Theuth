import { expect, test } from 'vitest'
import { studies } from './fixtures'
import { budgetBars, clock, filterRuns, fraction, layout, matches, metric, outcome, outcomeLabel, pct, phaseChips, phaseText, routineSentence, sigmas, sortStudies, stateLabel, ticks } from './model'
import type { ExperimentRow } from './types'

const row = (over: Partial<ExperimentRow>): ExperimentRow => ({
  n: 1,
  wave: null,
  slot: null,
  state: 'discarded',
  verdict: 'no_gain',
  line: '',
  idea: 'x',
  category: 'c',
  metric: 1,
  delta: 0,
  delta_pct: 0,
  z: 0,
  n_samples: 1,
  commit_sha: null,
  cost_usd: 0,
  duration_ms: 0,
  deviations: [],
  review: null,
  started_at: '2026-10-08T00:00:00Z',
  finished_at: null,
  ...over,
})

test('outcome: the ledger state decides, the verdict refines a discard', () => {
  expect(outcome('kept', 'improved')).toBe('kept')
  expect(outcome('kept', 'keep_simpler')).toBe('kept')
  expect(outcome('rejected_by_review', 'improved')).toBe('reverted')
  expect(outcome('crashed', 'crash')).toBe('failed')
  expect(outcome('checks_failed', 'checks_failed')).toBe('failed')
  expect(outcome('refused', 'refused')).toBe('failed')
  expect(outcome('discarded', 'guard_failed')).toBe('failed')
  expect(outcome('discarded', 'worse')).toBe('worse')
  expect(outcome('discarded', 'no_gain')).toBe('no_gain')
  // The model threw away an improvement it distrusted: still a discard, not a keep.
  expect(outcome('discarded', 'improved')).toBe('no_gain')
  expect(outcomeLabel('discarded', 'improved')).toBe('discarded')
  expect(outcome('interrupted', null)).toBe('lost')
  expect(outcome('abandoned', null)).toBe('lost')
  expect(outcome('measuring', null)).toBe('open')
  expect(outcomeLabel('kept', 'keep_simpler')).toBe('kept (simpler)')
  expect(outcomeLabel('abandoned', null)).toBe('not logged')
})

test('stateLabel: running names the experiment, paused and finished their reason', () => {
  expect(stateLabel({ state: 'running', reason: null, experiments: 34 }, 35)).toBe('running · experiment 35')
  expect(stateLabel({ state: 'running', reason: null, experiments: 0 }, null)).toBe('running')
  expect(stateLabel({ state: 'paused', reason: 'crash_streak', experiments: 12 })).toBe('paused · the benchmark keeps failing')
  expect(stateLabel({ state: 'paused', reason: 'user', experiments: 12 })).toBe('paused · by you')
  expect(stateLabel({ state: 'finished', reason: 'plateau', experiments: 61 })).toBe('finished · plateau')
  expect(stateLabel({ state: 'preparing', reason: null, experiments: 0 })).toBe('preparing')
})

test('budget bars: fractions clamp, a zero cap is full, text carries units', () => {
  expect(fraction(5, 10)).toBe(0.5)
  expect(fraction(15, 10)).toBe(1)
  expect(fraction(-1, 10)).toBe(0)
  expect(fraction(1, 0)).toBe(1)
  const bars = budgetBars({ usd: 11.4, usd_cap: 20, hours: 4.1, hours_cap: 10, experiments: 34, experiments_cap: 120 })
  expect(bars.map((b) => b.text)).toEqual(['$11.4 of $20', '4.1 of 10 h', '34 of 120'])
  expect(bars.map((b) => Math.round(b.fraction * 100))).toEqual([57, 41, 28])
})

test('number formatting: real minus sign, sigma suffix, metric precision, clocks', () => {
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
})

test('phase chips: editing before the first run, measuring k/N lights the sampling chip, review after', () => {
  const editing = phaseChips({ state: 'open', phase: null, sample: 0, of: 0 })
  expect(editing.map((c) => c.status)).toEqual(['done', 'on', 'todo', 'todo', 'todo'])
  const sampling = phaseChips({ state: 'measuring', phase: 'Sampling', sample: 2, of: 3 })
  expect(sampling.map((c) => `${c.label}:${c.status}`)).toEqual(['thinking:done', 'editing:done', 'measuring 2/3:on', 'checks:todo', 'review:todo'])
  const checks = phaseChips({ state: 'measuring', phase: 'Checks', sample: 3, of: 3 })
  expect(checks[3]).toEqual({ label: 'checks', status: 'on' })
  expect(phaseChips({ state: 'measured', phase: null, sample: 3, of: 3 })[4]?.status).toBe('on')
  expect(phaseText({ state: 'measuring', phase: 'Sampling', sample: 2, of: 3 })).toBe('measuring sample 2 of 3')
  expect(phaseText({ state: 'measuring', phase: 'Waiting', sample: 1, of: 1 })).toBe('bench slot')
  expect(phaseText({ state: 'open', phase: null, sample: 0, of: 0 })).toBe('editing')
})

test('run filters: kept is the ledger state; flagged is anything a person should look at', () => {
  const rows = [
    row({ n: 1, state: 'kept', verdict: 'improved' }),
    row({ n: 2, state: 'rejected_by_review', verdict: 'improved', review: { verdict: 'suspicious', reason: 'r' } }),
    row({ n: 3, state: 'kept', verdict: 'improved', deviations: ['lib/x.rb'] }),
    row({ n: 4, state: 'discarded', verdict: 'no_gain' }),
    row({ n: 5, state: 'refused', verdict: 'refused' }),
    row({ n: 6, state: 'discarded', verdict: 'guard_failed' }),
  ]
  expect(filterRuns(rows, 'all').map((r) => r.n)).toEqual([1, 2, 3, 4, 5, 6])
  expect(filterRuns(rows, 'kept').map((r) => r.n)).toEqual([1, 3])
  expect(filterRuns(rows, 'flagged').map((r) => r.n)).toEqual([2, 3, 5, 6])
})

test('rail order and filter: live states first, newest within; slug or goal matches', () => {
  const all = studies().map((s) => s.info)
  const sorted = sortStudies(all)
  expect(sorted[0]?.state).toBe('running')
  expect(sorted.map((s) => s.state).filter((st) => st === 'paused').length).toBe(5)
  expect(sorted.at(-1)?.state).toBe('failed')
  expect(sorted.findIndex((s) => s.state === 'finished')).toBeGreaterThan(sorted.findLastIndex((s) => s.state === 'paused'))
  expect(matches({ slug: 'sort-speed', goal: 'Make parse faster' }, 'PARSE')).toBe(true)
  expect(matches({ slug: 'sort-speed', goal: 'x' }, 'speed')).toBe(true)
  expect(matches({ slug: 'sort-speed', goal: 'x' }, 'alloc')).toBe(false)
  expect(matches({ slug: 'a', goal: 'b' }, '  ')).toBe(true)
})

test('routineSentence: the §13.1 line for the Routines row', () => {
  const info = studies()[0]?.info
  if (!info) throw new Error('fixture')
  expect(routineSentence(info)).toBe(`Optimising time_ms in liquid · 34 experiments · ${pct(info.delta_pct)} · running`)
})

test('ticks: 1-2-5 ladder within the range', () => {
  expect(ticks(39.5, 54)).toEqual([40, 42, 44, 46, 48, 50, 52, 54])
  expect(ticks(0, 1, 5)).toEqual([0, 0.2, 0.4, 0.6, 0.8, 1])
})

test('layout: the staircase starts at the base, marks follow their outcome, crashes have no y, holdout gets its own axis', () => {
  const d = studies()[0]?.detail
  if (!d) throw new Error('fixture')
  const l = layout(d.chart, { width: 900, height: 260, base: d.info.base_value, sigma: d.sigma, unit: 'ms', lastN: 35 })
  expect(l.stair.startsWith(`M${l.x(0).toFixed(1)},${l.y(52.1).toFixed(1)}`)).toBe(true)
  // Lower is better: every later step is drawn lower on screen (larger y) than the base.
  for (const s of d.chart.steps.slice(1)) expect(l.y(s.metric)).toBeGreaterThan(l.y(52.1))
  const crash = l.marks.find((m) => m.n === 5)
  expect(crash?.outcome).toBe('failed')
  expect(crash?.y).toBeNull()
  expect(l.marks.find((m) => m.n === 3)?.outcome).toBe('kept')
  expect(l.marks.find((m) => m.n === 14)?.outcome).toBe('reverted')
  expect(l.marks.find((m) => m.n === 33)?.outcome).toBe('lost')
  expect(l.marks.find((m) => m.n === 3)?.label).toBe('#3 kept · 50.4 ms')
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
  // No holdout → no right axis, narrow margin, empty path for no steps.
  const bare = layout({ points: [], steps: [], holdout: [], drift: [] }, { width: 400, height: 200, base: null, sigma: null, unit: '' })
  expect(bare.yh).toBeNull()
  expect(bare.right).toBe(16)
  expect(bare.stair).toBe('')
  expect(bare.band).toBeNull()
})
