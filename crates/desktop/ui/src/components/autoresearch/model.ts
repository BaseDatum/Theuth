// 140 §13.1: pure helpers of the Study view — outcome of a row, state pill, budget fractions, phase chips, number
// formatting and the chart's layout. No React, no RPC; `model.test.ts` covers them.

import type { BudgetSpec } from "@/rpc/bindings/method.autoresearch.output"
import type { BudgetUsed } from "@/rpc/bindings/method.autoresearch.output"
import type { Call } from "@/rpc/bindings/method.autoresearch.output"
import type { Phase as ExperimentPhase } from "@/rpc/bindings/method.autoresearch.output"
import type { ExperimentRow } from "@/rpc/bindings/method.autoresearch.output"
import type { ExperimentState } from "@/rpc/bindings/method.autoresearch.output"
import type { Verdict as ExperimentVerdict } from "@/rpc/bindings/method.autoresearch.output"
import type { PauseReason } from "@/rpc/bindings/method.autoresearch.output"
import type { ReviewVerdict } from "@/rpc/bindings/method.autoresearch.output"
import type { StopReason } from "@/rpc/bindings/method.autoresearch.output"
import type { ChartPoint as StudyChartPoint } from "@/rpc/bindings/method.autoresearch.output"
import type { ChartSeries as StudyChartSeries } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyInfo } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyState } from "@/rpc/bindings/method.autoresearch.output"

/** What a row means to the eye: the chart's colour and the table's badge. */
export type Outcome = 'kept' | 'no_gain' | 'worse' | 'failed' | 'reverted' | 'lost' | 'open'

export function outcome(state: ExperimentState | null, verdict: ExperimentVerdict | null): Outcome {
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
    case 'timeout':
      return 'lost'
    case 'open':
    case 'measuring':
    case 'measured':
    case 'waiting': // 159: between fires (a launch, a window, inner studies)
    case null:
      return 'open'
    case 'discarded':
      return verdict?.kind === 'worse' ? 'worse' : verdict?.kind === 'guard_failed' ? 'failed' : 'no_gain'
  }
}

/** The badge text: the verdict's words, the state's when the verdict adds nothing. */
export function outcomeLabel(state: ExperimentState | null, verdict: ExperimentVerdict | null): string {
  switch (state) {
    case 'kept':
      return verdict?.kind === 'keep_simpler' ? 'kept (simpler)' : 'kept'
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
    case 'timeout':
      return 'timed out'
    case 'open':
    case null:
      return 'editing'
    case 'measuring':
      return 'measuring'
    case 'measured':
      return 'measured'
    case 'waiting':
      return 'waiting for its window'
    case 'discarded':
      switch (verdict?.kind) {
        case 'worse':
          return 'worse'
        case 'guard_failed':
          return 'guard failed'
        case 'improved':
        case 'keep_simpler':
        case 'suspicious':
          return 'discarded'
        default:
          return 'no gain'
      }
  }
}

/**
 * `StudyChartPoint.verdict` is a free tag the plugin fills with the row's ledger state (or the verdict kind); both
 * vocabularies map here, anything else is drawn grey.
 */
export function chartOutcome(tag: string): Outcome {
  switch (tag) {
    case 'kept':
    case 'improved':
    case 'keep_simpler':
      return 'kept'
    case 'rejected_by_review':
    case 'reverted':
    case 'suspicious':
      return 'reverted'
    case 'crashed':
    case 'crash':
    case 'checks_failed':
    case 'refused':
    case 'guard_failed':
      return 'failed'
    case 'worse':
      return 'worse'
    case 'abandoned':
    case 'interrupted':
    case 'timeout':
    case 'lost':
      return 'lost'
    case 'open':
    case 'measuring':
    case 'measured':
    case 'waiting':
      return 'open'
    default:
      return 'no_gain'
  }
}

const OUTCOME_WORDS: Record<Outcome, string> = { kept: 'kept', no_gain: 'no gain', worse: 'worse', failed: 'failed', reverted: 'reverted by review', lost: 'interrupted', open: 'in flight' }

/** Tailwind classes of the badge (theme tokens only: malachite / carnelian / Egyptian blue). */
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

export function pauseReasonLabel(r: PauseReason): string {
  switch (r.kind) {
    case 'user':
      return 'by you'
    case 'crash_streak':
      return 'the benchmark keeps failing'
    case 'custody_refusals':
      return 'evaluator files changed'
    case 'review_flags':
      return 'too many review flags'
    case 'budget_confirm':
      return 'budget needs your OK'
    case 'launch_approval':
      return `launch #${r.n} needs your press`
    case 'other':
      return r.reason
  }
}

export function stopReasonLabel(r: StopReason): string {
  switch (r.kind) {
    case 'user':
      return 'stopped by you'
    case 'user_now':
      return 'stopped now by you'
    case 'experiments':
      return 'experiment budget reached'
    case 'budget':
      return 'money budget reached'
    case 'hours':
      return 'time budget reached'
    case 'deadline':
      return 'deadline reached'
    case 'plateau':
      return 'plateau'
    case 'overfit':
      return 'holdout disagreed (overfit)'
    case 'failed':
      return r.reason
  }
}

/** The pill: `running · experiment 35`, `paused · the benchmark keeps failing`, `finished · plateau`. */
export function stateLabel(state: StudyState, currentN?: number | null, stop?: StopReason | null): string {
  switch (state.state) {
    case 'running':
      return currentN ? `running · experiment ${currentN}` : 'running'
    case 'paused':
      return `paused · ${pauseReasonLabel(state.reason)}`
    case 'finished':
      return `finished${stop ? ` · ${stopReasonLabel(stop)}` : ''}`
    case 'failed':
      return `failed · ${state.reason}`
    default:
      return state.state
  }
}

export const STATE_CLASS: Record<StudyState['state'], string> = {
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
export function budgetBars(used: BudgetUsed, cap: BudgetSpec): Bar[] {
  return [
    { label: 'Money', text: `${usd(used.usd)} of ${usd(cap.usd)}`, fraction: fraction(used.usd, cap.usd) },
    { label: 'Time', text: `${hours(used.hours)} of ${hours(cap.hours)} h`, fraction: fraction(used.hours, cap.hours) },
    { label: 'Experiments', text: `${used.experiments} of ${cap.experiments}`, fraction: fraction(used.experiments, cap.experiments) },
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

/** `6:42` / `1:06:42` for elapsed clocks. */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`
}

/** `4s` / `12m` / `1.5h` for durations in a table. */
export function duration(ms: number): string {
  const s = Math.round(ms / 1000)
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.round(s / 60)}m` : `${(s / 3600).toFixed(1)}h`
}

/** A row's wall time: `finished_at − started_at`, else up to `now` for one still open. */
export const rowDuration = (r: Pick<ExperimentRow, 'started_at' | 'finished_at'>, now = Date.now()): number => Math.max(0, (r.finished_at ? Date.parse(r.finished_at) : now) - Date.parse(r.started_at))

export const shortSha = (sha: string | null | undefined): string => (sha ? sha.slice(0, 7) : '—')

/** `CLEAN` / `SUSPICIOUS` / `no verdict` — the reviewer's one word for a row or a changeset. */
export function reviewWord(v: ReviewVerdict | null): { word: string; tone: 'clean' | 'suspicious' | 'none' } {
  if (!v) return { word: 'unreviewed', tone: 'none' }
  switch (v.kind) {
    case 'clean':
      return { word: 'CLEAN', tone: 'clean' }
    case 'reverted':
    case 'flagged':
      return { word: 'SUSPICIOUS', tone: 'suspicious' }
    case 'error':
      return { word: 'no verdict', tone: 'none' }
  }
}

export type Chip = { label: string; status: 'done' | 'on' | 'todo' }

/** What `ExperimentProgress` gave for the experiment in flight; `null` phase before `experiment_run`. */
export type Progress = { phase: ExperimentPhase | null; sample: number; of: number; elapsed_ms: number }

const PHASE_LABEL: Record<ExperimentPhase, string> = { novelty: 'novelty', custody: 'custody', waiting: 'bench slot', sampling: 'measuring', checks: 'checks', review: 'review', cases: 'scoring cases', judging: 'judging', window: 'window' }

/**
 * The current-experiment chips: thinking → editing → measuring k/N → checks → review. Before `experiment_run`
 * (no phase yet) editing is on; a measuring phase lights its chip and everything before it is done.
 */
export function phaseChips(p: Progress): Chip[] {
  const order = ['thinking', 'editing', 'measuring', 'checks', 'review']
  const labels: Record<string, string> = { thinking: 'thinking', editing: 'editing', measuring: p.phase === 'sampling' && p.of > 0 ? `measuring ${p.sample}/${p.of}` : 'measuring', checks: 'checks', review: 'review' }
  const on = p.phase === null ? 'editing' : p.phase === 'checks' ? 'checks' : p.phase === 'review' ? 'review' : 'measuring'
  const at = order.indexOf(on)
  return order.map((k, i) => ({ label: labels[k] ?? k, status: i < at ? 'done' : i === at ? 'on' : 'todo' }))
}

/** The one-line phase for the header and the routines row: `measuring sample 2 of 3`, `editing`, `review`. */
export function phaseText(p: Progress): string {
  if (p.phase === null) return 'editing'
  if (p.phase === 'sampling') return p.of > 0 ? `measuring sample ${p.sample} of ${p.of}` : 'measuring'
  return PHASE_LABEL[p.phase]
}

export type RunFilter = 'all' | 'kept' | 'flagged'

/** `flagged` = anything a person should look at: reverted by review, a suspicious review, deviations, refusals, guard failures. */
export function filterRuns(rows: readonly ExperimentRow[], f: RunFilter): ExperimentRow[] {
  if (f === 'all') return [...rows]
  if (f === 'kept') return rows.filter((r) => r.state === 'kept')
  return rows.filter((r) => r.state === 'rejected_by_review' || r.state === 'refused' || r.review?.kind === 'reverted' || r.review?.kind === 'flagged' || r.deviations.length > 0 || r.verdict?.kind === 'guard_failed' || r.verdict?.kind === 'suspicious')
}

/** Kept / reverted / crashed / checks-failed counts of a run list (the Kept card). */
export function tally(rows: readonly ExperimentRow[]): { kept: number; reverted: number; crashed: number; checks_failed: number } {
  const t = { kept: 0, reverted: 0, crashed: 0, checks_failed: 0 }
  for (const r of rows) {
    if (r.state === 'kept') t.kept++
    else if (r.state === 'rejected_by_review') t.reverted++
    else if (r.state === 'crashed') t.crashed++
    else if (r.state === 'checks_failed') t.checks_failed++
  }
  return t
}

/** The holdout series has one point per sample; the chart and the cards want one value per checkpoint: the mean per `n` (R-89), ordered by `n`. */
export function holdoutByN(series: Pick<StudyChartSeries, 'holdout'>): { n: number; metric: number }[] {
  const sums = new Map<number, { sum: number; count: number }>()
  for (const h of series.holdout) {
    if (h.metric === null || !Number.isFinite(h.metric)) continue
    const e = sums.get(h.n) ?? { sum: 0, count: 0 }
    e.sum += h.metric
    e.count++
    sums.set(h.n, e)
  }
  return [...sums].map(([n, e]) => ({ n, metric: e.sum / e.count })).sort((a, b) => a.n - b.n)
}

/**
 * The Holdout card: first checkpoint = the base, last = the latest incumbent (means per `n`); raw Δ % (the caller judges
 * the direction). `null` until an incumbent was measured — the base alone is not a "base → base" result (140 §13 Revision 2).
 */
export function holdoutSummary(series: Pick<StudyChartSeries, 'holdout'>): { base: number; latest: number; delta_pct: number; n: number } | null {
  const pts = holdoutByN(series)
  const first = pts[0]
  const last = pts.at(-1)
  if (!first || !last || pts.length < 2) return null
  return { base: first.metric, latest: last.metric, delta_pct: first.metric === 0 ? 0 : ((last.metric - first.metric) / Math.abs(first.metric)) * 100, n: last.n }
}

/** The tooltip of a mark (R-97): the run's verdict in words from its row — `#n improved +2.4 % 3.1σ · 41.2 ms · idea` — falling back to outcome · value. */
export function markLabel(p: StudyChartPoint, unit: string, row: ExperimentRow | undefined): string {
  const o = chartOutcome(p.verdict)
  const value = p.metric === null ? '' : ` · ${metric(p.metric, unit)}`
  if (!row) return `#${p.n} ${OUTCOME_WORDS[o]}${value}`
  const numbers = [row.delta_pct !== null && pct(row.delta_pct), row.z !== null && sigmas(row.z)].filter(Boolean).join(' ')
  const idea = row.idea.trim()
  return `#${p.n} ${OUTCOME_WORDS[o]}${numbers ? ` ${numbers}` : ''}${value}${idea ? ` · ${idea}` : ''}`
}

/** Relative threshold a keep must clear (§5.2 step 2 at n = 1): `max(2σ·√2, min_effect)` over the incumbent. */
export function keepNeedsPct(sigma: number | null, incumbent: number | null, minEffect: string): number | null {
  if (sigma === null || incumbent === null || incumbent === 0) return null
  const m = minEffect.trim().endsWith('%') ? Number.parseFloat(minEffect) : (Number.parseFloat(minEffect) / Math.abs(incumbent)) * 100
  return Math.max(((2 * sigma * Math.SQRT2) / Math.abs(incumbent)) * 100, Number.isFinite(m) ? m : 0)
}

/** Lower is better → a negative Δ is an improvement. */
export const improved = (direction: 'lower' | 'higher', delta_pct: number | null): boolean => delta_pct !== null && (direction === 'lower' ? delta_pct < 0 : delta_pct > 0)

/**
 * The one Δ convention (140 §5.2 Revision 2): `delta_pct` is the raw relative change (negative = the value went down) and is
 * shown as such; better/worse is a word and a colour from the metric's direction, never a flipped sign.
 */
export function delta(direction: 'lower' | 'higher', delta_pct: number | null | undefined): { text: string; word: 'better' | 'worse' | ''; tone: 'success' | 'danger' | 'muted' } {
  const d = delta_pct ?? null
  if (d === null || !Number.isFinite(d) || d === 0) return { text: pct(d), word: '', tone: 'muted' }
  const better = improved(direction, d)
  return { text: pct(d), word: better ? 'better' : 'worse', tone: better ? 'success' : 'danger' }
}

/** 159 W1 H2: a per-objective `call` in the badge's colours. */
export const CALL_OUTCOME: Record<Call, Outcome> = { better: 'kept', same: 'no_gain', worse: 'worse', guard_failed: 'failed' }

export type ObjectiveColumn = { name: string; direction: 'lower' | 'higher' | null }

/**
 * 159 W1 H2: the objectives of the shown front rows (`gate.rule === 'dominance'`), first seen first. `ObjectiveDelta`
 * carries no direction; it is read off any row where the normalised Δ (positive = better) and the raw Δ % disagree in
 * sign (lower is better) or agree (higher), `null` when no row tells.
 */
export function objectiveColumns(rows: readonly ExperimentRow[]): ObjectiveColumn[] {
  const cols = new Map<string, ObjectiveColumn>()
  for (const r of rows) {
    if (r.gate?.rule !== 'dominance') continue
    for (const o of r.gate.objectives) {
      const col = cols.get(o.name) ?? { name: o.name, direction: null }
      if (col.direction === null && o.delta && o.delta_pct) col.direction = o.delta > 0 === o.delta_pct > 0 ? 'higher' : 'lower'
      cols.set(o.name, col)
    }
  }
  return [...cols.values()]
}

const STATE_ORDER: Record<StudyState['state'], number> = { running: 0, finalizing: 1, paused: 2, preparing: 3, finished: 4, failed: 5, draft: 6, discarded: 7 }

/** The rail's order: live studies first (running, finalizing, paused, preparing), then finished, failed; newest first within a state. */
export function sortStudies(all: readonly StudyInfo[]): StudyInfo[] {
  return [...all].sort((a, b) => STATE_ORDER[a.state.state] - STATE_ORDER[b.state.state] || Date.parse(b.started_at ?? '') - Date.parse(a.started_at ?? '') || a.slug.localeCompare(b.slug))
}

/** The rail's text filter: slug or goal, case-insensitive. */
export const matches = (info: Pick<StudyInfo, 'slug' | 'goal'>, q: string): boolean => {
  const needle = q.trim().toLowerCase()
  return needle === '' || info.slug.includes(needle) || info.goal.toLowerCase().includes(needle)
}

/** `34/120` for rails and the routines row. */
export const experimentsText = (info: Pick<StudyInfo, 'experiments' | 'budget'>): string => `${info.experiments}/${info.budget.experiments}`

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
  drift: { n: number; x: number; y: number; metric: number }[]
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
export function layout(series: StudyChartSeries, opts: { width: number; height: number; unit: string; lastN?: number; rows?: readonly ExperimentRow[] }): Layout {
  const { width, height } = opts
  const base = series.baseline_mean
  const sigma = series.sigma
  const holdoutPts = holdoutByN(series)
  const hasHoldout = holdoutPts.length > 0
  const left = 48
  const right = hasHoldout ? 56 : 16
  const top = 14
  const bottom = 24
  const values = [...series.experiments.map((p) => p.metric), ...series.incumbent.map((s) => s.metric), ...series.drift.map((d) => d.metric), base].filter((v): v is number => v !== null && Number.isFinite(v))
  if (base !== null && sigma !== null) values.push(base + sigma, base - sigma)
  let lo = values.length ? Math.min(...values) : 0
  let hi = values.length ? Math.max(...values) : 1
  if (hi === lo) {
    lo -= 1
    hi += 1
  }
  const pad = (hi - lo) * 0.08
  lo -= pad
  hi += pad
  const maxN = Math.max(opts.lastN ?? 0, ...series.experiments.map((p) => p.n), ...series.incumbent.map((s) => s.n), ...series.holdout.map((h) => h.n), 1) + 0.5
  const x = (n: number) => left + (n / maxN) * (width - left - right)
  const y = (v: number) => top + ((hi - v) / (hi - lo)) * (height - top - bottom)
  let yh: Layout['yh'] = null
  let ticksH: number[] = []
  if (hasHoldout) {
    const hv = holdoutPts.map((h) => h.metric)
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
  const band = base !== null && sigma !== null && sigma > 0 ? { y: y(base + sigma), height: y(base - sigma) - y(base + sigma) } : null
  const steps = series.incumbent.filter((s): s is StudyChartPoint & { metric: number } => s.metric !== null).sort((a, b) => a.n - b.n)
  let stair = ''
  const first = steps[0]
  if (first) {
    stair = `M${x(first.n).toFixed(1)},${y(first.metric).toFixed(1)}`
    for (const s of steps.slice(1)) stair += ` H${x(s.n).toFixed(1)} V${y(s.metric).toFixed(1)}`
    stair += ` H${x(maxN).toFixed(1)}`
  }
  const byN = new Map((opts.rows ?? []).map((r) => [r.n, r]))
  const marks: Mark[] = series.experiments.map((p) => ({ n: p.n, x: x(p.n), y: p.metric === null ? null : y(p.metric), outcome: chartOutcome(p.verdict), label: markLabel(p, opts.unit, byN.get(p.n)) }))
  const xStep = Math.max(1, niceStep(maxN, 8))
  const ticksX: number[] = []
  for (let n = 0; n <= maxN; n += xStep) ticksX.push(n)
  const yhf = yh
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
    holdout: yhf ? holdoutPts.map((h) => ({ n: h.n, x: x(h.n), y: yhf(h.metric), metric: h.metric })) : [],
    drift: series.drift.filter((d): d is StudyChartPoint & { metric: number } => d.metric !== null).map((d) => ({ n: d.n, x: x(d.n), y: y(d.metric), metric: d.metric })),
  }
}
