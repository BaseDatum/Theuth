// Design 123 §5.1, §5.4 #1 — orthogonal connectors: bend points → one `d` with r=8 rounded elbows; straight only
// when the ends share x or y. Markers by `arrow`.
import type { Point } from './types'

/** Elbow radius of every orthogonal connector (§5.4 #1). */
export const ELBOW_R = 8

/** `points` as an SVG path, each interior bend rounded by `r` (shrunk when a segment is shorter than `2r`). */
export function orthogonalPath(points: readonly Point[], r = ELBOW_R): string {
  if (points.length === 0) return ''
  const out = [`M${fmt(points[0]!.x)} ${fmt(points[0]!.y)}`]
  for (let i = 1; i < points.length - 1; i++) {
    const a = points[i - 1]!
    const b = points[i]!
    const c = points[i + 1]!
    const inLen = Math.hypot(b.x - a.x, b.y - a.y)
    const outLen = Math.hypot(c.x - b.x, c.y - b.y)
    const rr = Math.min(r, inLen / 2, outLen / 2)
    if (rr <= 0 || inLen === 0 || outLen === 0) {
      out.push(`L${fmt(b.x)} ${fmt(b.y)}`)
      continue
    }
    const p1 = { x: b.x - ((b.x - a.x) / inLen) * rr, y: b.y - ((b.y - a.y) / inLen) * rr }
    const p2 = { x: b.x + ((c.x - b.x) / outLen) * rr, y: b.y + ((c.y - b.y) / outLen) * rr }
    out.push(`L${fmt(p1.x)} ${fmt(p1.y)}`, `Q${fmt(b.x)} ${fmt(b.y)} ${fmt(p2.x)} ${fmt(p2.y)}`)
  }
  if (points.length > 1) {
    const last = points[points.length - 1]!
    out.push(`L${fmt(last.x)} ${fmt(last.y)}`)
  }
  return out.join(' ')
}

/** ELK sections (`start`, `bends`, `end`) → one rounded path. */
export function sectionsPath(sections: readonly { start: Point; bends: readonly Point[]; end: Point }[], r = ELBOW_R): string {
  return sections.map((s) => orthogonalPath([s.start, ...s.bends, s.end], r)).join(' ')
}

/** Trim to two decimals so the same layout always yields the same `d` (§5.3 #9). */
function fmt(n: number): string {
  return Number.isInteger(n) ? String(n) : n.toFixed(2).replace(/\.?0+$/, '')
}

export type Arrow = 'forward' | 'both' | 'none'

/** `markerStart` / `markerEnd` attributes for `arrow` given the marker id (`null` = no arrowheads). */
export function markers(arrow: Arrow | null | undefined, markerId: string | null): { markerStart?: string; markerEnd?: string } {
  if (markerId === null || arrow === 'none') return {}
  const url = `url(#${markerId})`
  return arrow === 'both' ? { markerStart: url, markerEnd: url } : { markerEnd: url }
}

/** The point `t` (0..1) of the way along the polyline, for a label or a bridge. */
export function pointAt(points: readonly Point[], t: number): Point {
  if (points.length === 0) return { x: 0, y: 0 }
  const lens: number[] = []
  let total = 0
  for (let i = 1; i < points.length; i++) {
    const l = Math.hypot(points[i]!.x - points[i - 1]!.x, points[i]!.y - points[i - 1]!.y)
    lens.push(l)
    total += l
  }
  let want = Math.min(1, Math.max(0, t)) * total
  for (let i = 1; i < points.length; i++) {
    const l = lens[i - 1]!
    if (want <= l || i === points.length - 1) {
      const k = l === 0 ? 0 : want / l
      return { x: points[i - 1]!.x + (points[i]!.x - points[i - 1]!.x) * k, y: points[i - 1]!.y + (points[i]!.y - points[i - 1]!.y) * k }
    }
    want -= l
  }
  return points[points.length - 1]!
}
