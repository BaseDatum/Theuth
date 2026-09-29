// Sidebar row → chat pane drag gesture. Pure: pointer facts in, one verb out; `useSessionDrag` (Panes.tsx) owns the
// timer, pointer capture and the panes store. Decided per event, never by media query: a mouse, pen or trackpad
// press drags after `SLOP` px of travel on every device; a finger must first hold still for `HOLD_MS` — travel
// before that is the list scrolling, a quick lift is the tap that navigates. In the phone Sheet (`sheet`) fingers
// never drag: phones do not tile.

/** Pointer travel (px) before a mouse press becomes a drag, or a finger press becomes a scroll. */
export const SLOP = 6
/** A finger held still this long starts a drag. */
export const HOLD_MS = 400

export type Drag =
  | { phase: 'idle' }
  | { phase: 'pressed'; x: number; y: number; touch: boolean }
  /** `moved`: the pointer travelled since the drag started; lifting an unmoved finger cancels quietly. */
  | { phase: 'dragging'; moved: boolean }

export type Act =
  | { type: 'down'; x: number; y: number; pointerType: string; button: number; modifier: boolean; sheet: boolean }
  | { type: 'move'; x: number; y: number }
  /** `HOLD_MS` elapsed since a finger's `down`. */
  | { type: 'timer' }
  | { type: 'up' }
  /** `pointercancel`: iOS took the sequence for a scroll, or the pointer left. */
  | { type: 'cancel' }

/** `hold`: arm the timer; `start`: begin the drag at the press point; `move`/`drop`/`cancel`: forward to the panes store. */
export type Verb = 'hold' | 'start' | 'move' | 'drop' | 'cancel' | null

export const IDLE: Drag = { phase: 'idle' }

export function step(d: Drag, a: Act): [Drag, Verb] {
  switch (d.phase) {
    case 'idle': {
      if (a.type !== 'down' || a.button !== 0 || a.modifier) return [d, null]
      const touch = a.pointerType === 'touch'
      if (touch && a.sheet) return [d, null]
      return [{ phase: 'pressed', x: a.x, y: a.y, touch }, touch ? 'hold' : null]
    }
    case 'pressed':
      switch (a.type) {
        case 'move':
          if (Math.hypot(a.x - d.x, a.y - d.y) < SLOP) return [d, null]
          return d.touch ? [IDLE, null] : [{ phase: 'dragging', moved: true }, 'start']
        case 'timer':
          return d.touch ? [{ phase: 'dragging', moved: false }, 'start'] : [d, null]
        case 'up':
        case 'cancel':
          return [IDLE, null]
        default:
          return [d, null]
      }
    case 'dragging':
      switch (a.type) {
        case 'move':
          return [{ phase: 'dragging', moved: true }, 'move']
        case 'up':
          return [IDLE, d.moved ? 'drop' : 'cancel']
        case 'cancel':
          return [IDLE, 'cancel']
        default:
          return [d, null]
      }
  }
}
