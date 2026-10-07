// 123 W2: `Funnel` — funnel (descending band widths by value, conversion % between stages) | pyramid (ranked
// bands, equal when no values) (design 123 §4.2, §5.2 b). Each stage is a unit whose id is its index as a string
// (the apps contract, `diagrams.ts`); the stage label sits
// left of the band in sans 600, value and conversion right of it in mono.
import { useCallback, useMemo } from 'react'
import type { z } from 'zod'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { MONO, TICK_PX } from '@/lib/diagram/axes'
import { CHART_W, ChartHost, ChartSvg, ownState, PAD, titleH, useChart } from '@/lib/diagram/chart/frame'
import { conversion } from '@/lib/diagram/chart/geometry'
import { NAME_PX, unitPaint, VALUE_PX } from '@/lib/diagram/chart/series'
import { describe, withUnit } from '@/lib/diagram/chart/text'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { funnelProps } from '../catalog'

export type FunnelData = z.infer<typeof funnelProps>

export interface FunnelBand {
  id: string
  index: number
  label: string
  value: number | null
  /** Trapezoid corners, clockwise from top-left. */
  points: string
  cx: number
  cy: number
  /** Conversion from the previous stage, whole percent. */
  pct: number | null
}

export interface FunnelLayout {
  W: number
  H: number
  bands: FunnelBand[]
  /** x where the labels end (right-aligned) and the values start. */
  labelX: number
  valueX: number
  pyramid: boolean
}

const ROW = 40
const BAND_H = 32
const LABEL_W = 120
const VALUE_W = 100

export function layoutFunnel(p: FunnelData): FunnelLayout {
  const stages = p.stages ?? []
  const pyramid = p.variant === 'pyramid'
  const values = stages.map((s) => (typeof s.value === 'number' && Number.isFinite(s.value) ? s.value : null))
  const max = Math.max(0, ...values.map((v) => v ?? 0))
  const top = PAD + titleH(p.title) + 8
  const W = CHART_W
  const H = top + stages.length * ROW + PAD
  const x0 = PAD + LABEL_W + 12
  const full = W - x0 - VALUE_W - PAD
  const cx = x0 + full / 2
  // Width fraction per stage: by value when given; pyramid ranks grow downward, funnel shrinks.
  const frac = (i: number): number => {
    const v = values[i]
    if (v != null && max > 0) return Math.max(0.08, v / max)
    const n = Math.max(1, stages.length)
    return pyramid ? (i + 1) / n : (n - i) / n
  }
  const bands: FunnelBand[] = stages.map((s, i) => {
    const y = top + i * ROW + (ROW - BAND_H) / 2
    const wTop = full * frac(i)
    const next = i + 1 < stages.length ? full * frac(i + 1) : wTop
    const wBot = pyramid ? wTop : (wTop + next) / 2
    const pts = [
      [cx - wTop / 2, y],
      [cx + wTop / 2, y],
      [cx + wBot / 2, y + BAND_H],
      [cx - wBot / 2, y + BAND_H],
    ]
    return { id: String(i), index: i, label: s.label, value: values[i]!, points: pts.map(([x, yy]) => `${r(x!)},${r(yy!)}`).join(' '), cx, cy: y + BAND_H / 2, pct: conversion(values, i) }
  })
  return { W, H, bands, labelX: PAD + LABEL_W, valueX: W - VALUE_W - PAD + 8, pyramid }
}

const r = (v: number) => Math.round(v * 100) / 100

export function FunnelSvg({ props: p, layout: l, tokens: t, units, id }: { props: FunnelData; layout: FunnelLayout; tokens: DiagramTokens; units: Units; id: string }) {
  const desc = describe('Funnel', p.title, p.stages?.length ?? 0, 'stage')
  const unit = p.unit ?? undefined
  return (
    <ChartSvg name="Funnel" id={id} title={p.title} desc={desc} W={l.W} H={l.H} tokens={t} interactive root={units.root}>
      {l.bands.map((b) => {
        const stage = p.stages?.[b.index]
        const s = unitPaint(units.paint(b.id, ownState(stage?.state)), t.series[b.index % t.series.length]!, t)
        const value = b.value !== null ? withUnit(b.value, unit) : null
        const aria = [b.label, value, b.pct !== null ? `${b.pct}% of previous` : null, stage?.note].filter(Boolean).join(', ')
        return (
          <g key={b.id} opacity={s.opacity} {...units.unit(b.id, aria)}>
            <rect x={PAD} y={b.cy - ROW / 2} width={l.W - 2 * PAD} height={ROW} fill="transparent" />
            <polygon points={b.points} fill={s.fill} fillOpacity={s.fillOpacity} stroke={s.stroke} strokeWidth={s.strokeWidth} strokeLinejoin="round" />
            <text x={l.labelX} y={b.cy} textAnchor="end" dominantBaseline="central" fontFamily={t.font} fontSize={NAME_PX} fontWeight={600} fill={t.foreground}>
              {b.label}
            </text>
            {value && (
              <text x={l.valueX} y={b.pct !== null || stage?.note ? b.cy - 6 : b.cy} dominantBaseline="central" fontFamily={MONO} fontSize={VALUE_PX} fill={t.foreground}>
                {value}
              </text>
            )}
            {(b.pct !== null || stage?.note) && (
              <text x={l.valueX} y={value ? b.cy + 7 : b.cy} dominantBaseline="central" fontFamily={MONO} fontSize={TICK_PX} fill={t.mutedForeground}>
                {[b.pct !== null ? `${b.pct}%` : null, stage?.note].filter(Boolean).join(' · ')}
              </text>
            )}
          </g>
        )
      })}
    </ChartSvg>
  )
}

export function FunnelView({ props, highlight, onSelect }: DiagramHostProps<FunnelData>) {
  const layout = useMemo(() => layoutFunnel(props), [props])
  const ids = useMemo(() => layout.bands.map((b) => b.id), [layout])
  const c = useChart({ ids, highlight, onSelect })
  const tip = useCallback(
    (id: string) => {
      const b = layout.bands.find((x) => x.id === id)
      if (!b) return null
      return {
        title: b.label,
        rows: [
          { label: 'value', value: b.value !== null ? withUnit(b.value, props.unit ?? undefined) : null },
          { label: 'conversion', value: b.pct !== null ? `${b.pct}%` : null },
          { label: 'note', value: props.stages?.[b.index]?.note },
        ],
      }
    },
    [layout, props],
  )
  return (
    <ChartHost name="Funnel" id={c.id} units={c.units} tip={tip}>
      <FunnelSvg props={props} layout={layout} tokens={c.tokens} units={c.units} id={c.id} />
    </ChartHost>
  )
}
