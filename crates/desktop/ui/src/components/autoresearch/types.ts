// 140 §12.3 / §12.5: a local mirror of the autoresearch RPC types, written before the contract worker's ts-rs
// bindings landed. Same names as the Rust types; every import of this file moves to `@/rpc/bindings/*` when
// `.bindings-ready` appears and this file is deleted (140-W4 brief, "Bindings").

export type StudyState = 'draft' | 'preparing' | 'running' | 'paused' | 'finalizing' | 'finished' | 'failed' | 'discarded'

export type Direction = 'lower' | 'higher'

/** §5.2 verdicts of `experiment_run`, plus the closing states §4.3 lists on the ledger row. */
export type Verdict =
  | 'improved'
  | 'keep_simpler'
  | 'no_gain'
  | 'worse'
  | 'crash'
  | 'checks_failed'
  | 'guard_failed'
  | 'refused'

/** §4.3: the ledger state of one experiment. */
export type ExperimentState =
  | 'open'
  | 'measuring'
  | 'measured'
  | 'kept'
  | 'discarded'
  | 'crashed'
  | 'checks_failed'
  | 'refused'
  | 'rejected_by_review'
  | 'abandoned'
  | 'interrupted'

/** `ExperimentProgress.phase` (§12.3). */
export type Phase = 'Novelty' | 'Custody' | 'Waiting' | 'Sampling' | 'Checks' | 'Review'

export type StopReason = 'user' | 'experiments' | 'budget_usd' | 'deadline' | 'plateau' | 'overfit' | 'failed'

/** Why a study is `paused` (§4.2); `user` for the button. */
export type PauseReason = 'user' | 'crash_streak' | 'custody_refusals' | 'review_flags' | 'budget_confirm'

export type Metric = { name: string; unit: string; direction: Direction; min_effect: string }

export type Budget = { usd: number; per_experiment_usd: number; hours: number; experiments: number }

/** §4.1, the parts the Study view shows. */
export type StudySpec = {
  slug: string
  goal: string
  repo: string
  base: string
  metric: Metric
  bench: { command: string; timeout_secs: number; repeats: number; exclusive: boolean }
  checks: { command: string; timeout_secs: number } | null
  holdout: { command: string; every: number } | null
  scope: string[]
  off_limits: string[]
  budget: Budget
  parallel: number
  models: { executor: string | null; review: string | null; advisor: string | null }
}

/** One row of `autoresearch.list` and `autoresearch_status`. */
export type StudyInfo = {
  slug: string
  goal: string
  repo: string
  state: StudyState
  /** The pause reason while `paused`, the stop reason once stopped, a failure message when `failed`. */
  reason: string | null
  metric: Metric
  base_value: number | null
  incumbent_value: number | null
  /** Direction-normalised: negative is better for `lower`, positive for `higher`; what the headline shows. */
  delta_pct: number | null
  experiments: number
  kept: number
  budget: Budget
  session: string | null
  started_at: string | null
  finished_at: string | null
  current_phase: Phase | null
}

export type ChartPoint = { n: number; state: ExperimentState; verdict: Verdict | null; metric: number | null; at: string }
export type Step = { n: number; metric: number }
export type HoldoutPoint = { n: number; metric: number; at: string }
export type DriftMark = { n: number; metric: number; at: string }

export type BudgetUsed = { usd: number; usd_cap: number; hours: number; hours_cap: number; experiments: number; experiments_cap: number }

/** The experiment in flight: what the header and the current-experiment panel show (§13.1). */
export type CurrentExperiment = {
  n: number
  state: ExperimentState
  idea: string | null
  category: string | null
  phase: Phase | null
  sample: number
  of: number
  started_at: string
  elapsed_ms: number
  /** The last lines of the running sample's log (≤ 30); empty before the first sample. */
  tail: string
  /** The verdict line of sample 1 while confirming, e.g. `sample 1: 40.6 ms (−1.5 %, 1.9σ) → confirming`. */
  note: string | null
}

export type Idea = {
  id: string
  text: string
  category: string
  source: 'user' | 'model' | 'advisor' | 'setup'
  priority: number
  status: 'open' | 'tried' | 'closed'
  tried_in: number[]
  at: string
}

export type Steer = { id: string; text: string; by: string; at: string; seen_in: number | null }

export type ReviewVerdict = { verdict: 'clean' | 'suspicious' | 'error'; reason: string }

/** §13.1 "Needs your eyes": the rows a person should look at. */
export type Attention =
  | { kind: 'reverted'; n: number; reason: string }
  | { kind: 'deviation'; n: number; paths: string[]; justification: string | null }
  | { kind: 'overfit'; search_delta_pct: number; holdout_delta_pct: number }
  | { kind: 'drift'; n: number; metric: number; expected: number }
  | { kind: 'unreviewed'; n: number }

export type Holdout = { base: number | null; latest: number | null; delta_pct: number | null; at_keep: number | null; overfit: boolean }

/** §11.2: one finalize changeset. */
export type Changeset = {
  k: number
  name: string
  branch: string
  commits: string[]
  files: string[]
  delta_alone_pct: number | null
  delta_together_pct: number | null
  /** False when the changeset no longer clears the gate alone ("only helps together with #depends_on"). */
  alone_clears: boolean
  depends_on: number | null
  review: 'clean' | 'suspicious' | 'unreviewed'
  pr_url: string | null
  merged: boolean
}

/** `autoresearch.get {slug}`. */
export type StudyDetail = {
  info: StudyInfo
  spec: StudySpec
  base_sha: string
  incumbent_sha: string
  sigma: number | null
  sigma_pct: number | null
  /** What a keep needs, relative: `max(z·σ·√(1+1/n), min_effect)` over the incumbent. */
  keep_needs_pct: number | null
  segment: number
  budget: BudgetUsed
  counts: { kept: number; reverted: number; crashed: number; no_gain: number; checks_failed: number }
  chart: { points: ChartPoint[]; steps: Step[]; holdout: HoldoutPoint[]; drift: DriftMark[] }
  holdout: Holdout | null
  current: CurrentExperiment | null
  ideas: Idea[]
  steers: Steer[]
  playbook: string
  attention: Attention[]
  headline: string | null
  changesets: Changeset[] | null
  report_doc: string | null
  stop_reason: StopReason | null
}

/** `autoresearch.experiments {slug, before?, limit}` rows (newest first). */
export type ExperimentRow = {
  n: number
  wave: number | null
  slot: number | null
  state: ExperimentState
  verdict: Verdict | null
  /** The verdict in words and numbers (§5.2.5). */
  line: string
  idea: string
  category: string
  metric: number | null
  delta: number | null
  delta_pct: number | null
  z: number | null
  n_samples: number
  commit_sha: string | null
  cost_usd: number
  duration_ms: number
  deviations: string[]
  review: ReviewVerdict | null
  started_at: string
  finished_at: string | null
}

export type Sample = {
  k: number
  role: 'candidate' | 'incumbent' | 'baseline' | 'holdout' | 'final'
  value: number | null
  secondaries: Record<string, number>
  exit: number | null
  wall_ms: number
  load: number
  noisy: boolean
  lost: boolean
  at: string
}

export type CaseMoved = { id: string; before: number; after: number; delta_pct: number }

/** `autoresearch.experiment {slug, n}`. */
export type ExperimentDetail = {
  row: ExperimentRow
  hypothesis: string
  description: string | null
  learned: string | null
  next: string | null
  justification: string | null
  samples: Sample[]
  cases_moved: CaseMoved[]
  /** Unified diff of the kept commit or the saved candidate patch (≤ 256 KiB); null for a crash before an edit. */
  diff: string | null
  logs: { name: string; tail: string }[]
}

/** §12.3 `ControlOp`, externally tagged as ts-rs renders a mixed enum. `OpenPr` / `Merge` are §11.5's landing buttons. */
export type ControlOp =
  | 'Pause'
  | 'Resume'
  | 'StopAfter'
  | 'StopNow'
  | 'RunOnce'
  | 'Finalize'
  | 'Discard'
  | { Segment: { reason: string } }
  | { Restore: { n: number } }
  | { KeepReverted: { n: number } }
  | { OpenPr: { k: number } }
  | { Merge: { k: number } }

export type IdeaChange = { Add: { text: string; category: string; priority: number } } | { Close: { id: string } } | { Priority: { id: string; priority: number } }

/** `autoresearch.settings` / `set_settings`: the §12.2 keys the UI exposes. */
export type AutoresearchSettings = {
  enabled: boolean
  defaults: { repeats: number; bench_timeout_secs: number; checks_timeout_secs: number; budget: Budget }
  models: { executor: string | null; review: string | null; advisor: string | null }
  /** What the engine resolves the `null` roles to (R1-D5). */
  resolved: { executor: string; review: string; advisor: string } | null
  review: { on_suspicious: 'revert' | 'flag' }
  bench: { lock: string; max_load: number }
  keep_finished_days: number
}

// Events (§12.3), `#[serde(tag = "type")]`.
export type StudyChanged = { type: 'StudyChanged'; slug: string; state: StudyState; reason: string | null }
export type ExperimentStarted = { type: 'ExperimentStarted'; slug: string; n: number; slot: number | null }
export type ExperimentProgress = { type: 'ExperimentProgress'; slug: string; n: number; phase: Phase; sample: number; of: number; elapsed_ms: number }
export type ExperimentFinished = { type: 'ExperimentFinished'; slug: string; n: number; verdict: Verdict; metric: number | null; delta_pct: number | null; z: number | null }
export type StudyFinished = { type: 'StudyFinished'; slug: string; reason: StopReason; headline: string; report_doc: string }
export type AutoresearchEvent = StudyChanged | ExperimentStarted | ExperimentProgress | ExperimentFinished | StudyFinished
