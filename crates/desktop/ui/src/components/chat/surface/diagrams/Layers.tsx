// 123 §5.2(c) (W3): `Layers` — one full-width band per layer, items as chips after the label, `note` right-aligned.
// `LayersSvg` is the pure drawing over `layersLayout()` (snapshot-tested); `LayersView` wraps it in `DiagramFrame`.
// `direction: bottom-up` reverses the vertical order only — unit ids stay the authored index.
import { useId, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { useThemeTokens } from '@/lib/snapshot-mode'
import { DiagramFrame } from '../../DiagramFrame'
import type { layersProps } from '../catalog'

export type LayersData = z.infer<typeof layersProps>

const M = 12
const GAP = 4
const PAD = 12
const FONT = 13
const MONO_PX = 11
const MONO = 'ui-monospace, monospace'
const CHIP_H = 20
const CHIP_PAD = 8
const CHIP_GAP = 6
/** ponytail: no DOM in layout — text widths are `chars × px × 0.6` estimates. */
const estimate = (text: string, px: number) => text.length * px * 0.6

export interface LayersBand {
  x: number
  y: number
  w: number
  h: number
  /** Chips laid left→right after the label; the last may be a `+n` overflow chip (`full` lists the dropped items). */
  chips: { x: number; w: number; text: string; full: string }[]
  /** Left edge of the note area (right-aligned note at `x + w - PAD`); chips never pass it. */
  noteStart: number
}

export interface LayersLayout {
  /** Index = authored layer index (bottom-up only changes `y`). */
  bands: LayersBand[]
  width: number
  height: number
}

export function layersLayout(d: LayersData, W = 560, top = 0): LayersLayout {
  const w = W - 2 * M
  const heights = d.layers.map((l) => (l.items && l.items.length > 0 ? 48 : 36))
  const order = d.layers.map((_, i) => i)
  if (d.direction === 'bottom-up') order.reverse()
  const ys: number[] = []
  let y = top + M
  for (const i of order) {
    ys[i] = y
    y += (heights[i] ?? 36) + GAP
  }
  const bands = d.layers.map((l, i): LayersBand => {
    const x = M
    const noteStart = l.note ? x + w - PAD - estimate(l.note, MONO_PX) - PAD : x + w - PAD
    const chipW = (t: string) => Math.ceil(estimate(t, MONO_PX) + 2 * CHIP_PAD)
    const chips: LayersBand['chips'] = []
    const items = l.items ?? []
    let cx = x + PAD + estimate(l.label, FONT) + PAD
    let k = 0
    for (; k < items.length; k++) {
      const text = truncate(items[k] ?? '', 14)
      const cw = chipW(text)
      if (cx + cw > noteStart) break
      chips.push({ x: cx, w: cw, text, full: items[k] ?? '' })
      cx += cw + CHIP_GAP
    }
    if (k < items.length) {
      // pop shown chips until the `+n` chip fits before the note
      for (;;) {
        const text = `+${items.length - k}`
        const cw = chipW(text)
        if (cx + cw <= noteStart || chips.length === 0) {
          chips.push({ x: cx, w: cw, text, full: items.slice(k).join(', ') })
          break
        }
        cx = chips.pop()!.x
        k--
      }
    }
    return { x, y: ys[i] ?? top + M, w, h: heights[i] ?? 36, chips, noteStart }
  })
  return { bands, width: W, height: Math.ceil(y - GAP + M) }
}

function tint(state: UnitState, t: DiagramTokens): { stroke: string; fill: string; fillOpacity: number; opacity: number; width: number } {
  switch (state) {
    case 'active':
      return { stroke: t.accent, fill: t.accent, fillOpacity: 0.1, opacity: 1, width: 2 }
    case 'done':
      return { stroke: t.success, fill: t.success, fillOpacity: 0.12, opacity: 1, width: 1.25 }
    case 'danger':
      return { stroke: t.danger, fill: t.danger, fillOpacity: 0.12, opacity: 1, width: 1.5 }
    case 'muted':
      return { stroke: t.border, fill: t.card, fillOpacity: 1, opacity: 0.5, width: 1 }
    default:
      return { stroke: t.border, fill: t.card, fillOpacity: 1, opacity: 1, width: 1 }
  }
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Layers arm). */
export function layersDesc(d: LayersData): string {
  return describe('Layers', d.title, d.layers.length, 'layers')
}

export function LayersSvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: LayersData; layout: LayersLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => d.layers.map((_, i) => String(i)), [d.layers])
  const u = useUnits({ ids, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const hover = u.state.hover === null ? null : d.layers[Number(u.state.hover)]
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Layers'}</title>
        <desc id={a.descId}>{layersDesc(d)}</desc>
        {d.title && (
          <text x={M} y={18} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {d.title}
          </text>
        )}
        {d.layers.map((layer, i) => {
          const b = l.bands[i]
          if (!b) return null
          const p = tint(u.paint(String(i), layer.state), t)
          const cy = b.y + b.h / 2
          return (
            <g key={i} {...u.unit(String(i), unitLabel(layer.label, layer.items?.join(', '), layer.state))} aria-description={layer.ref ?? undefined} opacity={p.opacity}>
              <rect x={b.x} y={b.y} width={b.w} height={b.h} rx={6} fill={p.fill} fillOpacity={p.fillOpacity} stroke={p.stroke} strokeWidth={p.width} />
              <text x={b.x + PAD} y={cy} dominantBaseline="central" fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
                {truncate(layer.label, 24)}
              </text>
              {b.chips.map((c) => (
                <g key={c.x}>
                  <title>{c.full}</title>
                  <rect x={c.x} y={cy - CHIP_H / 2} width={c.w} height={CHIP_H} rx={4} fill={t.muted} fillOpacity={0.6} />
                  <text x={c.x + c.w / 2} y={cy} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.foreground}>
                    {c.text}
                  </text>
                </g>
              ))}
              {layer.note && (
                <text x={b.x + b.w - PAD} y={cy} textAnchor="end" dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
                  {layer.note}
                </text>
              )}
            </g>
          )
        })}
      </svg>
      <DiagramTooltip hover={u.state.hover} root={ref.current} title={hover?.label ?? ''} rows={[{ label: 'items', value: hover?.items?.join(', ') }, { label: 'note', value: hover?.note }, { label: 'state', value: hover?.state }, { label: 'ref', value: hover?.ref }]} />
    </>
  )
}

export function LayersView({ props, highlight, onSelect }: DiagramHostProps<LayersData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const tokens = useThemeTokens()
  const layout = useMemo(() => layersLayout(props, 560, props.title ? 28 : 0), [props])
  return (
    <DiagramFrame name={`layers-${hash}`} label="layers" kind="layers-block">
      <LayersSvg data={props} layout={layout} tokens={tokens} markerId={`layers-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
