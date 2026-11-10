// The composer's phases and what its two send-shaped gestures do in each. `press` is Enter / the Send or Steer
// button; `interrupt` is ⌘↩ or a long press on that button. Pure, so the transitions are testable without the DOM.

/** Idle; a turn running with nothing queued; a turn running with steers queued behind it. */
export type Phase = 'idle' | 'running' | 'queued'

export const phase = (running: boolean, queued: number): Phase => (!running ? 'idle' : queued > 0 ? 'queued' : 'running')

/** `send` starts a turn, `steer` queues into the running one; `null` when there is nothing to send. */
export function press(p: Phase, filled: boolean): 'send' | 'steer' | null {
  if (!filled) return null
  return p === 'idle' ? 'send' : 'steer'
}

/**
 * `interject`: cancel the turn, the box's text is the next input. `flush`: the box is empty but steers are queued —
 * cancel the turn and deliver them now instead of dropping them. Idle: same as a press. `null`: nothing to do.
 * 186 D8: `contact` (a contact's turn runs) — the owner's input never cancels it: the text queues behind it (`steer`)
 * and queued steers wait.
 */
export function interrupt(p: Phase, filled: boolean, contact = false): 'send' | 'steer' | 'interject' | 'flush' | null {
  if (p === 'idle') return filled ? 'send' : null
  if (contact) return filled ? 'steer' : null
  if (filled) return 'interject'
  return p === 'queued' ? 'flush' : null
}
