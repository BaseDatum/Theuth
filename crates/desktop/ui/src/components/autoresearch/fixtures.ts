// 140 §15 "UI": the autoresearch RPCs answered from memory so the Library tab, the Study view, the routines row
// and Settings › Autoresearch render in every state against any engine. Dev only, behind
// `VITE_AUTORESEARCH_FIXTURE=studies|empty` (or `localStorage['autoresearch-fixture']`); the data is exported for
// the vitest over the model and the components. Mirrors `dev/dreaming-fixture.ts`.

import type { Api } from '@/rpc/api'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import { pct } from './model'
import type {
  Attention,
  AutoresearchSettings,
  Changeset,
  ChartPoint,
  CurrentExperiment,
  ExperimentDetail,
  ExperimentRow,
  ExperimentState,
  HoldoutPoint,
  Idea,
  PauseReason,
  Sample,
  Steer,
  Step,
  StudyDetail,
  StudyInfo,
  StudySpec,
  StudyState,
  Verdict,
} from './types'

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
  slug: string
  count: number
  base: number
  sigma: number
  keeps: Record<number, number>
  crash?: number[]
  checks?: number[]
  reverted?: Record<number, number>
  deviation?: number
  interrupted?: number[]
  /** The experiment in flight (running / measuring states only). */
  current?: Partial<CurrentExperiment> & { n: number }
}

/** The §4.1 example study as spec; slug/goal/budget overridden per fixture. */
const spec = (over: Partial<StudySpec> & { slug: string }): StudySpec => ({
  goal: 'Make parse+render of the fixture set faster without changing output.',
  repo: '/Users/Sky.Moore/Develop/liquid',
  base: '4f2c9e1a8b3d5e7f9012345678901234567890ab',
  metric: { name: 'time_ms', unit: 'ms', direction: 'lower', min_effect: '0.5%' },
  bench: { command: 'bench.sh', timeout_secs: 600, repeats: 3, exclusive: true },
  checks: { command: 'bundle exec rake test', timeout_secs: 300 },
  holdout: { command: 'bench-holdout.sh', every: 5 },
  scope: ['lib/liquid/**'],
  off_limits: ['fixtures/**', 'test/**'],
  budget: { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 },
  parallel: 1,
  models: { executor: null, review: null, advisor: null },
  ...over,
})

type Built = { rows: ExperimentRow[]; details: Map<number, ExperimentDetail>; points: ChartPoint[]; steps: Step[]; incumbent: number; kept: number; reverted: number; crashed: number; noGain: number; checksFailed: number }

/** The ledger of one study from a plan: every row's numbers follow the plan's keeps and the pseudo-noise. */
function build(p: Plan): Built {
  const rows: ExperimentRow[] = []
  const details = new Map<number, ExperimentDetail>()
  const points: ChartPoint[] = []
  const steps: Step[] = [{ n: 0, metric: p.base }]
  const sd = p.sigma * Math.sqrt(1 + 1 / 3)
  let inc = p.base
  const counts = { kept: 0, reverted: 0, crashed: 0, noGain: 0, checksFailed: 0 }
  const start = Date.now() - p.count * 7 * MIN
  for (let n = 1; n <= p.count; n++) {
    const before = inc
    let value: number | null
    let st: ExperimentState
    let verdict: Verdict | null
    let samples = 1
    let review: ExperimentRow['review'] = null
    const keep = p.keeps[n]
    const rev = p.reverted?.[n]
    if (keep !== undefined) {
      value = keep
      st = 'kept'
      verdict = 'improved'
      samples = 3
      review = { verdict: 'clean', reason: 'Replaces a per-node allocation with a reused buffer; measurement unchanged.' }
      inc = keep
      steps.push({ n, metric: keep })
      counts.kept++
    } else if (p.crash?.includes(n)) {
      value = null
      st = 'crashed'
      verdict = 'crash'
      counts.crashed++
    } else if (p.checks?.includes(n)) {
      value = inc - 0.9
      st = 'checks_failed'
      verdict = 'checks_failed'
      samples = 3
      counts.checksFailed++
    } else if (rev !== undefined) {
      value = rev
      st = 'rejected_by_review'
      verdict = 'improved'
      samples = 3
      review = { verdict: 'suspicious', reason: 'Precomputes the rendered output of the benchmark’s fixture names in a constant table.' }
      counts.reverted++
    } else if (p.interrupted?.includes(n)) {
      value = null
      st = 'interrupted'
      verdict = null
    } else {
      value = inc + (noise(n) - 0.3) * 1.5
      const z = (inc - value) / sd
      st = 'discarded'
      verdict = z < -2 ? 'worse' : 'no_gain'
      counts.noGain++
    }
    const delta = value === null ? null : value - before
    const deltaPct = delta === null ? null : (delta / before) * 100
    const z = value === null ? null : (before - value) / sd
    const line =
      value === null
        ? st === 'interrupted'
          ? 'interrupted: engine restarted mid-sample'
          : 'crash: exit 1 (tail below)'
        : st === 'kept' || st === 'rejected_by_review'
          ? `improved: ${value.toFixed(1)} ms vs ${before.toFixed(1)} ms (${pct(deltaPct)}), ${(z as number).toFixed(1)}σ over 3 samples, checks passed`
          : st === 'checks_failed'
            ? `checks failed: bundle exec rake test exit 1 (${pct(deltaPct)}, ${(z as number).toFixed(1)}σ)`
            : verdict === 'worse'
              ? `worse: ${pct(deltaPct)}, ${(z as number).toFixed(1)}σ (1 sample)`
              : `no gain: ${pct(deltaPct)}, ${(z as number).toFixed(1)}σ (1 sample)`
    const startedAt = new Date(start + (n - 1) * 7 * MIN).toISOString()
    const durationMs = (4 + samples * 2.5 + noise(n + 7) * 3) * MIN
    const deviations = p.deviation === n ? ['lib/liquid/profiler.rb'] : []
    const row: ExperimentRow = {
      n,
      wave: null,
      slot: null,
      state: st,
      verdict,
      line,
      idea: IDEAS[n % IDEAS.length] as string,
      category: CATS[n % CATS.length] as string,
      metric: value,
      delta,
      delta_pct: deltaPct,
      z,
      n_samples: value === null ? 0 : samples,
      commit_sha: st === 'kept' ? `${(0xa1b2c3 + n * 7919).toString(16).padStart(7, '0')}e4d5f60718293a4b5c6d7e8f90a1b2c3d4e5f6` : null,
      cost_usd: 0.28 + (n % 9) / 100 + samples * 0.04,
      duration_ms: durationMs,
      deviations,
      review,
      started_at: startedAt,
      finished_at: new Date(Date.parse(startedAt) + durationMs).toISOString(),
    }
    rows.push(row)
    points.push({ n, state: st, verdict, metric: value, at: startedAt })
    const sampleRows: Sample[] = value === null ? [] : Array.from({ length: samples }, (_, k) => ({ k: k + 1, role: 'candidate' as const, value: value + (noise(n * 10 + k) - 0.5) * p.sigma, secondaries: { alloc_mb: 118.2 + noise(n + k) }, exit: 0, wall_ms: 42_000 + noise(k) * 3000, load: 0.21 + noise(n + k) * 0.2, noisy: false, lost: false, at: new Date(Date.parse(startedAt) + (k + 1) * 50_000).toISOString() }))
    if (st === 'crashed') sampleRows.push({ k: 1, role: 'candidate', value: null, secondaries: {}, exit: 1, wall_ms: 3_200, load: 0.3, noisy: false, lost: false, at: startedAt })
    details.set(n, {
      row,
      hypothesis: `${row.idea} should cut the hot path's allocations, which the profiler put at 18 % of render time.`,
      description: `${row.idea}: ${st === 'kept' ? 'replaced the per-call allocation with a buffer owned by the context.' : 'tried the change on the render loop; the benchmark did not move past noise.'}`,
      learned: st === 'kept' ? 'Buffer reuse pays off anywhere the node walker allocates per visit.' : 'The lexer is not where the time goes; the render loop is.',
      next: 'Look at Expression#evaluate: 31 % of samples in the profile.',
      justification: deviations.length ? 'Removes a hook call on the hot path; profiler output unchanged.' : null,
      samples: sampleRows,
      cases_moved: value === null ? [] : [
        { id: 'templates/product_grid.liquid', before: 9.8, after: 9.8 + (deltaPct ?? 0) * 0.2, delta_pct: (deltaPct ?? 0) * 2.1 },
        { id: 'templates/cart.liquid', before: 4.1, after: 4.1 + (deltaPct ?? 0) * 0.05, delta_pct: (deltaPct ?? 0) * 1.3 },
        { id: 'templates/blog.liquid', before: 2.2, after: 2.2, delta_pct: 0 },
      ],
      diff: st === 'crashed' && n % 2 === 0 ? null : DIFF.replace('@@IDEA@@', row.idea),
      logs: [
        { name: 'sample-1.log', tail: value === null ? 'warmup 1/3 … ok\nfixtures 12/214 …\nTraceback (most recent call last):\n  File "bench.py", line 41, in <module>\n    render(t)\nNameError: name \'strainer\' is not defined' : `warmup 3/3 … ok\nfixtures 214/214 … ok\nMETRIC time_ms=${(value + 0.1).toFixed(2)}\nMETRIC alloc_mb=118.2\nASI gc_runs=41` },
        ...(st === 'checks_failed' ? [{ name: 'checks.log', tail: 'Run options: --seed 4121\n...F.\nFailure: test_whitespace_control (RenderTest)\n  expected "a b" got "ab"\n214 runs, 1 failure' }] : []),
      ],
    })
  }
  return { rows, details, points, steps, incumbent: inc, ...counts }
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

const HOLDOUT: HoldoutPoint[] = [
  { n: 0, metric: 61.0, at: ago(5 * H) },
  { n: 11, metric: 56.2, at: ago(3.2 * H) },
  { n: 23, metric: 51.6, at: ago(1.6 * H) },
  { n: 31, metric: 49.7, at: ago(0.5 * H) },
]

const IDEA_ROWS: Idea[] = [
  { id: 'i1', text: 'Lexer: scan by memchr for tag openers', category: 'algorithm', source: 'user', priority: 3, status: 'open', tried_in: [], at: ago(2 * H) },
  { id: 'i2', text: 'Cache compiled filter chains per template', category: 'caching', source: 'user', priority: 3, status: 'open', tried_in: [], at: ago(1.5 * H) },
  { id: 'i3', text: 'Intern variable names at parse time', category: 'allocation', source: 'model', priority: 1, status: 'open', tried_in: [], at: ago(4 * H) },
  { id: 'i4', text: 'Avoid regex in whitespace control', category: 'algorithm', source: 'advisor', priority: 2, status: 'open', tried_in: [], at: ago(3 * H) },
  { id: 'i5', text: 'Batch output appends into one buffer', category: 'io', source: 'model', priority: 1, status: 'open', tried_in: [], at: ago(3 * H) },
  { id: 'i6', text: 'Replace the strainer Hash with a frozen Struct', category: 'allocation', source: 'setup', priority: 1, status: 'tried', tried_in: [9], at: ago(5 * H) },
  { id: 'i7', text: 'Skip the parser for templates without tags', category: 'algorithm', source: 'model', priority: 0, status: 'closed', tried_in: [16], at: ago(4 * H) },
]

const STEERS: Steer[] = [
  { id: 's1', text: 'Try SIMD for the lexer; do not touch the filters module.', by: 'sky', at: ago(2.5 * H), seen_in: 19 },
  { id: 's2', text: 'The strainer lookups look hot in production profiles.', by: 'sky', at: ago(40 * MIN), seen_in: null },
]

const PLAYBOOK = `## Build
\`bundle exec rake compile\` is not needed: pure Ruby. Benchmarks warm in ~8 s.

## Hot path
\`Liquid::Render#render_node\` → \`Block#render_to\` → \`Variable#render\`. The profiler (stackprof, wall) puts 31 % of samples in \`Expression#evaluate\`, 18 % in per-node String allocation.

## Known dead ends
Regex changes in the lexer: the lexer is 4 % of time. Not worth another experiment.`

const SETTINGS: AutoresearchSettings = {
  enabled: true,
  defaults: { repeats: 3, bench_timeout_secs: 600, checks_timeout_secs: 300, budget: { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 } },
  models: { executor: null, review: null, advisor: null },
  resolved: { executor: 'openai/gpt-5.6-sol', review: 'anthropic/claude-fable-5-1', advisor: 'openai/gpt-5.6-sol' },
  review: { on_suspicious: 'revert' },
  bench: { lock: 'builtin', max_load: 0.5 },
  keep_finished_days: 14,
}

type Study = { info: StudyInfo; detail: StudyDetail; rows: ExperimentRow[]; details: Map<number, ExperimentDetail> }

function study(args: { slug: string; goal?: string; repo?: string; state: StudyState; reason?: PauseReason | string | null; plan: Omit<Plan, 'slug'>; holdout?: boolean; startedAgoMs: number; finished?: { headline: string; changesets: Changeset[]; report_doc: string; stop: StudyDetail['stop_reason'] }; attention?: Attention[]; usd?: number }): Study {
  const s = spec({ slug: args.slug, ...(args.goal ? { goal: args.goal } : {}), ...(args.repo ? { repo: args.repo } : {}) })
  const b = build({ slug: args.slug, ...args.plan })
  const base = args.plan.base
  const delta = args.plan.count === 0 ? null : ((b.incumbent - base) / base) * 100
  const started = ago(args.startedAgoMs)
  const info: StudyInfo = {
    slug: args.slug,
    goal: s.goal,
    repo: s.repo,
    state: args.state,
    reason: args.reason ?? null,
    metric: s.metric,
    base_value: args.state === 'preparing' ? null : base,
    incumbent_value: args.state === 'preparing' ? null : b.incumbent,
    delta_pct: args.state === 'preparing' ? null : delta,
    experiments: args.plan.count,
    kept: b.kept,
    budget: s.budget,
    session: args.state === 'running' || args.state === 'paused' ? `ses-${args.slug}` : null,
    started_at: started,
    finished_at: args.finished ? ago(20 * MIN) : null,
    current_phase: args.plan.current?.phase ?? null,
  }
  const current: CurrentExperiment | null = args.plan.current
    ? {
        state: 'measuring',
        idea: 'Reuse the token buffer across render calls instead of allocating per node',
        category: 'allocation',
        phase: 'Sampling',
        sample: 2,
        of: 3,
        started_at: ago(6 * MIN + 42_000),
        elapsed_ms: 6 * MIN + 42_000,
        tail: 'warmup 3/3 … ok\nfixtures 214/214 … ok\nMETRIC time_ms=40.71\nMETRIC alloc_mb=118.2\nASI gc_runs=41',
        note: 'sample 1: 40.6 ms (−1.5 %, 1.9σ) → confirming',
        ...args.plan.current,
      }
    : null
  const hoursUsed = Math.min(s.budget.hours, (Date.now() - Date.parse(started)) / H)
  const detail: StudyDetail = {
    info,
    spec: s,
    base_sha: s.base,
    incumbent_sha: b.rows.filter((r) => r.state === 'kept').at(-1)?.commit_sha ?? s.base,
    sigma: args.state === 'preparing' ? null : args.plan.sigma,
    sigma_pct: args.state === 'preparing' ? null : (args.plan.sigma / base) * 100,
    keep_needs_pct: args.state === 'preparing' ? null : Math.max(((2 * args.plan.sigma * Math.sqrt(1 + 1 / 3)) / b.incumbent) * 100, 0.5),
    segment: 0,
    budget: { usd: args.usd ?? args.plan.count * 0.335, usd_cap: s.budget.usd, hours: Number(hoursUsed.toFixed(1)), hours_cap: s.budget.hours, experiments: args.plan.count, experiments_cap: s.budget.experiments },
    counts: { kept: b.kept, reverted: b.reverted, crashed: b.crashed, no_gain: b.noGain, checks_failed: b.checksFailed },
    chart: { points: b.points, steps: b.steps, holdout: args.holdout ? HOLDOUT.filter((h) => h.n <= args.plan.count) : [], drift: args.plan.count > 25 ? [{ n: 25, metric: 44.9, at: ago(1.2 * H) }] : [] },
    holdout: args.holdout ? { base: 61.0, latest: 49.7, delta_pct: -18.5, at_keep: 5, overfit: false } : null,
    current,
    ideas: args.plan.count === 0 ? IDEA_ROWS.slice(0, 2) : IDEA_ROWS,
    steers: args.plan.count === 0 ? [] : STEERS,
    playbook: args.plan.count === 0 ? '' : PLAYBOOK,
    attention: args.attention ?? [],
    headline: args.finished?.headline ?? null,
    changesets: args.finished?.changesets ?? null,
    report_doc: args.finished?.report_doc ?? null,
    stop_reason: args.finished?.stop ?? null,
  }
  return { info, detail, rows: [...b.rows].reverse(), details: b.details }
}

const MAIN_PLAN: Omit<Plan, 'slug'> = { count: 34, base: 52.1, sigma: 0.42, keeps: { 3: 50.4, 7: 48.9, 11: 47.2, 18: 46.0, 23: 44.3, 27: 42.6, 31: 41.2 }, crash: [5, 16, 29], checks: [21], reverted: { 14: 45.0 }, deviation: 27, interrupted: [33] }
const EYES: Attention[] = [
  { kind: 'reverted', n: 14, reason: 'Precomputes the rendered output of the benchmark’s fixture names in a constant table.' },
  { kind: 'deviation', n: 27, paths: ['lib/liquid/profiler.rb'], justification: 'Removes a hook call on the hot path; profiler output unchanged.' },
  { kind: 'drift', n: 25, metric: 44.9, expected: 44.3 },
]

/** The §11.2 changesets of the finished study. */
export const CHANGESETS: Changeset[] = [
  { k: 1, name: 'buffer-reuse', branch: 'autoresearch/lexer-tokens/1-buffer-reuse', commits: ['a1b2c3d', 'b2c3d4e'], files: ['lib/liquid/render.rb', 'lib/liquid/context.rb'], delta_alone_pct: -11.2, delta_together_pct: -11.2, alone_clears: true, depends_on: null, review: 'clean', pr_url: null, merged: false },
  { k: 2, name: 'strainer-struct', branch: 'autoresearch/lexer-tokens/2-strainer-struct', commits: ['c3d4e5f'], files: ['lib/liquid/strainer_template.rb'], delta_alone_pct: -6.4, delta_together_pct: -5.9, alone_clears: true, depends_on: null, review: 'clean', pr_url: 'https://github.com/Shopify/liquid/pull/1891', merged: false },
  { k: 3, name: 'lazy-scopes', branch: 'autoresearch/lexer-tokens/3-lazy-scopes', commits: ['d4e5f60', 'e5f6071'], files: ['lib/liquid/context.rb', 'lib/liquid/variable.rb'], delta_alone_pct: -0.3, delta_together_pct: -3.1, alone_clears: false, depends_on: 1, review: 'clean', pr_url: null, merged: false },
  { k: 4, name: 'expression-inline', branch: 'autoresearch/lexer-tokens/4-expression-inline', commits: ['f607182'], files: ['lib/liquid/expression.rb'], delta_alone_pct: -2.2, delta_together_pct: -2.0, alone_clears: true, depends_on: null, review: 'unreviewed', pr_url: null, merged: false },
]

/** Every fixture study, in the rail's order (running first, then paused, preparing, finished, failed). */
export function studies(): Study[] {
  return [
    study({ slug: 'sort-speed', state: 'running', plan: { ...MAIN_PLAN, current: { n: 35 } }, holdout: true, startedAgoMs: 4.1 * H, attention: EYES, usd: 11.4 }),
    study({ slug: 'parse-alloc', goal: 'Cut peak allocation of the parser on the fixture set without slowing render.', state: 'paused', reason: 'crash_streak', plan: { count: 12, base: 131.6, sigma: 0.9, keeps: { 2: 124.1, 6: 119.8 }, crash: [9, 10, 11, 12] }, startedAgoMs: 1.6 * H, attention: [] }),
    study({ slug: 'json-emit', goal: 'Speed up JSON emission of the catalogue export.', repo: '/Users/Sky.Moore/Develop/catalogue', state: 'paused', reason: 'user', plan: { count: 8, base: 812, sigma: 6.2, keeps: { 4: 770 } }, startedAgoMs: 50 * MIN }),
    study({ slug: 'fixture-guard', goal: 'Faster template compile; the fixtures are off limits.', state: 'paused', reason: 'custody_refusals', plan: { count: 5, base: 18.4, sigma: 0.11, keeps: {} }, startedAgoMs: 35 * MIN }),
    study({ slug: 'cache-chains', goal: 'Cache compiled filter chains per template.', state: 'paused', reason: 'review_flags', plan: { count: 15, base: 52.1, sigma: 0.42, keeps: { 2: 49.0 }, reverted: { 5: 44.0, 9: 43.1, 13: 41.8 } }, startedAgoMs: 2.2 * H, attention: [{ kind: 'reverted', n: 13, reason: 'Caches the rendered output keyed by the benchmark’s fixture path.' }] }),
    study({ slug: 'overnight-sim', goal: 'Shorten the end-to-end simulation run.', repo: '/Users/Sky.Moore/Develop/sim', state: 'paused', reason: 'budget_confirm', plan: { count: 40, base: 5400, sigma: 38, keeps: { 10: 5100, 20: 4800, 30: 4650 } }, startedAgoMs: 8 * H, usd: 19.6 }),
    study({ slug: 'cold-start', goal: 'Cut the CLI cold start.', repo: '/Users/Sky.Moore/Develop/theuth', state: 'preparing', plan: { count: 0, base: 0, sigma: 0, keeps: {} }, startedAgoMs: 3 * MIN }),
    study({
      slug: 'lexer-tokens',
      state: 'finished',
      reason: 'plateau',
      plan: { count: 61, base: 52.1, sigma: 0.42, keeps: { 3: 50.4, 7: 48.9, 11: 47.2, 18: 46.0, 23: 44.3, 27: 42.6, 31: 41.2 }, crash: [5, 16, 29, 44], checks: [21, 50], reverted: { 14: 45.0 }, deviation: 27 },
      holdout: true,
      startedAgoMs: 11 * H,
      usd: 17.8,
      attention: [{ kind: 'unreviewed', n: 31 }, { kind: 'deviation', n: 27, paths: ['lib/liquid/profiler.rb'], justification: 'Removes a hook call on the hot path; profiler output unchanged.' }],
      finished: { headline: 'time_ms 52.1 → 41.2 (−20.9 %, CI ±0.8 %) · holdout 61.0 → 49.7 (−18.5 %)', changesets: CHANGESETS, report_doc: 'doc_autoresearch_lexer_tokens', stop: 'plateau' },
    }),
    study({ slug: 'flaky-bench', goal: 'Faster image thumbnailing.', repo: '/Users/Sky.Moore/Develop/thumbs', state: 'failed', reason: 'σ = 7.9 % of the metric: a keep would need ≥ 15.8 %; make the benchmark steadier or raise repeats', plan: { count: 0, base: 0, sigma: 0, keeps: {} }, startedAgoMs: 26 * H }),
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
  const row = (st: Study): ScheduleInfo => ({
    name: `autoresearch-${st.info.slug}`,
    when: 'every 0s',
    describe: `every 0s, up to ${st.info.budget.experiments + 10} runs`,
    enabled: st.info.state === 'running',
    quiet: true,
    next: st.info.state === 'running' ? ago(-MIN) : null,
    last: st.rows[0]?.finished_at ?? null,
    last_result: st.rows[0] ? 'end_turn' : null,
    session: st.info.session,
    running: st.detail.current !== null,
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
  })
  return {
    ...api,
    list_schedules: async () => {
      const rows = await api.list_schedules().catch(() => [] as ScheduleInfo[])
      return [...rows.filter((r) => !r.name.startsWith('autoresearch-')), ...all.filter((st) => st.info.state === 'running' || st.info.state === 'paused').map(row)]
    },
    'autoresearch.list': () => wait(all.map((st) => st.info)),
    'autoresearch.get': (slug) => wait(find(slug).detail),
    'autoresearch.experiments': (slug, before, limit) => {
      const rows = find(slug).rows.filter((r) => before === null || r.n < before)
      return wait(rows.slice(0, limit))
    },
    'autoresearch.experiment': (slug, n) => {
      const d = find(slug).details.get(n)
      return d ? wait(d) : Promise.reject(new Error(`no experiment ${n}`))
    },
    'autoresearch.control': (slug, op) => {
      const st = find(slug)
      const i = st.info
      const set = (next: Partial<StudyInfo>) => {
        Object.assign(i, next)
        st.detail.info = { ...i }
      }
      if (op === 'Pause') set({ state: 'paused', reason: 'user' })
      else if (op === 'Resume' || op === 'RunOnce') set({ state: 'running', reason: null })
      else if (op === 'StopAfter' || op === 'StopNow' || op === 'Finalize') set({ state: 'finalizing', reason: null })
      else if (op === 'Discard') set({ state: 'discarded', reason: null })
      else if (typeof op === 'object' && 'Restore' in op) st.detail.attention = st.detail.attention.filter((a) => !(a.kind === 'reverted' && a.n === op.Restore.n))
      else if (typeof op === 'object' && 'KeepReverted' in op) st.detail.attention = st.detail.attention.filter((a) => !(a.kind === 'reverted' && a.n === op.KeepReverted.n))
      else if (typeof op === 'object' && 'OpenPr' in op) {
        const c = st.detail.changesets?.find((x) => x.k === op.OpenPr.k)
        if (c) c.pr_url = `https://github.com/Shopify/liquid/pull/${1900 + c.k}`
      } else if (typeof op === 'object' && 'Merge' in op) {
        const c = st.detail.changesets?.find((x) => x.k === op.Merge.k)
        if (c) c.merged = true
      }
      return wait({ ...i })
    },
    'autoresearch.steer': (slug, text) => {
      const st = find(slug)
      const id = `s${st.detail.steers.length + 1}`
      st.detail.steers = [...st.detail.steers, { id, text, by: 'you', at: new Date().toISOString(), seen_in: null }]
      return wait(id)
    },
    'autoresearch.idea': (slug, op) => {
      const st = find(slug)
      if ('Add' in op) {
        const idea: Idea = { id: `i${st.detail.ideas.length + 1}`, text: op.Add.text, category: op.Add.category, source: 'user', priority: op.Add.priority, status: 'open', tried_in: [], at: new Date().toISOString() }
        st.detail.ideas = [idea, ...st.detail.ideas]
        return wait(idea)
      }
      const id = 'Close' in op ? op.Close.id : op.Priority.id
      const idea = st.detail.ideas.find((x) => x.id === id)
      if (!idea) return Promise.reject(new Error(`no idea ${id}`))
      const next: Idea = 'Close' in op ? { ...idea, status: 'closed' } : { ...idea, priority: op.Priority.priority }
      st.detail.ideas = st.detail.ideas.map((x) => (x.id === id ? next : x))
      return wait(next)
    },
    'autoresearch.settings': () => wait(settings),
    'autoresearch.set_settings': (next) => {
      settings = next
      return wait(settings)
    },
  }
}

