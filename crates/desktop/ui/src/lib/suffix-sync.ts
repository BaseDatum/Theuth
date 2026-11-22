// #3: connects `lib/suffixes` to the engine when it keeps prompt suffixes (mesh-wide), and keeps open views fresh:
// the `agent.suffixes` topic (a change here or on another machine), a reconnect reload, and coming back to the
// foreground (iOS suspends the socket; a change made meanwhile has no topic frame to replay).

import { useEffect } from 'react'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'
import { onTopic } from '@/store/sessions'
import { SUFFIX_METHODS, type SuffixEngine, useSuffixes } from '@/lib/suffixes'

export function useSuffixSync(): void {
  const api = useConnection((s) => s.api)
  const supported = useQuerySupport(SUFFIX_METHODS)
  useEffect(() => {
    const engine: SuffixEngine | null = api && supported ? api : null
    void useSuffixes.getState().connect(engine)
  }, [api, supported])
  useEffect(() => {
    const refresh = () => void useSuffixes.getState().refresh()
    const visible = () => document.visibilityState === 'visible' && refresh()
    window.addEventListener('theuth:reload-session', refresh)
    document.addEventListener('visibilitychange', visible)
    const off = onTopic('agent.suffixes@1', refresh)
    return () => {
      off()
      window.removeEventListener('theuth:reload-session', refresh)
      document.removeEventListener('visibilitychange', visible)
    }
  }, [])
}
