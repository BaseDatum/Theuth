// 123 §5.2(b) (W3): `Journey` — a stage × row grid, an optional mood polyline row and pain markers under their stage.
// `JourneySvg` is the pure drawing over `journeyLayout()` (snapshot-tested); `JourneyView` wraps it in `DiagramFrame`.
// Units are the stage columns (id = stage index); the whole column is the hit area.
import { useId, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { useThemeTokens } from '@/lib/snapshot-mode'
import { DiagramFrame } from '../../DiagramFrame'
import type { journeyProps } from '../catalog'

export type JourneyData = z.infer<typeof journeyProps>

const M = 12
const LABEL_W = 88
const HEADER_H = 32
const ROW_H = 40
const MOOD_H = 56
const MOOD_AMP = 24
const PAIN_H = 24
const FONT = 13
const CELL_PX = 12
const MONO_PX = 11
const MONO = 'ui-monospace, monospace'

export interface JourneyLayout {
  /** Grid top (below the title) and the left edge of the first stage column. */
  top: number
  x0: number
  colW: number
  /** Row tops, index = row index (`rows` past the fixture are clamped by the renderer). */
  rowsY: number[]
  /** Mood row: baseline `mid`, one point per stage that has a mood (clamped to −1..1). */
  mood: { y: number; mid: number; points: { x: number; y: number }[] } | null
  /** Pains with a valid stage, one 24 px line each, `x` = the stage centre. */
  pains: { x: number; y: number; stage: number; text: string }[]
  /** Bottom of the grid (rows + mood), where the column hit areas end. */
  bottom: number
  /** Characters of a cell that fit the column (≤ 22). */
  cellChars: number
  width: number
  height: number
}

export function journeyLayout(d: JourneyData, W = 640, top = 0): JourneyLayout {
  const n = d.stages.length
  const x0 = M + LABEL_W
  const colW = (W - 2 * M - LABEL_W) / Math.max(1, n)
  const cx = (i: number) => x0 + colW * (i + 0.5)
  const gridTop = top + M
  let y = gridTop + HEADER_H
  const rowsY = d.rows.map(() => {
    const at = y
    y += ROW_H
    return at
  })
  let mood: JourneyLayout['mood'] = null
  if (d.mood) {
    const mid = y + MOOD_H / 2
    mood = { y, mid, points: d.mood.slice(0, n).map((m, i) => ({ x: cx(i), y: mid - Math.max(-1, Math.min(1, m)) * MOOD_AMP })) }
    y += MOOD_H
  }
  const bottom = y
  const pains = (d.pains ?? [])
    .filter((p) => p.stage < n)
    .map((p) => {
      const at = y
      y += PAIN_H
      return { x: cx(p.stage), y: at, stage: p.stage, text: p.text }
    })
  // ponytail: no DOM in layout — 12 px cell text is ~7.2 px per character.
  const cellChars = Math.max(1, Math.min(22, Math.floor((colW - 16) / 7.2)))
  return { top: gridTop, x0, colW, rowsY, mood, pains, bottom, cellChars, width: W, height: Math.ceil(y + M) }
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Journey arm). */
export function journeyDesc(d: JourneyData): string {
  return describe('Journey', d.title, d.stages.length, 'stages')
}

export function JourneySvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: JourneyData; layout: JourneyLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => d.stages.map((_, i) => String(i)), [d.stages])
  const u = useUnits({ ids, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const n = d.stages.length
  const right = l.x0 + l.colW * n
  const h = u.state.hover === null ? -1 : Number(u.state.hover)
  const hoverRows = h < 0 ? [] : [...d.rows.map((r) => ({ label: r.label, value: r.cells[h] })), { label: 'mood', value: d.mood?.[h] === undefined ? undefined : String(d.mood[h]) }, { label: 'pains', value: l.pains.filter((p) => p.stage === h).map((p) => p.text).join('; ') || undefined }]
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Journey'}</title>
        <desc id={a.descId}>{journeyDesc(d)}</desc>
        {d.title && (
          <text x={M} y={18} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {d.title}
          </text>
        )}
        {/* bands: active column tints */}
        {d.stages.map((_, i) => (u.paint(String(i)) === 'active' ? <rect key={i} x={l.x0 + l.colW * i} y={l.top} width={l.colW} height={l.bottom - l.top} fill={t.accent} fillOpacity={0.08} stroke={t.accent} strokeWidth={2} /> : null))}
        {/* grid */}
        <line x1={M} y1={l.top + HEADER_H} x2={right} y2={l.top + HEADER_H} stroke={t.border} />
        {l.rowsY.slice(0, d.rows.length).map((y) => (
          <line key={y} x1={M} y1={y + ROW_H} x2={right} y2={y + ROW_H} stroke={t.border} />
        ))}
        {d.stages.map((_, i) => (
          <line key={i} x1={l.x0 + l.colW * i} y1={l.top} x2={l.x0 + l.colW * i} y2={l.bottom} stroke={t.border} />
        ))}
        {/* mood row */}
        {l.mood && (
          <g>
            <text x={M} y={l.mood.mid} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
              mood
            </text>
            <line x1={l.x0} y1={l.mood.mid} x2={right} y2={l.mood.mid} stroke={t.border} strokeDasharray="2 4" />
            {l.mood.points.length > 1 && <polyline points={l.mood.points.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ')} fill="none" stroke={t.accent} strokeWidth={1.5} />}
            {l.mood.points.map((p, i) => (
              <circle key={i} cx={p.x} cy={p.y} r={2} fill={t.accent} />
            ))}
          </g>
        )}
        {/* pains */}
        {l.pains.map((p, i) => (
          <g key={i}>
            {i === 0 && (
              <text x={M} y={p.y + PAIN_H / 2} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
                pains
              </text>
            )}
            <path d={`M${p.x - 5} ${p.y + 16} L${p.x} ${p.y + 8} L${p.x + 5} ${p.y + 16} z`} fill={t.danger} />
            <text x={p.x + 10} y={p.y + PAIN_H / 2} dominantBaseline="central" fontFamily={t.font} fontSize={MONO_PX} fill={t.foreground}>
              {truncate(p.text, 40)}
            </text>
          </g>
        ))}
        {/* row labels */}
        {d.rows.map((r, i) => {
          const y = l.rowsY[i]
          if (y === undefined) return null
          return (
            <text key={i} x={M} y={y + ROW_H / 2} dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.mutedForeground}>
              {truncate(r.label, 12)}
            </text>
          )
        })}
        {/* units: stage columns */}
        {d.stages.map((stage, i) => {
          const x = l.x0 + l.colW * i
          const cells = d.rows.map((r) => r.cells[i])
          return (
            <g key={i} {...u.unit(String(i), unitLabel(stage, cells.filter(Boolean).join(' · ')))} opacity={u.paint(String(i)) === 'muted' ? 0.5 : 1}>
              <rect x={x} y={l.top} width={l.colW} height={(l.pains.length ? l.pains[l.pains.length - 1]!.y + PAIN_H : l.bottom) - l.top} fill="transparent" />
              <text x={x + l.colW / 2} y={l.top + HEADER_H / 2} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
                {truncate(stage, 16)}
              </text>
              {cells.map((c, r) => {
                const y = l.rowsY[r]
                if (!c || y === undefined) return null
                return (
                  <text key={r} x={x + 8} y={y + ROW_H / 2} dominantBaseline="central" fontFamily={t.font} fontSize={CELL_PX} fill={t.foreground}>
                    {truncate(c, l.cellChars)}
                  </text>
                )
              })}
            </g>
          )
        })}
      </svg>
      <DiagramTooltip hover={u.state.hover} root={ref.current} title={h < 0 ? '' : (d.stages[h] ?? '')} rows={hoverRows} />
    </>
  )
}

export function JourneyView({ props, highlight, onSelect }: DiagramHostProps<JourneyData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const tokens = useThemeTokens()
  const layout = useMemo(() => journeyLayout(props, 640, props.title ? 28 : 0), [props])
  return (
    <DiagramFrame name={`journey-${hash}`} label="journey" kind="journey-block">
      <JourneySvg data={props} layout={layout} tokens={tokens} markerId={`journey-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
