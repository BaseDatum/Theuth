// The Yjs ↔ React Flow binding (§8.4), xyflow-free so it is testable: frames are parent nodes, the document keeps
// absolute coordinates (§2.2, §12.11) and the binding converts to parent-relative for React Flow and back on drag end.
// Nothing here writes; `dragEnd` returns the writes the canvas commits (positions are written on drag end only).
import { ridersOf } from '../ink/anchor'
import { containsPoint } from './geom'
import type { Board, Card, Frame, Id, Link, Stroke } from './model'
import type { Move } from './ops'

/** Frames sit under every card; React Flow draws a child above its parent anyway. */
export const FRAME_Z = -1_000_000

export type CardNodeData = { card: Card }
export type FrameNodeData = { frame: Frame }
export type FlowNode =
  | { id: Id; type: 'card'; position: { x: number; y: number }; parentId?: Id; width: number; height: number; zIndex: number; data: CardNodeData; selected: boolean; draggable: boolean }
  | { id: Id; type: 'frame'; position: { x: number; y: number }; width: number; height: number; zIndex: number; data: FrameNodeData; selected: boolean; draggable: boolean }
export type FlowEdge = {
  id: Id; source: Id; target: Id; sourceHandle?: string; targetHandle?: string; type: 'link'; data: { link: Link }; selected: boolean
}

/** Cards that are drawn: not archived. A card whose frame is gone is loose. */
export function nodesOf(board: Board, o: { selection: readonly Id[]; readOnly?: boolean; entered?: Id | null }): FlowNode[] {
  const sel = new Set(o.selection)
  const frames = Object.values(board.frames).sort((a, b) => a.y - b.y || a.x - b.x || (a.id < b.id ? -1 : 1))
  const out: FlowNode[] = frames.map((f) => ({
    id: f.id, type: 'frame', position: { x: f.x, y: f.y }, width: f.w, height: f.h, zIndex: FRAME_Z,
    data: { frame: f }, selected: sel.has(f.id), draggable: !o.readOnly,
  }))
  const cards = Object.values(board.cards).filter((c) => c.state !== 'archived').sort((a, b) => a.z - b.z || (a.id < b.id ? -1 : 1))
  for (const c of cards) {
    const parent = c.frame ? board.frames[c.frame] : undefined
    out.push({
      id: c.id, type: 'card',
      position: parent ? { x: c.x - parent.x, y: c.y - parent.y } : { x: c.x, y: c.y },
      ...(parent ? { parentId: parent.id } : {}),
      width: c.w, height: c.h, zIndex: c.z, data: { card: c }, selected: sel.has(c.id),
      draggable: !o.readOnly && o.entered !== c.id,
    })
  }
  return out
}

/** Links whose both cards are drawn. */
export function edgesOf(board: Board, selection: readonly Id[]): FlowEdge[] {
  const sel = new Set(selection)
  const shown = (id: Id) => {
    const c = board.cards[id]
    return !!c && c.state !== 'archived'
  }
  return Object.values(board.links)
    .filter((l) => shown(l.from) && shown(l.to))
    .map((l) => ({
      id: l.id, source: l.from, target: l.to, type: 'link' as const, data: { link: l }, selected: sel.has(l.id),
      ...(l.from_port ? { sourceHandle: `s-${l.from_port}` } : {}),
      ...(l.to_port ? { targetHandle: `s-${l.to_port}` } : {}),
    }))
}

/** A React Flow position (parent-relative for a child) → absolute world. */
export function toAbsolute(board: Board, id: Id, pos: { x: number; y: number }): { x: number; y: number } {
  const card = board.cards[id]
  const parent = card?.frame ? board.frames[card.frame] : undefined
  return parent ? { x: pos.x + parent.x, y: pos.y + parent.y } : { x: pos.x, y: pos.y }
}

/** The frame (smallest by area) whose rect holds a point, ignoring `except`. */
export function frameAt(board: Board, x: number, y: number, except?: Id): Frame | null {
  let best: Frame | null = null
  for (const f of Object.values(board.frames)) {
    if (f.id === except) continue
    if (!containsPoint([f.x, f.y, f.w, f.h], x, y)) continue
    if (!best || f.w * f.h < best.w * best.h) best = f
  }
  return best
}

export type DragWrites = { cards: Move[]; frames: { id: Id; x: number; y: number }[]; riders: Stroke[] }

/**
 * The writes of a drag end. `dropped` = the dragged nodes with their React Flow positions (parent-relative for cards in
 * a frame). A frame carries its member cards; a card's membership follows its centre; free strokes touching exactly
 * one moved card ride with it.
 */
export function dragEnd(board: Board, dropped: readonly { id: Id; position: { x: number; y: number } }[]): DragWrites {
  const frames: DragWrites['frames'] = []
  const cards = new Map<Id, Move>()
  const movedFrames = new Set<Id>()
  for (const d of dropped) {
    const f = board.frames[d.id]
    if (!f) continue
    const dx = d.position.x - f.x
    const dy = d.position.y - f.y
    if (dx === 0 && dy === 0) continue
    movedFrames.add(f.id)
    frames.push({ id: f.id, x: d.position.x, y: d.position.y })
    for (const c of Object.values(board.cards)) {
      if (c.frame === f.id && c.state !== 'archived') cards.set(c.id, { id: c.id, x: c.x + dx, y: c.y + dy })
    }
  }
  for (const d of dropped) {
    const c = board.cards[d.id]
    if (!c || (c.frame && movedFrames.has(c.frame))) continue
    const abs = toAbsolute(board, c.id, d.position)
    const host = frameAt(board, abs.x + c.w / 2, abs.y + c.h / 2)
    const frame = host ? (host.id === c.frame ? undefined : host.id) : c.frame ? null : undefined
    if (abs.x === c.x && abs.y === c.y && frame === undefined) continue
    cards.set(c.id, { id: c.id, x: abs.x, y: abs.y, ...(frame !== undefined ? { frame } : {}) })
  }
  const riders: Stroke[] = []
  const seen = new Set<Id>()
  for (const id of cards.keys()) {
    for (const s of ridersOf(board, id)) {
      if (seen.has(s.id)) continue
      seen.add(s.id)
      riders.push(s)
    }
  }
  return { cards: [...cards.values()], frames, riders }
}
