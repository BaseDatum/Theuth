// §8.6: perfect-freehand is the one renderer of committed ink (and of the stroke in flight on the pointer path).
import { getStroke, type StrokeOptions } from 'perfect-freehand'
import type { InkTool, Stroke } from '../canvas/model'
import { unpack, type Pt } from './pack'

export { inkCss } from './colour'

export const DEFAULT_WIDTH: Record<InkTool, number> = { monoline: 2.4, pen: 2.8, marker: 6 }
/** Opacity of the marker tool (multiplies the life opacity). */
export const MARKER_OPACITY = 0.45

export function freehandOptions(tool: InkTool, width: number, hasPressure: boolean, last = true): StrokeOptions {
  switch (tool) {
    case 'monoline': return { size: width, thinning: 0, smoothing: 0.5, streamline: 0.5, simulatePressure: false, last }
    case 'pen': return { size: width, thinning: 0.6, smoothing: 0.5, streamline: 0.45, simulatePressure: !hasPressure, last }
    case 'marker': return { size: width * 3, thinning: 0, smoothing: 0.6, streamline: 0.5, simulatePressure: false, last }
  }
}

const fmt = (n: number) => (Math.round(n * 100) / 100).toString()

/** perfect-freehand outline → a closed SVG path with quadratic midpoints. */
export function outlinePath(outline: number[][]): string {
  const n = outline.length
  if (n < 2) return ''
  const p = (i: number) => outline[i % n]!
  let d = `M${fmt(p(0)[0]!)},${fmt(p(0)[1]!)}Q`
  for (let i = 0; i < n; i++) {
    const [x0, y0] = p(i) as [number, number]
    const [x1, y1] = p(i + 1) as [number, number]
    d += `${fmt(x0)},${fmt(y0)} ${fmt((x0 + x1) / 2)},${fmt((y0 + y1) / 2)} `
  }
  return `${d.trimEnd()}Z`
}

export function pointsPath(pts: readonly Pt[], tool: InkTool, width: number, hasPressure: boolean, last = true): string {
  if (pts.length === 0) return ''
  const input = pts.map(([x, y, p]) => [x, y, p])
  return outlinePath(getStroke(input, freehandOptions(tool, width, hasPressure, last)))
}

const cache = new WeakMap<Uint8Array, string>()

/** The SVG path `d` of a committed stroke, in the stroke's space (world for free strokes, card-local for anchored ones). */
export function strokePath(stroke: Stroke): string {
  const hit = cache.get(stroke.pts)
  if (hit !== undefined) return hit
  const d = pointsPath(unpack(stroke.o, stroke.pts), stroke.tool, stroke.width, stroke.p === 1)
  cache.set(stroke.pts, d)
  return d
}
