// 140 §13.1: pure helpers of the Study view — outcome of a row, state pill, budget fractions, phase chips, number
// formatting and the chart's layout. No React, no RPC; `model.test.ts` covers them.

import type { BudgetUsed, ChartPoint, CurrentExperiment, DriftMark, ExperimentRow, ExperimentState, HoldoutPoint, Phase, Step, StudyInfo, StudyState, Verdict } from './types'

/** What a row means to the eye: the chart's colour and the table's badge. */
export type Outcome = 'kept' | 'no_gain' | 'worse' | 'failed' | 'reverted' | 'lost' | 'open'

export function outcome(state: ExperimentState, verdict: Verdict | null): Outcome {
  switch (state) {
    case 'kept':
      return 'kept'
    case 'rejected_by_review':
      return 'reverted'
    case 'crashed':
    case 'checks_failed':
    case 'refused':
      return 'failed'
    case 'abandoned':
    case 'interrupted':
      return 'lost'
    case 'open':
    case 'measuring':
    case 'measured':
      return 'open'
    case 'discarded':
      return verdict === 'worse' ? 'worse' : verdict === 'guard_failed' ? 'failed' : 'no_gain'
  }
}

/** The badge text: the verdict's words, the state's when the verdict adds nothing. */
export function outcomeLabel(state: ExperimentState, verdict: Verdict | null): string {
  switch (state) {
    case 'kept':
      return verdict === 'keep_simpler' ? 'kept (simpler)' : 'kept'
    case 'rejected_by_review':
      return 'reverted'
    case 'crashed':
      return 'crash'
    case 'checks_failed':
      return 'checks failed'
    case 'refused':
      return 'refused'
    case 'abandoned':
      return 'not logged'
    case 'interrupted':
      return 'interrupted'
    case 'open':
      return 'editing'
    case 'measuring':
      return 'measuring'
    case 'measured':
      return 'measured'
    case 'discarded':
      return verdict === 'worse' ? 'worse' : verdict === 'guard_failed' ? 'guard failed' : verdict === 'improved' || verdict === 'keep_simpler' ? 'discarded' : 'no gain'
  }
}

/** Tailwind classes of the badge (theme tokens only: malachite / carnelian / ochre). */
export const OUTCOME_CLASS: Record<Outcome, string> = {
  kept: 'bg-success-bg text-success',
  no_gain: 'bg-layer-2 text-fg-secondary',
  worse: 'bg-layer-2 text-danger',
  failed: 'bg-danger-bg text-danger',
  reverted: 'bg-warning-bg text-warning',
  lost: 'border border-border text-fg-tertiary',
  open: 'bg-info-bg text-info',
}

/** SVG paint of the chart's marks, as the theme variables so dark mode follows. */
export const OUTCOME_PAINT: Record<Outcome, string> = {
  kept: 'var(--status-success-fg)',
  no_gain: 'var(--fg-tertiary)',
  worse: 'var(--fg-tertiary)',
  failed: 'var(--status-danger-fg)',
  reverted: 'var(--status-warning-fg)',
  lost: 'var(--fg-tertiary)',
  open: 'var(--status-info-fg)',
}

export const PAUSE_REASON_LABEL: Record<string, string> = {
  user: 'by you',
  crash_streak: 'the benchmark keeps failing',
  custody_refusals: 'evaluator files changed',
  review_flags: 'too many review flags',
  budget_confirm: 'budget needs your OK',
}

export const STOP_REASON_LABEL: Record<string, string> = {
  user: 'stopped by you',
  experiments: 'experiment budget reached',
  budget_usd: 'money budget reached',
  deadline: 'time budget reached',
  plateau: 'plateau',
  overfit: 'holdout disagreed (overfit)',
  failed: 'failed',
}

/** The pill: `running · experiment 35`, `paused · the benchmark keeps failing`, `finished · plateau`. */
export function stateLabel(info: Pick<StudyInfo, 'state' | 'reason' | 'experiments'>, currentN?: number | null): string {
  switch (info.state) {
    case 'running':
      return currentN ? `running · experiment ${currentN}` : 'running'
    case 'paused':
      return `paused${info.reason ? ` · ${PAUSE_REASON_LABEL[info.reason] ?? info.reason}` : ''}`
    case 'finished':
      return `finished${info.reason ? ` · ${STOP_REASON_LABEL[info.reason] ?? info.reason}` : ''}`
    case 'failed':
      return `failed${info.reason ? ` · ${info.reason}` : ''}`
    default:
      return info.state
  }
}

export const STATE_CLASS: Record<StudyState, string> = {
  draft: 'border-border text-fg-secondary',
  preparing: 'border-info-border/40 bg-info-bg text-info',
  running: 'border-success-border/40 bg-success-bg text-success',
  paused: 'border-warning-border/40 bg-warning-bg text-warning',
  finalizing: 'border-info-border/40 bg-info-bg text-info',
  finished: 'border-border bg-layer-2 text-foreground',
  failed: 'border-danger-border/40 bg-danger-bg text-danger',
  discarded: 'border-border text-fg-tertiary',
}

/** `0 ≤ f ≤ 1`; a zero cap is full (nothing left to spend). */
export const fraction = (used: number, cap: number): number => (cap <= 0 ? 1 : Math.min(1, Math.max(0, used / cap)))

export type Bar = { label: string; text: string; fraction: number }

/** The three thin bars under the header: money, time, experiments. */
export function budgetBars(b: BudgetUsed): Bar[] {
  return [
    { label: 'Money', text: `${usd(b.usd)} of ${usd(b.usd_cap)}`, fraction: fraction(b.usd, b.usd_cap) },
    { label: 'Time', text: `${hours(b.hours)} of ${hours(b.hours_cap)} h`, fraction: fraction(b.hours, b.hours_cap) },
    { label: 'Experiments', text: `${b.experiments} of ${b.experiments_cap}`, fraction: fraction(b.experiments, b.experiments_cap) },
  ]
}

/** `$20` · `$11.4` · `$0.34`: whole dollars bare, one decimal from $10, two below. */
export const usd = (v: number): string => (Number.isInteger(v) || v >= 100 ? `$${Math.round(v)}` : v >= 10 ? `$${v.toFixed(1)}` : `$${v.toFixed(2)}`)
const hours = (h: number): string => (Number.isInteger(h) ? String(h) : h.toFixed(1))

/** `41.2 ms`: three significant digits below 1000, whole numbers above. */
export function metric(v: number | null | undefined, unit: string): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return '—'
  const a = Math.abs(v)
  const s = a >= 1000 ? Math.round(v).toLocaleString() : a >= 100 ? v.toFixed(0) : a >= 10 ? v.toFixed(1) : a >= 1 ? v.toFixed(2) : v.toPrecision(2)
  return unit ? `${s} ${unit}` : s
}

/** `−2.6 %` / `+0.4 %` with a real minus sign; `—` for null. */
export function pct(d: number | null | undefined, digits = 1): string {
  if (d === null || d === undefined || !Number.isFinite(d)) return '—'
  const s = d.toFixed(digits)
  return d < 0 ? `−${s.slice(1)} %` : d > 0 ? `+${s} %` : `${s} %`
}

/** `3.1σ`; `—` for null. */
export const sigmas = (z: number | null | undefined): string => (z === null || z === undefined || !Number.isFinite(z) ? '—' : `${z < 0 ? '−' : ''}${Math.abs(z).toFixed(1)}σ`)

/** `6:42` / `1:06:42` for elapsed clocks; `4s` / `12m` / `1.5h` for durations in a table. */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`
}

export function duration(ms: number): string {
  const s = Math.round(ms / 1000)
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.round(s / 60)}m` : `${(s / 3600).toFixed(1)}h`
}

export const shortSha = (sha: string | null): string => (sha ? sha.slice(0, 7) : '—')

export type Chip = { label: string; status: 'done' | 'on' | 'todo' }

const PHASE_LABEL: Record<Phase, string> = { Novelty: 'novelty', Custody: 'custody', Waiting: 'bench slot', Sampling: 'measuring', Checks: 'checks', Review: 'review' }

/**
 * The current-experiment chips: thinking → editing → measuring k/N → checks → review. Before `experiment_run`
 * (state `open`) editing is on; a measuring phase lights its chip and everything before it is done.
 */
export function phaseChips(cur: Pick<CurrentExperiment, 'state' | 'phase' | 'sample' | 'of'>): Chip[] {
  const order: string[] = ['thinking', 'editing', 'measuring', 'checks', 'review']
  const labels: Record<string, string> = { thinking: 'thinking', editing: 'editing', measuring: cur.of > 0 ? `measuring ${cur.sample}/${cur.of}` : 'measuring', checks: 'checks', review: 'review' }
  let on: string
  if (cur.state === 'open') on = 'editing'
  else if (cur.state === 'measured') on = 'review'
  else if (cur.phase === 'Checks') on = 'checks'
  else if (cur.phase === 'Review') on = 'review'
  else on = 'measuring'
  const at = order.indexOf(on)
  return order.map((k, i) => ({ label: labels[k] ?? k, status: i < at ? 'done' : i === at ? 'on' : 'todo' }))
}

/** The one-line phase for the header and the routines row: `measuring sample 2 of 3`, `editing`, `review`. */
export function phaseText(cur: Pick<CurrentExperiment, 'state' | 'phase' | 'sample' | 'of'>): string {
  if (cur.state === 'open') return 'editing'
  if (cur.state === 'measured') return 'logging'
  if (!cur.phase) return 'measuring'
  if (cur.phase === 'Sampling') return cur.of > 0 ? `measuring sample ${cur.sample} of ${cur.of}` : 'measuring'
  return PHASE_LABEL[cur.phase]
}

export type RunFilter = 'all' | 'kept' | 'flagged'

/** `flagged` = anything a person should look at: reverted by review, a suspicious review, deviations, refusals, guard failures. */
export function filterRuns(rows: readonly ExperimentRow[], f: RunFilter): ExperimentRow[] {
  if (f === 'all') return [...rows]
  if (f === 'kept') return rows.filter((r) => r.state === 'kept')
  return rows.filter((r) => r.state === 'rejected_by_review' || r.state === 'refused' || r.review?.verdict === 'suspicious' || r.deviations.length > 0 || r.verdict === 'guard_failed')
}

const STATE_ORDER: Record<StudyState, number> = { running: 0, finalizing: 1, paused: 2, preparing: 3, finished: 4, failed: 5, draft: 6, discarded: 7 }

/** The rail's order: live studies first (running, finalizing, paused, preparing), then finished, failed; newest first within a state. */
export function sortStudies(all: readonly StudyInfo[]): StudyInfo[] {
  return [...all].sort((a, b) => STATE_ORDER[a.state] - STATE_ORDER[b.state] || Date.parse(b.started_at ?? '') - Date.parse(a.started_at ?? '') || a.slug.localeCompare(b.slug))
}

/** The rail's text filter: slug or goal, case-insensitive. */
export const matches = (info: Pick<StudyInfo, 'slug' | 'goal'>, q: string): boolean => {
  const needle = q.trim().toLowerCase()
  return needle === '' || info.slug.includes(needle) || info.goal.toLowerCase().includes(needle)
}

/** `34 of 120` style count for rails and the routines row. */
export const experimentsText = (info: Pick<StudyInfo, 'experiments' | 'budget'>): string => `${info.experiments}/${info.budget.experiments}`

/** The routines row sentence (§13.1): `Optimising time_ms in liquid · 34 experiments · −14.2 % · running`. */
export function routineSentence(info: StudyInfo): string {
  const repo = info.repo.split('/').filter(Boolean).pop() ?? info.repo
  const parts = [`Optimising ${info.metric.name} in ${repo}`, `${info.experiments} ${info.experiments === 1 ? 'experiment' : 'experiments'}`]
  if (info.delta_pct !== null) parts.push(pct(info.delta_pct))
  parts.push(stateLabel(info))
  return parts.join(' · ')
}

// ── chart layout ──────────────────────────────────────────────────────────────────────────────────────────────────

export type Mark = { n: number; x: number; y: number | null; outcome: Outcome; label: string }
export type Layout = {
  width: number
  height: number
  left: number
  right: number
  top: number
  bottom: number
  x: (n: number) => number
  y: (v: number) => number
  yh: ((v: number) => number) | null
  ticksY: number[]
  ticksX: number[]
  ticksH: number[]
  band: { y: number; height: number } | null
  stair: string
  marks: Mark[]
  holdout: { n: number; x: number; y: number; metric: number }[]
  drift: { n: number; x: number; y: number; metric: number; expected: number }[]
}

/** Round "nice" tick steps for a range (1-2-5 ladder). */
export function niceStep(range: number, want = 6): number {
  if (!(range > 0)) return 1
  const raw = range / want
  const mag = 10 ** Math.floor(Math.log10(raw))
  const r = raw / mag
  return (r < 1.5 ? 1 : r < 3.5 ? 2 : r < 7.5 ? 5 : 10) * mag
}

export function ticks(lo: number, hi: number, want = 6): number[] {
  const step = niceStep(hi - lo, want)
  const out: number[] = []
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) out.push(Number(v.toFixed(10)))
  return out
}

/**
 * Pixel geometry of the chart from the detail's series. The left axis is the metric; the right one (when holdout
 * points exist) the holdout's own scale; x is the experiment number, 0 = the base. Pure so the SVG is a map over it.
 */
export function layout(
  input: { points: ChartPoint[]; steps: Step[]; holdout: HoldoutPoint[]; drift: DriftMark[] },
  opts: { width: number; height: number; base: number | null; sigma: number | null; unit: string; lastN?: number },
): Layout {
  const { width, height } = opts
  const hasHoldout = input.holdout.length > 0
  const left = 48
  const right = hasHoldout ? 56 : 16
  const top = 14
  const bottom = 24
  const values = [...input.points.map((p) => p.metric), ...input.steps.map((s) => s.metric), ...input.drift.map((d) => d.metric), opts.base].filter((v): v is number => v !== null && Number.isFinite(v))
  if (opts.base !== null && opts.sigma !== null) values.push(opts.base + opts.sigma, opts.base - opts.sigma)
  let lo = values.length ? Math.min(...values) : 0
  let hi = values.length ? Math.max(...values) : 1
  if (hi === lo) {
    lo -= 1
    hi += 1
  }
  const pad = (hi - lo) * 0.08
  lo -= pad
  hi += pad
  const maxN = Math.max(opts.lastN ?? 0, ...input.points.map((p) => p.n), ...input.steps.map((s) => s.n), ...input.holdout.map((h) => h.n), 1) + 0.5
  const x = (n: number) => left + (n / maxN) * (width - left - right)
  const y = (v: number) => top + ((hi - v) / (hi - lo)) * (height - top - bottom)
  let yh: Layout['yh'] = null
  let ticksH: number[] = []
  if (hasHoldout) {
    const hv = input.holdout.map((h) => h.metric)
    let hlo = Math.min(...hv)
    let hhi = Math.max(...hv)
    if (hhi === hlo) {
      hlo -= 1
      hhi += 1
    }
    const hp = (hhi - hlo) * 0.15
    hlo -= hp
    hhi += hp
    yh = (v: number) => top + ((hhi - v) / (hhi - hlo)) * (height - top - bottom)
    ticksH = ticks(hlo, hhi, 4)
  }
  const band = opts.base !== null && opts.sigma !== null && opts.sigma > 0 ? { y: y(opts.base + opts.sigma), height: y(opts.base - opts.sigma) - y(opts.base + opts.sigma) } : null
  const steps = [...input.steps].sort((a, b) => a.n - b.n)
  let stair = ''
  if (steps.length > 0) {
    const first = steps[0] as Step
    stair = `M${x(first.n).toFixed(1)},${y(first.metric).toFixed(1)}`
    for (const s of steps.slice(1)) stair += ` H${x(s.n).toFixed(1)} V${y(s.metric).toFixed(1)}`
    stair += ` H${x(maxN).toFixed(1)}`
  }
  const marks: Mark[] = input.points.map((p) => ({
    n: p.n,
    x: x(p.n),
    y: p.metric === null ? null : y(p.metric),
    outcome: outcome(p.state, p.verdict),
    label: `#${p.n} ${outcomeLabel(p.state, p.verdict)}${p.metric === null ? '' : ` · ${metric(p.metric, opts.unit)}`}`,
  }))
  const xStep = Math.max(1, niceStep(maxN, 8))
  const ticksX: number[] = []
  for (let n = 0; n <= maxN; n += xStep) ticksX.push(n)
  return {
    width,
    height,
    left,
    right,
    top,
    bottom,
    x,
    y,
    yh,
    ticksY: ticks(lo, hi, 5),
    ticksX,
    ticksH,
    band,
    stair,
    marks,
    holdout: yh ? input.holdout.map((h) => ({ n: h.n, x: x(h.n), y: (yh as (v: number) => number)(h.metric), metric: h.metric })) : [],
    drift: input.drift.map((d) => ({ n: d.n, x: x(d.n), y: y(d.metric), metric: d.metric, expected: d.metric })),
  }
}
