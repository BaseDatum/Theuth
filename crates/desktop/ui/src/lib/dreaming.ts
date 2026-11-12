// 136 §8: the pure parts of Settings › Dreaming and the Routines row — formatting, the settings reducer,
// the digest field. Wire types in `@/rpc/bindings/Dream*`; everything here is testable without React.

import { moneyParts } from '@/lib/money'
import type { DreamModels } from "@/rpc/bindings/method.dreaming.output"
import type { DreamSettings } from "@/rpc/bindings/method.dreaming.output"
import type { NightCounts } from "@/rpc/bindings/method.dreaming.output"
import type { NightSummary } from "@/rpc/bindings/method.dreaming.output"
import type { RunMode } from '@/rpc/bindings/method.dreaming.input'
import type { UndoReport } from "@/rpc/bindings/method.dreaming.output"

/** §9.2 `budget_usd: 1` — the nightly default. */
export const DEFAULT_BUDGET_USD = 1
/** The server's bound on any budget (`SetSettings`, R-34/R-15): `0 < usd ≤ 1000`. */
export const MAX_BUDGET_USD = 1000
export const PAUSE_DAYS = 7
/** Dream now's modes in display order. */
export const RUN_MODES: readonly [RunMode, string][] = [
  ['pass', 'One pass'],
  ['backlog', 'Backlog'],
]
export const BACKLOG_SENTENCE = 'Backlog repeats passes until nothing is untidied or the budget is spent.'

/** The four model tiers of `DreamModels`, in the order Settings shows them. */
export const TIERS = ['triage', 'tidy', 'propose', 'judge'] as const satisfies readonly (keyof DreamModels)[]
export type Tier = (typeof TIERS)[number]
export const TIER_LABEL: Record<Tier, string> = { triage: 'Triage', tidy: 'Tidy', propose: 'Propose', judge: 'Judge' }

/** Dreaming's money is `f64` USD on the wire; two decimals like every price in the pane (`$4.10`, `$1,000.00`). */
const USD = new Intl.NumberFormat('en', { style: 'currency', currency: 'USD' })
export const usd = (n: number): string => USD.format(n)
/** A budget field's text → USD, or `null` when empty, non-numeric or outside `(0, 1000]` (R-34: an empty field must never save $0). */
export function budgetFromInput(raw: string): number | null {
  const n = Number(raw)
  return raw.trim() !== '' && Number.isFinite(n) && n > 0 && n <= MAX_BUDGET_USD ? n : null
}
export const BUDGET_PROBLEM = `Enter an amount above $0 and at most ${usd(MAX_BUDGET_USD)}.`
/** The currency symbol for an input adornment (`$`). */
export const USD_SYMBOL = moneyParts({ nanos: '0', currency: 'USD' }).prefix

/**
 * The counts of a night as the §8 sentence: `14 merged, 31 archived, 1 skill edit kept, 2 rejected` — zero
 * counts omitted, `nothing to tidy` when every count is zero.
 */
export function countsLine(c: NightCounts): string {
  const parts: string[] = []
  const push = (v: bigint | number, one: string, many = `${one}s`) => {
    // `bigint` on the type, a number after `JSON.parse`.
    const k = Number(v)
    if (k > 0) parts.push(`${k} ${k === 1 ? one : many}`)
  }
  push(c.retitled, 'retitled', 'retitled')
  push(c.merged, 'merged', 'merged')
  push(c.superseded, 'superseded', 'superseded')
  push(c.archived, 'archived', 'archived')
  push(c.verified, 'verified', 'verified')
  push(c.cases, 'case')
  push(c.edits_accepted, 'skill edit kept', 'skill edits kept')
  push(c.edits_rejected, 'rejected', 'rejected')
  push(c.proposals, 'proposal')
  return parts.length ? parts.join(', ') : 'nothing to tidy'
}

/** The status card's line: `14 merged, 31 archived, $4.10`, `(undone)` appended for a reverted night; `no nights yet`. */
export function nightText(night: NightSummary | null): string {
  if (!night) return 'no nights yet'
  const line = `${countsLine(night.counts)}, ${usd(night.spend_usd)}`
  return night.undone ? `${line} (undone)` : line
}

/** The Routines row subtitle: `last night: 14 merged, 31 archived, $4.10`. */
export const nightLine = (night: NightSummary | null): string => (night ? `last night: ${nightText(night)}` : nightText(night))

/** `Mon 08:00` ⇄ `{ day, time }`; an unparseable digest reads as Monday 08:00 so the controls always show something. */
export const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const
export type Day = (typeof DAYS)[number]
export function parseDigest(s: string): { day: Day; time: string } {
  const [d, t] = s.trim().split(/\s+/)
  const day = DAYS.find((x) => x.toLowerCase() === d?.slice(0, 3).toLowerCase()) ?? 'Mon'
  const time = t && /^\d{2}:\d{2}$/.test(t) ? t : '08:00'
  return { day, time }
}
export const formatDigest = (day: Day, time: string): string => `${day} ${time}`

/** `paused_until` for a pause pressed at `now`: `now + 7 days`, RFC 3339. */
export const pauseUntil = (now: Date, days = PAUSE_DAYS): string => new Date(now.getTime() + days * 86_400_000).toISOString()

/** The pause is live while `paused_until` is in the future. */
export const isPaused = (pausedUntil: string | null, now: Date): boolean => pausedUntil !== null && new Date(pausedUntil).getTime() > now.getTime()

/**
 * Why Dream now cannot run right now (R-46), as the tail of "Dream now is unavailable: …" for the button's tooltip and
 * the note under the status card; `null` when it can. Dreaming off beats the routine switch beats the pause beats a running night.
 */
export function dreamNowBlocker(
  s: { enabled: boolean; routineEnabled: boolean | undefined; pausedUntil: string | null; running: boolean },
  now: Date,
): string | null {
  if (!s.enabled) return 'dreaming is off'
  if (s.routineEnabled === false) return 'the dreaming routine is switched off'
  if (isPaused(s.pausedUntil, now)) return `dreaming is paused until ${new Date(s.pausedUntil as string).toLocaleDateString()}`
  if (s.running) return 'a night is already running'
  return null
}
export const dreamNowUnavailable = (blocker: string): string => `Dream now is unavailable: ${blocker}.`
/** 185 A5: where a Dream now press runs — the hub (named) from a non-hub that reaches it, else this machine. */
export const dreamNowWhere = (runsOn: string | null): string => (runsOn ? `Runs on ${runsOn}, the hub,` : 'Runs on this machine')

/** One edit of Settings › Dreaming; `set_settings` takes the whole object, so the reducer produces it. */
export type SettingsChange =
  | { kind: 'enabled'; on: boolean }
  | { kind: 'budget'; usd: number }
  | { kind: 'window'; index: 0 | 1; time: string }
  | { kind: 'model'; tier: Tier; model: string | null }
  | { kind: 'effort'; tier: Tier; effort: string | null }
  | { kind: 'preferences_auto'; on: boolean }
  | { kind: 'digest_day'; day: Day }
  | { kind: 'digest_time'; time: string }
  | { kind: 'pause'; now: Date }
  | { kind: 'resume' }

/**
 * The next `DreamSettings`, or `null` when the change is a no-op or invalid (a budget outside `(0, 1000]`, a malformed
 * time) so the caller skips the round trip. `enabled` is the dreaming plugin's switch (188 F2), not the routine's (`enable_schedule`).
 */
export function reduceSettings(s: DreamSettings, c: SettingsChange): DreamSettings | null {
  switch (c.kind) {
    case 'enabled':
      return s.enabled === c.on ? null : { ...s, enabled: c.on }
    case 'budget':
      if (!Number.isFinite(c.usd) || c.usd <= 0 || c.usd > MAX_BUDGET_USD || c.usd === s.budget_usd) return null
      return { ...s, budget_usd: c.usd }
    case 'window': {
      if (!/^\d{2}:\d{2}$/.test(c.time) || s.window[c.index] === c.time) return null
      const window: [string, string] = [s.window[0], s.window[1]]
      window[c.index] = c.time
      return { ...s, window }
    }
    case 'model':
      if (s.models[c.tier] === c.model) return null
      return { ...s, models: { ...s.models, [c.tier]: c.model } }
    // 194: a tier's effort beside its pick; the engine clears it with the pick and drops one a new pick does not take.
    case 'effort':
      if ((s.efforts?.[c.tier] ?? null) === c.effort) return null
      return { ...s, efforts: { ...s.efforts, [c.tier]: c.effort } }
    case 'preferences_auto':
      return s.preferences_auto === c.on ? null : { ...s, preferences_auto: c.on }
    case 'digest_day': {
      const d = parseDigest(s.digest)
      return d.day === c.day ? null : { ...s, digest: formatDigest(c.day, d.time) }
    }
    case 'digest_time': {
      if (!/^\d{2}:\d{2}$/.test(c.time)) return null
      const d = parseDigest(s.digest)
      return d.time === c.time ? null : { ...s, digest: formatDigest(d.day, c.time) }
    }
    case 'pause':
      return { ...s, paused_until: pauseUntil(c.now) }
    case 'resume':
      return s.paused_until === null ? null : { ...s, paused_until: null }
  }
}

/**
 * What a tier's `null` resolves to, for the picker's first entry: the server's `resolved.*` when the engine exports it
 * (R-14: that is the model the night actually runs on; judge = propose means the same model judges); else the §9.2 rule
 * client-side — triage has no model (rules only); tidy follows the routine file's `model:`, then the title model, then
 * the subagent model; propose the subagent model; judge the session default when its provider differs from propose,
 * else propose itself (`same`).
 */
export function resolvedDefault(
  tier: Tier,
  d: { resolved?: DreamSettings['resolved']; routine?: string | null; title: string | null; subagent: string | null; session: string },
): { model: string | null; same: boolean } {
  if (tier === 'triage') return { model: null, same: false }
  if (d.resolved) return { model: d.resolved[tier], same: tier === 'judge' && d.resolved.judge === d.resolved.propose }
  const propose = d.subagent ?? d.session
  switch (tier) {
    case 'tidy':
      return { model: d.routine ?? d.title ?? propose, same: false }
    case 'propose':
      return { model: propose, same: false }
    case 'judge':
      return d.session.split('/')[0] !== propose.split('/')[0] ? { model: d.session, same: false } : { model: propose, same: true }
  }
}

/** `3 knowledge, 1 skill, 2 edits restored` (+ `· 1 error`) for the undo toast. */
export function undoLine(r: UndoReport): string {
  const parts = [
    r.knowledge_restored.length && `${r.knowledge_restored.length} knowledge`,
    r.skills_restored.length && `${r.skills_restored.length} ${r.skills_restored.length === 1 ? 'skill' : 'skills'}`,
    r.edits_undone.length && `${r.edits_undone.length} ${r.edits_undone.length === 1 ? 'edit' : 'edits'}`,
  ].filter((x): x is string => typeof x === 'string')
  const head = parts.length ? `${parts.join(', ')} restored` : 'nothing to restore'
  return r.errors.length ? `${head} · ${r.errors.length} ${r.errors.length === 1 ? 'error' : 'errors'}: ${r.errors[0]}` : head
}
