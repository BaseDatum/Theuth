/**
 * 147 §11.2 — the sidebar tree: perspectives as roots, elements pre-order to depth 3, virtualised 22 px mono rows
 * (`@tanstack/react-virtual`, Here's list). Row = fold toggle · name · path suffix · badges (`stale`, `imported`,
 * `read`/`edited` marks, `+N` bundles). Click = select (the canvas drills), ▸/▾ = fold only. Keys: ↑/↓ move, →/← fold,
 * Enter drill, Esc up. Collapsible to a 28 px rail.
 */
import { useVirtualizer } from '@tanstack/react-virtual'
import { type CSSProperties, type KeyboardEvent, useEffect, useRef, useState } from 'react'
import { shortLabels } from '@/lib/codemap/labels'
import { type TreeRow, treeKey } from '@/lib/codemap/map'
import { cn } from '@/lib/utils'
import './codemap.css'

const ROW = 22

export type MapTreeProps = {
  rows: TreeRow[]
  selected: string | null
  /** Elements lit by the search chain (§11.3). */
  chain: ReadonlySet<string>
  /** Perspective id → status word while generation runs (`generating…`, `queued`). */
  status?: ReadonlyMap<string, string>
  onSelect(id: string): void
  onToggle(id: string, open: boolean): void
  onDrill(id: string): void
  onUp(): void
  /** Static (non-virtualised) render for phones/tests. */
  static?: boolean
}

/** The badges after a row's label: every count drawn, colour states with their word (73 §6.5). */
export function RowBadges({ r, short }: { r: TreeRow; short?: string }) {
  return (
    <>
      {short && <span className="truncate text-fg-tertiary">{short}</span>}
      {r.marks.includes('edited') && <span className="codemap-mark edited" title="edited this session" aria-label="edited" />}
      {r.marks.includes('read') && <span className="codemap-mark read" title="read this session" aria-label="read" />}
      {r.stale && <span className="codemap-badge stale">stale</span>}
      {r.kind === 'perspective' && r.staleCount > 0 && <span className="codemap-badge stale">stale {r.staleCount}</span>}
      {r.origin === 'imported' && r.kind === 'perspective' && <span className="codemap-badge imp">imported</span>}
      {r.bundle && <span className="text-fg-secondary">+{r.count}</span>}
    </>
  )
}

export function MapTree(p: MapTreeProps) {
  const ref = useRef<HTMLDivElement>(null)
  const [focus, setFocus] = useState(0)
  const rows = p.rows
  const short = shortLabels(rows.map((r) => r.path ?? '').filter(Boolean))
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize: () => ROW,
    overscan: 8,
    getItemKey: (i) => rows[i]?.id ?? i,
    enabled: !p.static,
  })
  // keep the roving focus on the selected row when the selection moves from outside (canvas drill, search)
  useEffect(() => {
    const i = rows.findIndex((r) => r.id === p.selected)
    if (i >= 0) {
      setFocus(i)
      if (!p.static) v.scrollToIndex(i, { align: 'auto' })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- rows identity changes with every fold; the selection is the trigger
  }, [p.selected])

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const a = treeKey(e.key, rows, focus)
    if (!a) return
    e.preventDefault()
    e.stopPropagation()
    if (a.type === 'move') {
      setFocus(a.to)
      if (!p.static) v.scrollToIndex(a.to, { align: 'auto' })
      requestAnimationFrame(() => ref.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(rows[a.to]!.id)}"]`)?.focus())
    } else if (a.type === 'open') p.onToggle(a.id, true)
    else if (a.type === 'close') p.onToggle(a.id, false)
    else if (a.type === 'drill') p.onDrill(a.id)
    else p.onUp()
  }

  const row = (r: TreeRow, i: number, style?: CSSProperties) => {
    const sel = r.id === p.selected
    const status = r.kind === 'perspective' ? p.status?.get(r.id) : undefined
    const sh = r.path ? short.get(r.path) : undefined
    return (
      <div
        key={r.id}
        role="treeitem"
        aria-level={r.depth + 1}
        aria-selected={sel}
        aria-expanded={r.foldable ? r.open : undefined}
        tabIndex={i === focus ? 0 : -1}
        data-row={r.id}
        data-depth={r.depth}
        style={{ ...style, paddingLeft: 6 + r.depth * 12 }}
        className={cn(
          'codemap-tree-row flex h-[22px] cursor-pointer items-center gap-1 pr-1.5 whitespace-nowrap hover:bg-layer-2 focus:bg-layer-2 focus:outline-none',
          r.kind === 'perspective' && 'font-semibold',
          sel && 'codemap-tree-sel bg-layer-3',
          p.chain.size > 0 && (p.chain.has(r.id) ? 'codemap-chain' : 'opacity-50'),
          status && 'text-fg-secondary',
        )}
        onClick={() => {
          setFocus(i)
          p.onSelect(r.id)
        }}
        onDoubleClick={() => p.onDrill(r.id)}
        title={r.path ?? r.name}
      >
        {status === 'generating…' ? (
          <span className="codemap-spin" aria-label="generating" />
        ) : r.foldable ? (
          <button
            type="button"
            tabIndex={-1}
            aria-label={r.open ? 'fold' : 'unfold'}
            className="w-[9px] shrink-0 text-center text-fg-secondary"
            onClick={(e) => {
              e.stopPropagation()
              p.onToggle(r.id, !r.open)
            }}
          >
            {r.open ? '▾' : '▸'}
          </button>
        ) : (
          <span className="w-[9px] shrink-0 text-center text-fg-tertiary">{status ? '·' : ''}</span>
        )}
        <span className="min-w-0 flex-1 truncate">{r.name}</span>
        <RowBadges r={r} short={r.kind === 'element' && sh && sh !== r.name ? sh : undefined} />
        {r.kind === 'perspective' && <span className="font-normal text-fg-secondary">{status ?? r.count}</span>}
        {r.kind === 'element' && r.foldable && !r.bundle && <span className="text-fg-secondary">{r.count}</span>}
      </div>
    )
  }

  if (p.static) {
    return (
      <div role="tree" aria-label="Map tree" className="font-mono text-[11px]" onKeyDown={onKey}>
        {rows.map((r, i) => row(r, i))}
      </div>
    )
  }
  return (
    <div ref={ref} role="tree" aria-label="Map tree" className="h-full min-h-0 overflow-auto font-mono text-[11px]" onKeyDown={onKey}>
      <div className="relative w-full" style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((item) => row(rows[item.index]!, item.index, { position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }))}
      </div>
    </div>
  )
}
