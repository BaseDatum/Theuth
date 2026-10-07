// Design 71 §2.6 / M6 — the stepped tour, and the in-flow diagram that offers it. `BoardDiagram` is the wide block
// (`Graph` / `Sequence` with its units wired, `units.ts`) plus the Tour button; `Tour` is the fullscreen portal:
// stage left (the same diagram re-rendered with the active unit recoloured, scrolled to it), one numbered stop per
// unit right, Whiteboard's scroll-sync rule, `↑/↓ j/k Home/End Escape`, focus back to the element that opened it,
// stacked under 1080 px, `{blockId, index}` persisted per board session. "Tour this board" runs every anchored
// element of the document through the same component (`useTourBoard`, for the TOC).
import { ChevronLeft, ChevronRight, Route, X } from 'lucide-react'
import { type CSSProperties, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useStore } from 'zustand'
import { Excerpt } from '@/components/chat/surface/Excerpt'
import { EntitiesView } from '@/components/chat/surface/diagrams/Entities'
import { GraphDeltaView } from '@/components/chat/surface/diagrams/GraphDelta'
import { StateMachineView } from '@/components/chat/surface/diagrams/StateMachine'
import { SwimlaneView } from '@/components/chat/surface/diagrams/Swimlane'
import { TreeDiagramView } from '@/components/chat/surface/diagrams/TreeDiagram'
import { GraphView } from '@/components/chat/surface/Graph'
import { SequenceView } from '@/components/chat/surface/Sequence'
import { Button } from '@/components/ui/button'
import { usePointerCoarse } from '@/lib/mobile'
import { type TourStop, formatAnchor, intersect, useAnchorStore } from './anchors'
import { type BoardExcerpt, type Diagram, type DocBlock, classifyCode, excerptAnchor, rootOf } from './blocks'
import { StaggerFrame } from './Reveal'
import { LiveWindow, coveringExcerpt } from './SidePane'
import { boardStops, diagramStops, focusBy, highlightOf, unitsOf, useUnitPress, useUnits } from './units'

export const TOUR_PANE_MIN = 360
export const TOUR_PANE_MAX = 760
export const TOUR_PANE_DEFAULT = 594
export const TOUR_STAGE_MIN = 480
const PANE_KEY = 'board-tour-pane'

/**
 * Whiteboard's `activeTargetForScroll` (R §B.7): the first stop at or below the scroller top is active when it sits
 * in the top half of the scroller, else the one before it. `tops` are the stops' offsets in the scroller, ascending.
 */
export function activeStopForScroll(tops: readonly number[], scrollTop: number, height: number): number {
  if (tops.length === 0) return 0
  const first = tops.findIndex((t) => t >= scrollTop)
  if (first < 0) return tops.length - 1
  return tops[first]! <= scrollTop + height / 2 ? first : Math.max(0, first - 1)
}

/** The stop a tour key moves to (`↓`/`j` next, `↑`/`k` previous, `Home`/`End`), or `null` for any other key. */
export function tourKeyIndex(key: string, index: number, count: number): number | null {
  switch (key) {
    case 'ArrowDown':
    case 'j':
      return Math.min(index + 1, count - 1)
    case 'ArrowUp':
    case 'k':
      return Math.max(index - 1, 0)
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}

/** The pane width that keeps the stage ≥ 480 px (R §B.7) inside the viewport. */
function clampPane(w: number): number {
  return Math.min(TOUR_PANE_MAX, Math.max(TOUR_PANE_MIN, window.innerWidth - 10 - TOUR_STAGE_MIN), Math.max(TOUR_PANE_MIN, w))
}

const reducedMotion = () => matchMedia('(prefers-reduced-motion: reduce)').matches

/** A board `Diagram` with `highlight` by unit id (a Sequence step's id is its index). */
export function DiagramView({ diagram, highlight, onSelect }: { diagram: Diagram; highlight: readonly string[]; onSelect?: (id: string) => void }) {
  const steps = useMemo(() => highlight.map(Number), [highlight])
  switch (diagram.type) {
    case 'Graph':
      return <GraphView props={diagram.props} highlight={highlight} onSelect={onSelect} />
    case 'StateMachine':
      return <StateMachineView props={diagram.props} highlight={highlight} onSelect={onSelect} />
    case 'Entities':
      return <EntitiesView props={diagram.props} highlight={highlight} onSelect={onSelect} />
    case 'Swimlane':
      return <SwimlaneView props={diagram.props} highlight={highlight} onSelect={onSelect} />
    case 'GraphDelta':
      return <GraphDeltaView props={diagram.props} highlight={highlight} onSelect={onSelect} />
    case 'Tree':
      return <TreeDiagramView props={diagram.props} highlight={highlight} onSelect={onSelect} />
    case 'Sequence':
      return <SequenceView props={diagram.props} highlight={steps} onSelect={onSelect && ((i) => onSelect(String(i)))} />
  }
}

/** In flow (M14): the wide diagram block with its units anchored, and **Tour** when at least one unit has a `ref`; a fresh block rises and staggers (M7). */
export function BoardDiagram({ blockId, diagram, fresh }: { blockId: string; diagram: Diagram; fresh: boolean }) {
  const coarse = usePointerCoarse()
  const { stops, highlight, onSelect, openTour, wrapper } = useUnits(diagram, blockId, coarse)
  const anchored = stops.filter((s) => s.anchor).length
  return (
    <StaggerFrame blockId={blockId} fresh={fresh} className="board-block board-wide board-diagram" {...wrapper}>
      {anchored > 0 && (
        <div className="board-diagram-head">
          <button type="button" className="board-tour-button" data-anchor-id={`tour:${blockId}`} title="Tour (T)" onClick={() => openTour(null, `tour:${blockId}`)}>
            <Route aria-hidden /> Tour · {stops.length} {stops.length === 1 ? 'stop' : 'stops'}
          </button>
        </div>
      )}
      <DiagramView diagram={diagram} highlight={highlight} onSelect={onSelect} />
    </StaggerFrame>
  )
}

/** **Tour this board** (§2.6): every anchored element in reading order through the tour. For the TOC (step 6). */
export function useTourBoard(blocks: readonly DocBlock[]): (by?: string) => void {
  const store = useAnchorStore()
  return useCallback((by = '') => store.getState().openTour(boardStops(blocks), 0, by, null), [store, blocks])
}

/** `T` on the page (outside a diagram, which handles its own `T` / ⌘↵ and stops propagation) → Tour this board. */
export function useTourHotkeys(blocks: readonly DocBlock[]): void {
  const store = useAnchorStore()
  const tourBoard = useTourBoard(blocks)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.key !== 't' && e.key !== 'T') || e.metaKey || e.ctrlKey || e.altKey || e.defaultPrevented || store.getState().tour) return
      const t = e.target as HTMLElement | null
      if (t && (t.closest('input, textarea, [contenteditable="true"], .board-tour') || t.isContentEditable)) return
      e.preventDefault()
      tourBoard(t?.closest<HTMLElement>('[data-anchor-id]')?.dataset.anchorId ?? '')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [store, tourBoard])
}

/** R §B.7 `useTourPersist`: `{blockId, index}` in `sessionStorage` per board, so a reload lands on the same stop. */
export function useTourPersist(docId: string, blocks: readonly DocBlock[]): void {
  const store = useAnchorStore()
  const key = `board-tour:${docId}`
  const tour = useStore(store, (s) => s.tour)
  const restored = useRef(false)
  useEffect(() => {
    if (tour) sessionStorage.setItem(key, JSON.stringify({ blockId: tour.blockId, index: tour.index }))
    else if (restored.current) sessionStorage.removeItem(key)
  }, [tour, key])
  useEffect(() => {
    if (restored.current || tour || blocks.length === 0) return
    const saved = sessionStorage.getItem(key)
    if (!saved) {
      restored.current = true
      return
    }
    const { blockId, index } = JSON.parse(saved) as { blockId: string | null; index: number }
    if (blockId === null) {
      store.getState().openTour(boardStops(blocks), index, '', null)
      restored.current = true
      return
    }
    const block = blocks.find((b) => b.id === blockId)
    if (!block) return // not landed yet (M7 streams blocks); retried on the next change
    const c = block.type === 'codeBlock' ? classifyCode(block) : null
    if (c?.kind === 'diagram') store.getState().openTour(diagramStops(c.diagram, blockId), index, '', blockId)
    restored.current = true
  }, [store, key, blocks, tour])
}

type Stage = { blockId: string; diagram?: Diagram; excerpt?: BoardExcerpt }

/** The tour portal; mounted by `BoardReader` while `tour` is set. `page` is where focus returns on close. */
export function Tour({ blocks, excerpts, page }: { blocks: readonly DocBlock[]; excerpts: readonly BoardExcerpt[]; page: HTMLElement | null }) {
  const store = useAnchorStore()
  const tour = useStore(store, (s) => s.tour)
  const root = rootOf(blocks)
  const [width, setWidth] = useState(() => clampPane(Number(localStorage.getItem(PANE_KEY)) || TOUR_PANE_DEFAULT))
  const [dragging, setDragging] = useState(false)
  const dialog = useRef<HTMLDivElement>(null)
  const feed = useRef<HTMLDivElement>(null)
  const stageEl = useRef<HTMLDivElement>(null)
  /** The stop a programmatic reveal is scrolling to; scroll-sync stays quiet until the feed arrives there. */
  const revealing = useRef<number | null>(null)
  /** The stop scroll-sync chose; the reveal effect skips it (the reader scrolled there themselves). */
  const fromScroll = useRef<number | null>(null)
  const stages = useMemo(() => {
    const m: Record<string, Stage> = {}
    for (const b of blocks) {
      if (b.type !== 'codeBlock') continue
      const c = classifyCode(b)
      if (c.kind === 'diagram') m[b.id] = { blockId: b.id, diagram: c.diagram }
      else if (c.kind === 'excerpt') m[b.id] = { blockId: b.id, excerpt: { blockId: b.id, props: c.props, anchor: excerptAnchor(c.props) } }
    }
    return m
  }, [blocks])

  const index = tour?.index ?? 0
  const stops = tour?.stops ?? []
  const stop = stops[index]
  const stage = stop?.stage ? stages[stop.stage.blockId] : undefined
  const units = useMemo(() => (stage?.diagram ? unitsOf(stage.diagram) : []), [stage?.diagram])
  const highlight = stop?.stage?.unit ? [stop.stage.unit] : highlightOf(units, stop?.anchor ?? null)

  const close = () => {
    const by = store.getState().tour?.by ?? ''
    store.getState().closeTour()
    focusBy(page, by)
  }
  const step = (i: number) => store.getState().stepTour(i)

  // Keys: `↑/↓ j/k Home/End` move, `Escape` closes and returns focus (M6).
  useEffect(() => {
    dialog.current?.focus({ preventScroll: true })
    const onKey = (e: KeyboardEvent) => {
      const t = store.getState().tour
      if (!t || e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
        return
      }
      const next = tourKeyIndex(e.key, t.index, t.stops.length)
      if (next !== null) {
        e.preventDefault()
        step(next)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [store])

  // Reveal: the feed scrolls to the active stop (unless the reader scrolled there), the stage to the active unit.
  useEffect(() => {
    const behavior: ScrollBehavior = reducedMotion() ? 'auto' : 'smooth'
    if (fromScroll.current === index) {
      fromScroll.current = null
    } else {
      const el = feed.current?.querySelector<HTMLElement>(`[data-stop="${index}"]`)
      const scroller = feed.current
      if (el && scroller) {
        const top = el.offsetTop // the feed is `position: relative`, so stops offset from it
        if (Math.abs(scroller.scrollTop - top) > 1) {
          revealing.current = index
          scroller.scrollTo({ top, behavior })
        }
      }
    }
    const unit = highlight[0]
    if (unit !== undefined) {
      const g = stageEl.current?.querySelector<Element>(`[data-node="${CSS.escape(unit)}"], [data-step="${CSS.escape(unit)}"]`)
      g?.scrollIntoView({ block: 'center', inline: 'center', behavior })
    }
  }, [index, highlight[0]])

  const onFeedScroll = () => {
    const scroller = feed.current
    const t = store.getState().tour
    if (!scroller || !t) return
    const tops = [...scroller.querySelectorAll<HTMLElement>('[data-stop]')].map((el) => el.offsetTop)
    const i = activeStopForScroll(tops, scroller.scrollTop, scroller.clientHeight)
    if (revealing.current !== null) {
      if (i === revealing.current) revealing.current = null
      return
    }
    if (i !== t.index) {
      fromScroll.current = i
      step(i)
    }
  }

  const drag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId)
    setDragging(true)
    const move = (ev: PointerEvent) => setWidth(clampPane(Math.round(window.innerWidth - ev.clientX)))
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      setDragging(false)
      setWidth((w) => {
        localStorage.setItem(PANE_KEY, String(w))
        return w
      })
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  // A unit on the stage jumps to its stop: Enter through the view's `onSelect`, a press by delegation (the unit's
  // own click is swallowed by the frame's pointer capture).
  const jump = (id: string) => {
    const i = stops.findIndex((s) => s.stage !== null && s.stage.blockId === stage?.blockId && s.stage.unit === id)
    if (i >= 0) step(i)
  }
  const stagePress = useUnitPress(jump)

  if (!tour || !stop) return null
  return createPortal(
    <div
      ref={dialog}
      className="board-tour"
      role="dialog"
      aria-modal="true"
      aria-label={`Tour, stop ${index + 1} of ${stops.length}`}
      tabIndex={-1}
      style={{ '--board-tour-pane': `${width}px` } as CSSProperties}
    >
      <div ref={stageEl} className="board-tour-stage" {...stagePress}>
        {stage?.diagram ? (
          <DiagramView diagram={stage.diagram} highlight={highlight} onSelect={jump} />
        ) : stage?.excerpt ? (
          <Excerpt {...stage.excerpt.props} full active={stop.anchor ? intersect(stop.anchor, stage.excerpt.anchor.lines) : null} />
        ) : (
          <div className="board-tour-empty">No diagram in this section</div>
        )}
      </div>
      <div role="separator" aria-orientation="vertical" aria-label="Resize tour pane" className="board-tour-resizer" data-dragging={dragging || undefined} onPointerDown={drag} />
      <div className="board-tour-pane">
        <div className="board-tour-head">
          <span className="board-tour-count">
            Stop {index + 1} / {stops.length}
          </span>
          <Button variant="ghost" size="icon-sm" aria-label="Previous stop" disabled={index === 0} onClick={() => step(index - 1)}>
            <ChevronLeft />
          </Button>
          <Button variant="ghost" size="icon-sm" aria-label="Next stop" disabled={index === stops.length - 1} onClick={() => step(index + 1)}>
            <ChevronRight />
          </Button>
          <Button variant="ghost" size="icon-sm" aria-label="Close tour" onClick={close}>
            <X />
          </Button>
        </div>
        <div ref={feed} className="board-tour-feed" onScroll={onFeedScroll}>
          {stops.map((s, i) => (
            <Stop key={i} stop={s} index={i} active={i === index} excerpts={excerpts} root={root} onActivate={() => step(i)} />
          ))}
        </div>
      </div>
    </div>,
    document.body,
  )
}

/** One stop: badge · label · `from → to`, then the code window (M5 rules: the covering excerpt, else the live range —
 * step 9: the step-4 placeholder stood here until the reference boards showed every uncovered stop as a dead end),
 * or the explanation when the unit has no `ref`. */
function Stop({ stop, index, active, excerpts, root, onActivate }: { stop: TourStop; index: number; active: boolean; excerpts: readonly BoardExcerpt[]; root: string | null; onActivate: () => void }) {
  const excerpt = stop.anchor ? coveringExcerpt(excerpts, stop.anchor) : null
  return (
    <section className="tour-stop" data-stop={index} data-anchor={stop.anchor ? formatAnchor(stop.anchor) : undefined} data-active={active || undefined} aria-current={active ? 'step' : undefined}>
      <button type="button" className="tour-stop-head" onClick={onActivate}>
        <span className="tour-stop-badge">{index + 1}</span>
        <span className="tour-stop-label">{stop.label}</span>
        {stop.detail && <span className="tour-stop-detail">{stop.detail}</span>}
      </button>
      {stop.anchor ? (
        excerpt ? (
          <Excerpt {...excerpt.props} full active={intersect(stop.anchor, excerpt.anchor.lines)} />
        ) : stop.anchor.kind === 'code' ? (
          <LiveWindow anchor={stop.anchor} root={root} />
        ) : (
          <div className="tour-stop-note rounded-md border">
            <p className="text-foreground">{formatAnchor(stop.anchor)}</p>
            <p className="mt-1">A block anchor has no code window.</p>
          </div>
        )
      ) : (
        <p className="tour-stop-note">This unit has no code reference.</p>
      )}
    </section>
  )
}
