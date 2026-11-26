// §8.5 card chrome: title bar (title + kind), stacked edge and the `v2 of 3` scrubber, dimmed + tag chip, placeholder
// shimmer, selected outline + four handles, entered border; a shield over every live body until the card is entered.
import { Handle, NodeResizer, NodeToolbar, Position, type Node, type NodeProps } from '@xyflow/react'
import { useNavigate, useRouter } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight, ExternalLink, EyeOff, Eye, Archive, Maximize2, PanelRight, Pin, PinOff, RefreshCw, RotateCcw } from 'lucide-react'
import { lazy, memo, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { openViewFromCard } from '@/components/tiles/open'
import { useStore as useZustand } from 'zustand'
import { CardBoundary, Chip, hostOf, KIND_NAME, LiveBody, Snapshot } from './cards/bodies'
import { pageViewport } from './cards/PageCard'
import { useWebPreview, WebFavicon } from './cards/WebCard'
import { useCanvas } from './context'
import { LOCAL, OWNER, versionOf, type Card } from './model'
import { archive, dim, patchCard, restore } from './ops'
import { IN_PLACE } from './ops'
import { CardInk } from './InkLayer'
import { toAbsolute, type CardNodeData } from './binding'
import { headerOf, isBare } from './send'
import { applyFit, fitSize, fitsContent, measureBody } from './fit'
import { hidesOverflow, scrollChain, wheelRoute, ZOOM_SURFACE } from './scroll'
import { useResizeEnd } from './resize'
import { viewCardPath } from '../view-card'
import { openTaskPane, taskOf } from '../task-cards'

// Lazy: the task card's tools pull the task store and its queries.
const TaskTools = lazy(() => import('./cards/TaskCard').then((m) => ({ default: m.TaskTools })))
/** A press that moves less than this (screen px) before it lifts is a click: on a task card's body, it opens the task. */
const CLICK_SLOP = 6

export type CardNodeType = Node<CardNodeData, 'card'>

const PORTS = [
  ['top', Position.Top],
  ['right', Position.Right],
  ['bottom', Position.Bottom],
  ['left', Position.Left],
] as const

function Scrubber({ card }: { card: Card }) {
  const { store } = useCanvas()
  const shown = useZustand(store, (s) => s.scrub[card.id]) ?? card.versions.length
  const n = card.versions.length
  const go = (v: number) => {
    const scrub = { ...store.getState().scrub }
    if (v >= n) delete scrub[card.id]
    else scrub[card.id] = v
    store.getState().set({ scrub })
  }
  return (
    <div className="wb-scrub nodrag nopan" role="group" aria-label="Versions">
      <button type="button" aria-label="Previous version" disabled={shown <= 1} onClick={() => go(shown - 1)}>
        <ChevronLeft className="size-3.5" />
      </button>
      <span>
        v{shown} of {n}
      </span>
      <button type="button" aria-label="Next version" disabled={shown >= n} onClick={() => go(shown + 1)}>
        <ChevronRight className="size-3.5" />
      </button>
    </div>
  )
}

/** A view card's route in a pane (on a phone: the route itself). Only view cards mount it: it needs the app router. */
function OpenInPane({ path, title }: { path: string; title: string }) {
  const navigate = useNavigate()
  return (
    <button type="button" onClick={() => openViewFromCard({ path, title: title || undefined }, { navigate: (href) => void navigate({ href }) })}>
      <PanelRight className="size-4" /> Open in pane
    </button>
  )
}

function Toolbar({ card, refreshPreview, fitNow, resetView }: { card: Card; refreshPreview: (() => void) | null; fitNow: (() => void) | null; resetView: (() => void) | null }) {
  const { doc, open, stopCapturing, readOnly } = useCanvas()
  const [tagging, setTagging] = useState(false)
  const [tag, setTag] = useState('')
  const source = card.kind === 'view' ? versionOf(card)?.source : undefined
  const viewPath = source?.t === 'view' ? viewCardPath(source.path) : null
  if (tagging) {
    return (
      <form
        className="wb-tools nodrag nopan"
        onSubmit={(e) => {
          e.preventDefault()
          dim(doc, [card.id], tag)
          stopCapturing()
          setTagging(false)
          setTag('')
        }}
      >
        <input autoFocus aria-label="Why is it dimmed?" placeholder="Why? (shown under the card)" value={tag} maxLength={120} onChange={(e) => setTag(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
        <button type="submit">Dim</button>
        <button type="button" onClick={() => setTagging(false)}>
          Cancel
        </button>
      </form>
    )
  }
  return (
    <div className="wb-tools nodrag nopan" role="toolbar" aria-label={`${card.title || 'Card'} actions`}>
      <button type="button" onClick={() => open(card.id)}>
        <ExternalLink className="size-4" /> Open
      </button>
      {viewPath && <OpenInPane path={viewPath} title={card.title} />}
      {card.kind === 'task' && (
        <Suspense fallback={null}>
          <TaskTools card={card} />
        </Suspense>
      )}
      {refreshPreview && (
        <button type="button" aria-label="Refresh preview" onClick={refreshPreview}>
          <RefreshCw className="size-4" />
        </button>
      )}
      {resetView && (
        <button type="button" aria-label="Reset view" title="Reset the diagram's view (fit)" onClick={resetView}>
          <RotateCcw className="size-4" />
        </button>
      )}
      {!readOnly && fitNow && (
        <button type="button" aria-label="Fit to content" title="Fit to content" onClick={fitNow}>
          <Maximize2 className="size-4" />
        </button>
      )}
      {!readOnly && (
        <>
          <span className="sep" />
          {card.state === 'dimmed' ? (
            <button type="button" aria-label="Undim" onClick={() => { restore(doc, [card.id]); stopCapturing() }}>
              <Eye className="size-4" />
            </button>
          ) : (
            <button type="button" aria-label="Dim with a reason" onClick={() => setTagging(true)}>
              <EyeOff className="size-4" />
            </button>
          )}
          {card.kind === 'page' && (
            <button
              type="button"
              aria-label={card.pin ? 'Unpin live' : 'Pin live (never demoted to its snapshot; demotion loses the page state)'}
              aria-pressed={card.pin === true}
              onClick={() => { patchCard(doc, card.id, { pin: card.pin ? undefined : true }); stopCapturing() }}
            >
              {card.pin ? <PinOff className="size-4" /> : <Pin className="size-4" />}
            </button>
          )}
          <button
            type="button"
            aria-label={card.kind === 'task' ? 'Remove from board (the task stays)' : 'Archive'}
            title={card.kind === 'task' ? 'Remove from board (the task stays)' : undefined}
            onClick={() => { archive(doc, [card.id]); stopCapturing() }}
          >
            <Archive className="size-4" />
          </button>
        </>
      )}
    </div>
  )
}

export const CardNode = memo(function CardNode({ data, selected }: NodeProps<CardNodeType>) {
  const { card } = data
  const { store, lod, readOnly, enter, doc, stopCapturing, spot, board } = useCanvas()
  const entered = useZustand(store, (s) => s.entered === card.id)
  const hovered = useZustand(store, (s) => s.hover === card.id)
  // Hand (and Present, which locks it) pans: no card chrome there, a click-selected card included.
  const hand = useZustand(store, (s) => s.mode === 'hand')
  const scrub = useZustand(store, (s) => s.scrub[card.id])
  const level = lod[card.id] ?? 'chip'
  const source = versionOf(card, scrub)?.source
  const web = source?.t === 'web' ? source.url : null
  const refreshPreview = useWebPreview(card)
  const lastTap = useRef(0)
  // A task card's body: a click (no drag: a drag there pans the board) opens the task in a pane.
  const router = useRouter({ warn: false })
  const press = useRef<{ x: number; y: number } | null>(null)
  const task = taskOf(card)
  const versioned = !IN_PLACE.includes(card.kind)
  const stacked = versioned && card.versions.length > 1
  const placeholder = card.placeholder !== undefined
  // A diagram is just the diagram: no title bar; its title (if any) is a small caption on hover or selection.
  const bare = isBare(card)
  const live = (level === 'live' || entered) && !placeholder

  // Size to content (fit.ts): a card with `fit` pending is measured once its live body settles (the same size three
  // reads in a row, 250 ms apart; given up after 10 s), then sized. Pages size to their device viewport.
  const fitBox = useRef<HTMLDivElement>(null)
  const latest = useRef({ card, board, live })
  latest.current = { card, board, live }
  const naturalOf = useCallback((): { w: number; h: number } | null => {
    const { card: c, board: b, live: l } = latest.current
    if (c.kind === 'page') return pageViewport(c, c.frame ? b.frames[c.frame]?.preset : undefined)
    return l && fitBox.current ? measureBody(fitBox.current) : null
  }, [])
  useEffect(() => {
    if (!card.fit || readOnly || !live) return
    const t0 = performance.now()
    let last = ''
    let same = 0
    const timer = setInterval(() => {
      const n = naturalOf()
      if (performance.now() - t0 > 10_000) clearInterval(timer)
      if (!n) return
      const k = `${n.w}×${n.h}`
      same = k === last ? same + 1 : 0
      last = k
      if (same < 2) return
      clearInterval(timer)
      const { card: c, board: b } = latest.current
      applyFit(doc, b, c.id, fitSize(n, headerOf(c)), LOCAL, false)
    }, 250)
    return () => clearInterval(timer)
  }, [card.fit, readOnly, live, doc, naturalOf])
  const fitNow = fitsContent(card)
    ? () => {
        const n = naturalOf()
        if (n) applyFit(doc, board, card.id, fitSize(n, headerOf(card)), OWNER, true)
        else patchCard(doc, card.id, { fit: true, sized: undefined }) // not live here: sized when it next renders
        stopCapturing()
      }
    : null

  // Wheel and pinch over the body (scroll.ts `wheelRoute`): the board's unless the card is entered; entered, an inner
  // pan/zoom diagram or a scroll region that can still move takes it and it is stopped here, before the board's
  // zoom/pan (the inner handler runs first, on its own element). Never both.
  const clip = useRef<HTMLDivElement>(null)
  const enteredRef = useRef(entered)
  enteredRef.current = entered
  useEffect(() => {
    const el = clip.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      if (!(e.target instanceof Element)) return
      const zoomSurface = e.target.closest(ZOOM_SURFACE) !== null
      const chain = scrollChain(e.target, el.parentElement ?? el)
      if (wheelRoute(enteredRef.current, { zoomSurface, pinch: e.ctrlKey, chain, dx: e.deltaX, dy: e.deltaY }) === 'inner') e.stopPropagation()
    }
    el.addEventListener('wheel', onWheel, { passive: true })
    return () => el.removeEventListener('wheel', onWheel)
  }, [])
  // A diagram's inner pan/zoom is never kept on the board: it starts fitted, and leaving the card or "Reset view"
  // remounts it fitted (its double-click/tap also fits, inside DiagramFrame).
  const [viewEpoch, setViewEpoch] = useState(0)
  const wasEntered = useRef(entered)
  useEffect(() => {
    if (wasEntered.current && !entered && card.kind === 'diagram') setViewEpoch((n) => n + 1)
    wasEntered.current = entered
  }, [entered, card.kind])
  const resetView = card.kind === 'diagram' && live ? () => setViewEpoch((n) => n + 1) : null
  // A live body that hides content behind a scroll shows a fade and a "scroll ↓" hint until the card is entered.
  const [more, setMore] = useState(false)
  useEffect(() => {
    const el = fitBox.current
    if (!el || entered || !live) {
      setMore(false)
      return
    }
    const check = () => setMore(hidesOverflow(el))
    const t = setTimeout(check, 600)
    const ro = new ResizeObserver(check)
    ro.observe(el)
    return () => {
      clearTimeout(t)
      ro.disconnect()
    }
  }, [entered, live, source])

  const cls = [
    'wb-card',
    card.kind === 'sticky' && 'sticky',
    bare && 'bare',
    stacked && 'stacked',
    card.state === 'dimmed' && 'dimmed',
    placeholder && 'placeholder',
    selected && !entered && 'selected',
    entered && 'entered',
    spot === card.id && 'spot',
    hovered && 'hover',
  ].filter(Boolean).join(' ')
  const onResizeEnd = useResizeEnd((_, p) => {
    const at = toAbsolute(board, card.id, { x: p.x, y: p.y })
    // The owner sized it: never sized to content on its own again (its toolbar's "Fit to content" still does).
    patchCard(doc, card.id, { x: Math.round(at.x), y: Math.round(at.y), w: Math.round(p.width), h: Math.round(p.height), sized: true })
    stopCapturing()
  })

  return (
    <>
      {selected && !readOnly && !entered && !hand && (
        <NodeResizer
          minWidth={80}
          minHeight={60}
          color="var(--fg-accent)"
          lineStyle={{ borderColor: 'transparent' }}
          onResizeEnd={onResizeEnd}
        />
      )}
      {selected && !entered && !hand && (
        <NodeToolbar position={Position.Top} offset={14}>
          <Toolbar card={card} refreshPreview={refreshPreview} fitNow={fitNow} resetView={resetView} />
        </NodeToolbar>
      )}
      <div className={cls} data-card={card.id} aria-label={`${KIND_NAME[card.kind]}: ${card.title}`}>
        {bare ? (
          card.title && <div className="wb-caption">{card.title}</div>
        ) : (
          <div className="wb-head">
            {web !== null && <WebFavicon url={web} />}
            <span className="wb-title">{card.title || 'Untitled'}</span>
            <span className="wb-kind">
              {web !== null ? hostOf(web) : KIND_NAME[card.kind]}
              {versioned && card.versions.length > 0 ? ` · v${scrub ?? card.versions.length}` : ''}
            </span>
          </div>
        )}
        <div
          ref={clip}
          // Entered, a drag inside the body is the content's (scroll, select), not a board pan.
          className={entered ? 'wb-clip nopan' : 'wb-clip'}
          data-body
          onPointerDown={(e) => {
            press.current = { x: e.clientX, y: e.clientY }
          }}
          onPointerUp={(e) => {
            const p = press.current
            press.current = null
            if (task && !entered && e.pointerType !== 'pen' && p && Math.hypot(e.clientX - p.x, e.clientY - p.y) < CLICK_SLOP) {
              const mode = store.getState().mode
              if (mode === 'idle' || mode === 'hand') openTaskPane(task, card.title, router ? (href) => void router.navigate({ href }) : undefined)
              return
            }
            // Double-tap (finger) or double-click enters the card; a pen never reaches here (the ink filter takes it).
            if (entered || placeholder || e.pointerType === 'pen' || (readOnly && card.kind === 'sticky')) return
            if (e.timeStamp - lastTap.current < 320) {
              lastTap.current = 0
              enter(card.id)
            } else lastTap.current = e.timeStamp
          }}
        >
          {placeholder ? (
            <div className="wb-shimmer">{card.placeholder || 'building…'}</div>
          ) : live ? (
            // Content past the biggest fitted size renders at 1/scale and is scaled down whole (fit.ts), never cropped.
            <div
              ref={fitBox}
              className="absolute top-0 left-0 size-full origin-top-left"
              // Not entered, the body takes no pointer: the shield and the board get every press, wheel and pinch.
              style={{
                pointerEvents: entered ? undefined : 'none',
                ...(card.scale ? { width: `${100 / card.scale}%`, height: `${100 / card.scale}%`, transform: `scale(${card.scale})` } : {}),
              }}
            >
              <CardBoundary key={viewEpoch} card={card}>
                <LiveBody card={card} source={source} editing={entered} />
              </CardBoundary>
            </div>
          ) : level === 'snapshot' && !scrub ? (
            <Snapshot card={card} />
          ) : (
            <Chip card={card} />
          )}
          {!entered && level === 'live' && !placeholder && <div className="wb-shield" />}
          {more && !entered && (
            <div className="wb-more" aria-hidden>
              <span>scroll ↓</span>
            </div>
          )}
        </div>
        <CardInk card={card} />
        {stacked && <Scrubber card={card} />}
        {card.state === 'dimmed' && card.tag && <div className="wb-tag">{card.tag}</div>}
      </div>
      {/* ConnectionMode.Loose: every handle is a source and a target; ids are `s-<port>`. */}
      {PORTS.map(([name, pos]) => (
        <Handle key={name} id={`s-${name}`} type="source" position={pos} className="wb-handle" isConnectable={!readOnly} />
      ))}
    </>
  )
})
