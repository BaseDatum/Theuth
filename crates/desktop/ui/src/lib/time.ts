/**
 * Wall-clock helpers for the transcript (73 §5): the hover timestamp on a message, the gap divider between
 * distant entries, and the `<time …/>` line the engine prepends to inputs the model sees (never shown as text).
 */
import { humanSecs } from './injected'

const DAY = 86_400_000
/** Entries this far apart get a divider between them. */
export const GAP_MS = 3_600_000

const DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** `14:03` today, `Mon 14:03` within the last week, `28 Sep 14:03` older. Local time. */
export function formatAt(ms: number, now = Date.now()): string {
  const d = new Date(ms)
  const n = new Date(now)
  const time = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  if (d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate()) return time
  if (now - ms < 7 * DAY) return `${DAYS[d.getDay()]} ${time}`
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${time}`
}

/** The full local date-time, for a `title`. */
export const fullAt = (ms: number): string => new Date(ms).toLocaleString(undefined, { dateStyle: 'full', timeStyle: 'short' })

/** `1 h 12 min later · Mon 15:15` when `next` is at least {@link GAP_MS} after `prev`; `null` otherwise. */
export function gapLabel(prev: number | undefined, next: number | undefined, now = Date.now()): string | null {
  if (prev === undefined || next === undefined || next - prev < GAP_MS) return null
  return `${humanSecs(Math.floor((next - prev) / 1000))} later · ${formatAt(next, now)}`
}

/** `rows` with a `{ key, gap }` divider before every row at least {@link GAP_MS} after the previous timed one; derived at
 * render time, never stored. */
export function withGaps<R extends { key: string }>(rows: R[], at: (row: R) => number | undefined, now = Date.now()): (R | { key: string; gap: string })[] {
  const out: (R | { key: string; gap: string })[] = []
  let prev: number | undefined
  for (const row of rows) {
    const t = at(row)
    const gap = gapLabel(prev, t, now)
    if (gap) out.push({ key: `gap:${row.key}`, gap })
    out.push(row)
    if (t !== undefined) prev = t
  }
  return out
}

/** `text` without the leading `<time …/>` stamp line the engine prepends (73 §3). */
export function stripTime(text: string): string {
  const t = text.trimStart()
  if (!t.startsWith('<time ')) return text
  const end = t.indexOf('/>')
  if (end < 0) return text
  const rest = t.slice(end + 2)
  return rest.startsWith('\n') ? rest.slice(1) : rest
}
