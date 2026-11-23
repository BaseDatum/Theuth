// 208 §7.1 / §8.6: the Attention page's graph region — a 32 px strip collapsed; expanded, a resizable canvas with Fit,
// Follow, Full screen and TTL; full screen over the app. Owns the data hook, the TTL clock and the list ↔ graph wiring;
// the canvas itself (and `@xyflow/react`) loads lazily on first expand.
import { ChevronLeft, Crosshair, Maximize2, Minimize2, Scan } from 'lucide-react'
import { Fragment, lazy, Suspense, useEffect, useMemo, useRef, useState } from 'react'
import { CONTROL } from '@/components/attention/Page'
import type { SubtreeFilter } from '@/components/attention/model'
import { Button } from '@/components/ui/button'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { useIsMobile } from '@/hooks/use-mobile'
import { cn } from '@/lib/utils'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { EdgePopover } from './EdgePopover'
import { NodeBar } from './NodeBar'
import { defaultGraphHeight, useGraphPrefs } from './prefs'
import { graphNow } from './clock'
import { isStale } from './lifecycle'
import { useAgentGraph } from './queries'
import { attentionBySession, includeSessions, nodeLabel, stripCounts, stripText, subtreeIds } from './selectors'
import { Strip } from './Strip'
import { LegendPopover } from './Legend'
import { TtlPopover } from './TtlPopover'
import { nextTtlTick, visibleGraph } from './ttl'

const Canvas = lazy(() => import('./Canvas'))

export type GraphRegionProps = {
  /** The 207 cache, for attention counts and `include`. */
  items: readonly AttentionItem[] | undefined
  /** The list header's count of live (unexpired) items: the strip's "need you". */
  live: number
  /** Session of the focused entry's head, else of the open item. */
  highlight: string | null
  /** "Show in graph": expand, centre (no zoom change), highlight. */
  focus: { session: string; nonce: number } | null
  /** The current filter's session. */
  subtree: string | null
  /** Node click; re-sent when the subtree's ids change. */
  onSubtree: (f: SubtreeFilter | null) => void
  /** A tapped node also opens its session in the page's detail pane. */
  onOpenSession: (session: string) => void
}

const MIN_H = 160
const sameIds = (a: ReadonlySet<string>, b: ReadonlySet<string>) => a.size === b.size && [...a].every((x) => b.has(x))
const editable = (t: EventTarget | null) => t instanceof HTMLElement && (t.isContentEditable || t.closest(CONTROL) !== null)

export function GraphRegion({ items, live, highlight, focus, subtree, onSubtree, onOpenSession }: GraphRegionProps) {
  const prefs = useGraphPrefs()
  const mobile = useIsMobile()
  // Under md the region starts collapsed and opens as a full-screen pane; full screen is never stored.
  const [open, setOpen] = useState(() => !mobile && prefs.expanded)
  const [full, setFull] = useState(false)
  const [selected, setSelected] = useState<string | null>(null)
  const [edgeAt, setEdgeAt] = useState<{ id: string; at: { x: number; y: number } } | null>(null)
  const [fitNonce, setFitNonce] = useState(0)
  const realNow = Date.now()
  const include = useMemo(() => includeSessions(items ?? [], realNow), [items, Math.floor(realNow / 60_000)])
  // The stream lives with the page while the document is visible (review U4); the canvas mounts only when shown.
  const { state, supported, error } = useAgentGraph(prefs.ttl, include)
  const attention = useMemo(() => attentionBySession(items ?? [], Date.now()), [items])

  // The TTL clock: re-derived on every graph change and at the next visible change (nextTtlTick), never per second.
  // The clock is read in the same render as a new state (review U8: one derivation per delta, not one with the old
  // clock and another after an effect), and again when the TTL timer ticks.
  const [tick, setTick] = useState(0)
  const now = useMemo(() => graphNow(), [state, tick])
  const view = useMemo(() => (state ? visibleGraph(state, attention, prefs.ttl, now) : null), [state, attention, prefs.ttl, now])
  useEffect(() => {
    if (!state || !view) return
    const t = setTimeout(() => setTick((n) => n + 1), nextTtlTick(state, view, prefs.ttl, now))
    return () => clearTimeout(t)
  }, [state, view, prefs.ttl, now])

  // Keep the list's subtree filter in step with the graph: its ids follow spawns and deletions.
  const sent = useRef<ReadonlySet<string> | null>(null)
  const labelOf = useRef<{ id: string; label: string } | null>(null)
  useEffect(() => {
    if (!state || subtree === null) {
      sent.current = null
      return
    }
    const ids = subtreeIds(state.nodes, subtree)
    const node = state.nodes.get(subtree)
    if (!node || (sent.current && sameIds(sent.current, ids))) return
    sent.current = ids
    onSubtree({ session: subtree, label: nodeLabel(node), ids })
  }, [state, subtree, onSubtree])

  // "Show in graph": open (full screen under md), select nothing, the canvas centres on the nonce.
  useEffect(() => {
    if (!focus) return
    if (mobile) setFull(true)
    else setOpen(true)
  }, [focus?.nonce, mobile])

  const toggle = () => {
    if (mobile) {
      setFull((f) => !f)
      return
    }
    const next = !open
    setOpen(next)
    prefs.set({ expanded: next })
  }
  const latest = useRef({ toggle, full, setFull })
  latest.current = { toggle, full, setFull }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || editable(e.target)) return
      const l = latest.current
      // `g` would toggle the region hidden under full screen (review U5).
      if (e.key === 'g' && !e.shiftKey && !l.full) {
        e.preventDefault()
        l.toggle()
      } else if (e.key === 'G' && e.shiftKey) {
        e.preventDefault()
        l.setFull(!l.full)
      }
    }
    // Esc leaves full screen before anything below sees it (the page's own handlers swallow a bare Escape), unless a
    // popover, menu or dialog (command palette, confirm) is open — that one closes first (review U5).
    const onEsc = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.shiftKey || !latest.current.full || editable(e.target)) return
      if (document.querySelector('[data-radix-popper-content-wrapper], [role="dialog"], [role="alertdialog"]')) return
      e.preventDefault()
      e.stopPropagation()
      latest.current.setFull(false)
    }
    window.addEventListener('keydown', onKey)
    window.addEventListener('keydown', onEsc, true)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('keydown', onEsc, true)
    }
  }, [])

  const regionRef = useRef<HTMLDivElement>(null)
  const top = useRef(0)
  // The page's height bounds the drag; the region's direct parent is a `display: contents` wrapper (height 0).
  const maxH = () => Math.max(MIN_H, 0.8 * (regionRef.current?.closest('article')?.clientHeight ?? globalThis.innerHeight ?? 1000))
  const setHeight = (h: number) => prefs.set({ height: Math.round(Math.min(maxH(), Math.max(MIN_H, h))) })

  // Legacy discovery counts every method as present: an engine that then rejects `agent_graph` shows no region either.
  if (!supported || (error !== null && state === null)) return null
  // "need you" is the list's own count (the header's number); working / finished come from the graph, once loaded.
  const graphCounts = view ? stripCounts(view) : null
  const counts = { needYou: live, working: graphCounts?.working ?? null, finished: graphCounts?.finished ?? null }
  const height = prefs.height ?? defaultGraphHeight(globalThis.innerHeight ?? 900)
  const selectedNode = selected && state ? state.nodes.get(selected) : undefined
  const edge = edgeAt && state ? state.edges.get(edgeAt.id) : undefined
  const subtreeNode = subtree !== null ? state?.nodes.get(subtree) : undefined
  if (subtree !== null && subtreeNode) labelOf.current = { id: subtree, label: nodeLabel(subtreeNode) }
  // Review U6: a filtered tree that left the graph keeps its filter (its items may still be listed) under its last
  // known name, never a raw session id.
  const lastLabel = labelOf.current?.id === subtree ? labelOf.current.label : 'A session'
  const subtreeLabel = subtree === null ? null : subtreeNode ? nodeLabel(subtreeNode) : state ? `${lastLabel} (left the graph)` : lastLabel

  const nodeClick = (id: string) => {
    setEdgeAt(null)
    const node = state?.nodes.get(id)
    if (!node || !state) return
    if (selected === id) {
      setSelected(null)
      if (subtree === id) onSubtree(null)
      return
    }
    setSelected(id)
    if (node.kind !== 'peer') onSubtree({ session: id, label: nodeLabel(node), ids: subtreeIds(state.nodes, id) })
    // The tap opens the session itself beside the list; the inspector stays for the item and the filter.
    onOpenSession(id)
  }
  const controls = (
    <>
      <Button size="icon-xs" variant="ghost" aria-label="Fit" title="Fit and re-pack" onClick={() => setFitNonce((n) => n + 1)}>
        <Scan />
      </Button>
      <Button size="icon-xs" variant={prefs.follow ? 'secondary' : 'ghost'} aria-label="Follow activity" aria-pressed={prefs.follow} title="Follow activity" onClick={() => prefs.set({ follow: !prefs.follow })}>
        <Crosshair />
      </Button>
      <LegendPopover />
      <TtlPopover />
      <Button size="icon-xs" variant="ghost" aria-label={full ? 'Leave full screen' : 'Full screen'} aria-keyshortcuts="Shift+G" title={full ? 'Leave full screen (Esc)' : 'Full screen (Shift+G)'} onClick={() => setFull(!full)}>
        {full ? <Minimize2 /> : <Maximize2 />}
      </Button>
    </>
  )
  const canvas = state && view && (
    <div className="relative min-h-0 flex-1">
      <Suspense fallback={<div className="size-full bg-background" />}>
        <Canvas
          state={state}
          view={view}
          now={now}
          highlight={highlight}
          selected={selected}
          focus={focus}
          follow={prefs.follow}
          fitNonce={fitNonce}
          onNodeClick={nodeClick}
          onEdgeClick={(id, at) => setEdgeAt({ id, at })}
          onPaneClick={() => setEdgeAt(null)}
        />
      </Suspense>
      {selectedNode && (
        <NodeBar
          node={selectedNode}
          filtered={subtree === selectedNode.id}
          onFilter={() => (subtree === selectedNode.id ? onSubtree(null) : onSubtree({ session: selectedNode.id, label: nodeLabel(selectedNode), ids: subtreeIds(state.nodes, selectedNode.id) }))}
          onClear={() => {
            setSelected(null)
            if (subtree === selectedNode.id) onSubtree(null)
          }}
        />
      )}
      {edge && edgeAt && <EdgePopover edge={edge} nodes={state.nodes} at={edgeAt.at} now={now} onClose={() => setEdgeAt(null)} />}
    </div>
  )

  // Review U2: one element tree for inline and full screen (only the wrapper's class and the header change), so the
  // canvas keeps its viewport, LOD and open folds across Shift+G and Esc.
  const shown = open || full
  return (
    <div
      ref={regionRef}
      // Full screen sits under the phone's status bar and home indicator: pad by the safe area so the back button is reachable.
      className={full ? 'fixed inset-0 z-40 flex flex-col bg-background pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]' : 'relative flex shrink-0 flex-col border-b border-border-subtle'}
      style={!full && open ? { height } : undefined}
      role="region"
      aria-label={full ? 'Agent graph, full screen' : 'Agent graph'}
    >
      {full ? (
        <div key="head-full" className="flex h-11 shrink-0 items-center gap-2 border-b border-border-subtle px-2 text-xs">
          <Button size="icon-xs" variant="ghost" aria-label="Leave full screen" title="Back (Esc)" onClick={() => setFull(false)}>
            <ChevronLeft />
          </Button>
          <span className="text-sm font-semibold">Agents</span>
          <span className={cn('min-w-0 truncate text-fg-secondary tabular-nums', counts.needYou > 0 && 'text-danger')}>{stripText(counts)}</span>
          <div className="ml-auto flex items-center gap-0.5">{controls}</div>
        </div>
      ) : (
        <Strip key="head-strip" counts={counts} older={state?.older ?? null} stale={isStale(state !== null, error)} subtree={subtreeLabel} expanded={open} onToggle={toggle} onClearSubtree={() => onSubtree(null)}>
          {open && controls}
        </Strip>
      )}
      {shown && canvas && <Fragment key="canvas">{canvas}</Fragment>}
      {open && !full && (
        <ResizeHandle
          key="resize"
          orientation="horizontal"
          placement="center"
          className="absolute right-0 bottom-0 left-0 z-10"
          aria-label="Resize the agent graph"
          value={{ now: height, min: MIN_H, max: Math.round(maxH()) }}
          onStart={() => {
            top.current = regionRef.current?.getBoundingClientRect().top ?? 0
          }}
          onMove={(_x, y) => setHeight(y - top.current)}
          onNudge={(d) => setHeight(height + d * 16)}
          onReset={() => prefs.set({ height: null })}
        />
      )}
    </div>
  )
}
