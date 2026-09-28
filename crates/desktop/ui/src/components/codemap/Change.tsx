// 73 §6.4 — the impact ripple: four bands over `bandsLayout`, no edges at rest; the selection's edges are an overlay.
// Under 600 px the four sections as lists (§6.9).
import { type PointerEvent, useState } from 'react'
import { GraphSvg } from '@/components/chat/surface/Graph'
import { shortLabels } from '@/lib/codemap/labels'
import { bandsLayout, type Slots } from '@/lib/codemap/layout'
import type { ChangeFile, ChangeLens } from '@/lib/codemap/types'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { nodeAt, ROW } from './Fit'

export type ChangeProps = {
  change: ChangeLens
  width: number
  popped: boolean
  selected: string | null
  dim?: boolean
  tokens: DiagramTokens
  slots: Slots
  onSelect(id: string): void
  onHover(id: string | null): void
  onContext(id: string, at: { x: number; y: number }): void
  onExpandBundle(layer: number): void
}

const BANDS = ['changed', 'direct', 'transitive', 'tests'] as const
const BADGE = ['changed', 'impacted', 'transitive', 'test']
const STATUS: Record<string, string> = { modified: '~', added: '+', deleted: '−' }
const LINK: Record<string, string> = { ref: '✓', subject: '◇' }

/** Changed files lit by a selected direct file: `symbols[].name ∩ via ≠ ∅`; `<import>` in `via` lights every changed file (§6.4). */
export function litChanged(change: ChangeLens, selected: string | null): string[] {
  const d = selected === null ? undefined : change.direct.find((f) => f.path === selected)
  if (!d) return []
  if (d.via.includes('<import>')) return change.changed.map((f) => f.path)
  return change.changed.filter((f) => f.symbols.some((s) => d.via.includes(s.name))).map((f) => f.path)
}

/** `worktree · since 22b3945` / `a..b` / `N files` (§6.4). */
export function modeChip(c: ChangeLens): string {
  if (c.mode === 'range' && c.range) return `${c.range[0]}..${c.range[1]}`
  if (c.mode === 'files') return `${c.changed.length + c.more[0]} files`
  return c.meta.git_head ? `worktree · since ${c.meta.git_head.slice(0, 7)}` : 'worktree'
}

export function Change(p: ChangeProps) {
  if (p.width < 600) return <ChangeList {...p} />
  const placed = bandsLayout(p.change, p.slots, p.width, { caps: p.popped ? [20, 30, 30, 30] : [12, 16, 16, 16] })
  const lit = litChanged(p.change, p.selected)
  const box: Record<string, { x: number; y: number; w: number; h: number }> = {}
  for (const n of placed.layout.nodes) box[n.id] = n
  const from = p.selected === null ? undefined : box[p.selected]
  // GraphSvg's 12 px margin and viewBox, so the overlay's coordinates are the layout's.
  const M = 12
  const W = Math.ceil(placed.layout.width + 2 * M)
  const H = Math.ceil(placed.layout.height + 2 * M)
  return (
    <div className="font-mono text-[11px]" style={{ opacity: p.dim ? 0.5 : 1 }}>
      <div className="flex justify-end px-2 py-1 text-muted-foreground">{modeChip(p.change)}</div>
      <div
        className="relative"
        onPointerOver={(e) => p.onHover(nodeAt(e.target))}
        onPointerOut={(e: PointerEvent) => nodeAt(e.target) && !nodeAt(e.relatedTarget) && p.onHover(null)}
        onContextMenu={(e) => {
          const id = nodeAt(e.target)
          if (!id) return
          e.preventDefault()
          p.onContext(id, { x: e.clientX, y: e.clientY })
        }}
      >
        {/* 73 §6.5: no edges at rest in Change — `placed.graph.edges` is []; only the selection's edges are drawn, below. */}
        <GraphSvg
          graph={placed.graph}
          layout={placed.layout}
          tokens={p.tokens}
          markerId={null}
          patternId="codemap"
          badges={placed.badges}
          hatch={placed.hatch}
          nodeClass="codemap-node"
          highlight={p.selected ? [p.selected, ...lit] : []}
          onSelect={(id) => (id.startsWith('bundle:') ? p.onExpandBundle(Number(id.slice(7))) : p.onSelect(id))}
        />
        {from && lit.length > 0 && (
          <svg
            className="pointer-events-none absolute left-0 top-0"
            viewBox={`0 0 ${W} ${H}`}
            width={W}
            height={H}
            style={{ maxWidth: '100%', height: 'auto' }}
            aria-hidden="true"
          >
            <g transform={`translate(${M} ${M})`} stroke={p.tokens.accent} strokeWidth={2} fill="none">
              {lit.map((id) => {
                const to = box[id]
                return to ? <line key={id} x1={from.x + from.w / 2} y1={from.y} x2={to.x + to.w / 2} y2={to.y + to.h} /> : null
              })}
            </g>
          </svg>
        )}
      </div>
      <div className="px-2 py-1 text-muted-foreground">
        {p.change.ambiguous_skipped} ambiguous skipped · {p.change.unresolved_imports} unresolved imports
      </div>
    </div>
  )
}

/** Four sections as lists; `+N` expands the capped section in place, the engine's `more[i]` beyond that asks the panel (§6.9 phone). */
export function ChangeList(p: Omit<ChangeProps, 'width' | 'slots' | 'tokens'>) {
  const [open, setOpen] = useState<number[]>([])
  const c = p.change
  const caps = p.popped ? [20, 30, 30, 30] : [12, 16, 16, 16]
  const labels = shortLabels(BANDS.flatMap((b) => c[b].map((f) => f.path)))
  const row = (f: ChangeFile, layer: number) => (
    <button
      key={f.path}
      type="button"
      className={p.selected === f.path ? `${ROW} bg-muted` : ROW}
      title={f.path}
      onClick={() => p.onSelect(f.path)}
      onPointerEnter={() => p.onHover(f.path)}
      onPointerLeave={() => p.onHover(null)}
      onContextMenu={(ev) => {
        ev.preventDefault()
        p.onContext(f.path, { x: ev.clientX, y: ev.clientY })
      }}
    >
      <span className="w-3 text-muted-foreground">{layer === 0 ? (STATUS[f.status ?? ''] ?? '') : layer === 3 ? (LINK[f.test_link ?? ''] ?? '') : ''}</span>
      <span>{labels.get(f.path) ?? f.path}</span>
      {/* 73 §6.5: every colour state carries its badge word */}
      <span className="text-muted-foreground">{BADGE[layer]}</span>
      {c.unindexed.includes(f.path) && <span className="text-muted-foreground">stale</span>}
      {layer === 0 && f.symbols.length > 0 && <span className="truncate text-muted-foreground">{f.symbols.map((s) => s.name).join(', ')}</span>}
      {layer === 1 && f.via.length > 0 && <span className="truncate text-muted-foreground">via {f.via.join(', ')}</span>}
    </button>
  )
  return (
    <div className="font-mono text-[11px]" style={{ opacity: p.dim ? 0.5 : 1 }}>
      {BANDS.map((b, layer) => {
        const files = c[b]
        const more = c.more[layer] ?? 0
        const shown = open.includes(layer) ? files : files.slice(0, caps[layer])
        const hidden = files.length - shown.length
        return (
          <section key={b}>
            <h3 className="flex px-2 pt-2 text-muted-foreground">
              <span>
                {b} ({files.length + more})
              </span>
              {layer === 0 && <span className="ml-auto">{modeChip(c)}</span>}
            </h3>
            {shown.map((f) => row(f, layer))}
            {hidden > 0 && (
              <button type="button" className={`${ROW} text-muted-foreground`} onClick={() => setOpen([...open, layer])}>
                +{hidden}
              </button>
            )}
            {hidden === 0 && more > 0 && (
              <button type="button" className={`${ROW} text-muted-foreground`} onClick={() => p.onExpandBundle(layer)}>
                +{more} more
              </button>
            )}
          </section>
        )
      })}
      <div className="px-2 py-1 text-muted-foreground">
        {c.ambiguous_skipped} ambiguous skipped · {c.unresolved_imports} unresolved imports
      </div>
    </div>
  )
}
