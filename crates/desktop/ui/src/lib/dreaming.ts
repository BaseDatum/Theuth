// 136 §8: the pure parts of Settings › Dreaming and the Routines row — formatting, the settings reducer,
// the digest field. Wire types in `@/rpc/bindings/Dream*`; everything here is testable without React.

import { formatMoney, moneyParts } from '@/lib/money'
import type { DreamModels } from '@/rpc/bindings/DreamModels'
import type { DreamSettings } from '@/rpc/bindings/DreamSettings'
import type { NightCounts } from '@/rpc/bindings/NightCounts'
import type { NightSummary } from '@/rpc/bindings/NightSummary'
import type { RunMode } from '@/rpc/bindings/RunMode'
import type { UndoReport } from '@/rpc/bindings/UndoReport'

/** §9.2 `budget_usd: 1` — the nightly default. */
export const DEFAULT_BUDGET_USD = 1
/** §14.1: Dream now opens at $5. */
export const RUN_NOW_BUDGET_USD = 5
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

/** Dreaming's money is `f64` USD on the wire; the formatter wants integer nanos. `$4.100`. */
export function usd(n: number): string {
  return formatMoney({ nanos: String(Math.round(n * 1e9)), currency: 'USD' })
}
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

/** The status card's line: `14 merged, 31 archived, $4.100`, `(undone)` appended for a reverted night; `no nights yet`. */
export function nightText(night: NightSummary | null): string {
  if (!night) return 'no nights yet'
  const line = `${countsLine(night.counts)}, ${usd(night.spend_usd)}`
  return night.undone ? `${line} (undone)` : line
}

/** The Routines row subtitle: `last night: 14 merged, 31 archived, $4.100`. */
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

/** One edit of Settings › Dreaming; `set_settings` takes the whole object, so the reducer produces it. */
export type SettingsChange =
  | { kind: 'budget'; usd: number }
  | { kind: 'window'; index: 0 | 1; time: string }
  | { kind: 'model'; tier: Tier; model: string | null }
  | { kind: 'preferences_auto'; on: boolean }
  | { kind: 'digest_day'; day: Day }
  | { kind: 'digest_time'; time: string }
  | { kind: 'pause'; now: Date }
  | { kind: 'resume' }

/**
 * The next `DreamSettings`, or `null` when the change is a no-op or invalid (a negative budget, a malformed time) so
 * the caller skips the round trip. `enabled` is never touched here: it is the routine's switch (`enable_schedule`).
 */
export function reduceSettings(s: DreamSettings, c: SettingsChange): DreamSettings | null {
  switch (c.kind) {
    case 'budget':
      if (!Number.isFinite(c.usd) || c.usd < 0 || c.usd === s.budget_usd) return null
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
 * What a tier's `null` resolves to (§9.2 defaults), for the picker's first entry: triage has no model (`null`, rules
 * only); tidy follows the title model, then the subagent model; propose the subagent model; judge the session default
 * when its provider differs from propose, else propose again with another seed.
 */
export function resolvedDefault(tier: Tier, d: { title: string | null; subagent: string | null; session: string }): { model: string | null; seed: boolean } {
  const propose = d.subagent ?? d.session
  switch (tier) {
    case 'triage':
      return { model: null, seed: false }
    case 'tidy':
      return { model: d.title ?? propose, seed: false }
    case 'propose':
      return { model: propose, seed: false }
    case 'judge':
      return d.session.split('/')[0] !== propose.split('/')[0] ? { model: d.session, seed: false } : { model: propose, seed: true }
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
