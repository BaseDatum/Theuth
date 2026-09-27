// Design 71 §2.4 / M14: actors as lanes, one addressable step per message. Geometry is Whiteboard's (R §B.5): lanes
// 176 px, first message at 112 px, 76 px per step, return dashed `6 4`, async open arrowhead, 18 px hit stroke, a dot
// at the sender. Every step is `<g data-step={i} role="button" tabIndex=0 aria-description={ref}>` so the board reader
// (steps 4–5) can register units and drive `highlight` without touching this file. Like `Graph`, colours are inline
// attributes from the CSS tokens so `DiagramFrame`'s export carries them; `SequenceSvg` is the pure drawing.
import { useMemo, useSyncExternalStore } from 'react'
import type { z } from 'zod'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { FONT_PX } from '@/lib/graph-layout'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from '../DiagramFrame'
import type { sequenceProps } from './catalog'

export type SequenceData = z.infer<typeof sequenceProps>

const LANE = 176
const FIRST = 112
const GAP = 76
const HEAD_W = 140
const HEAD_H = 32
const M = 12
const LOOP_W = 40
const LOOP_H = 28
const ARROW: Record<'call' | 'return' | 'async', 'closed' | 'open'> = { call: 'closed', return: 'closed', async: 'open' }

/** Stable id for the export file stem; FNV-1a over the fields that change the picture. */
export function sequenceHash(s: SequenceData): string {
  const src = [s.title ?? '', ...s.actors.map((a) => `${a.id}\u0001${a.label}`), ...s.steps.map((st) => `${st.from}\u0001${st.to}\u0001${st.style ?? ''}\u0001${st.label}`)].join('\u0002')
  let h = 0x811c9dc5
  for (let i = 0; i < src.length; i++) h = Math.imul(h ^ src.charCodeAt(i), 0x01000193)
  return (h >>> 0).toString(16).padStart(8, '0')
}

export function SequenceSvg({
  data,
  tokens: t,
  markerId,
  highlight,
  onSelect,
}: {
  data: SequenceData
  tokens: DiagramTokens
  markerId: string
  /** Step indices drawn active; every other step's arrow goes muted (M4). */
  highlight?: readonly number[]
  /** A step was pressed (click, Enter, Space). */
  onSelect?: (index: number) => void
}) {
  const top = data.title ? 28 : 0
  const laneX: Record<string, number> = {}
  const labels: Record<string, string> = {}
  data.actors.forEach((a, i) => {
    laneX[a.id] = M + LANE / 2 + i * LANE
    labels[a.id] = a.label
  })
  const W = data.actors.length * LANE + 2 * M
  const H = top + FIRST + GAP * Math.max(0, data.steps.length - 1) + 48
  const lit = highlight && highlight.length > 0
  const tone = (active: boolean) => (active ? 'accent' : lit ? 'muted' : 'fg')
  const colour = { accent: t.accent, muted: t.mutedForeground, fg: t.foreground }
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} role="group" aria-label={data.title ?? 'Sequence'}>
      <defs>
        {(['closed', 'open'] as const).map((head) =>
          (['accent', 'muted', 'fg'] as const).map((c) => (
            <marker key={`${head}-${c}`} id={`${markerId}-${head}-${c}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
              {head === 'closed' ? <path d="M0 0 L10 5 L0 10 z" fill={colour[c]} /> : <path d="M1 1 L9 5 L1 9" fill="none" stroke={colour[c]} strokeWidth={1.5} />}
            </marker>
          )),
        )}
      </defs>
      {data.title && (
        <text x={M} y={18} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {data.title}
        </text>
      )}
      {data.actors.map((a) => {
        const x = laneX[a.id]!
        return (
          <g key={a.id} data-actor={a.id}>
            <line x1={x} y1={top + 8 + HEAD_H} x2={x} y2={H - 12} stroke={t.border} strokeWidth={1} />
            <rect x={x - HEAD_W / 2} y={top + 8} width={HEAD_W} height={HEAD_H} rx={6} fill={t.card} stroke={t.border} />
            <text x={x} y={top + 8 + HEAD_H / 2} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fill={t.foreground}>
              {a.label}
            </text>
          </g>
        )
      })}
      {data.steps.map((st, i) => {
        const y = top + FIRST + i * GAP
        const x1 = laneX[st.from] ?? M
        const x2 = laneX[st.to] ?? M
        const active = highlight?.includes(i) ?? false
        const c = colour[tone(active)]
        const style = st.style ?? 'call'
        const self = st.from === st.to
        // ponytail: one-line label; an 80-char label on a 176 px lane overruns its neighbours — wrap when a real board shows it.
        const d = self ? `M${x1} ${y} H${x1 + LOOP_W} V${y + LOOP_H} H${x1 + 6}` : `M${x1} ${y} H${x2}`
        const press = onSelect ? () => onSelect(i) : undefined
        return (
          <g
            key={i}
            data-step={i}
            role="button"
            tabIndex={0}
            aria-label={`${i + 1}. ${labels[st.from] ?? st.from} → ${labels[st.to] ?? st.to}: ${st.label}`}
            aria-description={st.ref ?? undefined}
            style={{ cursor: onSelect ? 'pointer' : undefined, outlineOffset: 4 }}
            onClick={press}
            onKeyDown={press && ((ev: React.KeyboardEvent) => (ev.key === 'Enter' || ev.key === ' ') && (ev.preventDefault(), press()))}
          >
            <path d={d} fill="none" stroke="transparent" strokeWidth={18} style={{ pointerEvents: 'stroke' }} />
            <path
              d={d}
              fill="none"
              stroke={c}
              strokeWidth={active ? 2.4 : 1.25}
              strokeDasharray={style === 'return' ? '6 4' : undefined}
              markerEnd={`url(#${markerId}-${ARROW[style]}-${tone(active)})`}
            />
            <circle cx={x1} cy={y} r={5.5} fill={active ? t.accent : t.card} stroke={active ? t.accent : t.border} strokeWidth={1.5} />
            <text
              x={self ? x1 + LOOP_W + 8 : (x1 + x2) / 2}
              y={self ? y + LOOP_H / 2 : y - 7}
              textAnchor={self ? 'start' : 'middle'}
              dominantBaseline={self ? 'central' : undefined}
              fontFamily={t.font}
              fontSize={11.5}
              fontWeight={active ? 700 : 650}
              fill={active ? t.accent : t.mutedForeground}
            >
              {st.label}
            </text>
          </g>
        )
      })}
    </svg>
  )
}

export interface SequenceViewProps {
  props: SequenceData
  highlight?: readonly number[]
  onSelect?: (index: number) => void
}

/** The Sequence card: the svg in the shared `DiagramFrame` (pan/zoom, export, fullscreen), recoloured with the theme. */
export function SequenceView({ props, highlight, onSelect }: SequenceViewProps) {
  const hash = useMemo(() => sequenceHash(props), [props])
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'), () => false)
  const tokens = useMemo(diagramTokens, [dark])
  return (
    <DiagramFrame name={`sequence-${hash}`} label="sequence" kind="sequence-block">
      <SequenceSvg data={props} tokens={tokens} markerId={`seq-${hash}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
