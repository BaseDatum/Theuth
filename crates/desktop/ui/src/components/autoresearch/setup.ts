// 151 §4.7 `setup.ts`: the pure half of the setup pane — the first send, the newest draft in a session, the texts of
// §4.6, the §4.3 rows/leaves and the recent-repositories list. No React; `setup.test.ts` covers it.

import { FRESH } from '@/components/chat/slash'
import { PLAIN_OPTIONS } from '@/lib/drafts'
import type { Api } from '@/rpc/api'
import type { DraftFieldState } from '@/rpc/bindings/DraftFieldState'
import type { DraftSummary } from '@/rpc/bindings/DraftSummary'
import type { ProbeKind } from '@/rpc/bindings/ProbeKind'
import type { StudyInfo } from '@/rpc/bindings/StudyInfo'
import type { StudySpec } from '@/rpc/bindings/StudySpec'
import { type Entry, useSessions } from '@/store/sessions'

/** §4.3: the main nine rows then the advanced five, in the panel's fixed order, with their labels. */
export const ROW_LABELS: Record<string, string> = {
  goal: 'Goal',
  repo: 'Repository',
  metric: 'Metric',
  bench: 'Benchmark',
  checks: 'Check',
  holdout: 'Holdout',
  scope: 'Scope',
  budget: 'Budget',
  window: 'When',
  secondary_guards: 'Secondary guards',
  warm: 'Warm-up',
  parallel: 'Parallel',
  models: 'Models',
  evaluator_paths: 'Evaluator paths',
}
export const ADVANCED: Record<string, true> = { secondary_guards: true, warm: true, parallel: true, models: true, evaluator_paths: true }
export const rowLabel = (row: string): string => ROW_LABELS[row] ?? row

/** §4.3: the `draft_edit` leaves of each row (`base → repo`, `off_limits → scope`). */
export const LEAVES: Record<string, readonly string[]> = {
  goal: ['goal'],
  repo: ['repo', 'base'],
  metric: ['metric.name', 'metric.unit', 'metric.direction', 'metric.min_effect'],
  bench: ['bench.command', 'bench.timeout', 'bench.repeats', 'bench.exclusive', 'bench.smoke'],
  checks: ['checks.command', 'checks.timeout'],
  holdout: ['holdout.command', 'holdout.every', 'holdout.repeats'],
  scope: ['scope', 'off_limits'],
  budget: ['budget.usd', 'budget.per_experiment_usd', 'budget.hours', 'budget.experiments'],
  window: ['window'],
  secondary_guards: ['secondary_guards'],
  warm: ['warm'],
  parallel: ['parallel'],
  models: ['models.executor', 'models.review', 'models.advisor'],
  evaluator_paths: ['evaluator_paths'],
}

/** A leaf's current text in the §4.3 grammar (lists `, `-joined, bools/numbers as words, absent = empty). */
export function leafValue(spec: StudySpec, leaf: string): string {
  let v: unknown = spec
  for (const seg of leaf.split('.')) {
    if (v === null || typeof v !== 'object') return ''
    v = (v as Record<string, unknown>)[seg]
  }
  if (v === null || v === undefined) return ''
  if (Array.isArray(v)) {
    // `MetricGuard` rows as the §4.3 line `name max_regression [direction]`, one per line.
    const text = (x: unknown) => (typeof x === 'object' && x !== null ? Object.values(x as Record<string, unknown>).filter((y) => y !== null && y !== '').join(' ') : String(x))
    return v.map(text).join(leaf === 'secondary_guards' ? '\n' : ', ')
  }
  return String(v)
}

/** Which probe backs a row's "Verified" (the sparkline); `null` for rows the engine verifies otherwise. */
export const PROBE_OF: Record<string, ProbeKind> = { bench: 'bench', checks: 'check', holdout: 'holdout' }

/** The badge of each §5 state (the mock's words). */
export const STATE_BADGE: Record<DraftFieldState, { text: string; className: string }> = {
  verified: { text: 'Verified', className: 'border-success-border/40 bg-success-bg text-success' },
  confirmed: { text: 'Confirmed', className: 'border-info-border/40 bg-info-bg text-info' },
  proposed: { text: 'Needs your OK', className: 'border-warning-border/40 bg-warning-bg text-warning' },
  default: { text: 'Default', className: 'border-border text-fg-secondary' },
  missing: { text: 'Missing', className: 'border-danger-border/40 bg-danger-bg text-danger' },
  optional: { text: 'Optional', className: 'border-border text-fg-tertiary' },
}

// ── §4.6 texts ────────────────────────────────────────────────────────────────────────────────────────────────────

export const seedText = (repo: string, goal: string): string => `[autoresearch setup · repo ${repo}]\n${goal.trim() || 'Help me set up a study for this repository'}`
export const basename = (path: string): string => path.split('/').filter(Boolean).pop() ?? path
export const sessionTitle = (repo: string, goal: string): string => `New study: ${goal.trim() || basename(repo)}`
export const editMessage = (slug: string, leaf: string, value: string): string => `[study draft ${slug}] you set ${leaf} to ${value} in the panel`
export const discardMessage = (slug: string): string => `[study draft ${slug}] discarded in the panel`
/** The compact card's one press (§4.6 / §4.8). */
export const CARD_PRESS = /^study draft ([a-z0-9-]{1,32}): open in autoresearch$/m
/** The §6.4 compact card's slug grammar; `StudyDraftBridge` and the pane both match it. */
export const DRAFTS_APP = 'study-drafts'

// ── the session's entries ─────────────────────────────────────────────────────────────────────────────────────────

export type StudyDraftRef = { draftId: string; callId: string }

/** The newest `autoresearch_draft` call with a `slug` in a session's entries (D3), or `null`. */
export function latestStudyDraft(entries: readonly Entry[]): StudyDraftRef | null {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (e?.kind !== 'tool' || e.name !== 'autoresearch_draft') continue
    const slug = (e.args as { slug?: unknown } | undefined)?.slug
    if (typeof slug === 'string' && slug) return { draftId: slug, callId: e.callId }
  }
  return null
}

const SETUP_TOOLS: Record<string, true> = { autoresearch_draft: true, autoresearch_probe: true }

/** The call id of the newest `autoresearch_draft` / `autoresearch_probe` *result* (D3: the panel refetches when it changes), or `null`. */
export function lastSetupResultId(entries: readonly Entry[]): string | null {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (e?.kind === 'tool' && SETUP_TOOLS[e.name] && e.result !== undefined) return e.callId
  }
  return null
}

/** D1: distinct repositories of studies ∪ drafts, newest first. */
export function recentRepos(studies: readonly Pick<StudyInfo, 'repo' | 'started_at'>[], drafts: readonly Pick<DraftSummary, 'repo' | 'updated_at'>[]): string[] {
  const at = new Map<string, number>()
  const see = (repo: string, when: string | null) => {
    if (!repo) return
    const t = Date.parse(when ?? '') || 0
    at.set(repo, Math.max(at.get(repo) ?? 0, t))
  }
  for (const s of studies) see(s.repo, s.started_at)
  for (const d of drafts) see(d.repo, d.updated_at)
  return [...at.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([repo]) => repo)
}

/** Sparkline points of a probe's samples in a `w × h` box, `null` with fewer than two values. */
export function sparkPoints(values: readonly number[], w = 56, h = 12): string | null {
  if (values.length < 2) return null
  const lo = Math.min(...values)
  const hi = Math.max(...values)
  const span = hi - lo || 1
  return values.map((v, i) => `${((i / (values.length - 1)) * w).toFixed(1)},${(h - 1.5 - ((v - lo) / span) * (h - 3)).toFixed(1)}`).join(' ')
}

/**
 * §2 step 3 — the first send, in Routines' order: a root with `cwd` = the repo, its title, the slice seeded with
 * `FRESH`, the page told (it writes `?chat=<id>` and mounts `LiveSession`), then `use_skill` with the §4.6 seed.
 */
export async function startSetupChat(
  api: Pick<Api, 'create_session' | 'rename_session' | 'use_skill'>,
  repo: string,
  goal: string,
  onSession: (id: string) => void,
): Promise<string> {
  const { session: id } = await api.create_session({ ...PLAIN_OPTIONS, cwd: repo })
  await api.rename_session(id, sessionTitle(repo, goal))
  useSessions.getState().loadHistory(id, FRESH, 'replace')
  onSession(id)
  await api.use_skill(id, 'autoresearch', seedText(repo, goal))
  return id
}
