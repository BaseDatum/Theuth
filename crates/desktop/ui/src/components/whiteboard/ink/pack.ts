// §2.6: 6 bytes a point, little-endian — int16 dx, int16 dy (⅛ px, deltas of absolute quantised offsets from `o`),
// uint8 pressure, uint8 dt (4 ms units). Plus the capture-side helpers: RDP simplification and the 4 000-point split.

/** A point in the stroke's space: x, y, pressure 0..1, t ms from the first point. */
export type Pt = [x: number, y: number, pressure: number, t: number]

export const MAX_POINTS = 4000
const STEP = 32767
/** ε of the Ramer–Douglas–Peucker pass, in screen px at capture zoom. */
export const RDP_SCREEN_PX = 0.35

/** round half away from zero */
const roundAway = (v: number) => Math.sign(v) * Math.round(Math.abs(v))

export function pack(o: readonly [number, number], pts: readonly Pt[]): Uint8Array {
  // Quantise absolutely, then insert interpolated points where a step would not fit an int16.
  const q: { x: number; y: number; p: number; t: number }[] = []
  for (const [x, y, p, t] of pts) {
    const cur = { x: roundAway((x - o[0]) * 8), y: roundAway((y - o[1]) * 8), p, t }
    const prev = q[q.length - 1]
    if (prev) {
      const k = Math.ceil(Math.max(Math.abs(cur.x - prev.x), Math.abs(cur.y - prev.y)) / STEP)
      for (let j = 1; j < k; j++) {
        const f = j / k
        q.push({
          x: prev.x + Math.round((cur.x - prev.x) * f),
          y: prev.y + Math.round((cur.y - prev.y) * f),
          p: prev.p + (cur.p - prev.p) * f,
          t: prev.t + (cur.t - prev.t) * f,
        })
      }
    }
    q.push(cur)
  }
  const out = new Uint8Array(q.length * 6)
  const view = new DataView(out.buffer)
  let px = 0
  let py = 0
  let pt = q[0]?.t ?? 0
  q.forEach((c, i) => {
    view.setInt16(i * 6, c.x - px, true)
    view.setInt16(i * 6 + 2, c.y - py, true)
    out[i * 6 + 4] = Math.round(Math.min(1, Math.max(0, c.p)) * 255)
    out[i * 6 + 5] = i === 0 ? 0 : Math.min(255, Math.max(0, Math.round((c.t - pt) / 4)))
    px = c.x
    py = c.y
    pt = c.t
  })
  return out
}

export function unpack(o: readonly [number, number], bytes: Uint8Array): Pt[] {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const n = Math.floor(bytes.length / 6)
  const out: Pt[] = []
  let qx = 0
  let qy = 0
  let t = 0
  for (let i = 0; i < n; i++) {
    qx += view.getInt16(i * 6, true)
    qy += view.getInt16(i * 6 + 2, true)
    t += 4 * bytes[i * 6 + 5]!
    out.push([o[0] + qx / 8, o[1] + qy / 8, bytes[i * 6 + 4]! / 255, t])
  }
  return out
}

/** Ramer–Douglas–Peucker on x/y; keeps pressure and time of the kept points. Iterative (no deep recursion on 4 000 points). */
export function rdp(pts: readonly Pt[], eps: number): Pt[] {
  if (pts.length < 3) return pts.slice()
  const keep = new Uint8Array(pts.length)
  keep[0] = 1
  keep[pts.length - 1] = 1
  const stack: [number, number][] = [[0, pts.length - 1]]
  while (stack.length) {
    const [a, b] = stack.pop()!
    const [ax, ay] = pts[a]!
    const [bx, by] = pts[b]!
    const dx = bx - ax
    const dy = by - ay
    const len = Math.hypot(dx, dy)
    let best = -1
    let far = 0
    for (let i = a + 1; i < b; i++) {
      const [x, y] = pts[i]!
      const d = len === 0 ? Math.hypot(x - ax, y - ay) : Math.abs(dy * x - dx * y + bx * ay - by * ax) / len
      if (d > far) {
        far = d
        best = i
      }
    }
    if (best >= 0 && far > eps) {
      keep[best] = 1
      stack.push([a, best], [best, b])
    }
  }
  return pts.filter((_, i) => keep[i] === 1)
}

/** Splits at `max` points; each next part starts on the previous part's last point (times rebased to its first point). */
export function split(pts: readonly Pt[], max = MAX_POINTS): Pt[][] {
  if (pts.length <= max) return [pts.slice()]
  const parts: Pt[][] = []
  let start = 0
  while (start < pts.length - 1) {
    const end = Math.min(pts.length, start + max)
    const t0 = pts[start]![3]
    parts.push(pts.slice(start, end).map(([x, y, p, t]) => [x, y, p, t - t0] as Pt))
    if (end === pts.length) break
    start = end - 1
  }
  return parts
}

/** The centre line's bounding box padded by `pad`. */
export function bboxOf(pts: readonly Pt[], pad: number): [number, number, number, number] {
  let x0 = Infinity
  let y0 = Infinity
  let x1 = -Infinity
  let y1 = -Infinity
  for (const [x, y] of pts) {
    if (x < x0) x0 = x
    if (y < y0) y0 = y
    if (x > x1) x1 = x
    if (y > y1) y1 = y
  }
  if (!Number.isFinite(x0)) return [0, 0, 0, 0]
  return [x0 - pad, y0 - pad, x1 - x0 + 2 * pad, y1 - y0 + 2 * pad]
}
