import { createFileRoute } from '@tanstack/react-router'
import { useCallback, useEffect } from 'react'
import { toast } from 'sonner'
import { Composer } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'
import { lastCode } from '@/components/chat/MessageAssistant'
import { Transcript } from '@/components/chat/Transcript'
import { useApi } from '@/lib/rpc'
import { useSessionField, useSessions } from '@/store/sessions'

export const Route = createFileRoute('/c/$session')({ component: SessionRoute })

const PAGE = 60

const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))

/** One chat: header, transcript, composer. Loads the newest page and the agent subtree on first visit; events keep both live. */
function SessionRoute() {
  const { session } = Route.useParams()
  const api = useApi()
  const title = useSessionField(session, 'title')
  const running = useSessionField(session, 'running')

  useEffect(() => {
    document.title = `${title ?? ''}${running ? ' ·' : ''}`
  }, [title, running])

  const load = useCallback(async () => {
    const [page, agents] = await Promise.all([api.load_session(session, null, PAGE), api.list_agents(session)])
    const { loadHistory, setChildren } = useSessions.getState()
    loadHistory(session, page, 'replace')
    setChildren(session, agents)
  }, [api, session])

  useEffect(() => {
    const store = useSessions.getState()
    store.ensure(session)
    if (!store.sessions[session]?.loaded) {
      load().catch(fail)
    } else {
      api.list_agents(session).then((a) => useSessions.getState().setChildren(session, a), fail)
    }
  }, [api, session, load])

  // Reconnect / `Lagged`: everything between is lost, so the visible session is re-fetched.
  useEffect(() => {
    const reload = () => void load().catch(fail)
    window.addEventListener('wth:reload-session', reload)
    return () => window.removeEventListener('wth:reload-session', reload)
  }, [load])

  useEffect(() => {
    const last = () => useSessions.getState().sessions[session]?.entries.findLast((e) => e.kind === 'assistant')
    const response = () => {
      const e = last()
      if (e) void navigator.clipboard.writeText(e.text)
    }
    const code = () => {
      const e = last()
      const block = e && lastCode(e.text)
      if (block) void navigator.clipboard.writeText(block)
    }
    window.addEventListener('wth:copy-last-response', response)
    window.addEventListener('wth:copy-last-code', code)
    return () => {
      window.removeEventListener('wth:copy-last-response', response)
      window.removeEventListener('wth:copy-last-code', code)
    }
  }, [session])

  return (
    <>
      <Header session={session} />
      <Transcript key={session} session={session} />
      <Composer key={session} session={session} />
    </>
  )
}
