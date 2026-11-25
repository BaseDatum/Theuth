// Agent layout control listener (Pattern A, alongside `lib/page-preview.ts` and `lib/fence-snapshot.ts`):
// subscribes to `artifact.layout-command-requested@1` and `artifact.layout-command-resolved@1`, executes the requested
// `LayoutOp` against `lib/panes.ts`, and replies via `answer_layout_command`.

import { applyLayoutOp, type LayoutCommandAnswer, type LayoutOp } from '@/lib/panes'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

export type LayoutCommandRequestedEvent = {
  type: 'LayoutCommandRequested'
  id: string
  session: string
  op: LayoutOp
}

export type LayoutCommandResolvedEvent = {
  type: 'LayoutCommandResolved'
  id: string
}

type RawOn = (topic: string, type: string, cb: (event: unknown) => void) => () => void
type LayoutApi = {
  answer_layout_command?: (id: string, answer: LayoutCommandAnswer, session?: string | null) => Promise<boolean>
}

export const LAYOUT_REQUESTED_TOPIC = 'artifact.layout-command-requested@1'
export const LAYOUT_RESOLVED_TOPIC = 'artifact.layout-command-resolved@1'

/**
 * Wire the two layout command events in the main window (never a popout, island, or overlay).
 * Returns the unsubscribe cleanup function.
 */
export function installLayoutAgent(
  navigate: (href: string) => void,
  getActiveRoute?: () => string,
): () => void {
  const handled = new Set<string>()
  const subscribe = on as unknown as RawOn

  const answer = (id: string, session: string, reply: LayoutCommandAnswer) => {
    const api = useConnection.getState().api as unknown as LayoutApi | null
    void api?.answer_layout_command?.(id, reply, session)?.catch(() => {})
  }

  const offs = [
    subscribe(LAYOUT_REQUESTED_TOPIC, 'LayoutCommandRequested', (raw) => {
      const e = raw as LayoutCommandRequestedEvent
      if (!e?.id || handled.has(e.id)) return
      if (!useConnection.getState().api) return
      handled.add(e.id)
      const activeRoute =
        getActiveRoute?.() ??
        (typeof window !== 'undefined' ? `${window.location.pathname}${window.location.search}` : '/')
      let reply: LayoutCommandAnswer
      try {
        reply = applyLayoutOp(e.op, e.session, navigate, activeRoute)
      } catch (err) {
        reply = { kind: 'failed', reason: err instanceof Error ? err.message : String(err) }
      }
      answer(e.id, e.session, reply)
    }),
    subscribe(LAYOUT_RESOLVED_TOPIC, 'LayoutCommandResolved', (raw) => {
      const e = raw as LayoutCommandResolvedEvent
      if (e?.id) handled.delete(e.id)
    }),
  ]

  return () => {
    for (const off of offs) off()
    handled.clear()
  }
}
