// Design 71 §2.4 / M14 + 123 §4.1, §5.4 #15 (W3): actors as lanes, one addressable step per message. Geometry is
// Whiteboard's (R §B.5): lanes 176 px, first message at 112 px, 76 px per step, return dashed `6 4`, async open
// arrowhead, 18 px hit stroke, a dot at the sender. Every step is a §5.3 unit (`useUnits`, id = step index) and keeps
// `data-step={i}` for the board reader; `state` recolours the arrow, `note` sits beside it on a `card` mask, activations
// are derived (call opens, return closes) and `fragments` draw frames with an `ALT` tab and `[else]` divider.
// Colours are inline attributes from the CSS tokens so `DiagramFrame`'s export carries them; `SequenceSvg` is pure.
import { useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import { useUnits } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { useThemeTokens } from '@/lib/snapshot-mode'
import { FONT_PX } from '@/lib/graph-layout'
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
const SMALL_PX = 11
/** Glyph width estimate for 11 px text (no DOM in the pure drawing). */
const SMALL_CH = SMALL_PX * 0.6
const MONO = 'ui-monospace, monospace'
const ARROW: Record<'call' | 'return' | 'async', 'closed' | 'open'> = { call: 'closed', return: 'closed', async: 'open' }
const COLOURS = ['accent', 'muted', 'fg', 'danger', 'success'] as const
type Colour = (typeof COLOURS)[number]

/** Stable id for the export file stem; FNV-1a over the fields that change the picture. */
export function sequenceHash(s: SequenceData): string {
  const src = [
    s.title ?? '',
    ...s.actors.map((a) => `${a.id}\u0001${a.label}`),
    ...s.steps.map((st) => `${st.from}\u0001${st.to}\u0001${st.style ?? ''}\u0001${st.label}\u0001${st.note ?? ''}\u0001${st.state ?? ''}`),
    ...(s.fragments ?? []).map((f) => `${f.kind}\u0001${f.from}\u0001${f.to}\u0001${f.label ?? ''}\u0001${f.else ?? ''}`),
  ].join('\u0002')
  let h = 0x811c9dc5
  for (let i = 0; i < src.length; i++) h = Math.imul(h ^ src.charCodeAt(i), 0x01000193)
  return (h >>> 0).toString(16).padStart(8, '0')
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Sequence arm). */
export function sequenceDesc(d: SequenceData): string {
  return describe('Sequence', d.title, d.steps.length, 'steps')
}

export interface Activation {
  actor: string
  from: number
  /** Closing step index; `null` when no return matched (drawn to the last step + 16). */
  to: number | null
}

/** Derived activations (§4.1): a `call` A → B opens one on B at that step; the next `return` B → A closes the most recent open one. */
export function activations(d: SequenceData): Activation[] {
  const out: Activation[] = []
  d.steps.forEach((st, i) => {
    const style = st.style ?? 'call'
    if (st.from === st.to || style === 'async') return
    if (style === 'call') {
      out.push({ actor: st.to, from: i, to: null })
      return
    }
    // ponytail: LIFO match (nested calls); a return with no open call is silently ignored.
    for (let k = out.length - 1; k >= 0; k--) {
      const a = out[k]!
      if (a.to === null && a.actor === st.from && d.steps[a.from]?.from === st.to) {
        a.to = i
        break
      }
    }
  })
  return out
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
  const ids = useMemo(() => data.steps.map((_, i) => String(i)), [data.steps])
  const lit = useMemo(() => highlight?.map(String), [highlight])
  const select = useMemo(() => onSelect && ((id: string) => onSelect(Number(id))), [onSelect])
  const u = useUnits({ ids, highlight: lit, onSelect: select })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const top = data.title ? 28 : 0
  const n = data.steps.length
  const laneX: Record<string, number> = {}
  const labels: Record<string, string> = {}
  data.actors.forEach((ac, i) => {
    laneX[ac.id] = M + LANE / 2 + i * LANE
    labels[ac.id] = ac.label
  })
  const stepY = (i: number) => top + FIRST + i * GAP
  // Streaming: clamp `to` past the end, drop `from` past the end or after `to`; `else` outside `from+1..=to` is ignored.
  const frames = (data.fragments ?? []).map((f, i) => ({ ...f, i, to: Math.min(f.to, n - 1) })).filter((f) => f.from < n && f.from <= f.to)
  const acts = activations(data)
  const W = data.actors.length * LANE + 2 * M
  const H = Math.max(top + FIRST + GAP * Math.max(0, n - 1) + 48, ...frames.map((f) => stepY(f.to) + 24 + M))
  const dim = (highlight !== undefined && highlight.length > 0) || u.state.selected !== null
  const colour: Record<Colour, string> = { accent: t.accent, muted: t.mutedForeground, fg: t.foreground, danger: t.danger, success: t.success }
  const hover = u.state.hover === null ? null : data.steps[Number(u.state.hover)]
  const name = (id: string) => labels[id] ?? id
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{data.title ?? 'Sequence'}</title>
        <desc id={a.descId}>{sequenceDesc(data)}</desc>
        <defs>
          {(['closed', 'open'] as const).map((head) =>
            COLOURS.map((c) => (
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
        {data.actors.map((ac) => {
          const x = laneX[ac.id]!
          return (
            <g key={ac.id} data-actor={ac.id}>
              <line x1={x} y1={top + 8 + HEAD_H} x2={x} y2={H - 12} stroke={t.border} strokeWidth={1} />
              <rect x={x - HEAD_W / 2} y={top + 8} width={HEAD_W} height={HEAD_H} rx={6} fill={t.card} stroke={t.border} />
              <text x={x} y={top + 8 + HEAD_H / 2} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fill={t.foreground}>
                {ac.label}
              </text>
            </g>
          )
        })}
        {frames.map((f) => {
          const x0 = M + 8
          const x1 = W - M - 8
          const y0 = stepY(f.from) - 30
          const kind = f.kind.toUpperCase()
          const label = f.label ? truncate(f.label, 24) : ''
          const kindW = Math.ceil(kind.length * SMALL_CH * 1.1)
          const tabW = 16 + kindW + (label ? 6 + Math.ceil(label.length * SMALL_CH) : 0)
          const elseY = f.kind === 'alt' && f.else !== undefined && f.else > f.from && f.else <= f.to ? stepY(f.else) - 38 : null
          return (
            <g key={f.i} data-fragment={f.i}>
              <rect x={x0} y={y0} width={x1 - x0} height={stepY(f.to) + 24 - y0} rx={2} fill="none" stroke={t.border} strokeWidth={1.25} />
              <rect x={x0} y={y0} width={tabW} height={20} rx={2} fill={t.card} stroke={t.border} strokeWidth={1.25} />
              <text x={x0 + 8} y={y0 + 10} dominantBaseline="central" fontFamily={MONO} fontSize={SMALL_PX} fontWeight={700} fill={t.foreground}>
                {kind}
              </text>
              {label && (
                <text x={x0 + 8 + kindW + 6} y={y0 + 10} dominantBaseline="central" fontFamily={t.font} fontSize={SMALL_PX} fill={t.mutedForeground}>
                  {label !== f.label && <title>{f.label}</title>}
                  {label}
                </text>
              )}
              {elseY !== null && (
                <>
                  <line x1={x0} y1={elseY} x2={x1} y2={elseY} stroke={t.border} strokeWidth={1} strokeDasharray="4 3" />
                  <text x={x0 + 8} y={elseY - 4} fontFamily={MONO} fontSize={SMALL_PX} fill={t.mutedForeground}>
                    [else]
                  </text>
                </>
              )}
            </g>
          )
        })}
        {acts.map((ac, k) => {
          const x = laneX[ac.actor]
          if (x === undefined) return null
          const y0 = stepY(ac.from)
          const y1 = ac.to === null ? stepY(n - 1) + 16 : stepY(ac.to)
          return <rect key={k} data-activation={ac.actor} x={x - 4} y={y0} width={8} height={y1 - y0} rx={2} fill={t.card} stroke={t.border} />
        })}
        {data.steps.map((st, i) => {
          const id = String(i)
          const y = stepY(i)
          const x1 = laneX[st.from] ?? M
          const x2 = laneX[st.to] ?? M
          const paint = u.paint(id, st.state)
          const key: Colour = paint === 'default' ? (dim ? 'muted' : 'fg') : paint === 'active' ? 'accent' : paint === 'done' ? 'success' : paint
          const active = paint === 'active'
          const strong = key !== 'fg' && key !== 'muted'
          const c = colour[key]
          const style = st.style ?? 'call'
          const self = st.from === st.to
          // ponytail: one-line label; an 80-char label on a 176 px lane overruns its neighbours — wrap when a real board shows it.
          const d = self ? `M${x1} ${y} H${x1 + LOOP_W} V${y + LOOP_H} H${x1 + 6}` : `M${x1} ${y} H${x2}`
          const { style: ustyle, role, tabIndex, ...unit } = u.unit(id, unitLabel(`${i + 1}. ${name(st.from)} → ${name(st.to)}: ${st.label}`, st.note, st.state))
          // Note beside the arrow's right end when the lane has room, else left of its left end (§4.1).
          const shown = st.note ? truncate(st.note, 24) : ''
          const noteW = Math.ceil(shown.length * SMALL_CH) + 8
          const right = self ? x1 + LOOP_W + 8 + Math.ceil(st.label.length * 11.5 * 0.6) : Math.max(x1, x2)
          const noteX = right + 12 + noteW <= W - M ? right + 12 : Math.min(x1, x2) - 12 - noteW
          const noteY = self ? y + LOOP_H / 2 : y
          return (
            <g key={i} data-step={i} role={role} tabIndex={tabIndex} {...unit} aria-description={st.ref ?? undefined} style={onSelect ? { ...ustyle, outlineOffset: 4 } : { ...ustyle, cursor: undefined, outlineOffset: 4 }}>
              <path d={d} fill="none" stroke="transparent" strokeWidth={18} style={{ pointerEvents: 'stroke' }} />
              <path d={d} fill="none" stroke={c} strokeWidth={active ? 2.4 : 1.25} strokeDasharray={style === 'return' ? '6 4' : undefined} markerEnd={`url(#${markerId}-${ARROW[style]}-${key})`} />
              <circle cx={x1} cy={y} r={5.5} fill={strong ? c : t.card} stroke={strong ? c : t.border} strokeWidth={1.5} />
              <text
                x={self ? x1 + LOOP_W + 8 : (x1 + x2) / 2}
                y={self ? y + LOOP_H / 2 : y - 7}
                textAnchor={self ? 'start' : 'middle'}
                dominantBaseline={self ? 'central' : undefined}
                fontFamily={t.font}
                fontSize={11.5}
                fontWeight={active ? 700 : 650}
                fill={strong ? c : t.mutedForeground}
              >
                {st.label}
              </text>
              {shown && (
                <>
                  <rect x={noteX} y={noteY - 8} width={noteW} height={16} rx={3} fill={t.card} />
                  <text x={noteX + 4} y={noteY} dominantBaseline="central" fontFamily={t.font} fontSize={SMALL_PX} fill={t.mutedForeground}>
                    <title>{st.note}</title>
                    {shown}
                  </text>
                </>
              )}
            </g>
          )
        })}
      </svg>
      <DiagramTooltip
        hover={u.state.hover}
        root={ref.current}
        title={hover?.label ?? ''}
        rows={[
          { label: 'from → to', value: hover && `${name(hover.from)} → ${name(hover.to)}` },
          { label: 'style', value: hover?.style ?? 'call' },
          { label: 'note', value: hover?.note },
          { label: 'state', value: hover?.state },
          { label: 'ref', value: hover?.ref },
        ]}
      />
    </>
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
  const tokens = useThemeTokens()
  return (
    <DiagramFrame name={`sequence-${hash}`} label="sequence" kind="sequence-block">
      <SequenceSvg data={props} tokens={tokens} markerId={`seq-${hash}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
