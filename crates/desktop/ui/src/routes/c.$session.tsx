import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { AdvisorPill } from '@/components/chat/AdvisorPill'
import { useAdvisor } from '@/components/chat/advisor'
import { Composer } from '@/components/chat/Composer'
import { messagingSearch, type MessagingSearch, useMessagingSheet, withoutMessaging } from '@/components/chat/messaging-sheet'
import { Header } from '@/components/chat/Header'
import { ObservedBar } from '@/components/chat/ObservedBar'
import { LinkBanner } from '@/components/link/LinkFailure'
import { lastCode } from '@/components/chat/MessageAssistant'
import { relistArtifacts } from '@/components/chat/ArtifactCard'
import { NewChatPane } from '@/components/chat/NewChatPane'
import { OrphansBanner } from '@/components/chat/OrphansBanner'
import { ResumeBanner } from '@/components/chat/ResumeBanner'
import { Panes } from '@/components/chat/Panes'
import { SearchSession } from '@/components/chat/SearchSession'
import { BackgroundArea } from '@/components/chat/BackgroundArea'
import { relistReminders, relistTerminals } from '@/components/chat/background'
import { Transcript } from '@/components/chat/Transcript'
import { SearchReturn } from '@/components/sidebar/SearchReturn'
import { chatName } from '@/lib/attach'
import { chatLeaves, closeWeb, isDraft, isTile, tileOf, usePanes, useRoutedPane } from '@/lib/panes'
import { isWeb, webId } from '@/lib/web-views'
import { WebFrame } from '@/components/chat/WebFrame'
import { WebPill } from '@/components/chat/WebPill'
import { TileLeaf } from '@/components/tiles/registry'
import { reportLoadError, useRemoteStart } from '@/lib/remote-start'
import { disconnectText, inTree, remaining, resumed, usePrivate } from '@/lib/private'
import { ownedElsewhere } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import { type SearchDestination, isSearchDestination, useSearchOrigin } from '@/lib/search'
import { useScope } from '@/store/engine-scope'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { freshness, on, useSessionField, useSessions } from '@/store/sessions'
import { type StampTries, stampTried, stampWanted, unstampedKey } from '@/store/reduce'
import { ContactAllowStrip } from '@/components/chat/ContactAllowStrip'
import { KernelStrip } from '@/components/cells/KernelStrip'

/** `seq`: a search hit's stored message seq (49 §7); anything but a safe non-negative integer is dropped.
 * `cell` (171 §14.3): `theuth://c/<session>?cell=<name|ordinal>` — the cell to scroll to and focus.
 * `messaging=1` (188): open this chat's Messaging… sheet; `peer` also focuses that pending peer request (a message id). */
type Search = { seq?: number; cell?: string } & MessagingSearch

export const Route = createFileRoute('/c/$session')({
  validateSearch: (s: Record<string, unknown>): Search => ({
    ...(typeof s.seq === 'number' && Number.isSafeInteger(s.seq) && s.seq >= 0 ? { seq: s.seq } : {}),
    ...((typeof s.cell === 'string' || typeof s.cell === 'number') && /^[a-z0-9-]{1,48}$/.test(String(s.cell)) ? { cell: String(s.cell) } : {}),
    ...messagingSearch(s),
  }),
  component: SessionRoute,
})

const PAGE = 60
/** cells-midturn D2: rows of the newest page read for a running turn's stored replies; a round is a reply and its tool rows. */
const STAMP_PAGE = 16
/** Settled reloads refused because an event raced the read are retried at most this often before waiting for the next notification. */
const SETTLE_TRIES = 3

/** `theuth:reload-session` detail: `session` narrows it to one chat; `settled` asks for the mutation-refresh path (48 §5). A bare `Event` reloads in full. */
type ReloadDetail = { session?: string; settled?: boolean }

/** One chat. With a validated `seq` the route is the read-only Earlier messages inspection ending at that message (49 §7),
 * keyed so another target remounts it; otherwise the live chat, tiled with any other panes the user split off. The live
 * tree is not mounted under `seq`, so nothing loads or appends to the canonical tail behind the preview. */
function SessionRoute() {
  const { session } = Route.useParams()
  const { seq, cell, messaging, peer } = Route.useSearch()
  const navigate = useNavigate()
  // The URL names the focused pane: a sidebar click lands in it, a pane press moves the URL to it.
  useRoutedPane(session)
  useEffect(() => (cell === undefined ? undefined : scrollToCell(cell)), [session, cell])
  // 188: open the chat's Messaging… sheet once, then drop the params so a reload does not reopen it.
  useEffect(() => {
    if (messaging !== '1') return
    useMessagingSheet.getState().show(session, peer)
    void navigate({ to: '/c/$session', params: { session }, search: withoutMessaging, replace: true })
  }, [session, messaging, peer, navigate])
  if (seq !== undefined) return <SearchSession key={`${session}:${seq}`} session={session} seq={seq} />
  // A draft leaf (a dropped New chat) has no session yet; it renders the `/` body until its first send.
  return (
    <Panes
      session={session}
      render={(id, first) =>
        isWeb(id) ? (
          <WebFrame id={webId(id)} first={first} onClose={() => closeWeb(webId(id))} />
        ) : isTile(id) ? (
          <TileLeaf id={tileOf(id)} first={first} />
        ) : isDraft(id) ? (
          <NewChatPane id={id} first={first} />
        ) : (
          <LiveSession session={id} first={first} />
        )
      }
    />
  )
}

/** 171 §14.3: scroll to and focus the newest cell named `cell` (or with that ordinal) once the transcript has mounted it; gives up after 10 s. */
function scrollToCell(cell: string): () => void {
  const find = () => {
    const all = [...document.querySelectorAll<HTMLElement>(/^\d+$/.test(cell) ? `[data-cell-ordinal="${cell}"]` : `[data-cell-name="${cell}"]`)]
    const el = all.at(-1)
    if (!el) return false
    el.scrollIntoView({ block: 'center' })
    ;(el.querySelector<HTMLElement>('.cm-content') ?? el).focus({ preventScroll: true })
    return true
  }
  if (find()) return () => undefined
  const watch = new MutationObserver(() => find() && stop())
  const timer = setTimeout(() => stop(), 10_000)
  const stop = () => {
    watch.disconnect()
    clearTimeout(timer)
  }
  watch.observe(document.body, { childList: true, subtree: true })
  return stop
}

/** Header, transcript, todo card, composer. Loads the newest page and the agent subtree on first visit; events keep both live.
 * `first`: the top-left pane, the only one that inserts the sidebar toggle and macOS traffic-light inset. Also the chat a
 * meeting page embeds under its transcript (`ControlCenter`), which passes `first={false}`. */
export function LiveSession({ session, first, aboveComposer }: { session: string; first: boolean; /** 151 R1: a host's strip right above the composer (the setup pane's phone "Draft n of m" pill). */ aboveComposer?: ReactNode }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const navigate = useNavigate()
  const focused = usePanes((s) => s.focused) === session
  // A chat beside web tiles only is the URL's chat: it has no close (it would come straight back, `useRoutedPane`).
  const split = usePanes((s) => s.root !== null && chatLeaves(s.root).length > 1)
  // The engine this route mounted under: an RPC landing after a switch writes to its own stores, never the new engine's (48 §5).
  const { sessions: store } = useScope()
  const title = useSessionField(session, 'title')
  const running = useSessionField(session, 'running')
  // 141 §9.2(d): a watch-only harness subagent has a transcript but nothing to say to — the bar stands where the composer would.
  const watching = useSessionField(session, 'kind') === 'observed'
  // A title hit opened this chat (49 §7): offer the way back to the results; any other origin belongs to another destination.
  const origin = useSearchOrigin()
  const destination = useMemo<SearchDestination>(() => ({ category: 'sessions', session, seq: null }), [session])
  const fromSearch = isSearchDestination(origin, destination)

  // Same fallback as the header: the list row until `TitleChanged` fills the slice (the query is shared with it).
  const sessions = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const row = sessions.data?.find((s) => s.id === session)
  useEffect(() => {
    if (focused) document.title = `${title || (row ? chatName(row) : '')}${running ? ' ·' : ''}`
  }, [title, row, running, focused])
  useEffect(() => {
    if (import.meta.env.DEV && import.meta.env.VITE_CODEMAP_MOCK === '1') void import('@/dev/codemap-fixture').then((m) => m.play(session))
  }, [session])
  useSessionLoad(session)
  // 183 §5.6: a chat just created on another machine that still cannot be read after the route's retries.
  const lost = useRemoteStart((s) => (s.by[session]?.unreachable ? s.by[session].machine : null))
  // 125 §8.2: the pill joins the strip's row only while an advisor exists, so the row stays absent when all is quiet.
  const advisorState = useAdvisor(session)?.state
  const advised = advisorState !== undefined && advisorState !== 'off'

  // ⇧⌘C / ⇧⌘; address the focused pane only.
  useEffect(() => {
    if (!focused) return
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
    window.addEventListener('theuth:copy-last-response', response)
    window.addEventListener('theuth:copy-last-code', code)
    return () => {
      window.removeEventListener('theuth:copy-last-response', response)
      window.removeEventListener('theuth:copy-last-code', code)
    }
  }, [session, store, focused])

  const close = () => {
    const to = usePanes.getState().close(session)
    if (to) void navigate({ to: '/c/$session', params: { session: to } })
  }

  // Keyed so refs/drafts reset per session; the keys must differ between siblings.
  return (
    <>
      <Header session={session} first={first} onClose={split ? close : undefined} />
      <LinkBanner />
      <PrivateStrip session={session} />
      <OrphansBanner session={session} />
      <ResumeBanner session={session} />
      {fromSearch && (
        <div className="mx-auto flex w-full shrink-0 justify-end md:max-w-[768px] px-4">
          <SearchReturn />
        </div>
      )}
      <Transcript key={`t:${session}`} session={session} />
      {lost && <UnreachableCard session={session} machine={lost} />}
      <BackgroundArea key={`d:${session}`} session={session} trailing={advised ? <AdvisorPill session={session} /> : undefined} />
      <WebPill session={session} />
      {aboveComposer}
      {watching ? (
        <>
          <KernelStrip session={session} />
          <ObservedBar session={session} />
        </>
      ) : (
        <Composer key={`c:${session}`} session={session} strip={
            <>
              <ContactAllowStrip session={session} embedded />
              <KernelStrip session={session} embedded />
            </>
          } />
      )}
    </>
  )
}

/**
 * Keep `session`'s slice loaded while a view of it is mounted: the newest page, agents and artifacts on first
 * visit, the settled path (48 §5) on `ContextChanged`, a full reload on reconnect. Shared by the chat and the
 * talk view (76 §0.12); either may be the first to open the session.
 *
 * Every RPC is bound to the effect that issued it (`signal`): one settling after the effect was cleaned up — the
 * route unmounted, the session or the engine changed — is dropped, no toast and no store write. "no such session"
 * for this session (a deleted chat, a stale deep link, a notification for another engine's session) replaces the
 * route with `/` silently; every other failure toasts. 183 §5.6: a chat this window just created on another
 * machine is never sent home: its reads are retried with backoff for `YOUNG_MS`, then it shows the unreachable card.
 */
export function useSessionLoad(session: string): void {
  const api = useApi()
  const canReminders = useQuerySupport(['list_reminders'])
  const canTerminals = useQuerySupport(['list_terminals'])
  const navigate = useNavigate()
  // The engine this route mounted under: an RPC landing after a switch writes to its own stores, never the new engine's (48 §5).
  const { sessions: store } = useScope()
  const tries = useRef(0)
  const report = useCallback(
    (e: unknown, signal: AbortSignal) => {
      if (signal.aborted) return
      reportLoadError(e, session, tries, {
        home: () => void navigate({ to: '/', replace: true }),
        toast: (message) => toast.error(message),
        retry: (ms) =>
          setTimeout(() => {
            if (!signal.aborted) window.dispatchEvent(new CustomEvent<ReloadDetail>('theuth:reload-session', { detail: { session } }))
          }, ms),
      })
    },
    [navigate, session],
  )

  const refreshBackground = useCallback((signal: AbortSignal) => {
    if (canReminders) void relistReminders(api, store, session, signal).catch((e: unknown) => report(e, signal))
    if (canTerminals) void relistTerminals(api, store, session, signal).catch((e: unknown) => report(e, signal))
  }, [api, store, session, canReminders, canTerminals, report])

  // The plan (47 §5.4): one `get_todos` when the page carried none (an older engine, a refused read) or a gap
  // marked it stale. `setTodos` only moves forward, so a cached list is never wiped; a failed read leaves the
  // list labelled stale (the card says so) rather than silently reading as empty.
  const refreshTodos = useCallback(
    (signal: AbortSignal) => {
      api.get_todos(session).then(
        (t) => {
          if (!signal.aborted) store.getState().setTodos(session, t)
        },
        (e) => {
          if (signal.aborted) return
          store.getState().markTodosStale(session)
          report(e, signal)
        },
      )
    },
    [api, session, store, report],
  )

  const load = useCallback(
    async (signal: AbortSignal) => {
      // A reconnect rebinds an owned private tree first (51 §4.3); a read before that lands on the old peer and is refused.
      await resumed()
      if (signal.aborted) return
      refreshBackground(signal)
      const before = store.getState().sessions[session]
      const fresh = before && freshness(before)
      const [page, agents] = await Promise.all([api.load_session(session, null, PAGE), api.list_agents(session), relistArtifacts(api, store, session)])
      if (signal.aborted) return
      const { loadHistory, setChildren } = store.getState()
      loadHistory(session, page, 'replace', fresh)
      setChildren(session, agents)
      const s = store.getState().sessions[session]
      if (!s?.todos || s.todosStale) refreshTodos(signal)
    },
    [api, session, store, refreshTodos, refreshBackground],
  )

  // The mutation refresh (48 §5): a settled-only page, installed only when nothing raced it. Calls
  // while one is in flight coalesce into one more pass. A refusal from the actor (busy) waits for its
  // handback's `ContextChanged`; a refusal from the store (an event landed meanwhile) retries while idle.
  const settling = useRef(false)
  const again = useRef(false)
  const settle = useCallback(
    (signal: AbortSignal) => {
      if (settling.current) {
        again.current = true
        return
      }
      settling.current = true
      void relistArtifacts(api, store, session)
      refreshBackground(signal)
      void (async () => {
        for (let tries = 0; tries < SETTLE_TRIES; tries++) {
          const s = store.getState().sessions[session]
          // Nothing to reconcile. A running turn is the engine's call, not this slice's `running` (stale after a gap,
          // 129 D3): a busy actor answers `settled: false` and its `TurnDone` publishes the `ContextChanged` that comes back here.
          if (!s?.contextStale) break
          const fresh = freshness(s)
          try {
            const page = await api.load_session(session, null, PAGE, true)
            if (signal.aborted) break
            if (store.getState().loadHistory(session, page, 'settled', fresh) || !page.settled || page.running) break
          } catch (e) {
            report(e, signal)
            break
          }
        }
        settling.current = false
        if (again.current && !signal.aborted) {
          again.current = false
          settle(signal)
        }
      })()
    },
    [api, session, store, report, refreshBackground],
  )

  // cells-midturn D2: the running turn's stored replies give their cells a stored seq, so they run before the turn
  // ends. The engine publishes `ContextUsed` just before it stores the round and the tool, injection events after, so
  // each asks when a closed reply lacks its row; one read in flight, calls meanwhile coalesce into one more. R1: reads
  // that stamp nothing for the same replies give up after two (`stampWanted`) until a reply closes or gets stamped.
  const stamping = useRef(false)
  const restamp = useRef(false)
  const stampTries = useRef<StampTries | undefined>(undefined)
  const stamp = useCallback(
    (signal: AbortSignal) => {
      const s = store.getState().sessions[session]
      const key = s ? unstampedKey(s) : null
      if (!s || key === null || !stampWanted(stampTries.current, key)) return
      if (stamping.current) {
        restamp.current = true
        return
      }
      stamping.current = true
      const fresh = freshness(s)
      void api
        .load_session(session, null, STAMP_PAGE)
        .then(
          (page) => {
            if (signal.aborted) return
            store.getState().stampTail(session, page, fresh)
            const after = store.getState().sessions[session]
            stampTries.current = stampTried(stampTries.current, key, after ? unstampedKey(after) : null)
          },
          (e: unknown) => report(e, signal),
        )
        .finally(() => {
          stamping.current = false
          if (restamp.current && !signal.aborted) {
            restamp.current = false
            stamp(signal)
          }
        })
    },
    [api, session, store, report],
  )

  useEffect(() => {
    const ctl = new AbortController()
    const st = store.getState()
    st.ensure(session)
    const s = st.sessions[session]
    if (!s?.loaded) {
      load(ctl.signal).catch((e: unknown) => report(e, ctl.signal))
    } else {
      refreshBackground(ctl.signal)
      api.list_agents(session).then(
        (a) => {
          if (!ctl.signal.aborted) store.getState().setChildren(session, a)
        },
        (e: unknown) => report(e, ctl.signal),
      )
      // A `ContextChanged` reached this slice while another chat was shown.
      if (s.contextStale) settle(ctl.signal)
      // The plan went stale (reconnect / `Lagged`) or never arrived while hidden: refresh before display (47 §5.4).
      if (!s.todos || s.todosStale) refreshTodos(ctl.signal)
    }
    return () => ctl.abort()
  }, [api, session, load, settle, store, refreshTodos, refreshBackground, report])

  // Reconnect (bare event): everything between is lost, so the visible session is re-fetched in full. A context
  // edit (`ContextChanged`, or the picker's `settled` event) and 129's `Lagged` resync go through the settled path instead.
  // 183 §5.1: a session owned elsewhere streams relayed events whose seq is its owner's, so the replicated rows are the
  // truth: its `TurnFinished` settles too (a local turn's own `ContextChanged` does that already).
  const qc = useQueryClient()
  useEffect(() => {
    const ctl = new AbortController()
    const reload = (ev: Event) => {
      const detail: ReloadDetail | undefined = ev instanceof CustomEvent ? ev.detail : undefined
      if (detail?.session !== undefined && detail.session !== session) return
      if (detail?.settled) settle(ctl.signal)
      else void load(ctl.signal).catch((e: unknown) => report(e, ctl.signal))
    }
    window.addEventListener('theuth:reload-session', reload)
    const offs = [
      on("agent.events@1", 'ContextChanged', (e) => {
        if (e.session === session) settle(ctl.signal)
      }),
      on("agent.events@1", 'TurnFinished', (e) => {
        if (e.session !== session || !ownedElsewhere(qc.getQueryData<SessionSummary[]>(queryKeys.sessions), session)) return
        store.getState().markStale(session)
        settle(ctl.signal)
      }),
      ...(['ContextUsed', 'ToolCallStarted', 'ToolCallFinished', 'InputInjected'] as const).map((type) =>
        on("agent.events@1", type, (e) => {
          if (e.session === session) stamp(ctl.signal)
        }),
      ),
    ]
    return () => {
      ctl.abort()
      window.removeEventListener('theuth:reload-session', reload)
      for (const off of offs) off()
    }
  }, [load, settle, stamp, session, report, qc, store])
}

/** `Private` badge for a chat in the owned tree, plus the bounded reconnect countdown while its transport is down (51 §4.3). */
function PrivateStrip({ session }: { session: string }) {
  const p = usePrivate((s) => s.current)
  const member = useSessions((s) => p !== null && inTree(p.root, session, s.sessions))
  const lost = p?.disconnectedAt
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (lost === undefined) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [lost])
  if (!member) return null
  return (
    <div className="mx-auto flex w-full shrink-0 items-center gap-3 px-4 py-1 text-xs md:max-w-[768px]">
      <Badge variant="outline">Private</Badge>
      {lost !== undefined && (
        <span role="status" className="text-warning">
          {disconnectText(remaining(lost, now))}
        </span>
      )}
    </div>
  )
}

/** 183 §5.6: a chat just created on `machine` whose reads still fail after `YOUNG_MS` is said in place, never a silent
 * trip home. Try again restarts the route's retries. */
function UnreachableCard({ session, machine }: { session: string; machine: string }) {
  return (
    <div className="mx-auto w-full shrink-0 px-4 md:max-w-[768px]">
      <div role="alert" className="mb-2 flex items-center gap-2 rounded-xl border border-warning-border/30 bg-warning-bg/40 px-3 py-2 text-sm">
        <span className="min-w-0 flex-1 text-fg-secondary">This chat isn&apos;t reachable: {machine} has not answered for it.</span>
        <Button
          size="sm"
          variant="outline"
          onClick={() => {
            useRemoteStart.getState().begin(session, machine)
            window.dispatchEvent(new CustomEvent<ReloadDetail>('theuth:reload-session', { detail: { session } }))
          }}
        >
          Try again
        </Button>
      </div>
    </div>
  )
}
