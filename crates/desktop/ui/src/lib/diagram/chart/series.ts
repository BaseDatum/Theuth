// 123 W2 — colour assignment for chart series and the paint of one unit in the `Graph.tsx` register: outlines,
// fills as 0.12 tints, `active` the one accent, `muted` dimmed to 50 %.
import type { DiagramTokens } from '../../diagram-theme'
import type { UnitState } from '../types'

/** A series' colour: its own state when it has one (done → success, danger, active → accent), else the palette. */
export function seriesColor(index: number, state: UnitState | null | undefined, t: DiagramTokens): string {
  switch (state) {
    case 'active':
      return t.accent
    case 'done':
      return t.success
    case 'danger':
      return t.danger
    case 'muted':
      return t.mutedForeground
    default:
      return t.series[index % t.series.length]!
  }
}

export interface UnitPaint {
  stroke: string
  fill: string
  fillOpacity: number
  strokeWidth: number
  opacity: number
}

/** Stroke, tint and opacity for a unit painted `state` whose base (series) colour is `base`. */
export function unitPaint(state: UnitState, base: string, t: DiagramTokens): UnitPaint {
  switch (state) {
    case 'active':
      return { stroke: t.accent, fill: t.accent, fillOpacity: 0.12, strokeWidth: 2, opacity: 1 }
    case 'done':
      return { stroke: t.success, fill: t.success, fillOpacity: 0.12, strokeWidth: 1.25, opacity: 1 }
    case 'danger':
      return { stroke: t.danger, fill: t.danger, fillOpacity: 0.12, strokeWidth: 1.5, opacity: 1 }
    case 'muted':
      return { stroke: base, fill: base, fillOpacity: 0.12, strokeWidth: 1, opacity: 0.5 }
    default:
      return { stroke: base, fill: base, fillOpacity: 0.12, strokeWidth: 1.25, opacity: 1 }
  }
}

/** Font sizes of the register: sans 600 names, mono values. */
export const NAME_PX = 13
export const VALUE_PX = 11
