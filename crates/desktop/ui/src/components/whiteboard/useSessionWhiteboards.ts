import { useQuery } from '@tanstack/react-query'
import { useMemo } from 'react'
import { whiteboardCall } from '@/components/chat/WhiteboardViewCard'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { useSessionField } from '@/store/sessions'
import { requirements } from './requirements'
import { useWhiteboards } from './useWhiteboards'
import { wbRpc } from './wire'

export type SessionWhiteboard = { doc: string; title: string }

/**
 * The whiteboards of chat `session`, for the bar above its composer: the ones its transcript created or showed
 * (`wb_create`, `show_whiteboard`, in order of first appearance, under their latest title), then the ones the engine
 * has it bound to (`session_whiteboards`: a `wb_*` write or the owner binding it) that the transcript never named,
 * titled from the whiteboard list. Empty where the engine has no whiteboards.
 */
export function useSessionWhiteboards(session: string): SessionWhiteboard[] {
  const discovery = useConnection((s) => s.discovery)
  const client = useConnection((s) => s.client)
  const ok = supports(discovery, requirements)
  const entries = useSessionField(session, 'entries')
  const named = useMemo(() => {
    const m = new Map<string, string>()
    for (const e of entries ?? []) {
      if (e.kind !== 'tool' || (e.name !== 'wb_create' && e.name !== 'show_whiteboard')) continue
      const c = whiteboardCall(e)
      if (c.doc) m.set(c.doc, c.title || m.get(c.doc) || '')
    }
    return m
  }, [entries])
  // Refetched when the transcript names another whiteboard (a create or show also binds).
  const bound = useQuery({
    enabled: ok && !!client,
    queryKey: ['whiteboard', 'of-session', session, named.size],
    queryFn: () => wbRpc(client!).ofSession(session),
    staleTime: 10_000,
  })
  const extra = useMemo(() => (bound.data ?? []).filter((d) => !named.has(d)), [bound.data, named])
  const list = useWhiteboards(500, ok && extra.length > 0)
  return useMemo(() => {
    if (!ok) return []
    const titled = new Map((list.data ?? []).map((w) => [w.id, w.title]))
    return [
      ...[...named].map(([doc, title]) => ({ doc, title: title || titled.get(doc) || 'Whiteboard' })),
      ...extra.map((doc) => ({ doc, title: titled.get(doc) || 'Whiteboard' })),
    ]
  }, [ok, named, extra, list.data])
}
