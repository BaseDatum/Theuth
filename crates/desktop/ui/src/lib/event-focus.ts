/**
 * 129 EVENT-FOCUS §3.4 / D3 / D4, the pure half: which sessions this window renders live text for, what to
 * tell the engine when that changes, how often a `Lagged` may resync, and the per-drain type histogram.
 * `App.tsx` wires these to the router, the panes, the connection and the event queue.
 */
import { isDraft } from '@/lib/panes'

/** The window's live-text sessions: the chat or talk route, every pane leaf, a popout's session and the island's desk. */
export function deriveFocus(input: { pathname: string; leaves: string[]; popout?: string | null; desk?: string | null }): string[] {
  const set = new Set<string>()
  const m = /^\/(?:c|talk)\/([^/]+)$/.exec(input.pathname)
  if (m?.[1]) set.add(decodeURIComponent(m[1]))
  for (const s of input.leaves) if (!isDraft(s)) set.add(s)
  if (input.popout) set.add(input.popout)
  if (input.desk) set.add(input.desk)
  return [...set].sort()
}

/** The last set sent on this connection; `reset()` after a reconnect (server state is per connection). */
export class FocusTracker {
  private sent: string[] | null = null
  reset(): void {
    this.sent = null
  }
  /** `send` when the set differs from what the connection has; `left` = sessions that dropped out (to mark stale). */
  update(next: string[]): { send: boolean; left: string[] } {
    const prev = this.sent
    if (prev !== null && prev.length === next.length && prev.every((s, i) => s === next[i])) return { send: false, left: [] }
    this.sent = next
    return { send: true, left: prev?.filter((s) => !next.includes(s)) ?? [] }
  }
}

/**
 * At most one `fn` per `everyMs`: the first call runs at once; later calls inside the window coalesce into one
 * trailing run when it ends (`trailing`), or are dropped (a toast has nothing to say late).
 */
export class Throttle {
  private last = Number.NEGATIVE_INFINITY
  private pending = false
  constructor(
    private readonly fn: () => void,
    private readonly everyMs: number,
    private readonly trailing: boolean,
  ) {}
  call(): void {
    const now = Date.now()
    const wait = this.last + this.everyMs - now
    if (wait <= 0) {
      this.last = now
      this.fn()
    } else if (this.trailing && !this.pending) {
      this.pending = true
      setTimeout(() => {
        if (!this.pending) return
        this.pending = false
        this.last = Date.now()
        this.fn()
      }, wait)
    }
  }
  /** Drop a trailing run (unmount). */
  cancel(): void {
    this.pending = false
  }
}

/** `{ TextDelta: 120, ToolCallFinished: 2 }` for one drained batch (D4). */
export function typeCounts(events: readonly { type: string }[]): Record<string, number> {
  const out: Record<string, number> = {}
  for (const e of events) out[e.type] = (out[e.type] ?? 0) + 1
  return out
}
