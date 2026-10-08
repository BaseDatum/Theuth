/**
 * 147 §11.3 — the canvas: one element's diagram, one level live, laid out by the ELK worker (`lib/graph-layout`: hash
 * cache, 2 s time-box → the adjacency text) inside `DiagramFrame` (pan/zoom/export), drawn with the Graph renderer's
 * primitives (`Shape`, `Label`, `EdgePath` geometry, `ArrowMarkers`, `paint`) under the unit contract
 * (`useGraphUnits`: `data-unit`, roving arrows, Enter/Space, Esc). Group nodes carry a bounded thumbnail of their child
 * diagram (W6: labels only at ≥ 50 % scale); click/Enter on a group zooms in (240 ms transform, none under
 * `prefers-reduced-motion`); direction TB in the pane, LR for a wide ≥ 1 000 px canvas (O8b); edge labels hide below
 * ~70 % zoom except the selection's; inferred edges dashed; stale hatched + the word; `read`/`edited` marks.
 */
import { type KeyboardEvent, type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { DiagramFrame } from '@/components/chat/DiagramFrame'
import { adjacencyText, ArrowMarkers, edgeLabelAt, edgeUnit, Label, MARGIN, markerFor, MONO, paint, Shape, useGraphUnits } from '@/components/chat/surface/Graph'
import { basename } from '@/lib/codemap/labels'
import { directionFor, edgeLabelShown, type ElementGraph, elementGraph, elementStats, evidenceCount, evidenceSummary, footnote, type Mark, THUMB_CHIP_H, THUMB_TITLE_H, thumbBox, thumbLabels, thumbTransform } from '@/lib/codemap/map'
import type { MapElement } from '@/lib/codemap/map-types'
import { svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv, type Layout, LINE_PX, textWidth, wrapLabel } from '@/lib/diagram/elk'
import { markers, sectionsPath } from '@/lib/diagram/path'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { FONT_PX, flattenElk, type GraphData, layout as elkLayoutOf, structuralHash, toElk, useElkLayout } from '@/lib/graph-layout'
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import { cn } from '@/lib/utils'
import './codemap.css'

export type MapCanvasProps = {
  element: MapElement
  popped: boolean
  tokens: DiagramTokens
  /** The child element selected (detail pane), or null. */
  selected: string | null
  /** Element ids lit by the search chain. */
  chain: ReadonlySet<string>
  marks: ReadonlyMap<string, readonly Mark[]>
  onSelect(childId: string | null): void
  onDrill(childId: string): void
  onUp(): void
  onCrumb(id: string): void
}

const ANIM_MS = 240
const reducedMotion = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches

/** `toElk` with the thumbnail nodes grown to their box (ELK ids are `n:<id>`, nested under groups). */
function toElkSized(graph: GraphData, thumbs: ReadonlySet<string>, box: { w: number; h: number }) {
  const root = toElk(graph)
  const visit = (n: { id: string; width?: number; height?: number; children?: unknown[] }) => {
    if (n.id.startsWith('n:') && thumbs.has(n.id.slice(2))) {
      n.width = box.w
      n.height = box.h
    }
    for (const c of n.children ?? []) visit(c as typeof n)
  }
  visit(root)
  return root
}

/** The on-screen scale of `svg` (natural width `w`): `DiagramFrame` fits it to the surface and scales it on wheel/pinch. */
function useZoom(svg: React.RefObject<SVGSVGElement | null>, w: number, key: string): number {
  const [zoom, setZoom] = useState(1)
  useLayoutEffect(() => {
    const el = svg.current
    if (!el || w <= 0) return
    const measure = () => setZoom(Math.round((el.getBoundingClientRect().width / w) * 100) / 100)
    measure()
    const box = el.parentElement
    const mo = box ? new MutationObserver(measure) : null
    if (box) mo!.observe(box, { attributes: true, attributeFilter: ['style'] })
    const ro = new ResizeObserver(measure)
    const surface = el.closest('[role=application]')
    if (surface) ro.observe(surface)
    return () => {
      mo?.disconnect()
      ro.disconnect()
    }
  }, [svg, w, key])
  return zoom
}

/** ELK layouts of the thumbnails (same cache, keyed by their own hash); late ones land when they land. */
function useThumbLayouts(thumbs: ReadonlyMap<string, GraphProps>): Map<string, Layout> {
  const [done, setDone] = useState<Map<string, Layout>>(() => new Map())
  const key = [...thumbs.keys()].join('\u0001')
  useEffect(() => {
    let live = true
    const next = new Map<string, Layout>()
    const all = [...thumbs].map(([id, g]) =>
      elkLayoutOf(g as GraphData).then(
        (l) => {
          next.set(id, l)
          if (live) setDone(new Map(next))
        },
        () => {},
      ),
    )
    void Promise.allSettled(all)
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the key names every thumbnail
  }, [key])
  return done
}

/** A child diagram as rects + edges inside `area`, labels only when the scale allows (W6). */
function Thumbnail({ graph: g, layout: l, area, t }: { graph: GraphProps; layout: Layout; area: { x: number; y: number; w: number; h: number }; t: DiagramTokens }) {
  const { scale, dx, dy } = thumbTransform(l.width, l.height, area)
  const labels = thumbLabels(scale)
  const names: Record<string, string> = {}
  for (const n of g.nodes) names[n.id] = n.label
  const sw = 0.8 / scale
  return (
    <g className="codemap-thumb" transform={`translate(${area.x + dx} ${area.y + dy}) scale(${scale})`} data-thumb-scale={scale.toFixed(2)} aria-hidden="true">
      {l.edges.map((e) => (
        <path key={e.index} d={sectionsPath(e.sections)} fill="none" stroke={t.mutedForeground} strokeWidth={sw} opacity={0.8} />
      ))}
      {l.nodes.map((n) => (
        <g key={n.id}>
          <rect x={n.x} y={n.y} width={n.w} height={n.h} rx={2 / scale} fill={t.muted} stroke={t.mutedForeground} strokeWidth={sw} />
          {labels && (
            <text x={n.x + n.w / 2} y={n.y + n.h / 2} dy="0.32em" textAnchor="middle" fontFamily={t.font} fontSize={FONT_PX} fill={t.foreground} data-thumb-label>
              {wrapLabel(names[n.id] ?? n.id)[0]}
            </text>
          )}
        </g>
      ))}
    </g>
  )
}

/** The edge label on a card mask: the model's why, the evidence count under it in mono; never truncated. */
function EdgeText({ text, sub, at, along, side, t, fill }: { text: string; sub?: string; at: { x: number; y: number }; along: 'horizontal' | 'vertical'; side: 'before' | 'after'; t: DiagramTokens; fill: string }) {
  const w = Math.ceil(Math.max(textWidth(text, FONT_PX - 2), sub ? textWidth(sub, FONT_PX - 3) : 0) + 8)
  const h = sub ? LINE_PX * 2 - 4 : LINE_PX
  const k = side === 'before' ? -1 : 1
  const cx = along === 'horizontal' ? at.x : at.x + k * (8 + w / 2)
  const cy = along === 'horizontal' ? at.y + k * (8 + h / 2) : at.y
  return (
    <g data-edge-label>
      <rect x={cx - w / 2} y={cy - h / 2} width={w} height={h} rx={3} fill={t.card} stroke={t.border} strokeWidth={0.5} />
      <text x={cx} y={sub ? cy - (LINE_PX - 4) / 2 : cy} dy="0.32em" textAnchor="middle" fontFamily={t.font} fontSize={FONT_PX - 2} fill={fill}>
        {text}
      </text>
      {sub && (
        <text x={cx} y={cy + (LINE_PX - 4) / 2} dy="0.32em" textAnchor="middle" fontFamily={MONO} fontSize={FONT_PX - 3} fill={t.mutedForeground}>
          {sub}
        </text>
      )}
    </g>
  )
}

export function MapCanvas(p: MapCanvasProps) {
  const { element: el, tokens: t } = p
  const stage = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState({ w: 0, h: 0 })
  useLayoutEffect(() => {
    const s = stage.current
    if (!s) return
    const measure = () => setSize({ w: s.clientWidth, h: s.clientHeight })
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(s)
    return () => ro.disconnect()
  }, [])
  // O8b: chosen per layout pass; the direction is part of the hash, so crossing the threshold is a new layout
  const direction = directionFor(size.w, size.h)
  const eg: ElementGraph = useMemo(() => elementGraph(el, direction), [el, direction])
  const box = thumbBox(p.popped)
  const thumbIds = useMemo(() => new Set(eg.thumbs.keys()), [eg])
  // ELK reserves label boxes: the *why* alone (≤ 4 words) keeps the picture compact; the evidence count joins at draw time
  const elkGraph = useMemo<GraphData>(() => ({ ...eg.graph, edges: eg.graph.edges.map((e, i) => ({ ...e, label: eg.edges[i]?.label || e.label })) }), [eg])
  const hash = useMemo(() => fnv(`${structuralHash(elkGraph)}|${[...thumbIds].join(',')}|${box.w}x${box.h}`), [elkGraph, thumbIds, box.w, box.h])
  const shown = useElkLayout(hash, () => toElkSized(elkGraph, thumbIds, box), flattenElk, size.w > 0)
  const thumbLayouts = useThumbLayouts(eg.thumbs)

  // unit contract: nodes then edges; the selected child and the search chain's nodes draw `active`
  const childByNode = eg.byNode
  const nodeOfChild = useMemo(() => {
    const m = new Map<string, string>()
    for (const [node, c] of childByNode) m.set(c.id, node)
    return m
  }, [childByNode])
  const selectedNode = p.selected ? (nodeOfChild.get(p.selected) ?? null) : null
  const highlight = useMemo(() => {
    const ids = selectedNode ? [selectedNode] : []
    for (const [node, c] of childByNode) if (p.chain.has(c.id) && node !== selectedNode) ids.push(node)
    return ids
  }, [selectedNode, childByNode, p.chain])
  const nodeIds = useMemo(() => eg.graph.nodes.map((n) => n.id), [eg])
  const onPress = (id: string) => {
    if (id.startsWith('edge:')) return
    const c = childByNode.get(id)
    if (!c) return
    p.onSelect(p.selected === c.id ? null : c.id)
  }
  const units = useGraphUnits(nodeIds, eg.graph.edges, highlight, onPress)
  const unitSel = units.state.selected
  useEffect(() => {
    if (unitSel === selectedNode) return
    units.dispatch({ type: 'clear' })
    if (selectedNode) units.dispatch({ type: 'select', id: selectedNode })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- sync the unit selection to the panel's; `units` is stable per render pass
  }, [selectedNode])
  // `DiagramFrame`'s surface takes pointer capture on pointerdown (pan/pinch), so `click` lands on the surface, not the
  // unit under the finger: remember the unit at pointerdown and resolve a still pointer (< 5 px) at pointerup ourselves.
  // Two presses on one node within 350 ms = drill (the chip presses drill at once); keyboard stays with the unit contract.
  const pressRef = useRef<{ id: string; drill: boolean; x: number; y: number; target: Element } | null>(null)
  const lastPress = useRef<{ id: string; at: number } | null>(null)
  const pointerDown = (id: string, drillChip: boolean) => (e: React.PointerEvent) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return
    pressRef.current = { id, drill: drillChip, x: e.clientX, y: e.clientY, target: e.currentTarget }
  }
  const pointerUp = (e: React.PointerEvent) => {
    const pr = pressRef.current
    pressRef.current = null
    if (!pr || Math.hypot(e.clientX - pr.x, e.clientY - pr.y) >= 5) return
    const twice = lastPress.current && lastPress.current.id === pr.id && e.timeStamp - lastPress.current.at < 350
    lastPress.current = { id: pr.id, at: e.timeStamp }
    if (pr.id.startsWith('edge:')) return units.dispatch({ type: 'select', id: pr.id })
    if (pr.drill || twice) return drill(pr.id, pr.target)
    units.dispatch({ type: 'select', id: pr.id })
    onPress(pr.id)
  }

  // zoom (label hiding) measured off the mounted svg
  const svgRef = useRef<SVGSVGElement>(null)
  const layout = shown && 'layout' in shown ? shown.layout : null
  const W = layout ? Math.ceil(layout.width + 2 * MARGIN) : 0
  const H = layout ? Math.ceil(layout.height + 2 * MARGIN) : 0
  const zoom = useZoom(svgRef, W, hash)

  // drill animation: a frame grows from the node's box to the stage (in) or shrinks to the centre (out), then the picture changes
  const [anim, setAnim] = useState<{ from: { x: number; y: number; w: number; h: number }; to: { x: number; y: number; w: number; h: number }; on: boolean } | null>(null)
  const timer = useRef(0)
  useEffect(() => () => clearTimeout(timer.current), [])
  const play = (from: DOMRect | null, dir: 'in' | 'out', then: () => void) => {
    const s = stage.current?.getBoundingClientRect()
    if (!s || reducedMotion()) return then()
    const full = { x: 0, y: 0, w: s.width, h: s.height }
    const centre = { x: s.width * 0.3, y: s.height * 0.3, w: s.width * 0.4, h: s.height * 0.4 }
    const node = from ? { x: from.left - s.left, y: from.top - s.top, w: from.width, h: from.height } : centre
    setAnim(dir === 'in' ? { from: node, to: full, on: false } : { from: full, to: centre, on: false })
    requestAnimationFrame(() => setAnim((a) => (a ? { ...a, on: true } : a)))
    timer.current = window.setTimeout(() => {
      setAnim(null)
      then()
    }, ANIM_MS)
  }
  const drill = (node: string, target: EventTarget | null) => {
    const c = childByNode.get(node)
    if (!c) return
    if (c.leaf) return p.onSelect(c.id)
    const g = (target as Element | null)?.closest?.('[data-node]') ?? null
    play(g?.getBoundingClientRect() ?? null, 'in', () => p.onDrill(c.id))
  }
  const up = () => play(null, 'out', p.onUp)
  const canUp = el.crumbs.length > 1

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && !unitSel && canUp) {
      e.stopPropagation()
      up()
    } else if (e.key === 'Backspace' && canUp && (e.target as HTMLElement).tagName !== 'INPUT') {
      e.preventDefault()
      up()
    } else units.root.onKeyDown(e)
  }

  let content: ReactNode
  if (layout) {
    const a = svgA11y(`map-${hash}`, true)
    const markerId = `map-arrow-${hash}`
    const hatchId = `map-hatch-${hash}`
    const nodes: Record<string, GraphData['nodes'][number]> = {}
    for (const n of eg.graph.nodes) nodes[n.id] = n
    content = (
      <svg ref={svgRef} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} data-map-direction={direction} data-zoom={zoom}>
        <title id={a.titleId}>{el.name}</title>
        <desc id={a.descId}>{elementStats(el)}</desc>
        <defs>
          <ArrowMarkers id={markerId} tokens={t} />
          <pattern id={hatchId} width={6} height={6} patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <line x1={0} y1={0} x2={0} y2={6} stroke={t.danger} strokeWidth={1.5} opacity={0.4} />
          </pattern>
        </defs>
        <g transform={`translate(${MARGIN} ${MARGIN})`}>
          {layout.edges.map((e) => {
            const edge = eg.graph.edges[e.index]
            if (!edge) return null
            const meta = eg.edges[e.index]
            const st = units.paint(edgeUnit(e.index), edge.state)
            const pt = paint(st, t)
            const d = sectionsPath(e.sections)
            const label = edge.label && edgeLabelShown(zoom, edge, selectedNode) ? edgeLabelAt(e) : null
            const full = meta ? evidenceSummary(meta) : (edge.label ?? '')
            const aria = [nodes[edge.from]?.label ?? edge.from, '→', nodes[edge.to]?.label ?? edge.to, edge.label ? `: ${edge.label}` : '', meta && !meta.backed ? ' (inferred)' : ''].join(' ')
            return (
              <g key={e.index} opacity={pt.opacity} data-edge={meta ? (meta.backed ? 'backed' : 'inferred') : 'diagram'} {...units.unit(edgeUnit(e.index), aria)} onClick={undefined} onPointerDown={pointerDown(edgeUnit(e.index), false)}>
                <title>{full}</title>
                <path d={d} fill="none" stroke="transparent" strokeWidth={18} />
                <path d={d} fill="none" stroke={pt.stroke} strokeWidth={st === 'active' ? 2 : 1.25} strokeDasharray={edge.style === 'dashed' ? '6 4' : undefined} {...markers(edge.arrow, markerFor(markerId, st))} />
                {label && <EdgeText text={meta ? meta.label || edge.label! : edge.label!} sub={meta ? evidenceCount(meta.evidence) || undefined : undefined} at={label.at} along={label.along} side={label.side} t={t} fill={meta && !meta.backed ? t.mutedForeground : pt.stroke} />}
              </g>
            )
          })}
          {layout.nodes.map((n) => {
            const node = nodes[n.id]
            if (!node) return null
            const c = childByNode.get(n.id)
            const st = units.paint(n.id, node.state)
            const s = paint(st, t)
            const stale = eg.hatch.includes(n.id)
            const thumb = eg.thumbs.has(n.id) ? thumbLayouts.get(n.id) : undefined
            const isThumb = thumbIds.has(n.id)
            const marks = c ? (p.marks.get(c.id) ?? []) : []
            const attrs = units.unit(n.id, unitLabel(node.label, node.sub, stale ? 'stale' : st))
            const onKeyDown = (ev: KeyboardEvent) => {
              if (ev.key === 'Enter' && c && !c.leaf) {
                ev.preventDefault()
                ev.stopPropagation()
                drill(n.id, ev.currentTarget)
              } else attrs.onKeyDown(ev)
            }
            const lines = wrapLabel(node.label).length
            const extra = (node.sub ? 1 : 0) + (node.tag ? 1 : 0)
            const top = n.y + n.h / 2 - ((lines + extra - 1) * LINE_PX) / 2
            const kind = node.kind ?? (c && !c.leaf ? 'rounded' : 'box')
            return (
              <g
                key={n.id}
                className="codemap-node"
                opacity={s.opacity}
                data-node={n.id}
                data-element={c?.id}
                data-stale={stale || undefined}
                data-thumb={isThumb || undefined}
                aria-description={node.ref ?? undefined}
                {...attrs}
                onClick={undefined}
                onPointerDown={pointerDown(n.id, false)}
                onKeyDown={onKeyDown}
              >
                <rect x={n.x - 4} y={n.y - 4} width={n.w + 8} height={n.h + 8} fill="transparent" />
                <Shape kind={kind} x={n.x} y={n.y} w={n.w} h={n.h} fill={s.fill} fillOpacity={s.fillOpacity} stroke={stale ? t.danger : s.stroke} strokeWidth={s.strokeWidth} />
                {stale && <rect x={n.x} y={n.y} width={n.w} height={n.h} rx={kind === 'rounded' ? 8 : undefined} fill={`url(#${hatchId})`} pointerEvents="none" />}
                {isThumb ? (
                  <>
                    <text x={n.x + 10} y={n.y + 16} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
                      {node.label}
                    </text>
                    {node.sub && (
                      <text x={n.x + 10} y={n.y + 30} fontFamily={MONO} fontSize={10} fill={t.mutedForeground}>
                        {node.sub.length > 28 ? `${node.sub.slice(0, 27)}…` : node.sub}
                      </text>
                    )}
                    {thumb && <Thumbnail graph={eg.thumbs.get(n.id)!} layout={thumb} area={{ x: n.x + 10, y: n.y + THUMB_TITLE_H, w: n.w - 20, h: n.h - THUMB_TITLE_H - THUMB_CHIP_H - 6 }} t={t} />}
                    <g
                      data-drill
                      transform={`translate(${n.x + n.w - 10} ${n.y + n.h - THUMB_CHIP_H + 2})`}
                      onPointerDown={(ev) => {
                        ev.stopPropagation()
                        pointerDown(n.id, true)(ev)
                      }}
                    >
                      <rect x={-72} y={-13} width={72} height={15} rx={7.5} fill={t.muted} />
                      <text x={-36} y={-5.5} textAnchor="middle" fontFamily={MONO} fontSize={9.5} fill={t.foreground}>
                        {c?.files && c.leaf ? `${c.files} files ›` : `${eg.thumbs.get(n.id)?.nodes.length ?? 0} inside ›`}
                      </text>
                    </g>
                  </>
                ) : (
                  <>
                    <Label text={node.label} cx={n.x + n.w / 2} cy={top + ((lines - 1) * LINE_PX) / 2} fill={t.foreground} t={t} weight={600} />
                    {node.sub && (
                      <text x={n.x + n.w / 2} y={top + lines * LINE_PX} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                        {node.sub}
                      </text>
                    )}
                    {node.tag && (
                      <text x={n.x + n.w / 2} y={top + (lines + (node.sub ? 1 : 0)) * LINE_PX} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={10} fill={t.mutedForeground}>
                        {node.tag}
                      </text>
                    )}
                  </>
                )}
                {stale && (
                  <text x={n.x + n.w - 4} y={n.y - 3} textAnchor="end" fontFamily={MONO} fontSize={10} fill={t.danger} letterSpacing="0.08em" data-badge="stale">
                    stale
                  </text>
                )}
                {marks.length > 0 && (
                  <g data-marks={marks.join(' ')} transform={`translate(${n.x + n.w - 8} ${n.y + 8})`}>
                    <title>{marks.join(' and ')} this session</title>
                    <circle r={3.5} fill={marks.includes('edited') ? t.accent : t.card} stroke={t.accent} strokeWidth={1.5} />
                  </g>
                )}
              </g>
            )
          })}
        </g>
      </svg>
    )
  } else if (shown && 'timeout' in shown) {
    content = <pre className="p-3 font-mono text-[11px] leading-5 whitespace-pre-wrap">{adjacencyText(eg.graph)}</pre>
  }

  const labels = new Map<string, string>()
  for (const n of eg.graph.nodes) labels.set(n.id, n.label)
  const foot = footnote(eg, labels)
  const fitPct = Math.round(zoom * 100)

  return (
    <section className="flex h-full min-h-0 flex-col" data-map-canvas data-view={el.id} onKeyDown={onKey}>
      <nav aria-label="Breadcrumbs" className="flex h-[30px] shrink-0 items-center gap-1.5 overflow-hidden border-b border-border-subtle px-2.5 whitespace-nowrap text-fg-secondary">
        {el.crumbs.map(([id, name], i) => {
          const last = i === el.crumbs.length - 1
          return (
            <span key={id} className="flex items-center gap-1.5">
              {i > 0 && <span className="opacity-60">›</span>}
              {last ? (
                <b className="text-foreground">{name}</b>
              ) : (
                <button type="button" className="hover:underline" onClick={() => p.onCrumb(id)}>
                  {name}
                </button>
              )}
            </span>
          )
        })}
        <span className="opacity-60">·</span>
        <span className="truncate">{elementStats(el)}</span>
        {canUp && (
          <button type="button" className="ml-auto rounded border border-border-subtle px-1.5 text-foreground hover:bg-layer-2" onClick={up} title="Up one level (Esc)">
            ‹ Back <i className="not-italic text-fg-secondary">Esc</i>
          </button>
        )}
      </nav>
      <div ref={stage} className="relative min-h-0 flex-1 p-1" onPointerUp={pointerUp} onPointerCancel={() => (pressRef.current = null)}>
        {shown ? (
          <DiagramFrame name={`codemap-map-${basename(el.path ?? el.id)}-${hash}`} label={`map · ${el.name} · ${direction}`} kind="codemap-map" fill>
            {content}
          </DiagramFrame>
        ) : (
          <p className="p-3 text-fg-secondary">laying out…</p>
        )}
        {layout && (
          <div className="pointer-events-none absolute right-3 bottom-2 flex items-center gap-1 rounded border border-border-subtle bg-layer-1 px-1.5 py-0.5 text-fg-secondary" data-zoom-strip title="scroll or pinch to zoom · double-click to fit">
            <span className="text-foreground">{fitPct}%</span>
          </div>
        )}
        {anim && (
          <div
            aria-hidden
            className={cn('codemap-zoom pointer-events-none absolute rounded-md border-2 border-fg-accent bg-layer-2/60', anim.on && 'on')}
            style={{ left: anim.from.x, top: anim.from.y, width: anim.from.w, height: anim.from.h, '--zx': `${anim.to.x - anim.from.x}px`, '--zy': `${anim.to.y - anim.from.y}px`, '--zsx': anim.to.w / anim.from.w, '--zsy': anim.to.h / anim.from.h } as React.CSSProperties}
          />
        )}
      </div>
      {foot && (
        <p className="shrink-0 truncate px-3 py-1 text-[10.5px] text-fg-secondary" title={foot}>
          {foot}
        </p>
      )}
    </section>
  )
}
