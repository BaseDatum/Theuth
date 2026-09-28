/** 73 §6.2 — Lens A, *What's here*: a virtualised tree of dir / file / symbol rows. No edges, no animation. */
import { useVirtualizer } from '@tanstack/react-virtual'
import { type CSSProperties, type KeyboardEvent, useMemo, useRef, useState } from 'react'
import { buildHereRows, type HereRow } from '@/lib/codemap/here'
import { anchorOf, kindGlyph } from '@/lib/codemap/labels'
import type { HereLens, MapRow } from '@/lib/codemap/types'
import { cn } from '@/lib/utils'

export type HereProps = {
  here: HereLens
  focus: readonly string[]
  edited: readonly string[]
  marker: string | null
  stale: boolean
  dim?: boolean
  onHover(row: MapRow | null): void
  onSelect(row: MapRow): void
  onContext(row: MapRow, at: { x: number; y: number }): void
  height?: number
}

type Handlers = Pick<HereProps, 'onHover' | 'onSelect' | 'onContext'> & { onToggle?(key: string, open: boolean): void }

const ROW = 22
const MUTED = 'text-muted-foreground'

function Row({ r, dim, h, style, index }: { r: HereRow; dim?: boolean; h: Handlers; style?: CSSProperties; index?: number }) {
  const sym = r.kind === 'symbol' ? r.row : null
  const open = r.kind === 'dir' || r.kind === 'file' ? r.open : null
  const activate = () => (sym ? h.onSelect(sym) : h.onToggle?.(r.key, r.kind === 'more' || !open))
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const el = e.currentTarget
    if (e.key === 'ArrowDown') (el.nextElementSibling as HTMLElement | null)?.focus()
    else if (e.key === 'ArrowUp') (el.previousElementSibling as HTMLElement | null)?.focus()
    else if (e.key === 'ArrowRight' && open === false) h.onToggle?.(r.key, true)
    else if (e.key === 'ArrowLeft' && open === true) h.onToggle?.(r.key, false)
    else if (e.key === 'Enter') activate()
    else return
    e.preventDefault()
  }
  let title: string | undefined
  let body
  // 73 §6.5: every count is drawn (hidden only at 0), kind is a glyph, chips are words; labels whole (row clips, title carries the path).
  if (r.kind === 'dir') {
    title = r.path
    body = (
      <>
        <span>{r.path}</span>
        {r.coverage && <span className={MUTED}>{`(${r.files} files)`}</span>}
        {r.importers > 0 && <span className={MUTED}>{`→${r.importers}`}</span>}
      </>
    )
  } else if (r.kind === 'file') {
    title = r.path
    const parts = r.path.split('/')
    body = (
      <>
        <span>{(r.edited ? '~' : '') + parts.slice(Math.min(2, parts.length - 1)).join('/')}</span>
        {r.importers > 0 && <span className={MUTED}>{`→${r.importers}`}</span>}
        {r.callers > 0 && <span className={MUTED}>{`↑${r.callers}`}</span>}
        {r.recent && <span className={MUTED}>new</span>}
        {r.entry && <span className={MUTED}>◆entry</span>}
        {r.isTest && <span className={MUTED}>test</span>}
      </>
    )
  } else if (sym) {
    title = anchorOf(sym.path, sym.line, sym.line_end)
    body = (
      <>
        <span>{`${kindGlyph(sym.kind)} ${sym.name}`}</span>
        <span className={MUTED}>{sym.kind}</span>
        <span className={MUTED}>{`L${sym.line}`}</span>
        {sym.callers > 0 && <span className={MUTED}>{`↑${sym.callers}`}</span>}
      </>
    )
  } else if (r.kind === 'more') {
    body = <span className={MUTED}>{`+${r.count} ${r.depth === 2 ? 'more' : 'directories'}`}</span>
  }
  return (
    <div
      role="button"
      tabIndex={0}
      data-index={index}
      style={style}
      title={title}
      className={cn(
        'flex h-[22px] items-center gap-1.5 overflow-hidden whitespace-nowrap px-1 font-mono text-[11px] outline-none hover:bg-accent/40 focus-visible:bg-accent/60',
        dim && 'opacity-50',
      )}
      onClick={activate}
      onKeyDown={onKey}
      onMouseEnter={() => h.onHover(sym)}
      onMouseLeave={() => h.onHover(null)}
      onContextMenu={
        sym
          ? (e) => {
              e.preventDefault()
              h.onContext(sym, { x: e.clientX, y: e.clientY })
            }
          : undefined
      }
    >
      <span className="w-3 shrink-0">{(r.kind === 'symbol' || r.kind === 'file') && r.marker ? '►' : ''}</span>
      <span className="w-2 shrink-0 text-primary">{(r.kind === 'symbol' || r.kind === 'file') && r.focus ? '▎' : ''}</span>
      <span className="shrink-0" style={{ width: r.depth * 12 }} />
      {open != null && <span className={cn('w-3 shrink-0', MUTED)}>{open ? '▾' : '▸'}</span>}
      {body}
    </div>
  )
}

/** Static list for the phone Sheet and tests; `onToggle` optional (no-op when absent). */
export function HereList({ rows, dim, ...h }: Handlers & { rows: HereRow[]; dim?: boolean }) {
  return (
    <div>
      {rows.map((r, i) => (
        <Row key={r.key} r={r} index={i} dim={dim} h={h} />
      ))}
    </div>
  )
}

export function Here(p: HereProps) {
  const [expanded, setExpanded] = useState(() => new Set<string>())
  const [collapsed, setCollapsed] = useState(() => new Set<string>())
  const rows = useMemo(
    () => buildHereRows({ here: p.here, expanded, collapsed, focus: p.focus, edited: p.edited, marker: p.marker }),
    [p.here, expanded, collapsed, p.focus, p.edited, p.marker],
  )
  const ref = useRef<HTMLDivElement>(null)
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize: () => ROW,
    overscan: 8,
    getItemKey: (i) => rows[i]?.key ?? i,
  })
  const h: Handlers = {
    onHover: p.onHover,
    onSelect: p.onSelect,
    onContext: p.onContext,
    onToggle: (key, open) => {
      const flip = (add: boolean) => (s: Set<string>) => {
        const n = new Set(s)
        if (add) n.add(key)
        else n.delete(key)
        return n
      }
      setExpanded(flip(open))
      setCollapsed(flip(!open))
    },
  }
  return (
    <div ref={ref} className="h-full min-h-0 overflow-auto" style={p.height ? { height: p.height } : undefined}>
      <div className="relative w-full" style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((item) => (
          <Row
            key={item.key}
            r={rows[item.index]!}
            index={item.index}
            dim={p.dim}
            h={h}
            style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}
          />
        ))}
      </div>
    </div>
  )
}
