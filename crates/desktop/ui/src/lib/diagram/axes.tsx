// Design 123 §5.1 — axes as `<line>` + `<text>` ticks (mono 10 px), titles (sans 11 px), gridlines at `border` 40 %.
// Inline token colours so the export carries them (`diagram-export.ts`).
import type { DiagramTokens } from '../diagram-theme'

export const TICK_PX = 10
export const TITLE_PX = 11
export const MONO = 'ui-monospace, SFMono-Regular, Menlo, monospace'
const TICK_LEN = 4
const TICK_GAP = 3

export interface Tick {
  /** Pixel position along the axis. */
  at: number
  label: string
}

export interface AxisProps {
  orient: 'left' | 'bottom'
  /** Pixel position of the axis line: x for `left`, y for `bottom`. */
  at: number
  /** Pixel extent along the axis: `[from, to]`. */
  span: [number, number]
  ticks: readonly Tick[]
  tokens: DiagramTokens
  title?: string | null
}

/** One axis line with its ticks; a `left` axis puts labels to its left, a `bottom` axis under it. */
export function Axis({ orient, at, span, ticks, tokens: t, title }: AxisProps) {
  const [a, b] = span
  const line = orient === 'left' ? { x1: at, y1: a, x2: at, y2: b } : { x1: a, y1: at, x2: b, y2: at }
  return (
    <g aria-hidden>
      <line {...line} stroke={t.border} strokeWidth={1} shapeRendering="crispEdges" />
      {ticks.map((k, i) =>
        orient === 'left' ? (
          <g key={i}>
            <line x1={at - TICK_LEN} y1={k.at} x2={at} y2={k.at} stroke={t.border} strokeWidth={1} shapeRendering="crispEdges" />
            <text x={at - TICK_LEN - TICK_GAP} y={k.at} dy="0.32em" textAnchor="end" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
              {k.label}
            </text>
          </g>
        ) : (
          <g key={i}>
            <line x1={k.at} y1={at} x2={k.at} y2={at + TICK_LEN} stroke={t.border} strokeWidth={1} shapeRendering="crispEdges" />
            <text x={k.at} y={at + TICK_LEN + TICK_GAP + TICK_PX} textAnchor="middle" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
              {k.label}
            </text>
          </g>
        ),
      )}
      {title &&
        (orient === 'left' ? (
          <text transform={`translate(${at - 36} ${(a + b) / 2}) rotate(-90)`} textAnchor="middle" fontFamily={t.font} fontSize={TITLE_PX} fill={t.mutedForeground}>
            {title}
          </text>
        ) : (
          <text x={(a + b) / 2} y={at + 34} textAnchor="middle" fontFamily={t.font} fontSize={TITLE_PX} fill={t.mutedForeground}>
            {title}
          </text>
        ))}
    </g>
  )
}

/** Horizontal (`y` positions) or vertical (`x` positions) gridlines across `span`, `border` at 40 %. */
export function Gridlines({ orient, at, span, tokens: t }: { orient: 'horizontal' | 'vertical'; at: readonly number[]; span: [number, number]; tokens: DiagramTokens }) {
  return (
    <g aria-hidden>
      {at.map((v, i) =>
        orient === 'horizontal' ? (
          <line key={i} x1={span[0]} y1={v} x2={span[1]} y2={v} stroke={t.border} strokeOpacity={0.4} strokeWidth={1} shapeRendering="crispEdges" />
        ) : (
          <line key={i} x1={v} y1={span[0]} x2={v} y2={span[1]} stroke={t.border} strokeOpacity={0.4} strokeWidth={1} shapeRendering="crispEdges" />
        ),
      )}
    </g>
  )
}

/** Room an axis needs beside the plot: left axis labels + title, bottom axis labels + title. */
export const AXIS_INSET = { left: 48, bottom: 40, leftNoTitle: 32, bottomNoTitle: 22 }
