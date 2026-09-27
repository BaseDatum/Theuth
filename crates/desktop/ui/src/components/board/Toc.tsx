// Design 71 §2.2 / M9 — the TOC: `#/##/###` entries, the active one chosen by the M6 scroll rule, an always-open
// 248 px rail at ≥ 1360 px and a 32×32 pill (→ card) below that (R §B.3, §C.4). Picking an entry expands its
// section before scrolling (the caller's `onSelect`). Layout only; nothing here is stored.
import { Route, TableOfContents, X } from 'lucide-react'
import { type RefObject, useEffect, useState, useSyncExternalStore } from 'react'
import type { TocEntry } from './Sections'

/** R §B.3: the rail exists on a wide shell (≥ 1360 px). With the pane open both only fit on a very wide one. */
const WIDE = '(min-width: 1360px)'
const WIDE_WITH_PANE = '(min-width: 1940px)' // sidebar 260 + rail 248 + column 860 + pane 570

const subscribeWide = (cb: () => void) => {
  const a = matchMedia(WIDE)
  const b = matchMedia(WIDE_WITH_PANE)
  a.addEventListener('change', cb)
  b.addEventListener('change', cb)
  return () => {
    a.removeEventListener('change', cb)
    b.removeEventListener('change', cb)
  }
}

/** Rail (always-open TOC in the grid's first column) vs pill: the pane yields the rail unless the window is very wide. */
export function useRail(pinned: boolean): boolean {
  const wide = useSyncExternalStore(subscribeWide, () => matchMedia(WIDE).matches)
  const wider = useSyncExternalStore(subscribeWide, () => matchMedia(WIDE_WITH_PANE).matches)
  return wide && (!pinned || wider)
}

/**
 * Whiteboard's `activeTargetForScroll` (R §B.7): the first heading at or below the scroller's top is active if it
 * sits in the top half of the scroller, else the one before it; past the last heading, the last.
 */
export function activeHeading(tops: number[], viewportHeight: number): number {
  if (tops.length === 0) return -1
  const first = tops.findIndex((t) => t >= -1)
  if (first === -1) return tops.length - 1
  return tops[first]! <= viewportHeight / 2 ? first : Math.max(0, first - 1)
}

/** The active entry index, re-computed on scroll of `scroller` and whenever the entries change. */
export function useActiveEntry(scroller: RefObject<HTMLElement | null>, entries: TocEntry[]): string | null {
  const [active, setActive] = useState<string | null>(null)
  useEffect(() => {
    const el = scroller.current
    if (!el) return
    let frame = 0
    const measure = () => {
      frame = 0
      const top = el.getBoundingClientRect().top
      const tops = entries.map((e) => {
        const h = document.getElementById(e.id)
        return h ? h.getBoundingClientRect().top - top : Number.POSITIVE_INFINITY
      })
      const i = activeHeading(tops, el.clientHeight)
      setActive(i < 0 ? null : entries[i]!.id)
    }
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(measure)
    }
    measure()
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      el.removeEventListener('scroll', onScroll)
      if (frame) cancelAnimationFrame(frame)
    }
  }, [scroller, entries])
  return active
}

export type TocProps = { entries: TocEntry[]; active: string | null; onSelect: (entry: TocEntry) => void; onTour?: (by: string) => void }

export function Toc({ entries, active, onSelect, onTour }: TocProps) {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !e.defaultPrevented) setOpen(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])
  if (entries.length === 0) return <nav className="board-toc board-chrome" aria-label="Contents" hidden />
  return (
    <nav className="board-toc board-chrome" aria-label="Contents" data-open={open || undefined}>
      <button type="button" className="board-toc-pill" aria-label="Contents" aria-expanded={open} onClick={() => setOpen(true)}>
        <TableOfContents size={16} strokeWidth={1.75} aria-hidden />
      </button>
      <div className="board-toc-card">
        <div className="board-toc-head">
          <span>Contents</span>
          {onTour && (
            <button
              type="button"
              className="board-toc-tour"
              data-anchor-id="tour:board"
              title="Tour this board (T)"
              onClick={() => {
                setOpen(false)
                onTour('tour:board')
              }}
            >
              <Route size={12} strokeWidth={1.75} aria-hidden /> Tour
            </button>
          )}
          <button type="button" className="board-toc-close" aria-label="Close contents" onClick={() => setOpen(false)}>
            <X size={14} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
        <ol className="board-toc-list">
          {entries.map((e) => (
            <li key={e.id} data-level={e.level}>
              <button
                type="button"
                className="board-toc-entry"
                data-active={e.id === active || undefined}
                aria-current={e.id === active ? 'location' : undefined}
                onClick={() => {
                  setOpen(false)
                  onSelect(e)
                }}
              >
                {e.text || 'Untitled'}
              </button>
            </li>
          ))}
        </ol>
      </div>
    </nav>
  )
}
