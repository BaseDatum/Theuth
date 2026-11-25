// 218: the hover preview, as Apple Freeform has it. While the Pencil hovers (WebKit on iPadOS: `pointerType: 'pen'`
// moves with no button down, no pointer-down) — or the mouse moves with an ink tool on the desktop — a dot at the tip
// shows where the mark lands, in the tool's colour at its drawn width; the eraser shows its circle. Nothing where this
// pointer would not ink (Hand, link, a finger). WebKit reports no hover height (`altitudeAngle` is the tilt), so the
// preview is constant rather than fading with distance.
import { inkCss } from '../ink/colour'
import { freehandOptions, MARKER_OPACITY } from '../ink/render'
import { type PointerKind, type RouteState, routePointer } from './input'
import type { InkColour, InkTool } from './model'

/** A Pencil hover that stops sending moves (lifted away, out of range) hides after this. */
export const HOVER_IDLE_MS = 200
/** The eraser's reach in screen px (diameter): what `eraseAt` takes, whatever the zoom. */
export const ERASER_PX = 12

export type HoverLook = { size: number; fill: string | null; ring: string | null; opacity: number }

/** What the tip shows for `kind` now, in screen px; null for nothing. */
export function hoverLook(kind: PointerKind, s: RouteState, pen: { tool: InkTool; colour: InkColour; width: number }, zoom: number, dark: boolean): HoverLook | null {
  const route = routePointer(kind, s)
  if (route === 'erase') return { size: ERASER_PX, fill: null, ring: dark ? '#fff' : '#000', opacity: 0.7 }
  if (route !== 'ink') return null
  const size = Math.max(3, (freehandOptions(pen.tool, pen.width, false).size ?? pen.width) * zoom)
  return { size, fill: inkCss(pen.colour, dark), ring: null, opacity: pen.tool === 'marker' ? MARKER_OPACITY : 0.85 }
}
