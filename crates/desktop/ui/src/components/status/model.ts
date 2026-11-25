// 217 §4–§5: what the status dot says. Connection and needs-you only; a notice's kind never colours it.

import type { ConnState } from '@/rpc/client'
import type { NoticeKind } from '@/store/notices'

export type Tone = 'ok' | 'attn' | 'idle' | 'down'
export type Dot = { tone: Tone; pulse: boolean }

export function dotOf(state: ConnState, needsYou: number, rate: number): Dot {
  if (state === 'open') return needsYou > 0 ? { tone: 'attn', pulse: false } : { tone: 'ok', pulse: rate > 0 }
  if (state === 'connecting') return { tone: 'idle', pulse: true }
  return { tone: 'down', pulse: false }
}

export const DOT_BG: Record<Tone, string> = {
  ok: 'bg-success',
  attn: 'bg-warning',
  idle: 'bg-fg-tertiary',
  down: 'bg-danger',
}

export const isDown = (state: ConnState) => state === 'reconnecting' || state === 'closed'

/** A frame received this recently proves the socket, whatever the last state event said. */
export const LIVE_MS = 5_000

/**
 * 217 §4: the state the bar shows, from evidence as well as events. Anything received in the last `LIVE_MS` reads as
 * `open`; so disconnected/reconnecting shows only when the client says so and the line has been silent that long.
 */
export const liveState = (state: ConnState, lastRx: number, now: number): ConnState => (now - lastRx < LIVE_MS ? 'open' : state)

export const KIND_TAG: Record<NoticeKind, { label: string; className: string }> = {
  info: { label: 'info', className: 'bg-layer-2 text-fg-secondary' },
  done: { label: 'done', className: 'bg-success-bg text-success' },
  warn: { label: 'warn', className: 'bg-warning-bg text-warning' },
  error: { label: 'error', className: 'bg-danger-bg text-danger' },
  progress: { label: 'working', className: 'bg-layer-2 text-fg-secondary' },
}

/** Whole seconds until `at`, never below zero. */
export const secondsUntil = (at: number, now: number) => Math.max(0, Math.ceil((at - now) / 1000))

/** The sparkline's polyline points in a `w`×`h` box, the busiest second at the top. */
export function sparkPoints(buckets: readonly number[], w: number, h: number): string {
  const max = Math.max(1, ...buckets)
  const step = buckets.length > 1 ? w / (buckets.length - 1) : 0
  return buckets.map((v, i) => `${(i * step).toFixed(1)},${(h - 1 - (v / max) * (h - 2)).toFixed(1)}`).join(' ')
}

export const SPARK_FULL_W = 184
export const SPARK_COMPACT_W = 46
export const SPARK_H = 14

/** Light 1-2-1 smoothing pass preserving endpoints so jagged 1 s spikes read smoothly without erasing bursts. */
export function smoothBuckets(buckets: readonly number[], passes = 1): number[] {
  let cur = [...buckets]
  for (let p = 0; p < passes; p++) {
    const next = [...cur]
    for (let i = 1; i < cur.length - 1; i++) {
      next[i] = cur[i - 1]! * 0.22 + cur[i]! * 0.56 + cur[i + 1]! * 0.22
    }
    cur = next
  }
  return cur
}

export type SparkSnap = {
  /** Seconds elapsed since the tension state changed. */
  age: number
  /** `-1` snapping back on notice arrival, `+1` stretching taut on notice clear, `0` resting. */
  dir: -1 | 0 | 1
}

/**
 * Smoothed cubic-Bezier SVG paths (`line` + bottom-closed `fill`) for the status bar sparkline.
 * When `snap` is active (C · Plucked Cord), the released left end (u = 0, next to the notice slot) whips inward ahead
 * of the right anchor (u = 1, next to `ev/s`) while a damped transverse wave rings down the cord and settles flat.
 */
export function sparkPath(buckets: readonly number[], w: number, h: number, snap?: SparkSnap): { line: string; fill: string } {
  if (!buckets.length) return { line: '', fill: '' }
  const vals = smoothBuckets(buckets, 1)
  const max = Math.max(1, ...vals)
  const n = vals.length
  const energy = !snap || snap.dir === 0 ? 0 : snap.dir === -1 ? Math.exp(-snap.age * 5.8) : Math.exp(-snap.age * 7.5) * 0.25

  const pts: { x: number; y: number }[] = []
  for (let i = 0; i < n; i++) {
    const u = n > 1 ? i / (n - 1) : 1
    let x = u * w
    let y = h - 1.5 - (vals[i]! / max) * (h - 3)

    if (energy > 0.008 && snap) {
      const bunch = snap.dir === -1 ? Math.sin(Math.PI * (1 - u)) * Math.max(0, 1 - snap.age * 4.2) * 14 * (1 - u) : 0
      x = Math.min(w, x + bunch)
      const env = Math.sin(Math.PI * Math.pow(1 - u, 0.75)) * (1 - u * 0.85)
      y += Math.sin(snap.age * 46 - u * 7.5) * env * energy * 3.4
    }

    pts.push({ x, y: Math.max(1.2, Math.min(h - 1.2, y)) })
  }

  if (pts.length === 1) {
    const only = `M ${pts[0]!.x.toFixed(1)} ${pts[0]!.y.toFixed(1)}`
    return { line: only, fill: '' }
  }

  const tension = 0.18
  let line = `M ${pts[0]!.x.toFixed(1)} ${pts[0]!.y.toFixed(1)}`
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)]!
    const p1 = pts[i]!
    const p2 = pts[i + 1]!
    const p3 = pts[Math.min(pts.length - 1, i + 2)]!
    const cp1x = p1.x + (p2.x - p0.x) * tension
    const cp1y = Math.max(1.2, Math.min(h - 1.2, p1.y + (p2.y - p0.y) * tension))
    const cp2x = p2.x - (p3.x - p1.x) * tension
    const cp2y = Math.max(1.2, Math.min(h - 1.2, p2.y - (p3.y - p1.y) * tension))
    line += ` C ${cp1x.toFixed(1)} ${cp1y.toFixed(1)}, ${cp2x.toFixed(1)} ${cp2y.toFixed(1)}, ${p2.x.toFixed(1)} ${p2.y.toFixed(1)}`
  }
  const fill = `${line} L ${pts[pts.length - 1]!.x.toFixed(1)} ${h} L ${pts[0]!.x.toFixed(1)} ${h} Z`
  return { line, fill }
}
