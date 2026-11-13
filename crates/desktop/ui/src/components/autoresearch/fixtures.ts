// 140 §15 "UI": the autoresearch RPCs answered from memory so the Agents › Autoresearch tab, the Study view, the routines row
// and Settings › Autoresearch render in every state against any engine. Dev only, behind
// `VITE_AUTORESEARCH_FIXTURE=studies|empty` (or `localStorage['autoresearch-fixture']`); the data is exported for
// the vitest over the model and the components. Mirrors `dev/dreaming-fixture.ts`.

import { receiveComponent } from '@/rpc/bindings/receive'
import type { Api } from '@/rpc/bindings/client'
import type { AutoresearchConfig } from "@/rpc/bindings/method.autoresearch.output"
import type { Sample as BenchSample } from "@/rpc/bindings/method.autoresearch.output"
import type { ExperimentDetail } from "@/rpc/bindings/method.autoresearch.output"
import type { ExperimentRow } from "@/rpc/bindings/method.autoresearch.output"
import type { ExperimentState } from "@/rpc/bindings/method.autoresearch.output"
import type { Verdict as ExperimentVerdict } from "@/rpc/bindings/method.autoresearch.output"
import type { PauseReason } from "@/rpc/bindings/method.autoresearch.output"
import type { ScheduleInfo } from "@/rpc/bindings/method.schedule.output"
import type { StopReason } from "@/rpc/bindings/method.autoresearch.output"
import type { Changeset as StudyChangeset } from "@/rpc/bindings/method.autoresearch.output"
import type { ChartPoint as StudyChartPoint } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyDetail } from "@/rpc/bindings/method.autoresearch.output"
import type { Idea as StudyIdea } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyInfo } from "@/rpc/bindings/method.autoresearch.output"
import type { StudySpec } from "@/rpc/bindings/method.autoresearch.output"
import type { StudyState } from "@/rpc/bindings/method.autoresearch.output"
import type { Steer as StudySteer } from "@/rpc/bindings/method.autoresearch.output"
import { pct } from './model'

export const STATES = ['empty', 'studies'] as const
export type State = (typeof STATES)[number]

const state = (): State | null => {
  if (!import.meta.env.DEV) return null
  const s = globalThis.localStorage?.getItem('autoresearch-fixture') ?? import.meta.env.VITE_AUTORESEARCH_FIXTURE
  return STATES.find((x) => x === s) ?? null
}

const MIN = 60_000
const H = 60 * MIN
const ago = (ms: number) => new Date(Date.now() - ms).toISOString()

const IDEAS = ['Hoist strainer lookup', 'Freeze literal strings', 'Skip blank-node walk', 'Precompile filter args', 'Lazy context scopes', 'Avoid dup on assigns', 'Inline Expression#evaluate', 'Memo tag registry lookups']
const CATS = ['algorithm', 'allocation', 'caching', 'io']

/** Deterministic pseudo-noise in [0, 1): the mock's `sin` hash, so numbers never change between runs. */
const noise = (n: number) => {
  const r = Math.sin(n * 12.9898) * 43758.5453
  return r - Math.floor(r)
}

type Plan = {
  count: number
  base: number
  sigma: number
  keeps: Record<number, number>
  crash?: number[]
  checks?: number[]
  reverted?: Record<number, number>
  deviation?: number
  interrupted?: number[]
  /** An experiment in flight (state `measuring`), numbered `count + 1`. */
  current?: boolean
}

/** The §4.1 example study as spec; slug/goal/repo overridden per fixture. */
const spec = (over: Partial<StudySpec> & { slug: string }): StudySpec => (receiveComponent("method.autoresearch.output.StudySpec", {
  goal: 'Make parse+render of the fixture set faster without changing output.',
  repo: '/Users/Sky.Moore/Develop/liquid',
  base: 'HEAD',
  metric: { name: 'time_ms', unit: 'ms', direction: 'lower', min_effect: '0.5%' },
  secondary_guards: [{ name: 'alloc_mb', max_regression: '5%', direction: 'lower' }],
  bench: { command: 'bench.sh', timeout: '600s', repeats: 3, exclusive: true, smoke: null },
  checks: { command: 'bundle exec rake test', timeout: '300s' },
  holdout: { command: 'bench-holdout.sh', every: 5, repeats: null },
  evaluator_paths: ['fixtures/', 'test/', 'Rakefile'],
  scope: ['lib/liquid/**'],
  off_limits: ['fixtures/**', 'test/**'],
  warm: 'bundle install',
  budget: { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 },
  parallel: 1,
  models: { executor: null, review: null, advisor: null },
  window: null,
  include_dirty: false,
  ...over,
}))

const BASE_SHA = '4f2c9e1a8b3d5e7f9012345678901234567890ab'

type Built = { rows: ExperimentRow[]; details: Map<number, ExperimentDetail>; points: StudyChartPoint[]; steps: StudyChartPoint[]; incumbent: number; incumbentSha: string; kept: number }

/** The ledger of one study from a plan: every row's numbers follow the plan's keeps and the pseudo-noise. */
function build(slug: string, p: Plan): Built {
  const rows: ExperimentRow[] = []
  const details = new Map<number, ExperimentDetail>()
  const points: StudyChartPoint[] = []
  const start = Date.now() - (p.count + 1) * 7 * MIN
  const steps: StudyChartPoint[] = [{ n: 0, verdict: '', metric: p.base, at: new Date(start).toISOString() }]
  const sd = p.sigma * Math.sqrt(1 + 1 / 3)
  let inc = p.base
  let incSha = BASE_SHA
  let kept = 0
  const total = p.count + (p.current ? 1 : 0)
  for (let n = 1; n <= total; n++) {
    const before = inc
    const inFlight = p.current === true && n === total
    let value: number | null
    let st: ExperimentState
    let verdict: ExperimentVerdict | null
    let samples = 1
    let review: ExperimentRow['review'] = null
    const keep = p.keeps[n]
    const rev = p.reverted?.[n]
    if (inFlight) {
      value = null
      st = 'measuring'
      verdict = null
      samples = 0
    } else if (keep !== undefined) {
      value = keep
      st = 'kept'
      verdict = { kind: 'improved' }
      samples = 3
      review = { kind: 'clean', reason: 'Replaces a per-node allocation with a reused buffer; measurement unchanged.' }
      inc = keep
      incSha = `${(0xa1b2c3 + n * 7919).toString(16).padStart(7, '0')}e4d5f60718293a4b5c6d7e8f90a1b2c3d4e5f6`
      kept++
      steps.push({ n, verdict: '', metric: keep, at: new Date(start + n * 7 * MIN).toISOString() })
    } else if (p.crash?.includes(n)) {
      value = null
      st = 'crashed'
      verdict = { kind: 'crash' }
    } else if (p.checks?.includes(n)) {
      value = inc - 0.9
      st = 'checks_failed'
      verdict = { kind: 'checks_failed' }
      samples = 3
    } else if (rev !== undefined) {
      value = rev
      st = 'rejected_by_review'
      verdict = { kind: 'improved' }
      samples = 3
      review = { kind: 'reverted', reason: 'Precomputes the rendered output of the benchmark’s fixture names in a constant table.' }
    } else if (p.interrupted?.includes(n)) {
      value = null
      st = 'interrupted'
      verdict = null
      samples = 0
    } else {
      value = inc + (noise(n) - 0.3) * 1.5
      const z = (inc - value) / sd
      st = 'discarded'
      verdict = { kind: z < -2 ? 'worse' : 'no_gain' }
    }
    const delta = value === null ? null : before - value
    const deltaPct = value === null ? null : ((value - before) / before) * 100
    const z = value === null ? null : (before - value) / sd
    const line =
      value === null
        ? st === 'measuring'
          ? 'measuring'
          : st === 'interrupted'
            ? 'interrupted: engine restarted mid-sample'
            : 'crash: exit 1 (tail below)'
        : st === 'kept' || st === 'rejected_by_review'
          ? `improved: ${value.toFixed(1)} ms vs ${before.toFixed(1)} ms (${pct(deltaPct)}), ${(z as number).toFixed(1)}σ over 3 samples, checks passed`
          : st === 'checks_failed'
            ? `checks failed: bundle exec rake test exit 1 (${pct(deltaPct)}, ${(z as number).toFixed(1)}σ)`
            : verdict?.kind === 'worse'
              ? `worse: ${pct(deltaPct)}, ${(z as number).toFixed(1)}σ (1 sample)`
              : `no gain: ${pct(deltaPct)}, ${(z as number).toFixed(1)}σ (1 sample)`
    const startedAt = inFlight ? ago(6 * MIN + 42_000) : new Date(start + (n - 1) * 7 * MIN).toISOString()
    const durationMs = (4 + samples * 2.5 + noise(n + 7) * 3) * MIN
    const deviations = p.deviation === n ? ['lib/liquid/profiler.rb'] : []
    const row: ExperimentRow = {
      slug,
      n,
      wave: null,
      slot: null,
      segment: 0,
      state: st,
      verdict,
      idea: inFlight ? 'Reuse the token buffer across render calls instead of allocating per node' : (IDEAS[n % IDEAS.length] as string),
      category: inFlight ? 'allocation' : (CATS[n % CATS.length] as string),
      metric: value,
      delta,
      delta_pct: deltaPct,
      z,
      n_samples: samples,
      commit_sha: st === 'kept' ? incSha : null,
      deviations,
      review,
      cost_usd: 0.28 + (n % 9) / 100 + samples * 0.04,
      started_at: startedAt,
      finished_at: inFlight ? null : new Date(Date.parse(startedAt) + durationMs).toISOString(),
      gate: null,
      line,
    }
    rows.push(row)
    points.push({ n, verdict: st, metric: value, at: startedAt })
    const sampleRows: BenchSample[] =
      value === null
        ? []
        : Array.from({ length: samples }, (_, k) => ({
            n,
            k: k + 1,
            tree_sha: st === 'kept' ? incSha : `c${n}${k}d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f6a1b`,
            role: 'candidate' as const,
            value: value + (noise(n * 10 + k) - 0.5) * p.sigma,
            secondaries: { alloc_mb: 118.2 + noise(n + k) },
            cases: {},
            asi: { gc_runs: '41' },
            exit: 0,
            wall_ms: 42_000 + noise(k) * 3000,
            load: 0.21 + noise(n + k) * 0.2,
            noisy: false,
            lost: false,
            log_path: `runs/${n}/sample-${k + 1}.log`,
            at: new Date(Date.parse(startedAt) + (k + 1) * 50_000).toISOString(),
            slot_holders: [],
            failure: null,
          }))
    if (st === 'crashed') sampleRows.push({ n, k: 1, tree_sha: 'deadbee', role: 'candidate', value: null, secondaries: {}, cases: {}, asi: {}, exit: 1, wall_ms: 3_200, load: 0.3, noisy: false, lost: false, log_path: `runs/${n}/sample-1.log`, at: startedAt, slot_holders: [], failure: 'exit 1' })
    details.set(n, {
      row,
      hypothesis: `${row.idea} should cut the hot path's allocations, which the profiler put at 18 % of render time.`,
      description: inFlight ? '' : `${row.idea}: ${st === 'kept' ? 'replaced the per-call allocation with a buffer owned by the context.' : 'tried the change on the render loop; the benchmark did not move past noise.'}`,
      learned: inFlight ? '' : st === 'kept' ? 'Buffer reuse pays off anywhere the node walker allocates per visit.' : 'The lexer is not where the time goes; the render loop is.',
      next: inFlight ? null : 'Look at Expression#evaluate: 31 % of samples in the profile.',
      decision: st === 'kept' || st === 'rejected_by_review' ? 'auto' : null,
      justification: deviations.length ? 'Removes a hook call on the hot path; profiler output unchanged.' : null,
      parent_sha: before === p.base ? BASE_SHA : incSha,
      patch_hash: value === null ? null : `ph${n.toString(16).padStart(6, '0')}`,
      samples: sampleRows,
      line,
      diff: (st === 'crashed' && n % 2 === 0) || inFlight ? null : DIFF.replace('@@IDEA@@', row.idea),
      tails:
        value === null && !inFlight
          ? ['warmup 1/3 … ok\nfixtures 12/214 …\nTraceback (most recent call last):\n  File "bench.py", line 41, in <module>\n    render(t)\nNameError: name \'strainer\' is not defined']
          : value === null
            ? []
            : [`warmup 3/3 … ok\nfixtures 214/214 … ok\nMETRIC time_ms=${(value + 0.1).toFixed(2)}\nMETRIC alloc_mb=118.2\nASI gc_runs=41`, ...(st === 'checks_failed' ? ['Run options: --seed 4121\n...F.\nFailure: test_whitespace_control (RenderTest)\n  expected "a b" got "ab"\n214 runs, 1 failure'] : [])],
      cases_moved:
        value === null
          ? []
          : [
              { id: 'templates/product_grid.liquid', before: 9.8, after: 9.8 + (deltaPct ?? 0) * 0.2, delta_pct: (deltaPct ?? 0) * 2.1 },
              { id: 'templates/cart.liquid', before: 4.1, after: 4.1 + (deltaPct ?? 0) * 0.05, delta_pct: (deltaPct ?? 0) * 1.3 },
              { id: 'templates/blog.liquid', before: 2.2, after: 2.2, delta_pct: 0 },
            ],
    })
  }
  return { rows, details, points, steps, incumbent: inc, incumbentSha: incSha, kept }
}

const DIFF = `diff --git a/lib/liquid/render.rb b/lib/liquid/render.rb
--- a/lib/liquid/render.rb
+++ b/lib/liquid/render.rb
@@ -12,9 +12,10 @@ module Liquid
   class Render
-    def render_node(node, context)
-      buf = +""
-      node.render_to(buf, context)
-      buf
+    # @@IDEA@@
+    def render_node(node, context)
+      buf = context.scratch_buffer
+      buf.clear
+      node.render_to(buf, context)
+      buf.dup
     end
   end
 end
`

/** One point per holdout *sample* (`holdout.repeats` = 3 per checkpoint), as `autoresearch.get` emits; the UI means them per `n` (R-89). */
const HOLDOUT: StudyChartPoint[] = [
  { n: 0, verdict: '', metric: 60.4, at: ago(5 * H) },
  { n: 0, verdict: '', metric: 61.0, at: ago(5 * H) },
  { n: 0, verdict: '', metric: 61.6, at: ago(5 * H) },
  { n: 11, verdict: '', metric: 55.9, at: ago(3.2 * H) },
  { n: 11, verdict: '', metric: 56.2, at: ago(3.2 * H) },
  { n: 11, verdict: '', metric: 56.5, at: ago(3.2 * H) },
  { n: 23, verdict: '', metric: 51.3, at: ago(1.6 * H) },
  { n: 23, verdict: '', metric: 51.6, at: ago(1.6 * H) },
  { n: 23, verdict: '', metric: 51.9, at: ago(1.6 * H) },
  { n: 31, verdict: '', metric: 49.1, at: ago(0.5 * H) },
  { n: 31, verdict: '', metric: 49.7, at: ago(0.5 * H) },
  { n: 31, verdict: '', metric: 50.3, at: ago(0.5 * H) },
]

const ideas = (slug: string): StudyIdea[] => [
  { id: 1, slug, text: 'Lexer: scan by memchr for tag openers', category: 'algorithm', source: 'user', priority: 3, status: 'open', tried_in: [], at: ago(2 * H) },
  { id: 2, slug, text: 'Cache compiled filter chains per template', category: 'caching', source: 'user', priority: 3, status: 'open', tried_in: [], at: ago(1.5 * H) },
  { id: 3, slug, text: 'Intern variable names at parse time', category: 'allocation', source: 'model', priority: 1, status: 'open', tried_in: [], at: ago(4 * H) },
  { id: 4, slug, text: 'Avoid regex in whitespace control', category: 'algorithm', source: 'advisor', priority: 2, status: 'open', tried_in: [], at: ago(3 * H) },
  { id: 5, slug, text: 'Batch output appends into one buffer', category: 'io', source: 'model', priority: 1, status: 'open', tried_in: [], at: ago(3 * H) },
  { id: 6, slug, text: 'Replace the strainer Hash with a frozen Struct', category: 'allocation', source: 'setup', priority: 1, status: 'tried', tried_in: [9], at: ago(5 * H) },
  { id: 7, slug, text: 'Skip the parser for templates without tags', category: 'algorithm', source: 'model', priority: 0, status: 'closed', tried_in: [16], at: ago(4 * H) },
]

/** Newest first, as `autoresearch.get` returns them. */
const steers = (slug: string): StudySteer[] => [
  { id: 2, slug, text: 'The strainer lookups look hot in production profiles.', by: 'user', at: ago(40 * MIN), seen_in: null },
  { id: 1, slug, text: 'Try SIMD for the lexer; do not touch the filters module.', by: 'user', at: ago(2.5 * H), seen_in: 19 },
]

const PLAYBOOK = `## Build
\`bundle exec rake compile\` is not needed: pure Ruby. Benchmarks warm in ~8 s.

## Hot path
\`Liquid::Render#render_node\` → \`Block#render_to\` → \`Variable#render\`. The profiler (stackprof, wall) puts 31 % of samples in \`Expression#evaluate\`, 18 % in per-node String allocation.

## Known dead ends
Regex changes in the lexer: the lexer is 4 % of time. Not worth another experiment.`

export const SETTINGS: AutoresearchConfig = {
  enabled: true,
  root: null,
  defaults: { repeats: 3, bench_timeout: '600s', checks_timeout: '300s', crash_retries: 2, budget: { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 }, parallel: 1 },
  gate: { z: 2, z_fast: 3.5, z_discard: 1, z_suspicious: 8, confirm_runs: 2, remeasure_after: 6, remeasure_every: 5, regime_sigma: 10 },
  noise: { max_cv: 0.05 },
  plateau: { window: 12 },
  pause: { crash_streak: 4, custody_refusals: 2 },
  review: { model: null, effort: null, on_suspicious: 'revert', pause_after: 3 },
  models: { executor: null, executor_effort: null, advisor: null, advisor_effort: null },
  brief: { max_bytes: 12288, recent: 8, dead_ends: 30, ideas: 12, playbook_bytes: 4096, code_map_tokens: 1200 },
  novelty: { threshold: 0.6 },
  bench: { lock: 'builtin', max_load: 0.5, wait: '20m' },
  max_parallel: 4,
  confirm_budget_usd: 50,
  keep_finished_days: 14,
  git: { author: null },
  notify: { new_best: false },
}

export type Study = { info: StudyInfo; detail: StudyDetail; rows: ExperimentRow[]; details: Map<number, ExperimentDetail> }

function study(args: { slug: string; goal?: string; repo?: string; state: StudyState; plan: Plan; holdout?: boolean; startedAgoMs: number; stop?: StopReason; changesets?: StudyChangeset[]; report?: string; eyes?: string[]; usd?: number }): Study {
  const s = spec({ slug: args.slug, ...(args.goal ? { goal: args.goal } : {}), ...(args.repo ? { repo: args.repo } : {}) })
  const b = build(args.slug, args.plan)
  const base = args.plan.base
  const measured = args.state.state !== 'preparing' && args.state.state !== 'failed'
  const started = ago(args.startedAgoMs)
  const hoursUsed = Math.min(s.budget.hours, (Date.now() - Date.parse(started)) / H)
  const repo = s.repo.split('/').filter(Boolean).pop() ?? s.repo
  const delta = measured && args.plan.count > 0 ? ((b.incumbent - base) / base) * 100 : null
  const currentRow = args.plan.current ? b.rows.at(-1) : undefined
  const info: StudyInfo = {
    slug: args.slug,
    goal: s.goal,
    repo: s.repo,
    state: args.state,
    metric: s.metric,
    base_sha: args.state.state === 'failed' ? null : BASE_SHA,
    incumbent: { sha: measured ? b.incumbentSha : BASE_SHA, mean: measured ? b.incumbent : null, kept: b.kept },
    baseline_mean: measured ? base : null,
    sigma: measured ? args.plan.sigma : null,
    segment: 0,
    experiments: args.plan.count,
    delta_pct: delta,
    headline: `Optimising ${s.metric.name} in ${repo} · ${args.plan.count} experiments${delta !== null ? ` · ${pct(delta)}` : ''} · ${args.state.state}`,
    budget_used: { usd: args.usd ?? args.plan.count * 0.335, hours: Number(hoursUsed.toFixed(1)), experiments: args.plan.count, by_channel: { session: args.usd ?? args.plan.count * 0.335, bench: 0 } },
    budget: s.budget,
    session: args.state.state === 'running' || args.state.state === 'paused' ? `ses-${args.slug}` : null,
    origin: `setup-${args.slug}`,
    current: currentRow?.n ?? null,
    phase: currentRow ? 'sampling' : null,
    started_at: started,
    finished_at: args.stop ? ago(20 * MIN) : null,
    stop_reason: args.stop ?? null,
    report_doc: args.report ?? null,
    kind: 'one metric',
    created_by: null,
  }
  const detail: StudyDetail = {
    info,
    spec: s,
    playbook: args.plan.count === 0 ? '' : PLAYBOOK,
    series: {
      experiments: b.points,
      incumbent: measured ? b.steps : [],
      holdout: args.holdout ? HOLDOUT.filter((h) => h.n <= args.plan.count) : [],
      drift: args.plan.count > 25 ? [{ n: 25, verdict: '', metric: 44.9, at: ago(1.2 * H) }] : [],
      baseline_mean: measured ? base : null,
      sigma: measured ? args.plan.sigma : null,
    },
    ideas: args.plan.count === 0 ? ideas(args.slug).slice(0, 2) : ideas(args.slug),
    steers: args.plan.count === 0 ? [] : steers(args.slug),
    changesets: args.changesets ?? [],
    needs_eyes: args.eyes ?? [],
    tail: info.current !== null ? 'CASE n4000 time_ms=144.196\nCASE n8000 time_ms=591.764\nCASE n12000 time_ms=1213.014\nMETRIC time_ms=1948.974' : null,
    final_holdout: args.state.state === 'finished' && args.holdout ? '61.0 → 49.7 (−18.5 %)' : null,
    view: { kind: 'scalar' },
  }
  return { info, detail, rows: [...b.rows].reverse(), details: b.details }
}

const MAIN_PLAN: Plan = { count: 34, base: 52.1, sigma: 0.42, keeps: { 3: 50.4, 7: 48.9, 11: 47.2, 18: 46.0, 23: 44.3, 27: 42.6, 31: 41.2 }, crash: [5, 16, 29], checks: [21], reverted: { 14: 45.0 }, deviation: 27, interrupted: [33] }
const EYES = [
  '#14 reverted by review — “Precomputes the rendered output of the benchmark’s fixture names in a constant table.”',
  '#27 deviation — touched lib/liquid/profiler.rb outside scope; justification: “Removes a hook call on the hot path; profiler output unchanged.”',
  '#25 drift — the incumbent re-measured at 44.9 ms, expected ≈ 44.3 ms; the next confirm doubled.',
]

/** The §11.2 changesets of the finished study. */
export const CHANGESETS: StudyChangeset[] = [
  { k: 1, name: 'buffer-reuse', branch: 'autoresearch/lexer-tokens/1-buffer-reuse', files: ['lib/liquid/render.rb', 'lib/liquid/context.rb'], experiments: [3, 7], delta_alone_pct: -11.2, delta_together_pct: -11.2, only_with: [], review: { kind: 'clean', reason: 'Buffer reuse; measurement unchanged.' } },
  { k: 2, name: 'strainer-struct', branch: 'autoresearch/lexer-tokens/2-strainer-struct', files: ['lib/liquid/strainer_template.rb'], experiments: [11], delta_alone_pct: -6.4, delta_together_pct: -5.9, only_with: [], review: { kind: 'clean', reason: 'A frozen Struct for the strainer.' } },
  { k: 3, name: 'lazy-scopes', branch: 'autoresearch/lexer-tokens/3-lazy-scopes', files: ['lib/liquid/context.rb', 'lib/liquid/variable.rb'], experiments: [18, 23], delta_alone_pct: -0.3, delta_together_pct: -3.1, only_with: [1], review: { kind: 'clean', reason: 'Scopes allocated on first write.' } },
  { k: 4, name: 'expression-inline', branch: 'autoresearch/lexer-tokens/4-expression-inline', files: ['lib/liquid/expression.rb'], experiments: [31], delta_alone_pct: -2.2, delta_together_pct: -2.0, only_with: [], review: null },
]

const paused = (kind: Exclude<PauseReason['kind'], 'other' | 'launch_approval'>): StudyState => ({ state: 'paused', reason: { kind } })

/** Every fixture study, in the rail's order (running first, then paused, preparing, finished, failed). */
export function studies(): Study[] {
  return [
    study({ slug: 'sort-speed', state: { state: 'running' }, plan: { ...MAIN_PLAN, current: true }, holdout: true, startedAgoMs: 4.1 * H, eyes: EYES, usd: 11.4 }),
    study({ slug: 'parse-alloc', goal: 'Cut peak allocation of the parser on the fixture set without slowing render.', state: paused('crash_streak'), plan: { count: 12, base: 131.6, sigma: 0.9, keeps: { 2: 124.1, 6: 119.8 }, crash: [9, 10, 11, 12] }, startedAgoMs: 1.6 * H }),
    study({ slug: 'json-emit', goal: 'Speed up JSON emission of the catalogue export.', repo: '/Users/Sky.Moore/Develop/catalogue', state: paused('user'), plan: { count: 8, base: 812, sigma: 6.2, keeps: { 4: 770 } }, startedAgoMs: 50 * MIN }),
    study({ slug: 'fixture-guard', goal: 'Faster template compile; the fixtures are off limits.', state: paused('custody_refusals'), plan: { count: 5, base: 18.4, sigma: 0.11, keeps: {} }, startedAgoMs: 35 * MIN }),
    study({ slug: 'cache-chains', goal: 'Cache compiled filter chains per template.', state: paused('review_flags'), plan: { count: 15, base: 52.1, sigma: 0.42, keeps: { 2: 49.0 }, reverted: { 5: 44.0, 9: 43.1, 13: 41.8 } }, startedAgoMs: 2.2 * H, eyes: ['#13 reverted by review — “Caches the rendered output keyed by the benchmark’s fixture path.”', '#9 reverted by review — “Skips rendering for fixture names seen before.”', '#5 reverted by review — “Shortens the warm-up loop the benchmark times.”'] }),
    study({ slug: 'overnight-sim', goal: 'Shorten the end-to-end simulation run.', repo: '/Users/Sky.Moore/Develop/sim', state: paused('budget_confirm'), plan: { count: 40, base: 5400, sigma: 38, keeps: { 10: 5100, 20: 4800, 30: 4650 } }, startedAgoMs: 8 * H, usd: 19.6 }),
    study({ slug: 'cold-start', goal: 'Cut the CLI cold start.', repo: '/Users/Sky.Moore/Develop/theuth', state: { state: 'preparing' }, plan: { count: 0, base: 0, sigma: 0, keeps: {} }, startedAgoMs: 3 * MIN }),
    study({
      slug: 'lexer-tokens',
      state: { state: 'finished' },
      plan: { count: 61, base: 52.1, sigma: 0.42, keeps: { 3: 50.4, 7: 48.9, 11: 47.2, 18: 46.0, 23: 44.3, 27: 42.6, 31: 41.2 }, crash: [5, 16, 29, 44], checks: [21, 50], reverted: { 14: 45.0 }, deviation: 27 },
      holdout: true,
      startedAgoMs: 11 * H,
      usd: 17.8,
      eyes: ['#31 unreviewed keep — the reviewer failed twice; the keep stands on the metric, custody and checks alone.', '#27 deviation — touched lib/liquid/profiler.rb outside scope; justification: “Removes a hook call on the hot path; profiler output unchanged.”'],
      stop: { kind: 'plateau' },
      changesets: CHANGESETS,
      report: 'doc_autoresearch_lexer_tokens',
    }),
    study({ slug: 'flaky-bench', goal: 'Faster image thumbnailing.', repo: '/Users/Sky.Moore/Develop/thumbs', state: { state: 'failed', reason: 'σ = 7.9 % of the metric: a keep would need ≥ 15.8 %; make the benchmark steadier or raise repeats' }, plan: { count: 0, base: 0, sigma: 0, keeps: {} }, startedAgoMs: 26 * H }),
  ]
}

function wait<T>(v: T): Promise<T> {
  const { promise, resolve } = Promise.withResolvers<T>()
  setTimeout(() => resolve(v), 120)
  return promise
}

const wrapped = new WeakMap<Api, Api>()

/** The engine's `Api` with the autoresearch methods (and the `autoresearch-<slug>` routine rows) answered here when a fixture state is on; the same object otherwise. */
export function autoresearchFixture(api: Api): Api {
  const s = state()
  if (!s) return api
  let w = wrapped.get(api)
  if (!w) wrapped.set(api, (w = wrap(api, s)))
  return w
}

function wrap(api: Api, s: State): Api {
  const all = s === 'empty' ? [] : studies()
  let settings = SETTINGS
  const find = (slug: string): Study => {
    const st = all.find((x) => x.info.slug === slug)
    if (!st) throw new Error(`no study ${slug}`)
    return st
  }
  const row = (st: Study): ScheduleInfo => (receiveComponent("method.schedule.output.ScheduleInfo", {
    name: `autoresearch-${st.info.slug}`,
    when: 'every 0s',
    describe: `every 0s, up to ${st.info.budget.experiments + 10} runs`,
    enabled: st.info.state.state === 'running',
    quiet: true,
    next: st.info.state.state === 'running' ? ago(-MIN) : null,
    last: st.rows[0]?.finished_at ?? null,
    last_result: st.rows[0] ? 'end_turn' : null,
    session: st.info.session,
    running: st.info.current !== null,
    error: null,
    until: null,
    max_runs: st.info.budget.experiments + 10,
    runs: st.info.experiments,
    approvals: false,
    cwd: `~/.theuth/autoresearch/${st.info.slug}/worktree`,
    agent: null,
    project: null,
    when_sensor: null,
    fire: 'while_true',
    mode: 'skip',
    memory: 'clear',
    expires: null,
    last_check: null,
    for: null,
    cooldown: null,
    timeout: null,
    model: null,
    run_timeout: '100m',
    created: st.info.started_at,
    sensor: null,
  }))
  return {
    ...api,
    list_schedules: async () => {
      const rows = await api.list_schedules().catch(() => [] as ScheduleInfo[])
      return [...rows.filter((r) => !r.name.startsWith('autoresearch-')), ...all.filter((st) => st.info.state.state === 'running' || st.info.state.state === 'paused').map(row)]
    },
    'autoresearch.list': () => wait(all.map((st) => st.info)),
    'autoresearch.get': (slug) => wait(find(slug).detail),
    'autoresearch.experiments': (slug, before, limit) => wait(find(slug).rows.filter((r) => before === null || r.n < before).slice(0, limit)),
    'autoresearch.experiment': (slug, n) => {
      const d = find(slug).details.get(n)
      return d ? wait(d) : Promise.reject(new Error(`no experiment ${n}`))
    },
    'autoresearch.control': (slug, op) => {
      // Never in place: React Query's structural sharing keeps the old reference when nothing new is allocated.
      const st = find(slug)
      const set = (next: Partial<StudyInfo>) => {
        st.info = { ...st.info, ...next }
        st.detail = { ...st.detail, info: st.info }
      }
      switch (op.op) {
        case 'pause':
          set({ state: paused('user'), current: null, phase: null })
          break
        case 'resume':
        case 'run_once':
          set({ state: { state: 'running' } })
          break
        case 'stop_after':
        case 'stop_now':
        case 'finalize':
          set({ state: { state: 'finalizing' }, current: null, phase: null })
          break
        case 'discard':
          set({ state: { state: 'discarded' } })
          break
        case 'restore':
        case 'keep_reverted':
          st.detail = { ...st.detail, needs_eyes: st.detail.needs_eyes.filter((line) => !line.startsWith(`#${op.n} reverted`)) }
          break
        case 'merge':
          break
        case 'segment':
          set({ segment: st.info.segment + 1, state: { state: 'running' } })
          break
      }
      return wait(st.info)
    },
    'autoresearch.steer': (slug, text) => {
      const st = find(slug)
      const id = st.detail.steers.length + 1
      st.detail = { ...st.detail, steers: [{ id, slug, text, by: 'user', at: new Date().toISOString(), seen_in: null }, ...st.detail.steers] }
      return wait(id)
    },
    'autoresearch.idea': (slug, op) => {
      const st = find(slug)
      if (op.op === 'add') {
        const idea: StudyIdea = { id: st.detail.ideas.length + 1, slug, text: op.text, category: op.category, source: 'user', priority: op.priority ?? 3, status: 'open', tried_in: [], at: new Date().toISOString() }
        st.detail = { ...st.detail, ideas: [idea, ...st.detail.ideas] }
        return wait(idea)
      }
      const idea = st.detail.ideas.find((x) => x.id === op.id)
      if (!idea) return Promise.reject(new Error(`no idea ${op.id}`))
      const next: StudyIdea = op.op === 'close' ? { ...idea, status: 'closed' } : op.op === 'reopen' ? { ...idea, status: 'open' } : { ...idea, priority: op.priority }
      st.detail = { ...st.detail, ideas: st.detail.ideas.map((x) => (x.id === op.id ? next : x)) }
      return wait(next)
    },
    'autoresearch.settings.get': () => wait(settings),
    'autoresearch.settings.set': (next) => {
      settings = receiveComponent('method.autoresearch.output.AutoresearchConfig', next)
      return wait(settings)
    },
  }
}
