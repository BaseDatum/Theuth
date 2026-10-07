// 123 W2 — what every chart component shares: the `<svg>` shell (a11y title/desc first, the drawn title, inline
// token colours, `min-width` = viewBox width inside an `overflow-x: auto` wrapper), the hooks a `<NameView>` needs
// (theme tokens, the unit contract, the legend toggle) and the `DiagramFrame` + tooltip host. The pure `<NameSvg>`s
// take a `Units` so a test renders them with `staticUnits()` and no hooks.
import { type KeyboardEvent, type ReactNode, useId, useMemo, useRef, useSyncExternalStore } from 'react'
import { type DiagramTokens, diagramTokens } from '../../diagram-theme'
import { subscribeDark } from '../../theme'
import { DiagramFrame } from '@/components/chat/DiagramFrame'
import { svgA11y } from '../a11y'
import { useLegend } from '../legend'
import { DiagramTooltip, type TooltipRow } from '../tooltip'
import type { UnitState } from '../types'
import { paintOf, UNITS_IDLE, type Units, useUnits } from '../units'
import { NAME_PX } from './series'

/** Default drawing width; charts widen for many categories. */
export const CHART_W = 560
/** Outer padding (4 px grid). */
export const PAD = 12
/** Height the drawn title takes above the plot. */
export const TITLE_H = 28
/** Minimum hit area of a unit (§5.3 #1). */
export const HIT = 18

/** Height reserved for `title` above the plot. */
export function titleH(title: string | null | undefined): number {
  return title ? TITLE_H : 0
}

/** A `Units` with no state or handlers: `highlight` ids draw `active`, everything else its own state. */
export function staticUnits(highlight?: readonly string[]): Units {
  return {
    state: UNITS_IDLE,
    paint: (id, own) => paintOf(id, own, UNITS_IDLE, [], highlight),
    unit: (id, label) => ({
      'data-unit': id,
      role: 'button',
      tabIndex: 0,
      'aria-label': label,
      'aria-pressed': false,
      style: { cursor: 'pointer' },
      onClick: () => {},
      onKeyDown: () => {},
      onPointerEnter: () => {},
      onPointerLeave: () => {},
      onFocus: () => {},
      onBlur: () => {},
    }),
    root: { onKeyDown: () => {} },
    dispatch: () => {},
  }
}

export interface ChartSvgProps {
  /** Component name (`BarChart`) — the a11y id base with `id`. */
  name: string
  /** Unique per mounted svg. */
  id: string
  title?: string | null
  /** The first line of the Rust lowering (`describe()`), the `<desc>`. */
  desc: string
  W: number
  H: number
  tokens: DiagramTokens
  interactive: boolean
  root?: { onKeyDown: (e: KeyboardEvent) => void }
  children: ReactNode
}

/** The svg shell: `<title>` first, `<desc>` second, the drawn title, then the drawing. */
export function ChartSvg({ name, id, title, desc, W, H, tokens: t, interactive, root, children }: ChartSvgProps) {
  const a = svgA11y(`${name.toLowerCase()}-${id}`, interactive)
  return (
    <div style={{ overflowX: 'auto' }}>
      <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ minWidth: W, height: 'auto', display: 'block' }} {...a.svg} {...root} data-chart={name}>
        <title id={a.titleId}>{title ?? name}</title>
        <desc id={a.descId}>{desc}</desc>
        {title && (
          <text x={PAD} y={PAD + NAME_PX} fontFamily={t.font} fontSize={NAME_PX} fontWeight={600} fill={t.foreground}>
            {title}
          </text>
        )}
        {children}
      </svg>
    </div>
  )
}

/** Theme tokens, re-read when the theme flips. */
export function useTokens(): DiagramTokens {
  const dark = useSyncExternalStore(
    subscribeDark,
    () => document.documentElement.classList.contains('dark'),
    () => false,
  )
  return useMemo(diagramTokens, [dark])
}

/** `useLegend()`'s shape (the kernel exports the hook, not the type). */
export interface LegendToggle {
  hidden: readonly string[]
  toggle: (id: string) => void
  visible: (id: string) => boolean
}

export interface ChartHooks {
  id: string
  tokens: DiagramTokens
  units: Units
  legend: LegendToggle
}

/** The hooks every `<NameView>` runs: tokens, the unit contract over `ids`, the legend toggle. */
export function useChart(opts: { ids: readonly string[]; incident?: (id: string) => readonly string[]; highlight?: readonly string[]; onSelect?: (id: string) => void }): ChartHooks {
  const id = useId().replace(/:/g, '')
  const tokens = useTokens()
  const units = useUnits(opts)
  const legend = useLegend()
  return { id, tokens, units, legend }
}

export interface Tip {
  title: string
  rows?: readonly TooltipRow[]
}

/** `DiagramFrame` around a chart, with the one tooltip for the hovered/focused unit. */
export function ChartHost({ name, id, units, tip, children }: { name: string; id: string; units: Units; tip: (id: string) => Tip | null; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  const kind = name.toLowerCase()
  const hover = units.state.hover
  const t = hover === null ? null : tip(hover)
  return (
    <DiagramFrame name={`${kind}-${id}`} label={kind} kind={`${kind}-block`}>
      <div ref={ref}>{children}</div>
      {t && <DiagramTooltip hover={hover} root={ref.current} title={t.title} rows={t.rows} />}
    </DiagramFrame>
  )
}

/** The state a unit's own `state` field contributes, `null` for absent. */
export function ownState(s: string | null | undefined): UnitState | null {
  return (s as UnitState | undefined) ?? null
}
