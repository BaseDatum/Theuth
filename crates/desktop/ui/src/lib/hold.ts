// 89 VOICE-HOLD (amends 76 §0.12): the composer's microphone is one gesture control. Tap → dictation; hold ≥ 200 ms →
// record a voice note while held; slide up ≥ `LOCK_PX` → lock hands-free (on the move); slide left ≥ `CANCEL_PX` →
// cancel (on the move, no release needed); release anywhere while held → send. The first axis to travel `AXIS_PX`
// wins and the other is ignored for the rest of the hold, so a diagonal cannot both lock and cancel.
// An interruption while held (`pointercancel`, window blur, the page hidden) locks: the recording goes on hands-free
// and the user decides afterwards. Pure: pointer facts in, one verb out; `MicButton` owns the timer and the DOM.

/** Press → recording. */
export const HOLD_MS = 200
/** Upward travel that locks hands-free. */
export const LOCK_PX = 72
/** Leftward travel that cancels. */
export const CANCEL_PX = 96
/** Travel before the gesture commits to one axis. */
export const AXIS_PX = 12
/** A press that wanders more than this before release is not a tap. */
export const TAP_SLOP = 10
/** A note released sooner than this (from `rec.start()`) is a slip, not a message: the Composer discards it. */
export const MIN_NOTE_MS = 800

export type Axis = 'none' | 'x' | 'y'

export type Hold =
  | { phase: 'idle' }
  | { phase: 'pressed'; x: number; y: number }
  /** Recording while the finger is down; `dx` < 0 = moved left (towards cancel), `dy` > 0 = moved up (towards lock). */
  | { phase: 'held'; x: number; y: number; axis: Axis; dx: number; dy: number }
  /** Recording hands-free; the locked bar's Send / Delete end it. */
  | { phase: 'locked' }

export type Act =
  | { type: 'down'; x: number; y: number }
  | { type: 'move'; x: number; y: number }
  | { type: 'up'; x: number; y: number }
  /** `HOLD_MS` elapsed since `down`. */
  | { type: 'timer' }
  /** The pointer sequence broke off (`pointercancel`, blur, hidden): a held note locks, a press is dropped. */
  | { type: 'interrupt' }
  /** The recording ended (sent, cancelled, Escape, error): back to idle whatever the finger does. */
  | { type: 'reset' }

export type Verb = 'tap' | 'record' | 'lock' | 'send' | 'cancel' | null

export const IDLE: Hold = { phase: 'idle' }

export function step(h: Hold, a: Act): [Hold, Verb] {
  if (a.type === 'reset') return [IDLE, null]
  switch (h.phase) {
    case 'idle':
      return a.type === 'down' ? [{ phase: 'pressed', x: a.x, y: a.y }, null] : [h, null]
    case 'pressed':
      if (a.type === 'timer') return [{ phase: 'held', x: h.x, y: h.y, axis: 'none', dx: 0, dy: 0 }, 'record']
      if (a.type === 'up') return [IDLE, Math.hypot(a.x - h.x, a.y - h.y) <= TAP_SLOP ? 'tap' : null]
      if (a.type === 'interrupt') return [IDLE, null]
      return [h, null]
    case 'held': {
      if (a.type === 'move') {
        const dx = a.x - h.x
        const dy = h.y - a.y
        let axis = h.axis
        if (axis === 'none' && Math.max(Math.abs(dx), Math.abs(dy)) >= AXIS_PX) axis = Math.abs(dx) >= Math.abs(dy) ? 'x' : 'y'
        if (axis === 'y' && dy >= LOCK_PX) return [{ phase: 'locked' }, 'lock']
        if (axis === 'x' && -dx >= CANCEL_PX) return [IDLE, 'cancel']
        return [{ ...h, axis, dx: axis === 'y' ? 0 : dx, dy: axis === 'x' ? 0 : dy }, null]
      }
      if (a.type === 'up') return [IDLE, 'send']
      if (a.type === 'interrupt') return [{ phase: 'locked' }, 'lock']
      return [h, null]
    }
    case 'locked':
      // Send / Delete are the locked bar's buttons → `onStop`; the reducer only leaves via `reset`.
      return [h, null]
  }
}
