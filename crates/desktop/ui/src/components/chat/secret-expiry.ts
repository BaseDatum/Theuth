/**
 * The secret card's expiry control (one per card; it applies to every value stored from it): Never | In [n]
 * [minutes | hours | days] | On [a local date and time]. Resolved to absolute Unix ms when the user stores, so "In 2
 * hours" counts from then. The agent's `expires` suggestion (`30m` / `2h` / `3d`, or an RFC 3339 instant the engine
 * already resolved) only pre-fills it.
 */

export type ExpiryUnit = 'minutes' | 'hours' | 'days'

export type ExpiryChoice =
  | { mode: 'never' }
  | { mode: 'in'; amount: string; unit: ExpiryUnit }
  /** `local`: an `<input type="datetime-local">` value, `YYYY-MM-DDTHH:MM` in the user's time zone. */
  | { mode: 'on'; local: string }

export type ResolvedExpiry = { ok: true; at: number | null } | { ok: false; error: string }

export const NEVER: ExpiryChoice = { mode: 'never' }

const UNIT_MS: Record<ExpiryUnit, number> = { minutes: 60_000, hours: 3_600_000, days: 86_400_000 }
const SUFFIX: Record<string, ExpiryUnit> = { m: 'minutes', h: 'hours', d: 'days' }

/** The absolute expiry `c` names at `now`; `at: null` = never. A zero, negative or fractional amount, an unreadable
 * date or a time not after `now` is an error the card shows inline. */
export function resolveExpiry(c: ExpiryChoice, now: number): ResolvedExpiry {
  if (c.mode === 'never') return { ok: true, at: null }
  if (c.mode === 'in') {
    const n = Number(c.amount)
    if (c.amount.trim() === '' || !Number.isInteger(n)) return { ok: false, error: 'Enter a whole number' }
    if (n <= 0) return { ok: false, error: 'Enter a number above zero' }
    return { ok: true, at: now + n * UNIT_MS[c.unit] }
  }
  const at = localInputToMs(c.local)
  if (at === null) return { ok: false, error: 'Pick a date and time' }
  if (at <= now) return { ok: false, error: 'That time is in the past' }
  return { ok: true, at }
}

/** The control's starting state for the agent's suggestion; `Never` when there is none or it does not parse. */
export function choiceFromSuggestion(s: string | null | undefined): ExpiryChoice {
  if (!s) return NEVER
  const rel = /^\s*(\d+)\s*([mhd])\s*$/.exec(s)
  if (rel) return { mode: 'in', amount: rel[1] ?? '', unit: SUFFIX[rel[2] ?? 'h'] ?? 'hours' }
  const t = Date.parse(s)
  return Number.isNaN(t) ? NEVER : { mode: 'on', local: msToLocalInput(t) }
}

/** `YYYY-MM-DDTHH:MM` (the user's time zone) → Unix ms; `null` when it is not one. */
export function localInputToMs(local: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(local)
  if (!m) return null
  const [y, mo, d, h, mi] = m.slice(1).map(Number) as [number, number, number, number, number]
  const t = new Date(y, mo - 1, d, h, mi)
  return t.getFullYear() === y && t.getMonth() === mo - 1 && t.getDate() === d ? t.getTime() : null
}

/** Unix ms → `YYYY-MM-DDTHH:MM` in the user's time zone (seconds dropped). */
export function msToLocalInput(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`
}

/** `Sun 22 Nov, 14:00` in the user's locale and time zone; the year too when it is not this year's. */
export function formatExpiry(ms: number, now = Date.now(), locale?: string): string {
  const d = new Date(ms)
  const sameYear = d.getFullYear() === new Date(now).getFullYear()
  const day = d.toLocaleDateString(locale, { weekday: 'short', day: 'numeric', month: 'short', ...(sameYear ? {} : { year: 'numeric' }) })
  const time = d.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' })
  return `${day.replace(/,/g, '')}, ${time}`
}

/** For lists (`SecretInfo.expires`, RFC 3339): `expires in 40 min` / `expires in 3 h` within a day, else `expires
 * Sun 22 Nov, 14:00`; `expired` once it passed (until the engine's sweeper deleted it). */
export function describeExpiry(iso: string, now = Date.now()): string {
  const at = Date.parse(iso)
  if (Number.isNaN(at)) return `expires ${iso}`
  const left = at - now
  if (left <= 0) return 'expired'
  if (left < 3_600_000) return `expires in ${Math.ceil(left / 60_000)} min`
  if (left < 86_400_000) return `expires in ${Math.round(left / 3_600_000)} h`
  return `expires ${formatExpiry(at, now)}`
}
