// 76 §0.12 (amends 54 §0.8): the composer's microphone is one gesture control. Tap → dictation; hold ≥ 350 ms →
// record a voice note while held; slide up while holding → lock hands-free; slide down or release outside →
// cancel. Pure: pointer facts in, one verb out; `MicButton` owns the timer and the DOM.

export const HOLD_MS = 350
/** Upward travel that locks hands-free. */
export const LOCK_PX = 56
/** Downward travel that cancels on release (and shows the cancel hint filling). */
export const CANCEL_PX = 56
/** A press that wanders more than this before release is not a tap. */
const TAP_SLOP = 10

export type Hold =
  | { phase: 'idle' }
  | { phase: 'pressed'; x: number; y: number }
  /** Recording while the finger is down; `dy` > 0 = moved up (towards lock), < 0 = down (towards cancel). */
  | { phase: 'held'; x: number; y: number; dy: number }
  /** Recording hands-free; the control is now stop-and-send. */
  | { phase: 'locked' }

export type Act =
  | { type: 'down'; x: number; y: number }
  | { type: 'move'; x: number; y: number }
  /** `inside`: the pointer was released over the control. */
  | { type: 'up'; x: number; y: number; inside: boolean }
  /** `HOLD_MS` elapsed since `down`. */
  | { type: 'timer' }
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
      if (a.type === 'timer') return [{ phase: 'held', x: h.x, y: h.y, dy: 0 }, 'record']
      if (a.type === 'up') return [IDLE, Math.hypot(a.x - h.x, a.y - h.y) <= TAP_SLOP ? 'tap' : null]
      return [h, null]
    case 'held': {
      if (a.type === 'move') {
        const dy = h.y - a.y
        return dy >= LOCK_PX ? [{ phase: 'locked' }, 'lock'] : [{ ...h, dy }, null]
      }
      if (a.type === 'up') return [IDLE, !a.inside || h.y - a.y <= -CANCEL_PX ? 'cancel' : 'send']
      return [h, null]
    }
    case 'locked':
      // A tap on the locked control is `MicButton`'s click → `send`; the reducer only leaves via `reset`.
      return [h, null]
  }
}
