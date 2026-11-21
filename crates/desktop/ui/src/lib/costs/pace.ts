// The pace of a subscription usage window: where usage is now and where it lands at the reset. Pure; the Costs page's
// pace tiles draw it. With enough recorded readings the projection follows the recent rate (a burst shows quickly);
// otherwise it is the straight line from the window start through the current reading.

import type { UsageSample, UsageWindow } from '@/rpc/bindings/method.agent.output'

export type PaceMode = 'history' | 'fallback'

/** One point of the usage line: time (ms) and used fraction. */
export type PacePoint = { t: number; v: number }

export type Pace = {
  /** Window start (ms): `resets_at_ms - window_secs * 1000`. */
  start: number
  /** Reset (ms). */
  reset: number
  /** `now` clamped into the window (ms): where the usage line ends and the projection starts. */
  at: number
  /** Share of the window's time gone, 0…1. */
  elapsed: number
  /** Current used fraction (may exceed 1). */
  used: number
  /** Used fraction expected at the reset; `null` too early in the window to tell. */
  projected: number | null
  mode: PaceMode
  /** When the projection crosses 100 % (ms); `null` when it does not, or usage is already at/over 100 %. */
  hitsAt: number | null
  /** The usage line, oldest first, ending at (`at`, used). */
  line: PacePoint[]
}

/** History needs this many readings… */
const MIN_SAMPLES = 3
/** …spanning at least this share of the window. */
const MIN_SPAN = 0.1
/** The recent rate is measured over this trailing share of the window. */
const RECENT = 0.2
/** The straight-line fallback needs this much of the window gone. */
const MIN_ELAPSED = 0.02

/** The recent rate (fraction per ms) from recorded readings, or `null` when they are too few or too close together. */
export function recentRate(history: readonly UsageSample[], windowMs: number, now: number): number | null {
  if (history.length < MIN_SAMPLES) return null
  const first = history[0]!
  const newest = history[history.length - 1]!
  if (newest.at_ms - first.at_ms < MIN_SPAN * windowMs) return null
  const since = now - RECENT * windowMs
  const inSpan = history.filter((s) => s.at_ms >= since)
  const anchor = inSpan.length >= 2 ? inSpan[0]! : history[history.length - 2]!
  const dt = newest.at_ms - anchor.at_ms
  if (dt <= 0) return null
  // Usage within a window never goes down; a dip is a rounding artefact, not negative use.
  return Math.max(0, (newest.used_fraction - anchor.used_fraction) / dt)
}

/** The pace of `w` at `now`; `null` without a reset time or a window length (no chart then). */
export function pace(w: Pick<UsageWindow, 'used_fraction' | 'resets_at_ms' | 'window_secs'> & { history?: readonly UsageSample[] | null }, now: number): Pace | null {
  if (w.resets_at_ms == null || w.window_secs == null || w.window_secs <= 0) return null
  const reset = w.resets_at_ms
  const windowMs = w.window_secs * 1000
  const start = reset - windowMs
  const elapsed = Math.min(1, Math.max(0, (now - start) / windowMs))
  const at = Math.min(reset, Math.max(start, now))
  const used = w.used_fraction
  const history = (w.history ?? []).filter((s) => s.at_ms >= start && s.at_ms <= reset)

  const rate = recentRate(history, windowMs, at)
  let projected: number | null
  let mode: PaceMode
  let line: PacePoint[]
  if (rate != null) {
    mode = 'history'
    projected = used + rate * (reset - at)
    line = history.map((s) => ({ t: s.at_ms, v: s.used_fraction }))
    if (line[line.length - 1]!.t < at) line.push({ t: at, v: used })
  } else {
    mode = 'fallback'
    projected = elapsed >= MIN_ELAPSED ? used / elapsed : null
    line = [
      { t: start, v: 0 },
      { t: at, v: used },
    ]
  }
  const hitsAt = projected != null && projected > 1 && used < 1 ? at + ((1 - used) / (projected - used)) * (reset - at) : null
  return { start, reset, at, elapsed, used, projected, mode, hitsAt, line }
}
