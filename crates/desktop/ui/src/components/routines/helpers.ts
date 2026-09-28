import type { LastCheck } from '@/rpc/bindings/LastCheck'
import type { ScheduleInfo } from '@/rpc/bindings/ScheduleInfo'
import type { Verdict } from '@/rpc/bindings/Verdict'

export const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'] as const

/** `in 14h` / `3m ago` / `now`: future-aware, unlike the sidebar's past-only `ago` (34 §8). */
export function relative(iso: string, now = Date.now()): string {
  const s = (Date.parse(iso) - now) / 1000
  const a = Math.abs(s)
  if (a < 60) return 'now'
  const n = a < 3600 ? `${Math.floor(a / 60)}m` : a < 86400 ? `${Math.floor(a / 3600)}h` : `${Math.floor(a / 86400)}d`
  return s > 0 ? `in ${n}` : `${n} ago`
}

/** `4s` / `2m` / `1h` between two instants. */
export function took(from: string, to: string): string {
  const s = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000))
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : `${Math.floor(s / 3600)}h`
}

/** `—` for a plain routine, `14` with `until` only, `14/200` with `max_runs` (33 §6). */
export function runCount(s: Pick<ScheduleInfo, 'runs' | 'max_runs' | 'until'>): string {
  if (s.max_runs !== null) return `${s.runs}/${s.max_runs}`
  return s.until !== null ? String(s.runs) : '—'
}

const num = /^\d+$/

/**
 * A cron expression in words for the common shapes (`every day 09:00`, `weekdays 09:00`, `every Monday 09:00`,
 * `every 15 min`, `hourly at :30`, `monthly on day 1 09:00`); `null` for anything else — the caller falls back to
 * the engine's `describe`.
 */
export function cronWords(expr: string): string | null {
  const f = expr.trim().split(/\s+/)
  if (f.length !== 5) return null
  const [m, h, dom, mon, dow] = f as [string, string, string, string, string]
  if (mon !== '*') return null
  const step = /^\*\/(\d+)$/.exec(m)
  if (step && h === '*' && dom === '*' && dow === '*') return `every ${step[1]} min`
  if (!num.test(m)) return null
  if (h === '*' && dom === '*' && dow === '*') return `hourly at :${m.padStart(2, '0')}`
  if (!num.test(h)) return null
  const at = `${h.padStart(2, '0')}:${m.padStart(2, '0')}`
  if (dom === '*' && dow === '*') return `every day ${at}`
  if (dom === '*' && dow === '1-5') return `weekdays ${at}`
  if (dom === '*' && num.test(dow) && Number(dow) <= 7) return `every ${DAYS[Number(dow) % 7]} ${at}`
  if (dow === '*' && num.test(dom)) return `monthly on day ${dom} ${at}`
  return null
}

/** `once at 16 Sep 15:00` from an `at:` clause as written (RFC 3339 or `YYYY-MM-DDTHH:MM[:SS]`, local). */
function onceAt(clause: string): string {
  const t = Date.parse(clause)
  if (Number.isNaN(t)) return `once at ${clause}`
  return `once at ${new Date(t).toLocaleString(undefined, { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hour12: false })}`
}

/**
 * Cadence in words from a routine's `when` clause (`cron 0 9 * * *` / `every 5m` / `at 2026-09-16T15:00`):
 * `every day 09:00` · `every 5m` · `once at 16 Sep 15:00`; cron shapes without words use `describe`.
 */
export function cadence(s: Pick<ScheduleInfo, 'when' | 'describe'>): string {
  const [kind, ...rest] = s.when.split(' ')
  const clause = rest.join(' ')
  if (kind === 'every') return `every ${clause}`
  if (kind === 'at') return onceAt(clause)
  if (kind === 'cron') return cronWords(clause) ?? s.describe
  return s.describe
}

/** The 40 §1.12 verdict as one line, the shape `/schedules test` prints (mirrors `schedule::validate::render`). */
export function verdictText(v: Verdict): string {
  if (v.error !== null) return `sensor ${v.sensor} · check broken: ${v.error}`
  const hash = v.hash ? `${v.hash.slice(0, 8)}…` : '—'
  let s = `sensor ${v.sensor} · ${(v.elapsed_ms / 1000).toFixed(1)} s · fire: ${v.fire} · would ${v.would_fire ? '' : 'NOT '}fire now · hash ${hash}`
  if (v.probe !== null) s += ` · probe ${v.probe}`
  if (v.needs_approvals_off) s += ' · needs approvals: false (body mutates)'
  if (v.flaps) s += ' · payload flaps — include only stable fields'
  return s
}

/** `10:15:03 · quiet · 1.2 s` for a watcher's last sensor check (40 §6). */
export function lastCheckText(c: LastCheck): string {
  const at = new Date(c.at).toLocaleTimeString(undefined, { hour12: false })
  return `${at} · ${c.outcome} · ${(c.elapsed_ms / 1000).toFixed(1)} s`
}

export type Result = 'ok' | 'failed' | 'skipped'

/** Chip for `last_result`: `end_turn` and loop ends are ok; `skipped: …` skipped; every other end (error, timeout, cancelled, lost, until broken) failed. */
export function resultKind(r: string | null): Result | null {
  if (r === null) return null
  if (r === 'end_turn' || r.startsWith('done') || r.startsWith('max_runs')) return 'ok'
  if (r.startsWith('skipped')) return 'skipped'
  return 'failed'
}

/** List order: broken files first (they need a hand), then enabled by soonest `next`, then paused; name breaks ties. */
export function sortRoutines<T extends Pick<ScheduleInfo, 'name' | 'enabled' | 'error' | 'next'>>(list: readonly T[]): T[] {
  const rank = (s: T) => (s.error !== null ? 0 : s.enabled ? 1 : 2)
  const when = (s: T) => (s.next ? Date.parse(s.next) : Number.POSITIVE_INFINITY)
  return [...list].sort((a, b) => rank(a) - rank(b) || when(a) - when(b) || a.name.localeCompare(b.name))
}

export const SECTIONS = ['overview', 'prompt', 'history', 'danger'] as const
export type Section = (typeof SECTIONS)[number]
export const isSection = (s: string): s is Section => (SECTIONS as readonly string[]).includes(s)

/** Settings › Schedules moved here (Sky, 2026-09-25): where a legacy `/settings/<tab>` lands, or `null` when it is still a settings tab. */
export function movedSettingsTab(tab: string): '/routines' | null {
  return tab === 'schedules' ? '/routines' : null
}
