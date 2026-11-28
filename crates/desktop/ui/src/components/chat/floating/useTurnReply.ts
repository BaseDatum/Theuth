// Shared reply bubble logic and turn-end watcher for floating chat docks (Whiteboard and Task Graph).
// After a turn of the target session ends (`running` true -> false), re-reads session history so the
// last assistant reply shows in a glass bubble that collapses to one line after 6 s.

import { useEffect, useRef, useState } from 'react'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { type Entry, useSessions } from '@/store/sessions'

export const COLLAPSE_MS = 6_000

/** The last assistant text of the turn that just ended (after the last user row), trimmed; null when there is none. */
export function lastReply(entries: readonly Entry[]): string | null {
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]
    if (!e) continue
    if (e.kind === 'user') return null
    if (e.kind === 'assistant' && e.text.trim()) return e.text.trim()
  }
  return null
}

/** One line for the collapsed bubble (markdown markers and newlines folded). */
export function oneLine(text: string, max = 140): string {
  const flat = text.replace(/[`*_#>]+/g, '').replace(/\s+/g, ' ').trim()
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat
}

export type BubbleTimer = { start(): void; expand(): void; dispose(): void }

/** Collapse after `ms` of being shown; `expand` (a tap) shows it whole and restarts the clock. */
export function bubbleTimer(onCollapse: (collapsed: boolean) => void, ms = COLLAPSE_MS): BubbleTimer {
  let t: ReturnType<typeof setTimeout> | null = null
  const stop = () => {
    if (t !== null) clearTimeout(t)
    t = null
  }
  const start = () => {
    stop()
    onCollapse(false)
    t = setTimeout(() => onCollapse(true), ms)
  }
  return { start, expand: start, dispose: stop }
}

/** Pure transition check for a session's running flag: ignores switching from a running session to an idle one. */
export function nextTurnTransition(
  prevRunning: boolean,
  prevSession: string | null,
  running: boolean,
  session: string | null,
): 'started' | 'ended' | 'noop' {
  if (running) return 'started'
  if (prevRunning && session !== null && prevSession === session) return 'ended'
  return 'noop'
}

/** Load the target session's history once so the reply bubble has its text. */
export function useLoadedSession(session: string | null) {
  const api = useApi()
  const can = useQuerySupport(['load_session'])
  useEffect(() => {
    if (!session || !can) return
    const st = useSessions.getState()
    st.ensure(session)
    if (st.sessions[session]?.loaded) return
    api.load_session(session, null, 60).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      () => {},
    )
  }, [api, session, can])
}

export type TurnReply = { text: string; session: string }

/**
 * Watches a floating dock's target session for `running` dropping `true -> false`: re-reads the session history,
 * fires `onTurnEnd`, and captures the latest assistant reply once it lands in the session store.
 */
export function useTurnReply({
  session,
  running,
  onTurnEnd,
  onReply,
}: {
  session: string | null
  running: boolean
  onTurnEnd?: (session: string) => void
  onReply?: (reply: TurnReply) => void
}) {
  const api = useApi()
  const [replyState, setReplyState] = useState<TurnReply | null>(null)
  const [awaiting, setAwaiting] = useState<string | null>(null)
  const prev = useRef<{ running: boolean; session: string | null }>({ running, session })
  const onTurnEndRef = useRef(onTurnEnd)
  onTurnEndRef.current = onTurnEnd
  const onReplyRef = useRef(onReply)
  onReplyRef.current = onReply

  useEffect(() => {
    const step = nextTurnTransition(prev.current.running, prev.current.session, running, session)
    prev.current = { running, session }
    if (step === 'started') return setAwaiting(null)
    if (step !== 'ended' || !session) return
    setAwaiting(session)
    onTurnEndRef.current?.(session)
    api.load_session(session, null, 60).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      () => {},
    )
  }, [running, session, api])

  // Subscribed only while awaiting: streaming deltas of a running turn do not re-render the host view.
  const latest = useSessions((s) => (awaiting ? lastReply(s.sessions[awaiting]?.entries ?? []) : null))
  useEffect(() => {
    if (!awaiting || !latest) return
    const next: TurnReply = { text: latest, session: awaiting }
    setAwaiting(null)
    setReplyState(next)
    onReplyRef.current?.(next)
  }, [awaiting, latest])

  return {
    reply: replyState,
    dismiss: () => setReplyState(null),
    setReply: setReplyState,
  }
}
