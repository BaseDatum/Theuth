import type { ReactNode } from 'react'
import type { EngineProvider } from '@/lib/doc/provider'
import type { Box, Id, InkColour, InkTool, Region } from './model'
import type { WbStore } from './store'

export type View = { x: number; y: number; zoom: number }           // screen = world × zoom + (x, y), canvas-element-relative
export type PointDescription = { selector: string; role: string; label: string; rect: Box }
export type CanvasHost = {
  doc: string
  /** An object URL for a stored blob (cached and revoked by B4), or null. */
  blobUrl(key: string): Promise<string | null>
  blobPut(key: string, blob: Blob): Promise<void>
  /** Native capture of a rectangle of the window in CSS px; null where there is no native capture (web, failure). */
  capture(rect: { x: number; y: number; width: number; height: number }, maxSide: number): Promise<Blob | null>
  /** `theuth/describe-point` on a live page frame, already sanitised; null on timeout (300 ms) or refusal. */
  describePoint(frame: HTMLIFrameElement, x: number, y: number): Promise<PointDescription | null>
  /** 4 on iPad and phone, 8 on desktop (design §3.6). */
  liveBudget: number
}
export type CapturedStroke = { pts: [x: number, y: number, pressure: number, t: number][]; tool: InkTool; colour: InkColour; width: number; hasPressure: boolean; at: number } // world coords, t in ms from the first point
export type PlannedMark = { id: Id; strokes: Id[]; card?: Id; ver?: number; bb: Box; region?: Region }
export type PlannedImage = { card: Id | null; marks: Id[]; rect: DOMRect | null /* window rect when fully on screen */; world: Box; zoomOf?: Id /* a second, zoomed image of this mark */ }
export type SendPlan = { marks: PlannedMark[]; images: PlannedImage[]; offScreen: number }

export type WhiteboardCanvasProps = {
  provider: EngineProvider
  store: WbStore
  host: CanvasHost
  readOnly?: boolean          // phone canvas, viewers: no ink, no edits
  embedded?: boolean          // inside a tile: the canvas keys only while the focus is inside it
  className?: string
  /** Things B4 pins to a card (the reply bubble): rendered at the card's right-top, flipped when clipped; `card: null` is not rendered. */
  anchored?: { key: string; card: Id | null; node: ReactNode }[]
  onCanvasTap?(e: { pointerType: 'touch' | 'mouse' }): void    // a finger or mouse tap on empty canvas — never a pen, never during the palm window
  onPenActivity?(down: boolean): void
  onViewChange?(view: View, phase: 'move' | 'end'): void       // 'end' also once after mount
  onCardOpen?(card: Id): void                                    // the card's Open button
}
export type WhiteboardCanvasHandle = {
  fit(target?: 'all' | 'selection' | { ids: Id[] }): void
  focus(id: Id, opts?: { spotlight?: boolean; animate?: boolean }): void
  view(): View
  setView(v: View, opts?: { animate?: boolean }): void
  /** The canvas element's rect in the window (to turn View into window coordinates). */
  frameRect(): DOMRect
  screenRectOf(id: Id): DOMRect | null
  visible(): { frames: Id[]; cards: Id[] }                       // frames fully in view, then loose cards in view
  freeSpot(w: number, h: number): { x: number; y: number }       // near the view centre, on no card and no ink
  undo(): void; redo(): void; canUndo(): boolean; canRedo(): boolean
  inkStats(): { fresh: number; freshInView: number; inkedCards: Id[] }
  /** Cluster the fresh strokes in view (plus `offScreen` ones on request) into marks; nothing is written. */
  planSend(opts?: { offScreen?: boolean }): SendPlan
  /** Fallback pixels when `host.capture` is null: stored snapshot (or a labelled box) with vector ink, long side ≤ maxSide. */
  composeImage(image: PlannedImage, maxSide: number): Promise<Blob>
  /** One `wb-local` transaction: every `sent` mark → `answered`, then the plan's marks as `sent`. */
  commitSend(plan: SendPlan, extra: { to: string | null; hw?: Record<Id, string>; blobs?: Id[] }): void
}
