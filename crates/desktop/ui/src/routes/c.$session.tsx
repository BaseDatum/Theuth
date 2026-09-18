import { createFileRoute } from '@tanstack/react-router'
import { useCallback, useEffect, useMemo, useRef } from 'react'
import { toast } from 'sonner'
import { Composer } from '@/components/chat/Composer'
import { Header } from '@/components/chat/Header'
import { lastCode } from '@/components/chat/MessageAssistant'
import { SearchSession } from '@/components/chat/SearchSession'
import { TodoCard } from '@/components/chat/TodoCard'
import { Transcript } from '@/components/chat/Transcript'
import { SearchReturn } from '@/components/sidebar/SearchReturn'
import { useApi } from '@/lib/rpc'
import { type SearchDestination, isSearchDestination, useSearchOrigin } from '@/lib/search'
import { useScope } from '@/store/harness-scope'
import { freshness, on, useSessionField } from '@/store/sessions'

/** `seq`: a search hit's stored message seq (49 §7); anything but a safe non-negative integer is dropped. */
type Search = { seq?: number }

export const Route = createFileRoute('/c/$session')({
  validateSearch: (s: Record<string, unknown>): Search =>
    typeof s.seq === 'number' && Number.isSafeInteger(s.seq) && s.seq >= 0 ? { seq: s.seq } : {},
  component: SessionRoute,
})

const PAGE = 60
/** Settled reloads refused because an event raced the read are retried at most this often before waiting for the next notification. */
const SETTLE_TRIES = 3

const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))

/** `wth:reload-session` detail: `session` narrows it to one chat; `settled` asks for the mutation-refresh path (48 §5). A bare `Event` reloads in full. */
type ReloadDetail = { session?: string; settled?: boolean }

/** One chat. With a validated `seq` the route is the read-only Earlier messages inspection ending at that message (49 §7),
 * keyed so another target remounts it; otherwise the live chat. The live tree is not mounted under `seq`, so nothing
 * loads or appends to the canonical tail behind the preview. */
function SessionRoute() {
  const { session } = Route.useParams()
  const { seq } = Route.useSearch()
  if (seq !== undefined) return <SearchSession key={`${session}:${seq}`} session={session} seq={seq} />
  return <LiveSession session={session} />
}

/** Header, transcript, todo card, composer. Loads the newest page and the agent subtree on first visit; events keep both live. */
function LiveSession({ session }: { session: string }) {
  const api = useApi()
  // The harness this route mounted under: an RPC landing after a switch writes to its own stores, never the new harness's (48 §5).
  const { sessions: store } = useScope()
  const title = useSessionField(session, 'title')
  const running = useSessionField(session, 'running')
  // A title hit opened this chat (49 §7): offer the way back to the results; any other origin belongs to another destination.
  const origin = useSearchOrigin()
  const destination = useMemo<SearchDestination>(() => ({ category: 'sessions', session, seq: null }), [session])
  const fromSearch = isSearchDestination(origin, destination)

  useEffect(() => {
    document.title = `${title ?? ''}${running ? ' ·' : ''}`
  }, [title, running])

  // The plan (47 §5.4): one `get_todos` when the page carried none (an older harness, a refused read) or a gap
  // marked it stale. `setTodos` only moves forward, so a cached list is never wiped; a failed read leaves the
  // list labelled stale (the card says so) rather than silently reading as empty. Bound to this route's harness
  // and session, so a late reply after a switch still lands in the right slice.
  const refreshTodos = useCallback(() => {
    api.get_todos(session).then(
      (t) => store.getState().setTodos(session, t),
      (e) => {
        store.getState().markTodosStale(session)
        fail(e)
      },
    )
  }, [api, session, store])

  const load = useCallback(async () => {
    const before = store.getState().sessions[session]
    const fresh = before && freshness(before)
    const [page, agents, artifacts] = await Promise.all([
      api.load_session(session, null, PAGE),
      api.list_agents(session),
      // A harness without the artifact plugin has no such method.
      api.list_artifacts(session, null).catch(() => []),
    ])
    const { loadHistory, setChildren, setArtifacts } = store.getState()
    loadHistory(session, page, 'replace', fresh)
    setChildren(session, agents)
    setArtifacts(session, artifacts)
    const s = store.getState().sessions[session]
    if (!s?.todos || s.todosStale) refreshTodos()
  }, [api, session, store, refreshTodos])

  // The mutation refresh (48 §5): a settled-only page, installed only when nothing raced it. Calls
  // while one is in flight coalesce into one more pass. A refusal from the actor (busy) waits for its
  // handback's `ContextChanged`; a refusal from the store (an event landed meanwhile) retries while idle.
  const settling = useRef(false)
  const again = useRef(false)
  const settle = useCallback(() => {
    if (settling.current) {
      again.current = true
      return
    }
    settling.current = true
    void (async () => {
      for (let tries = 0; tries < SETTLE_TRIES; tries++) {
        const s = store.getState().sessions[session]
        // Nothing to reconcile, or a turn runs: its `TurnDone` publishes the `ContextChanged` that comes back here.
        if (!s?.contextStale || s.running) break
        const fresh = freshness(s)
        try {
          const page = await api.load_session(session, null, PAGE, true)
          if (store.getState().loadHistory(session, page, 'settled', fresh) || !page.settled || page.running) break
        } catch (e) {
          fail(e)
          break
        }
      }
      settling.current = false
      if (again.current) {
        again.current = false
        settle()
      }
    })()
  }, [api, session, store])

  useEffect(() => {
    const st = store.getState()
    st.ensure(session)
    const s = st.sessions[session]
    if (!s?.loaded) {
      load().catch(fail)
    } else {
      api.list_agents(session).then((a) => store.getState().setChildren(session, a), fail)
      // A `ContextChanged` reached this slice while another chat was shown.
      if (s.contextStale) settle()
      // The plan went stale (reconnect / `Lagged`) or never arrived while hidden: refresh before display (47 §5.4).
      if (!s.todos || s.todosStale) refreshTodos()
    }
  }, [api, session, load, settle, store, refreshTodos])

  // Reconnect / `Lagged` (bare event): everything between is lost, so the visible session is re-fetched in full.
  // A context edit (`ContextChanged`, or the picker's `settled` event) goes through the settled path instead.
  useEffect(() => {
    const reload = (ev: Event) => {
      const detail: ReloadDetail | undefined = ev instanceof CustomEvent ? ev.detail : undefined
      if (detail?.session !== undefined && detail.session !== session) return
      if (detail?.settled) settle()
      else void load().catch(fail)
    }
    window.addEventListener('wth:reload-session', reload)
    const off = on('ContextChanged', (e) => {
      if (e.session === session) settle()
    })
    return () => {
      window.removeEventListener('wth:reload-session', reload)
      off()
    }
  }, [load, settle, session])

  useEffect(() => {
    const last = () => store.getState().sessions[session]?.entries.findLast((e) => e.kind === 'assistant')
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
  }, [session, store])

  // Keyed so refs/drafts reset per session; the keys must differ between siblings.
  return (
    <>
      <Header session={session} />
      {fromSearch && (
        <div className="mx-auto flex w-full shrink-0 justify-end md:max-w-[768px] px-4">
          <SearchReturn />
        </div>
      )}
      <Transcript key={`t:${session}`} session={session} />
      <TodoCard key={`d:${session}`} session={session} />
      <Composer key={`c:${session}`} session={session} />
    </>
  )
}
