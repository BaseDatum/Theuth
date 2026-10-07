// Design 123 §4.2, §5.2(a) (W1): `StateMachine` — ELK layered in the worker (self-loops allowed), `initial` a filled
// 16 px dot, `final` a double circle, `choice` a diamond, states rounded boxes with `sub` in mono; transitions are
// r=8 orthogonal connectors whose label mask reads `event [guard]`. `StateMachineSvg` is the pure drawing (fixed
// layout in tests), `StateMachineView` the worker/memo/`DiagramFrame` host the registry and the apps mount.
import type { ElkNode } from 'elkjs/lib/elk-api.js'
import { type ReactNode, useEffect, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { ELK_OPTIONS, fnv, labelSize, nodeSize, norm } from '@/lib/diagram/elk'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { EDGE_OPTIONS, flattenElk, FONT_PX, type Layout, LINE_PX, useDiagramTokens, useElkLayout } from '@/lib/graph-layout'
import { ArrowMarkers, EdgePath, ElkFrame, Label, MARGIN, MONO, edgeUnit, paint, Shape, type Tip, TITLE_H, useGraphUnits } from '../Graph'
import type { stateMachineProps } from '../catalog'

export type StateMachineData = z.infer<typeof stateMachineProps>
type State = StateMachineData['states'][number]
type Transition = StateMachineData['transitions'][number]

const DOT = 16
const FINAL = 20

/** `label [guard]` — the twin of `crates/surface::elk_family::transition_text`. */
export function transitionText(t: Pick<Transition, 'label' | 'guard'>): string {
  const l = norm(t.label)
  const g = norm(t.guard)
  return l && g ? `${l} [${g}]` : l || (g ? `[${g}]` : '')
}

/** States present; transitions whose ends both exist (an open fence streams states after the transitions). */
export function dropDangling(p: StateMachineData): StateMachineData {
  const ids: Record<string, true> = {}
  for (const s of p.states) ids[s.id] = true
  return { ...p, transitions: p.transitions.filter((t) => ids[t.from] && ids[t.to]) }
}

export function structuralHash(p: StateMachineData): string {
  return fnv(
    [
      `sm:${p.direction ?? 'LR'}:${norm(p.title)}`,
      ...p.states.map((s) => `s\u0001${s.id}\u0001${s.kind ?? ''}\u0001${norm(s.label)}\u0001${norm(s.sub)}\u0001${s.state ?? ''}`),
      ...p.transitions.map((t) => `t\u0001${t.from}\u0001${t.to}\u0001${transitionText(t)}\u0001${t.style ?? ''}\u0001${t.state ?? ''}`),
    ].join('\u0002'),
  )
}

function stateSize(s: State): { w: number; h: number } {
  switch (s.kind) {
    case 'initial':
      return { w: DOT, h: DOT }
    case 'final':
      return { w: FINAL, h: FINAL }
    default:
      return nodeSize(s.label, s.sub ? 1 : 0)
  }
}

/** The machine as an ELK graph: every state a node (`n:id`), every transition an edge (`e:i`) with its text as label. */
export function toElk(p: StateMachineData): ElkNode {
  return {
    id: 'root',
    layoutOptions: { ...ELK_OPTIONS, ...EDGE_OPTIONS, 'elk.direction': p.direction === 'TB' ? 'DOWN' : 'RIGHT' },
    children: p.states.map((s) => ({ id: `n:${s.id}`, ...stateSize(s) })).map(({ id, w, h }) => ({ id, width: w, height: h })),
    edges: p.transitions.map((t, i) => {
      const text = transitionText(t)
      const l = labelSize(text)
      return { id: `e:${i}`, sources: [`n:${t.from}`], targets: [`n:${t.to}`], labels: l ? [{ id: `l:${i}`, text, width: l.w, height: l.h }] : [] }
    }),
  }
}

/** `◉ Draft → Review  submit [complete]` per transition — the TS twin of the Rust lowering, for the time-out. */
export function stateMachineText(p: StateMachineData): string {
  const by: Record<string, State> = {}
  for (const s of p.states) by[s.id] = s
  const mark = (id: string) => {
    const s = by[id]
    if (!s) return '•'
    if (s.kind === 'initial') return '◉'
    if (s.kind === 'final') return '◎'
    if (s.kind === 'choice') return '◇'
    return { active: '◉', done: '●', muted: '○', danger: '✗', default: '•' }[s.state ?? 'default']
  }
  const name = (id: string) => by[id]?.label ?? id
  const touched: Record<string, true> = {}
  const lines = p.transitions.map((t) => {
    touched[t.from] = touched[t.to] = true
    const arrow = t.state === 'danger' ? '✗→' : t.style === 'dashed' ? '⇢' : '→'
    const text = transitionText(t)
    return `${mark(t.from)} ${name(t.from)} ${arrow} ${name(t.to)}${text ? `  ${text}` : ''}`
  })
  for (const s of p.states) if (!touched[s.id]) lines.push(`${mark(s.id)} ${s.label}${s.sub ? ` (${s.sub})` : ''}`)
  return [p.title, ...lines].filter(Boolean).join('\n')
}

/** The tooltip of the hovered state (label, sub, kind, state) or transition (event, guard). */
export function stateMachineTooltip(p: StateMachineData, hover: string | null): Tip | null {
  if (hover === null) return null
  if (hover.startsWith('edge:')) {
    const t = p.transitions[Number(hover.slice(5))]
    if (!t) return null
    const name = (id: string) => p.states.find((s) => s.id === id)?.label ?? id
    return { title: `${name(t.from)} → ${name(t.to)}`, rows: [{ label: 'event', value: t.label }, { label: 'guard', value: t.guard }, { label: 'state', value: t.state }] }
  }
  const s = p.states.find((x) => x.id === hover)
  return s ? { title: s.label, rows: [{ label: 'sub', value: s.sub }, { label: 'kind', value: s.kind }, { label: 'state', value: s.state }, { label: 'ref', value: s.ref }] } : null
}

function StateShape({ s, x, y, w, h, st, t }: { s: State; x: number; y: number; w: number; h: number; st: UnitState; t: DiagramTokens }) {
  const p = paint(st, t)
  switch (s.kind) {
    case 'initial':
      return <circle cx={x + w / 2} cy={y + h / 2} r={DOT / 2} fill={p.stroke} />
    case 'final':
      return (
        <g>
          <circle cx={x + w / 2} cy={y + h / 2} r={FINAL / 2} fill={p.fill} fillOpacity={p.fillOpacity} stroke={p.stroke} strokeWidth={p.strokeWidth} />
          <circle cx={x + w / 2} cy={y + h / 2} r={FINAL / 2 - 4} fill={p.stroke} />
        </g>
      )
    case 'choice':
      return <Shape kind="diamond" x={x} y={y} w={w} h={h} fill={p.fill} fillOpacity={p.fillOpacity} stroke={p.stroke} strokeWidth={p.strokeWidth} />
    default:
      return <Shape kind="rounded" x={x} y={y} w={w} h={h} fill={p.fill} fillOpacity={p.fillOpacity} stroke={p.stroke} strokeWidth={p.strokeWidth} />
  }
}

/** The svg at `layout`; `markerId` unique per mounted svg. Draw order: edges → states → labels. */
export function StateMachineSvg({ props: p, layout: l, tokens: t, markerId, units, root }: { props: StateMachineData; layout: Layout; tokens: DiagramTokens; markerId: string; units?: Units; root?: { onKeyDown: (e: React.KeyboardEvent) => void } }) {
  const top = MARGIN + (p.title ? TITLE_H : 0)
  const W = Math.ceil(l.width + 2 * MARGIN)
  const H = Math.ceil(top + l.height + MARGIN)
  const a = svgA11y(markerId, !!units)
  const states: Record<string, State> = {}
  for (const s of p.states) states[s.id] = s
  const name = (id: string) => states[id]?.label ?? id
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{p.title ?? 'StateMachine'}</title>
      <desc id={a.descId}>{describe('StateMachine', p.title, p.states.length, 'states')}</desc>
      <defs>
        <ArrowMarkers id={markerId} tokens={t} />
      </defs>
      {p.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {p.title}
        </text>
      )}
      <g transform={`translate(${MARGIN} ${top})`}>
        {l.edges.map((e) => {
          const tr = p.transitions[e.index]
          if (!tr) return null
          const st = units ? units.paint(edgeUnit(e.index), tr.state) : (tr.state ?? 'default')
          const text = transitionText(tr)
          return (
            <g key={e.index} opacity={paint(st, t).opacity} data-unit={edgeUnit(e.index)} {...(units ? units.unit(edgeUnit(e.index), `${name(tr.from)} → ${name(tr.to)}${text ? `: ${text}` : ''}`) : {})}>
              <EdgePath edge={e} state={st} style={tr.style} markerId={markerId} tokens={t} label={text} />
            </g>
          )
        })}
        {l.nodes.map((n) => {
          const s = states[n.id]
          if (!s) return null
          const st = units ? units.paint(n.id, s.state) : (s.state ?? 'default')
          const pad = Math.max(0, (18 - Math.min(n.w, n.h)) / 2) + 4
          return (
            <g key={n.id} opacity={paint(st, t).opacity} data-unit={n.id} data-kind={s.kind ?? 'state'} aria-description={s.ref ?? undefined} {...(units ? units.unit(n.id, unitLabel(s.label, s.sub, st)) : {})}>
              <rect x={n.x - pad} y={n.y - pad} width={n.w + 2 * pad} height={n.h + 2 * pad} fill="transparent" />
              <StateShape s={s} x={n.x} y={n.y} w={n.w} h={n.h} st={st} t={t} />
              {s.kind === 'initial' || s.kind === 'final' ? (
                <text x={n.x + n.w / 2} y={n.y + n.h + 12} textAnchor="middle" fontFamily={t.font} fontSize={11} fill={t.mutedForeground}>
                  {s.label}
                </text>
              ) : (
                <>
                  <Label text={s.label} cx={n.x + n.w / 2} cy={n.y + n.h / 2 - (s.sub ? LINE_PX / 2 : 0)} fill={t.foreground} t={t} weight={600} />
                  {s.sub && (
                    <text x={n.x + n.w / 2} y={n.y + n.h / 2 + LINE_PX / 2} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                      {s.sub}
                    </text>
                  )}
                </>
              )}
            </g>
          )
        })}
      </g>
    </svg>
  )
}

export function StateMachineView({ props, loading, highlight, onSelect }: DiagramHostProps<StateMachineData>) {
  const p = useMemo(() => (loading ? dropDangling(props) : props), [props, loading])
  const hash = useMemo(() => structuralHash(p), [p])
  const tokens = useDiagramTokens()
  const shown = useElkLayout(hash, () => toElk(p), flattenElk)
  const drawnRef = useRef(p)
  const drawn = shown && shown.hash === hash ? p : drawnRef.current
  useEffect(() => {
    drawnRef.current = drawn
  }, [drawn])
  const ids = useMemo(() => drawn.states.map((s) => s.id), [drawn])
  const units = useGraphUnits(ids, drawn.transitions, highlight, onSelect)
  let content: ReactNode
  if (shown && 'layout' in shown) content = <StateMachineSvg props={drawn} layout={shown.layout} tokens={tokens} markerId={`sm-${shown.hash}`} units={units} root={units.root} />
  else if (shown) content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{stateMachineText(drawn)}</pre>
  return (
    <ElkFrame kind="statemachine" hash={hash} timeout={!!shown && 'timeout' in shown} hover={units.state.hover} tip={stateMachineTooltip(drawn, units.state.hover)}>
      {content}
    </ElkFrame>
  )
}
