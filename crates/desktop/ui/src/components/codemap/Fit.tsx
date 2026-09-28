// 73 §6.3 — the butterfly: callers | focus | callees over `columnsLayout`; under 600 px the two lists (§6.9).
import type { PointerEvent } from 'react'
import { GraphSvg } from '@/components/chat/surface/Graph'
import { anchorOf, kindGlyph, shortLabels } from '@/lib/codemap/labels'
import { columnsLayout, type Slots } from '@/lib/codemap/layout'
import type { FitEdge, FitLens, FitNode } from '@/lib/codemap/types'
import type { DiagramTokens } from '@/lib/diagram-theme'
import './codemap.css'

export type FitProps = {
  fit: FitLens
  width: number
  popped: boolean
  stale: ReadonlySet<string>
  selected: string | null
  dim?: boolean
  tokens: DiagramTokens
  slots: Slots
  onSelect(id: string): void
  onHover(id: string | null): void
  onContext(id: string, at: { x: number; y: number }): void
  onExpandBundle(side: 'callers' | 'callees'): void
}

/** The `data-node` id under an event target (GraphSvg wraps every node in `<g data-node>`). */
export function nodeAt(target: EventTarget | null): string | null {
  return (target as Element | null)?.closest?.('[data-node]')?.getAttribute('data-node') ?? null
}

export function Fit(p: FitProps) {
  if (p.width < 600) return <FitList {...p} />
  const placed = columnsLayout(p.fit, p.slots, p.width, { stale: p.stale, limit: p.popped ? 40 : 20 })
  return (
    <div
      className="relative font-mono text-[11px]"
      style={{ opacity: p.dim ? 0.5 : 1 }}
      onPointerOver={(e) => p.onHover(nodeAt(e.target))}
      onPointerOut={(e: PointerEvent) => nodeAt(e.target) && !nodeAt(e.relatedTarget) && p.onHover(null)}
      onContextMenu={(e) => {
        const id = nodeAt(e.target)
        if (!id) return
        e.preventDefault()
        p.onContext(id, { x: e.clientX, y: e.clientY })
      }}
    >
      {/* 73 §6.5: no arrowheads in Fit (markerId null); every coloured node carries its badge word. */}
      <GraphSvg
        graph={placed.graph}
        layout={placed.layout}
        tokens={p.tokens}
        markerId={null}
        patternId="codemap"
        badges={placed.badges}
        hatch={placed.hatch}
        nodeClass="codemap-node"
        highlight={p.selected ? [p.selected] : []}
        onSelect={(id) => (id.startsWith('bundle:') ? p.onExpandBundle(id === 'bundle:0' ? 'callers' : 'callees') : p.onSelect(id))}
      />
    </div>
  )
}

/** One list row (FitList, ChangeList). */
export const ROW = 'flex w-full items-center gap-2 px-2 py-0.5 text-left hover:bg-muted/50 focus:outline-none focus-visible:bg-muted/50'

/** Header row = focus, then callers and callees as lists with `+N more` rows; edge style as ━━━ / ╍╍╍ (§6.3 phone). */
export function FitList(p: Omit<FitProps, 'width' | 'slots' | 'tokens'>) {
  const { fit } = p
  const labels = shortLabels([...fit.callers, ...fit.callees].map((n) => n.path))
  const row = (n: FitNode, e: FitEdge | undefined) => (
    <button
      key={n.moniker}
      type="button"
      className={p.selected === n.moniker ? `${ROW} bg-muted` : ROW}
      title={anchorOf(n.path, n.line_start, n.line_end)}
      onClick={() => p.onSelect(n.moniker)}
      onPointerEnter={() => p.onHover(n.moniker)}
      onPointerLeave={() => p.onHover(null)}
      onContextMenu={(ev) => {
        ev.preventDefault()
        p.onContext(n.moniker, { x: ev.clientX, y: ev.clientY })
      }}
    >
      <span className="text-muted-foreground">{labels.get(n.path) ?? n.path}</span>
      <span className="truncate">
        {kindGlyph(n.kind)} {n.name}
      </span>
      {n.is_test && <span className="text-muted-foreground">test</span>}
      {p.stale.has(n.moniker) && <span className="text-muted-foreground">stale</span>}
      {e && e.count > 1 && <span className="text-muted-foreground">×{e.count}</span>}
      {/* 73 §6.5: solid = confidence 1.0, dashed < 1.0 — the glyph, never a number on the row */}
      <span className="ml-auto text-muted-foreground">{e ? (e.confidence < 1 ? '╍╍╍' : '━━━') : ''}</span>
    </button>
  )
  const side = (title: 'callers' | 'callees', nodes: FitNode[], more: number, edgeOf: (n: FitNode) => FitEdge | undefined) => (
    <section>
      <h3 className="px-2 pt-2 text-muted-foreground">
        {title} ({nodes.length})
      </h3>
      {nodes.map((n) => row(n, edgeOf(n)))}
      {more > 0 && (
        <button type="button" className={`${ROW} text-muted-foreground`} onClick={() => p.onExpandBundle(title)}>
          +{more} more
        </button>
      )}
    </section>
  )
  const f = fit.focus
  return (
    <div className="font-mono text-[11px]" style={{ opacity: p.dim ? 0.5 : 1 }}>
      <button type="button" className={`${ROW} font-semibold`} title={anchorOf(f.path, f.line_start, f.line_end)} onClick={() => p.onSelect(f.moniker)}>
        <span>
          {kindGlyph(f.kind)} {f.name}
        </span>
        <span className="text-muted-foreground">
          · {f.path}:{f.line_start} · {f.kind}
          {fit.ambiguous > 0 && ` · ?${fit.ambiguous}`}
        </span>
      </button>
      {side('callers', fit.callers, fit.more_callers, (n) => fit.edges.find((e) => e.from === n.moniker && e.to === f.moniker))}
      {side('callees', fit.callees, fit.more_callees, (n) => fit.edges.find((e) => e.from === f.moniker && e.to === n.moniker))}
    </div>
  )
}
