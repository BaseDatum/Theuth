// 123 §5.2(c) / §5.4 #14 (W3): `Sankey` — d3-sankey columns (`nodeAlign` by authored `stage`, `nodeWidth` 12,
// `nodePadding` 16), ribbons as the d3 link path stroked at the link width (control points at the midpoint, no
// arrowheads). `SankeySvg` is the pure drawing over `sankeyLayout()` (snapshot-tested); `SankeyView` wraps it in
// `DiagramFrame`. The layout drops dangling / self / cycle-closing flows so an open fence never throws mid-stream.
import { sankey, sankeyJustify, sankeyLinkHorizontal } from 'd3-sankey'
import { useId, useMemo, useRef, useSyncExternalStore } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { MaskedLabel, truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from '../../DiagramFrame'
import type { sankeyProps } from '../catalog'

export type SankeyData = z.infer<typeof sankeyProps>

const M = 12
const NODE_W = 12
const NODE_PAD = 16
const FONT = 13
const MONO = 'ui-monospace, monospace'

// `type`, not `interface`: d3-sankey's generics need an implicit index signature.
type SNode = { id: string; stage?: number }
/** `flow` = index into `data.flows` (d3 overwrites `index`, so the authored position rides here). */
type SLink = { flow: number; value: number }

export interface SankeyLayout {
  nodes: { id: string; x0: number; x1: number; y0: number; y1: number; value: number; column: number }[]
  links: { index: number; path: string; width: number; source: string; target: string; y0: number; y1: number }[]
  columns: number
  width: number
  height: number
}

/** Flows worth laying out: both ends known, `from ≠ to`, and not closing a cycle over the flows accepted before it. */
export function sankeyFlows(d: SankeyData): number[] {
  const known = new Set(d.nodes.map((n) => n.id))
  const out = new Map<string, string[]>()
  const reaches = (from: string, to: string): boolean => {
    const seen = new Set<string>()
    const stack = [from]
    for (let cur = stack.pop(); cur !== undefined; cur = stack.pop()) {
      if (cur === to) return true
      if (seen.has(cur)) continue
      seen.add(cur)
      stack.push(...(out.get(cur) ?? []))
    }
    return false
  }
  const kept: number[] = []
  d.flows.forEach((f, i) => {
    if (!known.has(f.from) || !known.has(f.to) || f.from === f.to || !(f.value > 0) || reaches(f.to, f.from)) return
    out.set(f.from, [...(out.get(f.from) ?? []), f.to])
    kept.push(i)
  })
  return kept
}

/** d3-sankey over the kept flows; `H = max(240, 28·n)`. Without flows (or if d3 throws) nodes sit zero-height at their stage. */
export function sankeyLayout(d: SankeyData, W = 560, top = 0): SankeyLayout {
  const H = Math.max(240, 28 * d.nodes.length)
  const height = top + 2 * M + H
  const kept = sankeyFlows(d)
  const staged = d.nodes.length > 0 && d.nodes.every((n) => n.stage !== undefined)
  const empty = (): SankeyLayout => {
    // ponytail: fallback placement — one column per stage, nodes evenly spaced, zero height.
    const stages = [...new Set(d.nodes.map((n) => n.stage ?? 0))].sort((a, b) => a - b)
    const kx = stages.length > 1 ? (W - 2 * M - NODE_W) / (stages.length - 1) : 0
    const nodes = d.nodes.map((n) => {
      const column = stages.indexOf(n.stage ?? 0)
      const peers = d.nodes.filter((p) => (p.stage ?? 0) === (n.stage ?? 0))
      const y = top + M + (H * (peers.indexOf(n) + 1)) / (peers.length + 1)
      const x0 = M + column * kx
      return { id: n.id, x0, x1: x0 + NODE_W, y0: y, y1: y, value: 0, column }
    })
    return { nodes, links: [], columns: stages.length, width: W, height }
  }
  if (kept.length === 0) return empty()
  const nodes: SNode[] = d.nodes.map((n) => (n.stage === undefined ? { id: n.id } : { id: n.id, stage: n.stage }))
  const links = kept.flatMap((i) => {
    const f = d.flows[i]
    return f ? [{ flow: i, value: f.value, source: f.from, target: f.to }] : []
  })
  try {
    const gen = sankey<SNode, SLink>()
      .nodeId((n) => n.id)
      .nodeWidth(NODE_W)
      .nodePadding(NODE_PAD)
      .nodeSort(null)
      .linkSort(null)
      .extent([
        [M, top + M],
        [W - M, top + M + H],
      ])
    // ponytail: d3 clamps the align to its depth count, so a stage past the deepest flow lands in the last column.
    if (staged) gen.nodeAlign((n) => n.stage ?? 0)
    else gen.nodeAlign(sankeyJustify)
    const g = gen({ nodes, links })
    const shape = sankeyLinkHorizontal<SNode, SLink>()
    const xs = [...new Set(g.nodes.map((n) => n.x0 ?? 0))].sort((a, b) => a - b)
    const outNodes = g.nodes.map((n) => ({ id: n.id, x0: n.x0 ?? 0, x1: n.x1 ?? 0, y0: n.y0 ?? 0, y1: n.y1 ?? 0, value: n.value ?? 0, column: xs.indexOf(n.x0 ?? 0) }))
    if (outNodes.some((n) => !Number.isFinite(n.x0 + n.y0 + n.y1))) return empty()
    const outLinks = g.links.map((l) => {
      const s = l.source as SNode
      const t = l.target as SNode
      // path numbers rounded to 0.1 px: the snapshot stays small and the same props still give the same `d`
      return { index: l.flow, path: (shape(l) ?? '').replace(/-?\d+\.\d+/g, (m) => (+m).toFixed(1)), width: l.width ?? 0, source: s.id, target: t.id, y0: l.y0 ?? 0, y1: l.y1 ?? 0 }
    })
    return { nodes: outNodes, links: outLinks, columns: outNodes.reduce((m, n) => Math.max(m, n.column), 0) + 1, width: W, height }
  } catch {
    return empty()
  }
}

/** Flow unit ids: `from→to`, a repeated pair after the first gets `#<flow index>`. */
export function flowIds(d: SankeyData, kept: readonly number[]): Map<number, string> {
  const seen = new Set<string>()
  const ids = new Map<number, string>()
  for (const i of kept) {
    const f = d.flows[i]
    if (!f) continue
    const base = `${f.from}→${f.to}`
    ids.set(i, seen.has(base) ? `${base}#${i}` : base)
    seen.add(base)
  }
  return ids
}

const fmt = (v: number, unit?: string) => `${+v.toFixed(2)}${unit ? ` ${unit}` : ''}`

function nodeTint(state: UnitState, t: DiagramTokens): { stroke?: string; width: number; opacity: number } {
  switch (state) {
    case 'active':
      return { stroke: t.accent, width: 2, opacity: 1 }
    case 'done':
      return { stroke: t.success, width: 1.5, opacity: 1 }
    case 'danger':
      return { stroke: t.danger, width: 1.5, opacity: 1 }
    case 'muted':
      return { width: 0, opacity: 0.5 }
    default:
      return { width: 0, opacity: 1 }
  }
}

function flowTint(state: UnitState, series: string, t: DiagramTokens): { stroke: string; strokeOpacity: number; opacity: number } {
  switch (state) {
    case 'active':
      return { stroke: t.accent, strokeOpacity: 0.6, opacity: 1 }
    case 'done':
      return { stroke: t.success, strokeOpacity: 0.45, opacity: 1 }
    case 'danger':
      return { stroke: t.danger, strokeOpacity: 0.45, opacity: 1 }
    case 'muted':
      return { stroke: series, strokeOpacity: 0.35, opacity: 0.5 }
    default:
      return { stroke: series, strokeOpacity: 0.35, opacity: 1 }
  }
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Sankey arm). */
export function sankeyDesc(d: SankeyData): string {
  return describe('Sankey', d.title, d.flows.length, 'flows')
}

export function SankeySvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: SankeyData; layout: SankeyLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const byId = useMemo(() => new Map<string, SankeyData['nodes'][number]>(d.nodes.map((n) => [n.id, n])), [d.nodes])
  const nodeAt = useMemo(() => new Map<string, SankeyLayout['nodes'][number]>(l.nodes.map((n) => [n.id, n])), [l.nodes])
  const fids = useMemo(() => flowIds(d, l.links.map((k) => k.index)), [d, l.links])
  const ids = useMemo(() => [...l.nodes.filter((n) => byId.has(n.id)).map((n) => n.id), ...l.links.map((k) => fids.get(k.index) ?? '')], [l.nodes, l.links, byId, fids])
  const incident = useMemo(
    () => (id: string) => {
      const link = l.links.find((k) => fids.get(k.index) === id)
      if (link) return [link.source, link.target]
      return l.links.filter((k) => k.source === id || k.target === id).map((k) => fids.get(k.index) ?? '')
    },
    [l.links, fids],
  )
  const u = useUnits({ ids, incident, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const last = l.columns - 1
  const hoverNode = u.state.hover === null ? undefined : byId.get(u.state.hover)
  const hoverLink = u.state.hover === null || hoverNode ? undefined : l.links.find((k) => fids.get(k.index) === u.state.hover)
  const hoverFlow = hoverLink ? d.flows[hoverLink.index] : undefined
  const title = hoverNode ? hoverNode.label : hoverFlow ? `${byId.get(hoverFlow.from)?.label ?? hoverFlow.from} → ${byId.get(hoverFlow.to)?.label ?? hoverFlow.to}` : ''
  const rows = hoverNode
    ? [
        { label: 'total', value: fmt(nodeAt.get(hoverNode.id)?.value ?? 0, d.unit) },
        { label: 'state', value: hoverNode.state },
        { label: 'stage', value: hoverNode.stage === undefined ? undefined : String(hoverNode.stage) },
      ]
    : [
        { label: 'value', value: hoverFlow ? fmt(hoverFlow.value, d.unit) : undefined },
        { label: 'label', value: hoverFlow?.label },
        { label: 'state', value: hoverFlow?.state },
      ]
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Sankey'}</title>
        <desc id={a.descId}>{sankeyDesc(d)}</desc>
        {d.title && (
          <text x={M} y={18} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {d.title}
          </text>
        )}
        {l.links.map((k) => {
          const f = d.flows[k.index]
          const id = fids.get(k.index)
          const s = nodeAt.get(k.source)
          if (!f || !id || !s) return null
          const p = flowTint(u.paint(id, f.state), t.series[s.column % 6] ?? t.primary, t)
          const w = Math.max(1, +k.width.toFixed(1))
          return (
            <g key={id} {...u.unit(id, `${byId.get(f.from)?.label ?? f.from} → ${byId.get(f.to)?.label ?? f.to}: ${fmt(f.value, d.unit)}`)} opacity={p.opacity}>
              {w < 18 && <path d={k.path} fill="none" stroke="transparent" strokeWidth={18} />}
              <path d={k.path} fill="none" stroke={p.stroke} strokeOpacity={p.strokeOpacity} strokeWidth={w} />
            </g>
          )
        })}
        {l.links.map((k) => {
          const f = d.flows[k.index]
          const s = nodeAt.get(k.source)
          const tn = nodeAt.get(k.target)
          if (!f?.label || !s || !tn) return null
          return <MaskedLabel key={k.index} text={f.label} at={{ x: (s.x1 + tn.x0) / 2, y: +((k.y0 + k.y1) / 2).toFixed(1) }} along="horizontal" side="before" tokens={t} />
        })}
        {l.nodes.map((n) => {
          const node = byId.get(n.id)
          if (!node) return null
          const p = nodeTint(u.paint(n.id, node.state), t)
          const y = +n.y0.toFixed(1)
          const h = Math.max(2, +(n.y1 - n.y0).toFixed(1))
          const right = n.column < last
          const lx = right ? n.x1 + 8 : n.x0 - 8
          return (
            <g key={n.id} {...u.unit(n.id, unitLabel(node.label, fmt(n.value, d.unit), node.state))} opacity={p.opacity}>
              <rect x={n.x0 - 4} y={y - 4} width={NODE_W + 8} height={h + 8} fill="transparent" />
              <rect x={n.x0} y={y} width={NODE_W} height={h} rx={2} fill={t.series[n.column % 6] ?? t.primary} stroke={p.stroke} strokeWidth={p.width} />
              <text x={lx} y={+(y + h / 2).toFixed(1)} dy="0.32em" textAnchor={right ? 'start' : 'end'} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
                {truncate(node.label, 20)}
                <tspan dx={6} fontFamily={MONO} fontSize={11} fontWeight={400} fill={t.mutedForeground}>
                  {fmt(n.value, d.unit)}
                </tspan>
              </text>
            </g>
          )
        })}
      </svg>
      <DiagramTooltip hover={u.state.hover} root={ref.current} title={title} rows={rows} />
    </>
  )
}

export function SankeyView({ props, highlight, onSelect }: DiagramHostProps<SankeyData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'), () => false)
  const tokens = useMemo(diagramTokens, [dark])
  const layout = useMemo(() => sankeyLayout(props, 560, props.title ? 28 : 0), [props])
  return (
    <DiagramFrame name={`sankey-${hash}`} label="sankey" kind="sankey-block">
      <SankeySvg data={props} layout={layout} tokens={tokens} markerId={`sankey-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
