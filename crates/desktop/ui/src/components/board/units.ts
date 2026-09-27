// Design 71 §2.4 / M14 / M4 / M6 — diagram units are objects, not pictures. A `Graph` node or a `Sequence` step with a
// `ref` is an anchored element like a prose span: it joins the registry, lights when the active anchor overlaps its
// own (`highlight` on the view), pins on click / Enter and is a tour stop. Pure derivations first (units, highlight,
// stops for one diagram and for the whole board), then the hook that wires one in-flow diagram. No JSX here; the
// views and the portal live in `Tour.tsx`.
import { useEffect, useMemo, useRef } from 'react'
import { useStore } from 'zustand'
import { type Anchor, parseAnchor } from '@/components/chat/surface/anchor'
import { HOVER_MS, type TourStop, activeOf, contains, overlaps, useAnchorStore } from './anchors'
import { type Diagram, type DocBlock, type Inline, classifyCode, codeAnchor, excerptAnchor, plainText } from './blocks'

/** One addressable unit: a node (`id` = node id) or a step (`id` = its index as a string). */
export type Unit = { id: string; label: string; detail: string | null; anchor: Anchor | null }

/** Every unit in authored order (Graph: `nodes`; Sequence: `steps`); a `ref` that fails the grammar counts as none. */
export function unitsOf(d: Diagram): Unit[] {
  const ref = (r: string | null | undefined): Anchor | null => {
    if (!r) return null
    const a = parseAnchor(r)
    return typeof a === 'string' ? null : a
  }
  if (d.type === 'Graph') return d.props.nodes.map((n) => ({ id: n.id, label: n.label, detail: null, anchor: ref(n.ref) }))
  const actors: Record<string, string> = {}
  for (const a of d.props.actors) actors[a.id] = a.label
  return d.props.steps.map((s, i) => ({ id: String(i), label: s.label, detail: `${actors[s.from] ?? s.from} → ${actors[s.to] ?? s.to}`, anchor: ref(s.ref) }))
}

/** Ids of the units whose anchor overlaps `active` — what the view's `highlight` recolours (M4). */
export function highlightOf(units: readonly Unit[], active: Anchor | null): string[] {
  if (!active) return []
  return units.filter((u) => u.anchor && overlaps(u.anchor, active)).map((u) => u.id)
}

/** M6: one stop per unit in authored order; a unit without a `ref` is an "explanation" stop (R §B.7). */
export function diagramStops(d: Diagram, blockId: string): TourStop[] {
  return unitsOf(d).map((u) => ({ anchor: u.anchor, kind: 'unit', label: u.label, detail: u.detail, stage: { blockId, unit: u.id } }))
}

/**
 * §2.6 "Tour this board": every anchored element in document order — prose spans, diagram units, excerpts. The
 * stage of a span or an excerpt is its section's first diagram (a `##` opens a section) if there is one, else the
 * excerpt that covers it (an excerpt stop stands on itself).
 */
export function boardStops(blocks: readonly DocBlock[]): TourStop[] {
  type Section = { diagram: string | null; excerpts: { blockId: string; anchor: Anchor }[]; stops: (TourStop & { at: 'span' | 'excerpt' | 'unit' })[] }
  const sections: Section[] = [{ diagram: null, excerpts: [], stops: [] }]
  const spans = (content: DocBlock['content'], claim: string, into: Section) => {
    if (!Array.isArray(content)) return
    for (const c of content as Inline[]) {
      if (c.type !== 'link') continue
      const anchor = codeAnchor(c.href)
      if (anchor) into.stops.push({ at: 'span', anchor, kind: 'span', label: c.content.map((t) => t.text).join(''), detail: claim, stage: null })
    }
  }
  const walk = (list: readonly DocBlock[]) => {
    for (const b of list) {
      if (b.type === 'heading' && b.props.level <= 2) sections.push({ diagram: null, excerpts: [], stops: [] })
      const s = sections[sections.length - 1]!
      if (b.type === 'codeBlock') {
        const c = classifyCode(b)
        if (c.kind === 'diagram') {
          s.diagram ??= b.id
          for (const st of diagramStops(c.diagram, b.id)) s.stops.push({ ...st, at: 'unit' })
        } else if (c.kind === 'excerpt') {
          const anchor = excerptAnchor(c.props)
          s.excerpts.push({ blockId: b.id, anchor })
          s.stops.push({ at: 'excerpt', anchor, kind: 'excerpt', label: c.props.caption ?? `${c.props.path}:${anchor.lines[0]}-${anchor.lines[1]}`, detail: null, stage: { blockId: b.id, unit: null } })
        }
      } else if (b.type !== 'frontmatter') {
        spans(b.content, plainText(b.content), s)
      }
      if (b.children.length > 0) walk(b.children as DocBlock[])
    }
  }
  walk(blocks)
  const out: TourStop[] = []
  for (const s of sections) {
    for (const { at, ...stop } of s.stops) {
      if (at === 'span') {
        const covering = stop.anchor && s.excerpts.find((e) => contains(e.anchor, stop.anchor!))
        stop.stage = s.diagram ? { blockId: s.diagram, unit: null } : covering ? { blockId: covering.blockId, unit: null } : null
      } else if (at === 'excerpt' && s.diagram) {
        stop.stage = { blockId: s.diagram, unit: null }
      }
      out.push(stop)
    }
  }
  return out
}

/** The registry id (and `pinned.by` / `tour.by`) of a unit: `unit:<blockId>:<unitId>`. */
export function unitBy(blockId: string, unitId: string): string {
  return `unit:${blockId}:${unitId}`
}

/** Focus the element `by` names: a `data-anchor-id` (spans, excerpts, buttons) or a unit's `<g data-node|data-step>`. */
export function focusBy(root: ParentNode | null, by: string): void {
  if (!root || !by) return
  let el = root.querySelector<HTMLElement>(`[data-anchor-id="${CSS.escape(by)}"]`)
  if (!el && by.startsWith('unit:')) {
    const sep = by.indexOf(':', 'unit:'.length)
    if (sep > 0) {
      const block = CSS.escape(by.slice('unit:'.length, sep))
      const unit = CSS.escape(by.slice(sep + 1))
      el = root.querySelector<HTMLElement>(`[data-block="${block}"] :is([data-node="${unit}"], [data-step="${unit}"])`)
    }
  }
  el?.focus()
}

/** The unit element under an event target, if any. */
function unitOf(target: EventTarget | null): string | null {
  const g = (target as Element | null)?.closest?.<HTMLElement>('[data-node], [data-step]')
  return g ? (g.dataset.node ?? g.dataset.step ?? null) : null
}

/**
 * A press on a unit, delegated from an ancestor. `DiagramFrame`'s pan/zoom surface takes pointer capture on
 * `pointerdown`, so Chromium retargets the `click` to the surface and a unit's own `onClick` never fires; the wrapper
 * remembers the unit under `pointerdown` and treats a `pointerup` that has not moved past `slop` as a press.
 */
export function useUnitPress(onPress: (id: string) => void, slop = 6) {
  const pressed = useRef<{ id: string; x: number; y: number } | null>(null)
  return {
    onPointerDownCapture: (e: React.PointerEvent) => {
      const id = e.button === 0 ? unitOf(e.target) : null
      pressed.current = id !== null ? { id, x: e.clientX, y: e.clientY } : null
    },
    onPointerUp: (e: React.PointerEvent) => {
      const p = pressed.current
      pressed.current = null
      if (p && Math.hypot(e.clientX - p.x, e.clientY - p.y) <= slop) onPress(p.id)
    },
  }
}

/**
 * Wire one in-flow diagram: its anchored units join the registry, `highlight` follows the active anchor, click /
 * Enter pin (claim = the unit's label), hover (fine pointer, 120 ms) and focus set `hover`, ⌘↵ opens the tour at
 * that unit. Pointer / focus / key events are delegated from the wrapper so `Graph.tsx` / `Sequence.tsx` stay plain.
 */
export function useUnits(diagram: Diagram, blockId: string, coarse: boolean) {
  const store = useAnchorStore()
  const units = useMemo(() => unitsOf(diagram), [diagram])
  const stops = useMemo(() => diagramStops(diagram, blockId), [diagram, blockId])
  const anchored = useMemo(() => units.filter((u) => u.anchor), [units])
  useEffect(() => {
    const offs = anchored.map((u) => store.getState().register(unitBy(blockId, u.id), { anchor: u.anchor!, kind: 'unit', blockId }))
    return () => offs.forEach((off) => off())
  }, [store, anchored, blockId])
  // `activeOf` hands back the stored anchor object itself, so the selector is reference-stable between changes.
  const active = useStore(store, activeOf)
  const highlight = useMemo(() => highlightOf(units, active), [units, active])
  const timer = useRef<number | undefined>(undefined)
  /** The unit under the pointer; `mouseover` fires per child element, and moving within one unit must not restart the 120 ms. */
  const over = useRef<string | null>(null)
  const byId = (id: string | null) => units.find((u) => u.id === id) ?? null
  const clear = () => {
    clearTimeout(timer.current)
    over.current = null
    const h = store.getState().hover
    if (h && anchored.some((u) => overlaps(u.anchor!, h))) store.getState().setHover(null)
  }
  const openTour = (at: string | null, by: string) => store.getState().openTour(stops, Math.max(0, units.findIndex((u) => u.id === at)), by, blockId)
  const onSelect = (id: string | number) => {
    const u = byId(String(id))
    if (u?.anchor) store.getState().pin(u.anchor, unitBy(blockId, u.id), u.detail ? `${u.detail}: ${u.label}` : u.label)
  }
  const wrapper = {
    ...useUnitPress(onSelect),
    onMouseOver: coarse
      ? undefined
      : (e: React.MouseEvent) => {
          const id = unitOf(e.target)
          if (id === over.current) return
          clearTimeout(timer.current)
          over.current = id
          const u = byId(id)
          if (u?.anchor) timer.current = window.setTimeout(() => store.getState().setHover(u.anchor), HOVER_MS)
          else clear()
        },
    onMouseLeave: coarse ? undefined : clear,
    onFocus: (e: React.FocusEvent) => {
      const u = byId(unitOf(e.target))
      if (u?.anchor) store.getState().setHover(u.anchor)
    },
    onBlur: clear,
    // Capture phase: ⌘↵ must not also reach the unit's own Enter (which would pin under the tour).
    onKeyDownCapture: (e: React.KeyboardEvent) => {
      const at = unitOf(e.target)
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && at !== null) {
        e.preventDefault()
        e.stopPropagation()
        openTour(at, unitBy(blockId, at))
      } else if ((e.key === 't' || e.key === 'T') && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault()
        e.stopPropagation()
        openTour(at, at !== null ? unitBy(blockId, at) : `tour:${blockId}`)
      }
    },
  }
  return { units, stops, highlight, onSelect, openTour, wrapper }
}
