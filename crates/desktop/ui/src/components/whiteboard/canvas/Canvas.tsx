// 218 §8.2–§8.7: the whiteboard canvas — React Flow viewport, cards, frames, links, the ink layer and pointer ink.
// The only importer of `@xyflow/react` and its CSS in the whiteboard (B4 lazy-loads this file).
import '@xyflow/react/dist/style.css'
import './canvas.css'
import {
  Background, BackgroundVariant, ConnectionMode, MarkerType, Panel, ReactFlow, ReactFlowProvider, SelectionMode,
  useReactFlow, useViewport, ViewportPortal,
  type Edge, type FinalConnectionState, type Node, type OnConnect, type OnNodesChange, type Viewport,
} from '@xyflow/react'
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react'
import { useStore as useZustand } from 'zustand'
import { useDark } from '@/lib/theme'
import { inkCss } from '../ink/colour'
import { freshStrokes } from '../ink/marks'
import { pointsPath } from '../ink/render'
import { unpack, type Pt } from '../ink/pack'
import { buildStrokes } from '../ink/stroke'
import { lookOf } from '../ink/life'
import type { CapturedStroke, View, WhiteboardCanvasHandle, WhiteboardCanvasProps } from './api'
import { dragEnd, edgesOf, nodesOf } from './binding'
import { CardNode } from './CardNode'
import { pagePoint, pageViewport } from './cards/PageCard'
import type { PenMessage } from './penRelay'
import { composeImage } from './compose'
import { Ctx, type CanvasCtx } from './context'
import { FrameNode } from './FrameNode'
import { contains, intersects, pad, scaleOf, shownCards, strokeWorldBox, union } from './geom'
import { FreeInk, indexInk } from './InkLayer'
import { MultiTap, PALM_MS, pointerKind, routePointer, type PointerKind } from './input'
import { LinkEdge } from './LinkEdge'
import { assignLod, emptyLodState, type Lod } from './lod'
import { beside } from './beside'
import { WB_SCHEMA, type Board, type Box, type Id, type Port, type Stroke } from './model'
import { addCard, addFrame, addLink, addStrokes, eraseStrokes, holdGroup, moveItems, pinMark, raise, setSnap, archive, removeLink, removeFrame, dim } from './ops'
import { lassoSelect } from './lasso'
import { InkSelection, LassoPath, Offscreen, SelectionBar } from './Overlays'
import { CARD_HEADER, commitSend as writeSend, planSend as plan, type Caption } from './send'
import { createUndo } from './undo'
import { sourceOf } from './useBoard'

const NODE_TYPES = { card: CardNode, frame: FrameNode }
const EDGE_TYPES = { link: LinkEdge }
const SNAP_SETTLE_MS = 500
const SNAP_MAX = 1024
const LOD_TICK_MS = 5000
const STICKY = { w: 220, h: 150 }
const LONG_PRESS_MS = 550
/** The pointer id of a stroke relayed from an entered page (real pointer ids are ≥ 0). */
const PAGE_PEN = -1
/** Controls that a Pencil may tap without inking (toolbars, scrubbers, fields). */
const CONTROL = '.wb-tools, .wb-scrub, .wb-menu, button, input, textarea, select, a, [contenteditable="true"]'

const reducedMotion = () => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
const nextFrame = () => {
  const { promise, resolve } = Promise.withResolvers<void>()
  requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  return promise
}
const portOf = (h: string | null | undefined): Port | undefined => {
  const p = h?.replace(/^[st]-/, '')
  return p === 'top' || p === 'right' || p === 'bottom' || p === 'left' ? p : undefined
}

type Wet = { id: number; kind: PointerKind; erase: boolean; pts: Pt[]; t0: number; hasPressure: boolean }
type Rubber = { id: number; x0: number; y0: number; x1: number; y1: number }

/** B4's anchored nodes (the reply bubble, 262 px wide): beside a card in screen space, on the first side that stays
 *  in the frame (minus `right`, the raised cluster) and covers no other card (`beside.ts`). */
function Anchored({ items, board, size, right }: { items: NonNullable<WhiteboardCanvasProps['anchored']>; board: Board; size: { w: number; h: number }; right: number }) {
  const { x, y, zoom } = useViewport()
  const screen = (c: { x: number; y: number; w: number; h: number }) => ({ x: c.x * zoom + x, y: c.y * zoom + y, w: c.w * zoom, h: c.h * zoom })
  return (
    <>
      {items.map((a) => {
        const c = a.card ? board.cards[a.card] : undefined
        if (!c || c.state === 'archived') return null
        const others = Object.values(board.cards).filter((o) => o.id !== c.id && o.state !== 'archived').map(screen)
        const at = beside(screen(c), { w: 262, h: 120 }, others, { ...size, right })
        return (
          <div key={a.key} className="absolute z-10" style={{ left: at.x, top: at.y }}>
            {a.node}
          </div>
        )
      })}
    </>
  )
}

/** Hit test for the eraser: does world point (x, y) touch the stroke's centre line within `tol`? */
function touches(s: Stroke, board: Board, x: number, y: number, tol: number): boolean {
  const wb = strokeWorldBox(s, board.cards)
  if (!wb || !intersects(pad(wb, tol), [x, y, 0, 0])) return false
  let ox = 0
  let oy = 0
  let sx = 1
  let sy = 1
  if (s.card) {
    const c = board.cards[s.card]!
    ;[sx, sy] = scaleOf(s, c)
    ox = c.x
    oy = c.y
  }
  const pts = unpack(s.o, s.pts)
  const r = s.width / 2 + tol
  for (let i = 0; i < pts.length; i++) {
    const ax = ox + pts[i]![0] * sx
    const ay = oy + pts[i]![1] * sy
    const b = pts[i + 1]
    if (!b) return Math.hypot(x - ax, y - ay) <= r
    const bx = ox + b[0] * sx
    const by = oy + b[1] * sy
    const dx = bx - ax
    const dy = by - ay
    const len = dx * dx + dy * dy
    const t = len === 0 ? 0 : Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / len))
    if (Math.hypot(x - (ax + t * dx), y - (ay + t * dy)) <= r) return true
  }
  return false
}

function Inner(props: WhiteboardCanvasProps & { handle: React.ForwardedRef<WhiteboardCanvasHandle> }) {
  const { provider, store, host, embedded, className, anchored, onCanvasTap, onPenActivity, onViewChange, onCardOpen, handle } = props
  const doc = provider.doc
  const src = sourceOf(doc)
  const board = useSyncExternalStore(src.subscribe, src.get, src.get)
  const dark = useDark()
  const rf = useReactFlow()
  const readOnly = !!props.readOnly || board.meta.v > WB_SCHEMA
  const wrap = useRef<HTMLDivElement>(null)
  const wetCanvas = useRef<HTMLCanvasElement>(null)
  const undo = useMemo(() => createUndo(doc), [doc])
  useEffect(() => () => undo.destroy(), [undo])

  const mode = useZustand(store, (s) => s.mode)
  const selection = useZustand(store, (s) => s.selection)
  const entered = useZustand(store, (s) => s.entered)
  const clusterOpen = useZustand(store, (s) => s.clusterOpen)

  const boardRef = useRef(board)
  boardRef.current = board
  const viewRef = useRef<View>({ x: 0, y: 0, zoom: 1 })
  const captions = useRef(new Map<Id, Caption>()).current
  const frames = useRef(new Map<Id, HTMLIFrameElement>()).current
  const [spot, setSpot] = useState<Id | null>(null)
  const [lod, setLod] = useState<Record<Id, Lod>>({})
  const lodState = useRef(emptyLodState())
  const [size, setSize] = useState({ w: 0, h: 0 })
  const [rubber, setRubber] = useState<Rubber | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; world: { x: number; y: number }; from: Id; port?: Port } | null>(null)
  const menuOpen = useRef(false)
  menuOpen.current = menu !== null
  const linkFrom = useRef<Id | null>(null)
  /** Strokes taken by the lasso (ink has no React Flow node; the store's selection holds their marks). */
  const [inkSel, setInkSel] = useState<Id[]>([])
  const inkSelRef = useRef(inkSel)
  inkSelRef.current = inkSel
  const [lasso, setLasso] = useState<{ id: number; pts: [number, number][] } | null>(null)
  const lassoRef = useRef(lasso)
  const [markMenu, setMarkMenu] = useState<{ x: number; y: number; mark: Id } | null>(null)

  /** One eraser drag is one undo step (its writes stay separate, bounded transactions). */
  const releaseErase = useRef<(() => void) | null>(null)
  /** Delete = archive cards, remove links and frames (their cards stay), erase lassoed ink. One undo step. */
  const deleteSelection = useCallback(() => {
    const s = store.getState()
    const b = boardRef.current
    const release = holdGroup()
    try {
      const cards = s.selection.filter((id) => b.cards[id])
      if (cards.length) archive(doc, cards)
      for (const id of s.selection) {
        if (b.links[id]) removeLink(doc, id)
        if (b.frames[id]) removeFrame(doc, id)
      }
      if (inkSelRef.current.length) eraseStrokes(doc, inkSelRef.current)
    } finally {
      release()
    }
    s.set({ selection: [] })
    setInkSel([])
  }, [doc, store])

  // ---- geometry ----
  const worldView = useCallback((): Box => {
    const v = viewRef.current
    const el = wrap.current
    const w = el?.clientWidth ?? 0
    const h = el?.clientHeight ?? 0
    return [-v.x / v.zoom, -v.y / v.zoom, w / v.zoom, h / v.zoom]
  }, [])
  const toWorld = useCallback((clientX: number, clientY: number) => {
    const r = wrap.current!.getBoundingClientRect()
    const v = viewRef.current
    return { x: (clientX - r.left - v.x) / v.zoom, y: (clientY - r.top - v.y) / v.zoom }
  }, [])
  const windowRect = useCallback((b: Box, full = true): DOMRect | null => {
    const el = wrap.current
    if (!el) return null
    const r = el.getBoundingClientRect()
    const v = viewRef.current
    const rect = new DOMRect(r.left + b[0] * v.zoom + v.x, r.top + b[1] * v.zoom + v.y, b[2] * v.zoom, b[3] * v.zoom)
    if (!full) return rect
    return rect.left >= r.left - 0.5 && rect.top >= r.top - 0.5 && rect.right <= r.right + 0.5 && rect.bottom <= r.bottom + 0.5 ? rect : null
  }, [])
  const boxOf = useCallback((id: Id): Box | null => {
    const b = boardRef.current
    const c = b.cards[id]
    if (c && c.state !== 'archived') return [c.x, c.y, c.w, c.h]
    const f = b.frames[id]
    if (f) return [f.x, f.y, f.w, f.h]
    const m = b.marks[id]
    if (m) {
      let u: Box | null = null
      for (const s of m.strokes) {
        const st = b.ink[s]
        const w = st && strokeWorldBox(st, b.cards)
        if (w) u = union(u, w)
      }
      return u
    }
    return null
  }, [])

  // ---- snapshots (§8.5): 500 ms after a live card settles fully on screen, again when the theme changed (A1), and
  // once more just before a demotion ----
  const theme: 'light' | 'dark' = dark ? 'dark' : 'light'
  const themeRef = useRef(theme)
  themeRef.current = theme
  const snapping = useRef(new Set<string>())
  const noCapture = useRef(false)
  const stale = (c: Board['cards'][string]) =>
    c.placeholder === undefined && c.versions.length > 0 && !(c.snap && c.snap.v === c.versions.length && c.snap.theme === themeRef.current)
  /** Captures a live card's body if its snapshot is stale and it is fully on screen. Never throws. */
  const captureCard = useCallback(async (id: Id): Promise<void> => {
    const c = boardRef.current.cards[id]
    if (!c || noCapture.current || !stale(c) || store.getState().scrub[id] !== undefined) return
    const v = c.versions.length
    const th = themeRef.current
    const key = `snap/${c.id}/${v}`
    if (snapping.current.has(key + th)) return
    const rect = windowRect([c.x, c.y + CARD_HEADER, c.w, c.h - CARD_HEADER])
    if (!rect || rect.width < 8 || rect.height < 8) return
    snapping.current.add(key + th)
    try {
      const blob = await host.capture({ x: rect.x, y: rect.y, width: rect.width, height: rect.height }, SNAP_MAX)
      if (!blob) {
        noCapture.current = true // no native capture on this client (web, relay): cards stay chips when not live
        return
      }
      await host.blobPut(key, blob)
      setSnap(doc, c.id, { v, at: Date.now(), theme: th })
    } catch {
      snapping.current.delete(key + th)
    }
  }, [doc, host, store, windowRect])

  // ---- LOD (re-assigned on view end, board change, enter, and a slow tick for the 30 s demotion) ----
  const lodRef = useRef(lod)
  lodRef.current = lod
  const holding = useRef(new Set<Id>())
  const recomputeLod = useCallback(() => {
    const v = viewRef.current
    const b = boardRef.current
    const r = assignLod(shownCards(b), {
      zoom: v.zoom, view: worldView(), budget: host.liveBudget, entered: store.getState().entered, now: Date.now(), state: lodState.current,
    })
    lodState.current = r.state
    const next = { ...r.lod }
    // A card about to be demoted with a stale snapshot stays live for one capture first.
    for (const [id, was] of Object.entries(lodRef.current)) {
      const c = b.cards[id]
      if (was !== 'live' || next[id] === 'live' || !c || !stale(c) || noCapture.current || holding.current.has(id)) continue
      holding.current.add(id)
      next[id] = 'live'
      void captureCard(id).finally(() => {
        setTimeout(() => {
          recomputeLod()
          holding.current.delete(id)
        }, 0)
      })
    }
    setLod((prev) => {
      const keys = Object.keys(next)
      return keys.length === Object.keys(prev).length && keys.every((k) => prev[k] === next[k]) ? prev : next
    })
  }, [captureCard, host.liveBudget, store, worldView])
  useEffect(recomputeLod, [board, entered, size, recomputeLod])
  useEffect(() => {
    const t = setInterval(recomputeLod, LOD_TICK_MS)
    return () => clearInterval(t)
  }, [recomputeLod])

  const snapTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const scheduleSnapshots = useCallback(() => {
    clearTimeout(snapTimer.current)
    snapTimer.current = setTimeout(async () => {
      for (const [id, l] of Object.entries(lodRef.current)) if (l === 'live') await captureCard(id)
    }, SNAP_SETTLE_MS)
  }, [captureCard])
  useEffect(() => {
    scheduleSnapshots()
    return () => clearTimeout(snapTimer.current)
  }, [scheduleSnapshots, lod, theme])

  // ---- nodes and edges: derived from the board; React Flow keeps measurements and in-flight drag positions ----
  const derived = useMemo(() => nodesOf(board, { selection, readOnly, entered }) as unknown as Node[], [board, selection, readOnly, entered])
  const [nodes, setNodes] = useState<Node[]>(derived)
  const dragging = useRef(new Set<Id>())
  useEffect(() => {
    setNodes((prev) => {
      const old = new Map(prev.map((n) => [n.id, n]))
      return derived.map((n) => {
        const o = old.get(n.id)
        if (!o) return n
        return { ...n, measured: o.measured, ...(dragging.current.has(n.id) ? { position: o.position, dragging: true } : {}) }
      })
    })
  }, [derived])
  // `deleteKeyCode` is off, so no remove changes arrive: deletion is the owner's archive (keyboard/toolbar).
  const onNodesChange: OnNodesChange = useCallback((changes) => setNodes((ns) => changes.applyTo(ns)), [])
  const edges = useMemo(
    () =>
      edgesOf(board, selection).map((e): Edge => {
        const arrow = e.data.link.arrow
        const marker = { type: MarkerType.ArrowClosed, width: 16, height: 16, color: 'var(--wb-link)' }
        return { ...e, markerEnd: arrow === 'none' ? undefined : marker, markerStart: arrow === 'both' ? marker : undefined }
      }),
    [board, selection],
  )

  // ---- owner actions shared with nodes ----
  const enter = useCallback((id: Id | null) => {
    store.getState().set({ entered: id, ...(id ? { selection: [id] } : {}) })
  }, [store])
  const inkByCard = useMemo(() => indexInk(board.ink), [board.ink])
  // A pen on an entered live page inks or erases exactly when it would anywhere else on the pointer path (§8.4).
  const pagePen = useZustand(store, (s) => {
    const r = routePointer('pen', { mode: s.mode, fingerInk: s.fingerInk, nativeInk: s.nativeInk && s.inkEngine === 'auto', readOnly, penDown: false, palmUntil: 0, now: 0 })
    return r === 'ink' || r === 'erase'
  })
  const pagePenRef = useRef(pagePen)
  pagePenRef.current = pagePen
  const penRef = useRef<(card: Id, m: PenMessage) => void>(() => {})
  const penFromPage = useCallback((card: Id, m: PenMessage) => penRef.current(card, m), [])
  const ctx: CanvasCtx = useMemo(
    () => ({
      doc, store, host, board, lod, dark, readOnly, frames, enter, open: (id) => onCardOpen?.(id), stopCapturing: () => undo.stopCapturing(), spot, inkByCard,
      pagePen, penFromPage,
    }),
    [doc, store, host, board, lod, dark, readOnly, frames, enter, onCardOpen, undo, spot, inkByCard, pagePen, penFromPage],
  )

  const focus = useCallback((id: Id, opts?: { spotlight?: boolean; animate?: boolean }) => {
    const b = boxOf(id)
    if (!b) return
    void rf.fitBounds({ x: b[0], y: b[1], width: b[2], height: b[3] }, { padding: 0.25, duration: opts?.animate === false || reducedMotion() ? 0 : 450 })
    if (opts?.spotlight) {
      setSpot(id)
      setTimeout(() => setSpot((s) => (s === id ? null : s)), 1800)
    }
  }, [boxOf, rf])

  // Follow (P4 stretch): when on, the agent's `wb_focus` moves this viewport.
  const focusAt = board.meta.focus?.at
  useEffect(() => {
    const f = boardRef.current.meta.focus
    if (!f || !store.getState().follow) return
    focus(f.target, { spotlight: f.spotlight, animate: true })
  }, [focusAt, focus, store])

  // ---- pointer ink, eraser, frame rubber band, palm rule, multi-finger taps (§8.4, §8.6) ----
  const wet = useRef<Wet | null>(null)
  const palmUntil = useRef(0)
  const lastKind = useRef<PointerKind>('mouse')
  const suppressClick = useRef(false)
  const press = useRef<{ id: number; x: number; y: number; timer: ReturnType<typeof setTimeout> } | null>(null)
  const taps = useRef(new MultiTap()).current
  const rubberRef = useRef<Rubber | null>(null)
  const propsRef = useRef({ onPenActivity, onCanvasTap, readOnly })
  propsRef.current = { onPenActivity, onCanvasTap, readOnly }

  const drawWet = useCallback(() => {
    const cv = wetCanvas.current
    const w = wet.current
    if (!cv) return
    const ctx2 = cv.getContext('2d')
    if (!ctx2) return
    const dpr = window.devicePixelRatio || 1
    if (cv.width !== cv.clientWidth * dpr || cv.height !== cv.clientHeight * dpr) {
      cv.width = cv.clientWidth * dpr
      cv.height = cv.clientHeight * dpr
    }
    ctx2.setTransform(1, 0, 0, 1, 0, 0)
    ctx2.clearRect(0, 0, cv.width, cv.height)
    if (!w || w.erase) return
    const { tool, colour, width } = store.getState()
    const v = viewRef.current
    ctx2.setTransform(dpr, 0, 0, dpr, 0, 0)
    const screen = w.pts.map(([x, y, p, t]) => [x * v.zoom + v.x, y * v.zoom + v.y, p, t] as Pt)
    ctx2.globalAlpha = tool === 'marker' ? 0.45 : 1
    ctx2.fillStyle = inkCss(colour, dark)
    ctx2.fill(new Path2D(pointsPath(screen, tool, width * v.zoom, w.hasPressure, false)))
  }, [dark, store])

  const eraseAt = useCallback((x: number, y: number) => {
    const b = boardRef.current
    const st = store.getState()
    const tol = 6 / viewRef.current.zoom
    const hit: Id[] = []
    for (const s of Object.values(b.ink)) {
      if (lookOf(b, s, st).opacity === 0) continue
      if (touches(s, b, x, y, tol)) hit.push(s.id)
    }
    if (hit.length) eraseStrokes(doc, hit)
  }, [doc, store])

  /** The mark whose visible ink passes under a world point, if any. */
  const markAt = useCallback((x: number, y: number): Id | null => {
    const b = boardRef.current
    const st = store.getState()
    const tol = 10 / viewRef.current.zoom
    for (const s of Object.values(b.ink)) {
      const m = b.markOf[s.id]
      if (!m || lookOf(b, s, st).opacity === 0) continue
      if (touches(s, b, x, y, tol)) return m
    }
    return null
  }, [store])

  const askCaption = useCallback((s: Stroke) => {
    const b = boardRef.current
    const c = s.card ? b.cards[s.card] : undefined
    if (!c || c.kind !== 'page') return
    const frame = frames.get(c.id)
    if (!frame) return
    const pts = unpack(s.o, s.pts)
    const lx = pts.reduce((a, p) => a + p[0], 0) / pts.length
    const ly = pts.reduce((a, p) => a + p[1], 0) / pts.length
    const [fx, fy] = pagePoint(c, c.frame ? b.frames[c.frame]?.preset : undefined, lx, ly)
    void host.describePoint(frame, fx, fy).then((d) => {
      if (d) captions.set(s.id, { ...d, at: [lx, ly] })
    })
  }, [captions, frames, host])

  const commit = useCallback((c: CapturedStroke): Id[] => {
    const { strokes } = buildStrokes(c, viewRef.current.zoom, shownCards(boardRef.current))
    if (strokes.length === 0) return []
    addStrokes(doc, strokes)
    undo.stopCapturing()
    for (const s of strokes) askCaption(s)
    return strokes.map((s) => s.id)
  }, [askCaption, doc, undo])

  // ---- Pencil over an entered live page (pointer path): the page relays pen events (canvas/penRelay.ts) ----
  penRef.current = (cardId, m) => {
    const b = boardRef.current
    const c = b.cards[cardId]
    if (!c || (m.phase === 'down' && !pagePenRef.current)) return
    const vp = pageViewport(c, c.frame ? b.frames[c.frame]?.preset : undefined)
    const scale = Math.min(c.w / vp.w, (c.h - CARD_HEADER) / vp.h)
    const pts = m.pts.map(([x, y, p, t]) => [c.x + x * scale, c.y + CARD_HEADER + y * scale, p, t] as Pt)
    if (m.phase === 'down') {
      if (wet.current) return
      const [x, y, p, t] = pts[0]!
      const erase = store.getState().mode === 'erase'
      const hasPressure = p > 0 && p !== 0.5
      wet.current = { id: PAGE_PEN, kind: 'pen', erase, pts: [[x, y, hasPressure ? p : 0.5, 0]], t0: t, hasPressure }
      store.getState().set({ penDown: true })
      onPenActivity?.(true)
      if (erase) {
        releaseErase.current?.()
        releaseErase.current = holdGroup()
        eraseAt(x, y)
      } else requestAnimationFrame(drawWet)
      return
    }
    const w = wet.current
    if (!w || w.id !== PAGE_PEN) return
    for (const [x, y, p, t] of m.phase === 'move' ? pts : pts.slice(-1)) {
      if (w.erase) eraseAt(x, y)
      else if (m.phase === 'move') w.pts.push([x, y, w.hasPressure ? p : 0.5, Math.max(0, t - w.t0)])
    }
    if (m.phase === 'move') {
      if (!w.erase) requestAnimationFrame(drawWet)
      return
    }
    wet.current = null
    palmUntil.current = performance.now() + PALM_MS
    store.getState().set({ penDown: false })
    onPenActivity?.(false)
    if (w.erase) {
      releaseErase.current?.()
      releaseErase.current = null
      return
    }
    if (m.phase === 'cancel' && w.pts.length < 2) {
      drawWet()
      return
    }
    const s = store.getState()
    commit({ pts: w.pts.map((p) => [...p] as [number, number, number, number]), tool: s.tool, colour: s.colour, width: s.width, hasPressure: w.hasPressure, at: Date.now() })
    void nextFrame().then(() => {
      if (!wet.current) drawWet()
    })
  }

  useEffect(() => {
    const el = wrap.current
    if (!el) return
    const stateNow = () => {
      const s = store.getState()
      return { mode: s.mode, fingerInk: s.fingerInk, nativeInk: s.nativeInk && s.inkEngine === 'auto', readOnly: propsRef.current.readOnly, penDown: s.penDown, palmUntil: palmUntil.current, now: performance.now() }
    }
    const isControl = (t: EventTarget | null) => t instanceof Element && t.closest(CONTROL) !== null
    const onPane = (t: EventTarget | null) => t instanceof Element && t.closest('.react-flow__node, .react-flow__edge, .react-flow__handle, .react-flow__nodesselection') === null
    const penUp = () => {
      palmUntil.current = performance.now() + PALM_MS
      store.getState().set({ penDown: false })
      propsRef.current.onPenActivity?.(false)
    }

    const down = (e: PointerEvent) => {
      const kind = pointerKind(e.pointerType)
      lastKind.current = kind
      suppressClick.current = false
      if (kind === 'touch') taps.down(e.pointerId, e.clientX, e.clientY, e.timeStamp)
      if (isControl(e.target) && kind !== 'touch') return
      const route = routePointer(kind, stateNow())
      const s = store.getState()
      if (s.mode === 'frame' && kind !== 'pen' && route === 'pass' && onPane(e.target) && !propsRef.current.readOnly) {
        e.preventDefault()
        e.stopPropagation()
        el.setPointerCapture(e.pointerId)
        const p = toWorld(e.clientX, e.clientY)
        rubberRef.current = { id: e.pointerId, x0: p.x, y0: p.y, x1: p.x, y1: p.y }
        setRubber(rubberRef.current)
        return
      }
      if (s.mode === 'select' && kind !== 'pen' && route === 'pass' && onPane(e.target) && !isControl(e.target)) {
        e.preventDefault()
        e.stopPropagation()
        el.setPointerCapture(e.pointerId)
        const p = toWorld(e.clientX, e.clientY)
        lassoRef.current = { id: e.pointerId, pts: [[p.x, p.y]] }
        setLasso(lassoRef.current)
        return
      }
      if (route === 'pass') {
        // Long-press on a mark's ink (finger or mouse) → Pin / Unpin.
        clearTimeout(press.current?.timer)
        const cx = e.clientX
        const cy = e.clientY
        press.current = {
          id: e.pointerId, x: cx, y: cy,
          timer: setTimeout(() => {
            press.current = null
            const p = toWorld(cx, cy)
            const mark = markAt(p.x, p.y)
            if (!mark || propsRef.current.readOnly) return
            suppressClick.current = true
            const r = el.getBoundingClientRect()
            setMarkMenu({ x: cx - r.left, y: cy - r.top, mark })
          }, LONG_PRESS_MS),
        }
        return
      }
      e.preventDefault()
      e.stopPropagation()
      if (route === 'ignore' || route === 'swallow') {
        if (kind === 'pen' && route === 'swallow') {
          // Native ink draws it; the palm rule and the cluster still follow the pen.
          store.getState().set({ penDown: true })
          propsRef.current.onPenActivity?.(true)
        }
        return
      }
      if (wet.current) return
      el.setPointerCapture(e.pointerId)
      const p = toWorld(e.clientX, e.clientY)
      const hasPressure = kind === 'pen' && e.pressure > 0 && e.pressure !== 0.5
      wet.current = { id: e.pointerId, kind, erase: route === 'erase', pts: [[p.x, p.y, hasPressure ? e.pressure : 0.5, 0]], t0: e.timeStamp, hasPressure }
      if (kind === 'pen') {
        store.getState().set({ penDown: true })
        propsRef.current.onPenActivity?.(true)
      }
      if (route === 'erase') {
        releaseErase.current?.()
        releaseErase.current = holdGroup()
        eraseAt(p.x, p.y)
      } else requestAnimationFrame(drawWet)
    }

    const move = (e: PointerEvent) => {
      if (e.pointerType === 'touch') taps.move(e.pointerId, e.clientX, e.clientY)
      const pr = press.current
      if (pr && pr.id === e.pointerId && Math.hypot(e.clientX - pr.x, e.clientY - pr.y) > 8) {
        clearTimeout(pr.timer)
        press.current = null
      }
      const ls = lassoRef.current
      if (ls && ls.id === e.pointerId) {
        e.stopPropagation()
        const p = toWorld(e.clientX, e.clientY)
        const last = ls.pts[ls.pts.length - 1]!
        if (Math.hypot(p.x - last[0], p.y - last[1]) * viewRef.current.zoom >= 3) {
          lassoRef.current = { ...ls, pts: [...ls.pts, [p.x, p.y]] }
          setLasso(lassoRef.current)
        }
        return
      }
      const rb = rubberRef.current
      if (rb && rb.id === e.pointerId) {
        e.stopPropagation()
        const p = toWorld(e.clientX, e.clientY)
        rubberRef.current = { ...rb, x1: p.x, y1: p.y }
        setRubber(rubberRef.current)
        return
      }
      const w = wet.current
      if (!w || w.id !== e.pointerId) return
      e.preventDefault()
      e.stopPropagation()
      const events = typeof e.getCoalescedEvents === 'function' ? e.getCoalescedEvents() : []
      for (const ev of events.length ? events : [e]) {
        const p = toWorld(ev.clientX, ev.clientY)
        if (w.erase) eraseAt(p.x, p.y)
        else w.pts.push([p.x, p.y, w.hasPressure ? ev.pressure : 0.5, Math.max(0, ev.timeStamp - w.t0)])
      }
      if (!w.erase) {
        // Predicted points draw ahead of the finger/pen for the frame only; they are never committed.
        const predicted = typeof e.getPredictedEvents === 'function' ? e.getPredictedEvents() : []
        if (predicted.length) {
          const keep = w.pts.length
          for (const ev of predicted) {
            const p = toWorld(ev.clientX, ev.clientY)
            w.pts.push([p.x, p.y, w.hasPressure ? ev.pressure : 0.5, Math.max(0, ev.timeStamp - w.t0)])
          }
          drawWet()
          w.pts.length = keep
        } else requestAnimationFrame(drawWet)
      }
    }

    const up = (e: PointerEvent) => {
      if (e.pointerType === 'touch') {
        const n = taps.up(e.pointerId, e.timeStamp)
        if (n === 2 && !propsRef.current.readOnly) undo.undo()
        if (n === 3 && !propsRef.current.readOnly) undo.redo()
      }
      if (press.current?.id === e.pointerId) {
        clearTimeout(press.current.timer)
        press.current = null
      }
      const ls = lassoRef.current
      if (ls && ls.id === e.pointerId) {
        e.stopPropagation()
        suppressClick.current = true
        lassoRef.current = null
        setLasso(null)
        if (e.type !== 'pointerup' || ls.pts.length < 3) return
        const st = store.getState()
        const hit = lassoSelect(boardRef.current, ls.pts, st)
        st.set({ selection: [...hit.frames, ...hit.cards, ...hit.links, ...hit.marks], entered: null })
        setInkSel(hit.strokes)
        return
      }
      const rb = rubberRef.current
      if (rb && rb.id === e.pointerId) {
        e.stopPropagation()
        suppressClick.current = true
        rubberRef.current = null
        setRubber(null)
        const x = Math.min(rb.x0, rb.x1)
        const y = Math.min(rb.y0, rb.y1)
        const w = Math.abs(rb.x1 - rb.x0)
        const h = Math.abs(rb.y1 - rb.y0)
        if (w >= 40 && h >= 40 && e.type === 'pointerup') {
          // The drawn frame takes every card whose centre it encloses (from a larger frame too: the innermost wins, as on drop).
          const inside = shownCards(boardRef.current).filter((c) => contains([x, y, w, h], [c.x + c.w / 2, c.y + c.h / 2, 0, 0])).map((c) => c.id)
          const id = addFrame(doc, { title: 'Frame', x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(h), cards: inside })
          undo.stopCapturing()
          store.getState().set({ mode: 'idle', selection: [id] })
        }
        return
      }
      const wt = wet.current
      if (!wt || wt.id !== e.pointerId) return
      e.stopPropagation()
      wet.current = null
      suppressClick.current = true // the click that follows a stroke is not a tap on the canvas
      if (wt.kind === 'pen') penUp()
      if (wt.erase) {
        releaseErase.current?.()
        releaseErase.current = null
        return
      }
      if (e.type === 'pointercancel' && wt.pts.length < 2) {
        drawWet()
        return
      }
      const s = store.getState()
      commit({ pts: wt.pts.map((p) => [...p] as [number, number, number, number]), tool: s.tool, colour: s.colour, width: s.width, hasPressure: wt.hasPressure, at: Date.now() })
      // Clear the wet stroke after the committed one has painted.
      void nextFrame().then(() => {
        if (!wet.current) drawWet()
      })
    }

    const swallowSwallowedPenUp = (e: PointerEvent) => {
      if (e.pointerType === 'pen' && store.getState().penDown && !wet.current) penUp()
    }

    // iOS sends touch events for the Pencil (touchType 'stylus') beside pointer events; d3-zoom listens to touches.
    const touch = (e: TouchEvent) => {
      const stylus = Array.from(e.changedTouches).some((t) => (t as Touch & { touchType?: string }).touchType === 'stylus')
      const s = stateNow()
      // In link mode the Pencil is a pointer: its touches reach React Flow (node taps, handle drags).
      const pencilInk = stylus && s.mode !== 'link'
      const palm = !stylus && (s.penDown || s.now < s.palmUntil)
      const fingerInking = wet.current !== null && wet.current.kind === 'touch'
      if (pencilInk || palm || fingerInking || rubberRef.current || lassoRef.current) {
        e.stopPropagation()
        if (e.cancelable && !isControl(e.target)) e.preventDefault()
      }
    }
    const mouse = (e: MouseEvent) => {
      if (wet.current || rubberRef.current || lassoRef.current) e.stopPropagation()
    }

    el.addEventListener('pointerdown', down, { capture: true })
    el.addEventListener('pointermove', move, { capture: true })
    el.addEventListener('pointerup', up, { capture: true })
    el.addEventListener('pointercancel', up, { capture: true })
    el.addEventListener('pointerup', swallowSwallowedPenUp)
    el.addEventListener('touchstart', touch, { capture: true, passive: false })
    el.addEventListener('touchmove', touch, { capture: true, passive: false })
    el.addEventListener('mousedown', mouse, { capture: true })
    return () => {
      el.removeEventListener('pointerdown', down, { capture: true })
      el.removeEventListener('pointermove', move, { capture: true })
      el.removeEventListener('pointerup', up, { capture: true })
      el.removeEventListener('pointercancel', up, { capture: true })
      el.removeEventListener('pointerup', swallowSwallowedPenUp)
      el.removeEventListener('touchstart', touch, { capture: true })
      el.removeEventListener('touchmove', touch, { capture: true })
      el.removeEventListener('mousedown', mouse, { capture: true })
    }
  }, [commit, doc, drawWet, eraseAt, markAt, store, taps, toWorld, undo])

  // Wrapper size (for LOD and the wet canvas).
  useEffect(() => {
    const el = wrap.current
    if (!el) return
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // ---- keyboard (full window only). The shell owns V P E L F N O, ⌘0, ⌘↵ and Esc for cards/composer/cluster;
  // the canvas owns undo/redo, Enter (enter the selected card), Delete (archive / remove / erase) and Esc for its
  // own modes and selection. ----
  useEffect(() => {
    if (embedded) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target
      if (t instanceof HTMLElement && (t.isContentEditable || t.closest('input, textarea, select, [role="dialog"]'))) return
      const s = store.getState()
      const mod = e.metaKey || e.ctrlKey
      if (e.key === 'Escape') {
        if (s.entered) return // the shell leaves the card
        if (menuOpen.current) setMenu(null)
        if (s.mode === 'link' || s.mode === 'select' || s.mode === 'frame') s.set({ mode: 'idle' })
        else if (s.selection.length || inkSelRef.current.length) {
          s.set({ selection: [] })
          setInkSel([])
        }
        linkFrom.current = null
        return
      }
      if (mod && e.key.toLowerCase() === 'z') {
        e.preventDefault()
        if (readOnly) return
        if (e.shiftKey) undo.redo()
        else undo.undo()
        return
      }
      if (mod && e.key.toLowerCase() === 'y') {
        e.preventDefault()
        if (!readOnly) undo.redo()
        return
      }
      if (mod || e.altKey) return
      if (e.key === 'Enter' && s.selection.length === 1 && boardRef.current.cards[s.selection[0]!]) {
        e.preventDefault()
        enter(s.selection[0]!)
        return
      }
      if ((e.key === 'Delete' || e.key === 'Backspace') && (s.selection.length || inkSelRef.current.length) && !readOnly) {
        e.preventDefault()
        deleteSelection()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [deleteSelection, embedded, enter, readOnly, store, undo])

  // ---- the handle (§8.2) ----
  const freeSpot = useCallback((w: number, h: number) => {
    const b = boardRef.current
    const [vx, vy, vw, vh] = worldView()
    const cx = vx + vw / 2 - w / 2
    const cy = vy + vh / 2 - h / 2
    const blocked: Box[] = [
      ...shownCards(b).map((c) => pad([c.x, c.y, c.w, c.h], 16)),
      ...Object.values(b.ink).map((s) => strokeWorldBox(s, b.cards)).filter((x): x is Box => x !== null),
    ]
    const free = (x: number, y: number) => !blocked.some((r) => intersects(r, [x, y, w, h]))
    const step = 24
    for (let ring = 0; ring < 80; ring++) {
      for (let i = -ring; i <= ring; i++) {
        for (const [dx, dy] of [[i, -ring], [i, ring], [-ring, i], [ring, i]] as const) {
          const x = Math.round(cx + dx * step)
          const y = Math.round(cy + dy * step)
          if (free(x, y)) return { x, y }
        }
      }
    }
    return { x: Math.round(cx), y: Math.round(cy) }
  }, [worldView])
  const freeSpotRef = useRef(freeSpot)
  freeSpotRef.current = freeSpot

  useImperativeHandle(handle, (): WhiteboardCanvasHandle => ({
    fit(target = 'all') {
      const duration = reducedMotion() ? 0 : 350
      if (target === 'all') {
        void rf.fitView({ padding: 0.1, duration })
        return
      }
      const ids = target === 'selection' ? store.getState().selection : target.ids
      let u: Box | null = null
      for (const id of ids) {
        const b = boxOf(id)
        if (b) u = union(u, b)
      }
      if (!u) return
      // fitBounds has no maxZoom: one small card would fill the window at 3×+. Cap at 100 %, centred.
      const r = wrap.current?.getBoundingClientRect()
      const zoom = r ? Math.min(1, r.width / (u[2] * 1.3), r.height / (u[3] * 1.3)) : 1
      void rf.setCenter(u[0] + u[2] / 2, u[1] + u[3] / 2, { zoom, duration })
    },
    focus,
    view: () => ({ ...viewRef.current }),
    setView(v, opts) {
      void rf.setViewport({ x: v.x, y: v.y, zoom: v.zoom }, { duration: opts?.animate && !reducedMotion() ? 350 : 0 })
    },
    frameRect: () => wrap.current?.getBoundingClientRect() ?? new DOMRect(),
    screenRectOf(id) {
      const b = boxOf(id)
      return b ? windowRect(b, false) : null
    },
    visible() {
      const b = boardRef.current
      const view = worldView()
      const fr = Object.values(b.frames).filter((f) => contains(view, [f.x, f.y, f.w, f.h])).sort((a, c) => a.y - c.y || a.x - c.x)
      const inFrame = new Set(fr.map((f) => f.id))
      const cards = shownCards(b)
        .filter((c) => !(c.frame && b.frames[c.frame]) && intersects(view, [c.x, c.y, c.w, c.h]))
        .sort((a, c) => a.y - c.y || a.x - c.x)
        .slice(0, 12)
      return { frames: [...inFrame], cards: cards.map((c) => c.id) }
    },
    freeSpot,
    undo: () => undo.undo(),
    redo: () => undo.redo(),
    canUndo: () => undo.canUndo(),
    canRedo: () => undo.canRedo(),
    inkStats() {
      const b = boardRef.current
      const f = freshStrokes(b, worldView())
      const cards = new Set<Id>()
      for (const s of f.inView) if (s.card) cards.add(s.card)
      return { fresh: f.inView.length + f.offScreen.length, freshInView: f.inView.length, inkedCards: [...cards] }
    },
    async commitCaptured(strokes) {
      const ids: Id[] = []
      for (const c of strokes) ids.push(...commit(c))
      await nextFrame()
      return ids
    },
    planSend: (opts) => plan(boardRef.current, { view: worldView(), offScreen: opts?.offScreen, captions, windowRect: (w) => windowRect(w) }),
    composeImage: (image, maxSide) => {
      const s = store.getState()
      return composeImage(boardRef.current, image, maxSide, { dark, blobUrl: (k) => host.blobUrl(k), showAnswered: s.showAnswered, scrub: s.scrub })
    },
    commitSend(p, extra) {
      writeSend(doc, boardRef.current, p, extra)
    },
  }), [boxOf, captions, commit, dark, doc, focus, freeSpot, host, rf, store, undo, windowRect, worldView])

  // ---- React Flow callbacks ----
  const onMove = useCallback((ev: unknown, v: Viewport) => {
    if (ev) fitted.current = true // the owner moved: no late fit
    viewRef.current = { x: v.x, y: v.y, zoom: v.zoom }
    onViewChange?.(viewRef.current, 'move')
  }, [onViewChange])
  const onMoveEnd = useCallback((_: unknown, v: Viewport) => {
    viewRef.current = { x: v.x, y: v.y, zoom: v.zoom }
    onViewChange?.(viewRef.current, 'end')
    recomputeLod()
    scheduleSnapshots()
  }, [onViewChange, recomputeLod, scheduleSnapshots])
  const onInit = useCallback(() => {
    const v = rf.getViewport()
    viewRef.current = { x: v.x, y: v.y, zoom: v.zoom }
    onViewChange?.(viewRef.current, 'end')
    recomputeLod()
  }, [onViewChange, recomputeLod, rf])
  // The document often arrives after mount (engine sync): fit once when the first cards show up, unless the owner moved.
  const fitted = useRef(Object.keys(board.cards).length > 0)
  const hasCards = Object.keys(board.cards).length > 0
  useEffect(() => {
    if (fitted.current || !hasCards) return
    fitted.current = true
    const t = setTimeout(() => void rf.fitView({ padding: 0.1, maxZoom: 1 }), 50)
    return () => clearTimeout(t)
  }, [hasCards, rf])

  const onConnect: OnConnect = useCallback((c) => {
    if (!c.source || !c.target || c.source === c.target) return
    addLink(doc, { from: c.source, to: c.target, from_port: portOf(c.sourceHandle), to_port: portOf(c.targetHandle) })
    undo.stopCapturing()
  }, [doc, undo])
  const onConnectEnd = useCallback((e: MouseEvent | TouchEvent, st: FinalConnectionState) => {
    if (st.isValid || !st.fromNode || st.toNode) return
    const pt = 'changedTouches' in e ? e.changedTouches[0] : e
    if (!pt || !wrap.current) return
    const r = wrap.current.getBoundingClientRect()
    setMenu({ x: pt.clientX - r.left, y: pt.clientY - r.top, world: toWorld(pt.clientX, pt.clientY), from: st.fromNode.id, port: portOf(st.fromHandle?.id) })
  }, [toWorld])

  const onNodeClick = useCallback((_: unknown, n: Node) => {
    const s = store.getState()
    if (s.mode !== 'link' || readOnly || !boardRef.current.cards[n.id]) return
    if (!linkFrom.current) {
      linkFrom.current = n.id
      s.set({ selection: [n.id] })
    } else if (linkFrom.current !== n.id) {
      addLink(doc, { from: linkFrom.current, to: n.id })
      undo.stopCapturing()
      linkFrom.current = null
      s.set({ selection: [] })
    }
  }, [doc, readOnly, store, undo])
  const onPaneClick = useCallback(() => {
    const kind = lastKind.current
    if (kind === 'pen' || suppressClick.current) {
      suppressClick.current = false
      return
    }
    const s = store.getState()
    setMenu(null)
    linkFrom.current = null
    s.set({ entered: null, selection: [], ...(s.mode === 'link' ? { mode: 'idle' } : {}) })
    setInkSel([])
    setMarkMenu(null)
    propsRef.current.onCanvasTap?.({ pointerType: kind === 'touch' ? 'touch' : 'mouse' })
  }, [store])
  const onSelectionChange = useCallback(({ nodes: ns, edges: es }: { nodes: Node[]; edges: Edge[] }) => {
    // Marks ride in the selection beside nodes and edges (the lasso puts them there); React Flow does not know them.
    const marks = store.getState().selection.filter((id) => boardRef.current.marks[id])
    const next = [...ns.map((n) => n.id), ...es.map((e) => e.id), ...marks]
    const s = store.getState()
    if (next.length === s.selection.length && next.every((id, i) => s.selection[i] === id)) return
    s.set({ selection: next, ...(s.entered && !next.includes(s.entered) ? { entered: null } : {}) })
  }, [store])
  const onNodeDragStart = useCallback((_: unknown, __: Node, ns: Node[]) => {
    for (const n of ns) dragging.current.add(n.id)
  }, [])
  const onNodeDragStop = useCallback((_: unknown, node: Node, ns: Node[]) => {
    const b = boardRef.current
    const dropped = (ns.length ? ns : [node]).map((n) => ({ id: n.id, position: n.position }))
    dragging.current.clear()
    if (readOnly) return
    moveItems(doc, b, dragEnd(b, dropped))
    if (dropped.length === 1 && b.cards[dropped[0]!.id]) raise(doc, dropped[0]!.id)
    undo.stopCapturing()
  }, [doc, readOnly, undo])

  const bg = board.meta.bg
  const cls = ['wb-canvas relative size-full', mode === 'link' && 'link-mode', className].filter(Boolean).join(' ')
  return (
    <Ctx.Provider value={ctx}>
      <div ref={wrap} className={cls} data-mode={mode}>
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={NODE_TYPES}
          edgeTypes={EDGE_TYPES}
          onNodesChange={onNodesChange}
          onInit={onInit}
          onMove={onMove}
          onMoveEnd={onMoveEnd}
          onConnect={onConnect}
          onConnectEnd={onConnectEnd}
          onNodeClick={onNodeClick}
          onPaneClick={onPaneClick}
          onSelectionChange={onSelectionChange}
          onNodeDragStart={onNodeDragStart}
          onNodeDragStop={onNodeDragStop}
          nodesDraggable={!readOnly}
          nodesConnectable={!readOnly}
          elementsSelectable
          connectionMode={ConnectionMode.Loose}
          panOnDrag={mode !== 'select' && mode !== 'frame'}
          selectionOnDrag={mode === 'select'}
          selectionMode={SelectionMode.Partial}
          panActivationKeyCode="Space"
          zoomOnDoubleClick={false}
          deleteKeyCode={null}
          onlyRenderVisibleElements={false}
          minZoom={0.1}
          maxZoom={4}
          fitView={Object.keys(board.cards).length > 0}
          fitViewOptions={{ padding: 0.1, maxZoom: 1 }}
          proOptions={{ hideAttribution: true }}
          forceColorMode={dark ? 'dark' : 'light'}
          aria-label="Whiteboard canvas"
        >
          {bg === 'dots' && <Background variant={BackgroundVariant.Dots} gap={22} size={1.4} color="color-mix(in oklab, var(--fg-tertiary) 42%, transparent)" />}
          {bg === 'grid' && <Background variant={BackgroundVariant.Lines} gap={44} color="color-mix(in oklab, var(--fg-tertiary) 18%, transparent)" />}
          <ViewportPortal>
            <FreeInk />
            {inkSel.length > 0 && <InkSelection board={board} ids={inkSel} />}
          </ViewportPortal>
          {clusterOpen && (
            <Panel position="top-left" className="wb-attribution">
              <a href="https://reactflow.dev" target="_blank" rel="noreferrer">
                React Flow
              </a>
            </Panel>
          )}
          {anchored && anchored.length > 0 && <Anchored items={anchored} board={board} size={size} right={clusterOpen ? 94 : 24} />}
        </ReactFlow>
        <canvas ref={wetCanvas} className="wb-wet" aria-hidden />
        {rubber && <RubberBand r={rubber} />}
        {lasso && <LassoPath pts={lasso.pts} />}
        <Offscreen board={board} width={size.w} height={size.h} onGo={(id) => focus(id, { animate: true })} />
        {markMenu && board.marks[markMenu.mark] && (
          <Menu x={markMenu.x} y={markMenu.y}>
            {board.marks[markMenu.mark]!.state === 'pinned' ? (
              <button type="button" onClick={() => { pinMark(doc, markMenu.mark, false); undo.stopCapturing(); setMarkMenu(null) }}>
                Unpin mark
              </button>
            ) : (
              <button type="button" onClick={() => { pinMark(doc, markMenu.mark, true); undo.stopCapturing(); setMarkMenu(null) }}>
                Pin mark as an annotation
              </button>
            )}
            <button type="button" onClick={() => setMarkMenu(null)}>
              Cancel
            </button>
          </Menu>
        )}
        {!readOnly && (inkSel.length > 0 || selection.length > 1) && (
          <SelectionBar
            board={board}
            selection={selection}
            ink={inkSel}
            onDim={(tag) => { dim(doc, selection.filter((id) => board.cards[id]), tag); undo.stopCapturing() }}
            onPin={(pinned) => { for (const id of selection) if (board.marks[id]) pinMark(doc, id, pinned); undo.stopCapturing() }}
            onDelete={deleteSelection}
            onClear={() => { store.getState().set({ selection: [] }); setInkSel([]) }}
          />
        )}
        {menu && (
          <Menu x={menu.x} y={menu.y}>
            <button
              type="button"
              onClick={() => {
                const id = addCard(doc, { kind: 'sticky', title: 'Sticky', x: Math.round(menu.world.x - STICKY.w / 2), y: Math.round(menu.world.y - STICKY.h / 2), ...STICKY, source: { t: 'sticky', text: '' } })
                addLink(doc, { from: menu.from, to: id, from_port: menu.port })
                undo.stopCapturing()
                setMenu(null)
                store.getState().set({ selection: [id], entered: id })
              }}
            >
              New sticky linked here
            </button>
            <button type="button" onClick={() => setMenu(null)}>
              Cancel
            </button>
          </Menu>
        )}
      </div>
    </Ctx.Provider>
  )
}

function RubberBand({ r }: { r: Rubber }) {
  const { x, y, zoom } = useViewport()
  const left = Math.min(r.x0, r.x1) * zoom + x
  const top = Math.min(r.y0, r.y1) * zoom + y
  return <div className="wb-rubber" style={{ left, top, width: Math.abs(r.x1 - r.x0) * zoom, height: Math.abs(r.y1 - r.y0) * zoom }} />
}

function Menu({ x, y, children }: { x: number; y: number; children: ReactNode }) {
  return (
    <div className="wb-menu" style={{ left: x, top: y }}>
      <div className="wb-tools" role="menu">
        {children}
      </div>
    </div>
  )
}

const Canvas = forwardRef<WhiteboardCanvasHandle, WhiteboardCanvasProps>(function Canvas(props, ref) {
  return (
    <ReactFlowProvider>
      <Inner {...props} handle={ref} />
    </ReactFlowProvider>
  )
})

export default Canvas
