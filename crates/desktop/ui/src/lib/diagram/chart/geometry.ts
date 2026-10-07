// 123 W2 — pure geometry for the chart family on top of the kernel scales: running totals, line/area/step paths,
// the beeswarm sweep, polar coordinates, the Gantt time span, the quadrant cell. Deterministic (§5.3 #9).
import { parseIso } from '../scale'
import type { Point } from '../types'

export interface Rect {
  x: number
  y: number
  w: number
  h: number
}

/** `lo..hi` of `values`, `[0, 1]` when empty, 0 always included so bars have a baseline. */
export function extent(values: readonly number[], includeZero = true): [number, number] {
  let lo = includeZero ? 0 : Number.POSITIVE_INFINITY
  let hi = includeZero ? 0 : Number.NEGATIVE_INFINITY
  for (const v of values) {
    if (!Number.isFinite(v)) continue
    if (v < lo) lo = v
    if (v > hi) hi = v
  }
  if (lo === Number.POSITIVE_INFINITY) return [0, 1]
  return [lo, hi]
}

/** The level after each `Waterfall` step: deltas add, `total`/`subtotal` set it — `running_total` in `charts.rs`. */
export function runningTotal(steps: readonly { value: number; kind?: 'delta' | 'total' | 'subtotal' | null }[]): number[] {
  let cur = 0
  return steps.map((s) => {
    if (s.kind === 'total' || s.kind === 'subtotal') cur = s.value
    else cur += s.value
    return cur
  })
}

/** `M x y L …` through `points`; `step` draws horizontal runs with vertical jumps. */
export function linePath(points: readonly Point[], step = false): string {
  let d = ''
  points.forEach((p, i) => {
    if (i === 0) d = `M${r(p.x)} ${r(p.y)}`
    else if (step) d += ` H${r(p.x)} V${r(p.y)}`
    else d += ` L${r(p.x)} ${r(p.y)}`
  })
  return d
}

/** The line path closed down to `baseY` for an area fill. */
export function areaPath(points: readonly Point[], baseY: number, step = false): string {
  if (points.length === 0) return ''
  const first = points[0]!
  const last = points[points.length - 1]!
  return `${linePath(points, step)} L${r(last.x)} ${r(baseY)} L${r(first.x)} ${r(baseY)} Z`
}

const r = (v: number) => Math.round(v * 100) / 100

/**
 * Beeswarm: dots of radius `radius` at pixel positions `xs` dodged along the other axis so none overlap — a
 * deterministic 1-D sweep in input order (§5.2 b). Returns the offset of each dot from the centre line.
 */
export function beeswarm(xs: readonly number[], radius: number): number[] {
  const placed: { x: number; y: number }[] = []
  const out: number[] = []
  const d = radius * 2
  for (const x of xs) {
    const candidates = [0]
    for (const p of placed) {
      const dx = Math.abs(p.x - x)
      if (dx < d) {
        const dy = Math.sqrt(d * d - dx * dx)
        candidates.push(p.y + dy, p.y - dy)
      }
    }
    candidates.sort((a, b) => Math.abs(a) - Math.abs(b) || a - b)
    const y = candidates.find((c) => placed.every((p) => (p.x - x) ** 2 + (p.y - c) ** 2 >= d * d - 1e-6)) ?? 0
    placed.push({ x, y })
    out.push(y)
  }
  return out
}

/** Point at `fraction` of the radius on spoke `i` of `n`, clockwise from the top. */
export function polar(cx: number, cy: number, radius: number, i: number, n: number, fraction = 1): Point {
  const a = -Math.PI / 2 + (2 * Math.PI * i) / n
  return { x: r(cx + radius * fraction * Math.cos(a)), y: r(cy + radius * fraction * Math.sin(a)) }
}

/** SVG `d` of the ring sector from spoke `i` to `i + 1` (minus a gap) out to `fraction` of the radius. */
export function sector(cx: number, cy: number, radius: number, i: number, n: number, fraction: number, gap = 0.08): string {
  const a0 = -Math.PI / 2 + (2 * Math.PI * (i + gap / 2)) / n
  const a1 = -Math.PI / 2 + (2 * Math.PI * (i + 1 - gap / 2)) / n
  const rr = radius * fraction
  const p0 = { x: r(cx + rr * Math.cos(a0)), y: r(cy + rr * Math.sin(a0)) }
  const p1 = { x: r(cx + rr * Math.cos(a1)), y: r(cy + rr * Math.sin(a1)) }
  const large = a1 - a0 > Math.PI ? 1 : 0
  return `M${r(cx)} ${r(cy)} L${p0.x} ${p0.y} A${r(rr)} ${r(rr)} 0 ${large} 1 ${p1.x} ${p1.y} Z`
}

/** `ScatterChart`/`Quadrant` bubble radius: area ∝ `size`, `maxR` for the largest. */
export function bubbleRadius(size: number, maxSize: number, maxR: number, minR = 4): number {
  if (!(maxSize > 0) || !(size > 0)) return minR
  return Math.max(minR, maxR * Math.sqrt(size / maxSize))
}

/** The instants a task list spans; a date-only `end` is inclusive (runs to the next midnight). */
export function timeSpan(tasks: readonly { start: string; end: string }[], extra: readonly (string | null | undefined)[] = []): [Date, Date] | null {
  let lo = Number.POSITIVE_INFINITY
  let hi = Number.NEGATIVE_INFINITY
  for (const t of tasks) {
    const s = parseIso(t.start)
    const e = endOf(t.end)
    if (!s || !e) continue
    lo = Math.min(lo, s.getTime())
    hi = Math.max(hi, e.getTime())
  }
  for (const x of extra) {
    const d = x ? parseIso(x) : null
    if (d) {
      lo = Math.min(lo, d.getTime())
      hi = Math.max(hi, d.getTime())
    }
  }
  if (!Number.isFinite(lo)) return null
  if (hi <= lo) hi = lo + 86_400_000
  return [new Date(lo), new Date(hi)]
}

/** A task's end instant: a date-only `end` runs to the next midnight — `end_of` in `charts.rs`. */
export function endOf(end: string): Date | null {
  const d = parseIso(end)
  return d && end.length === 10 ? new Date(d.getTime() + 86_400_000) : d
}

/** Quadrant index of an item: 0 tl, 1 tr, 2 bl, 3 br (`x`, `y` ≥ 0.5 are right / top) — `quadrant_of` in `charts.rs`. */
export function quadrantOf(x: number, y: number): 0 | 1 | 2 | 3 {
  return ((x >= 0.5 ? 1 : 0) + (y < 0.5 ? 2 : 0)) as 0 | 1 | 2 | 3
}

/** Conversion from the previous stage as a whole percent, `null` when either value is missing. */
export function conversion(values: readonly (number | null | undefined)[], i: number): number | null {
  const prev = values[i - 1]
  const cur = values[i]
  if (i === 0 || prev == null || cur == null || !(prev > 0)) return null
  return Math.round((cur / prev) * 100)
}

/** Orthogonal `from → to` with one vertical jog at `midX`: `[from, (midX, from.y), (midX, to.y), to]`. */
export function elbow(from: Point, to: Point, midX: number): Point[] {
  if (from.y === to.y) return [from, to]
  return [from, { x: midX, y: from.y }, { x: midX, y: to.y }, to]
}

/** Whether the segment `a → b` (axis-aligned) crosses the interior of `rect`. */
export function segmentCrosses(a: Point, b: Point, rect: Rect): boolean {
  const x0 = Math.min(a.x, b.x)
  const x1 = Math.max(a.x, b.x)
  const y0 = Math.min(a.y, b.y)
  const y1 = Math.max(a.y, b.y)
  return x1 > rect.x && x0 < rect.x + rect.w && y1 > rect.y && y0 < rect.y + rect.h
}
